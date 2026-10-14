"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AnimatePresence,
  motion,
  useAnimationControls,
  useReducedMotion,
} from "motion/react";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import {
  LuBot,
  LuCircleCheck,
  LuCircleX,
  LuCopy,
  LuExternalLink,
  LuFileDown,
  LuFileText,
  LuFolderOpen,
  LuHistory,
  LuLoader,
  LuPaperclip,
  LuRefreshCw,
  LuRotateCcw,
  LuSettings2,
  LuSparkles,
  LuTrash2,
} from "react-icons/lu";
import { toast } from "sonner";
import { AiAssistantPage } from "@/components/ai-assistant-page";
import { AiRewriteSettingsDialog } from "@/components/ai-rewrite-settings-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

interface AiRewritePageProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  subPage?: boolean;
  embedded?: boolean;
}

interface AiStatus {
  server_time?: string;
  config_ready?: boolean;
  billing?: Record<string, unknown> | null;
}

interface ProgressPayload {
  task_key: string;
  index: number;
  total: number;
  video_id: string;
  title: string;
  source_url?: string;
  platform?: string;
  stage: string;
  percent: number;
  message: string;
  speed_text?: string;
}

interface LogPayload {
  task_key: string;
  index?: number;
  video_id?: string;
  source_url?: string;
  level: string;
  message: string;
  time: string;
}

interface RunResult {
  video_id?: string;
  title?: string;
  source_url?: string;
  platform?: string;
  task_id?: number;
  script_id?: number;
  score?: number;
  transcript?: string;
  transcript_segments?: unknown;
  transcript_zh?: string;
  analyze?: unknown;
  rewrite_ref?: { label?: string; points?: RefPoints };
  rewrite?: unknown;
  score_json?: unknown;
  reused?: boolean;
  success: boolean;
  error?: string;
}

type TaskStage =
  | "pending"
  | "start"
  | "download"
  | "asr"
  | "analyze"
  | "rewrite"
  | "score"
  | "done"
  | "error";

type TaskStatus = "pending" | "running" | "done" | "error" | "interrupted";

interface TaskLog {
  time: string;
  level: string;
  message: string;
}

interface TaskItem {
  key: string;
  title: string;
  url: string;
  platform?: string;
  stage: TaskStage;
  percent: number;
  message: string;
  speedText: string;
  logs: TaskLog[];
  status: TaskStatus;
  result?: RunResult;
  error?: string;
  /** 实时产物（走到哪一步就推哪一步，不等全流程结束） */
  transcript?: string;
  transcriptSegments?: unknown;
  transcriptZh?: string;
  analyze?: unknown;
  rewriteRef?: { label?: string; points?: RefPoints };
  rewrite?: unknown;
  scoreJson?: unknown;
  /** 是否从本地持久化文件加载（历史已完成任务） */
  persisted?: boolean;
}

/** 爆点库各层参考明细（给用户看，与注入提示词的文本同源） */
interface RefPoints {
  work_records?: string[];
  same_work?: string[];
  same_track?: string[];
  cross_track?: string[];
  fallback?: string[];
}

const REF_LAYER_LABEL: Record<keyof RefPoints, string> = {
  work_records: "同作品同集历史",
  same_work: "作品记忆",
  same_track: "同赛道高热开头",
  cross_track: "跨赛道通用网感",
  fallback: "全库最热",
};

interface ContentPayload {
  task_key: string;
  source_url?: string;
  field:
    | "transcript"
    | "transcript_segments"
    | "transcript_zh"
    | "analyze"
    | "rewrite_ref"
    | "rewrite"
    | "score";
  value: unknown;
}

interface DetailData {
  title?: string;
  platform?: string;
  play_count?: number;
  source_url?: string;
  created_at?: string;
  transcript?: string;
  transcriptSegments?: unknown;
  transcriptZh?: string;
  analyze?: unknown;
  rewriteRef?: { label?: string; points?: RefPoints };
  rewrite?: unknown;
  score?: unknown;
}

const STAGE_LABEL: Record<string, string> = {
  pending: "等待",
  start: "准备",
  download: "下载",
  asr: "转写",
  analyze: "分析",
  rewrite: "洗稿",
  score: "打分",
  done: "完成",
  error: "失败",
  interrupted: "中断",
};

/** 流水线阶段顺序（用于步骤条） */
const PIPELINE_STAGES: { key: TaskStage; label: string }[] = [
  { key: "download", label: "下载" },
  { key: "asr", label: "转写" },
  { key: "analyze", label: "分析" },
  { key: "rewrite", label: "洗稿" },
  { key: "score", label: "打分" },
];

function stageOrder(stage: TaskStage): number {
  if (stage === "pending") return -1;
  if (stage === "start") return 0;
  if (stage === "done") return PIPELINE_STAGES.length;
  if (stage === "error") return -1;
  const i = PIPELINE_STAGES.findIndex((s) => s.key === stage);
  return i < 0 ? 0 : i;
}

/** 本地视频扩展名（拖入/选择时过滤；无扩展名的路径视为文件夹放行） */
const VIDEO_EXT = /\.(mp4|mkv|mov|avi|webm|flv|ts|m4v|wmv|mpg|mpeg|3gp)$/i;

const PLATFORM_LABEL: Record<string, string> = {
  tiktok: "TikTok",
  youtube: "YouTube",
  douyin: "抖音",
};

function platformLabel(p?: string): string {
  if (!p) return "—";
  return PLATFORM_LABEL[p] ?? p;
}

function formatCount(n?: number): string {
  if (n === undefined || n === null) return "—";
  if (n >= 100000000)
    return `${(n / 100000000).toFixed(1).replace(/\.0$/, "")}亿`;
  if (n >= 10000) return `${(n / 10000).toFixed(1).replace(/\.0$/, "")}万`;
  if (n >= 1000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  return String(n);
}

function asArray(v: unknown): unknown[] {
  return Array.isArray(v) ? v : [];
}

function toList(v: unknown): string[] {
  if (Array.isArray(v)) return v.map((x) => String(x));
  if (typeof v === "string") {
    return v
      .split(/[,\n]/)
      .map((s) => s.trim())
      .filter(Boolean);
  }
  return [];
}

function bodyParas(v: unknown): string[] {
  if (Array.isArray(v)) return v.map((x) => String(x));
  if (typeof v === "string") return v.split(/\n+/).filter(Boolean);
  return [];
}

function scoreColor(score?: number | null): string {
  if (score === undefined || score === null) return "text-muted-foreground";
  if (score >= 90) return "text-emerald-500";
  if (score >= 75) return "text-blue-500";
  if (score >= 60) return "text-orange-500";
  return "text-destructive";
}

/** 取洗稿文案的完整纯文本（钩子 + 正文 + 结尾引导）。 */
function rewriteFullText(rewrite: unknown): string {
  const w = (rewrite ?? {}) as Record<string, unknown>;
  return [w.hook, ...bodyParas(w.body), w.cta].filter(Boolean).join("\n");
}

/** 去掉文件名里的非法字符。 */
function safeFileName(name: string): string {
  const s = (name || "字幕").replace(/[\\/:*?"<>|]/g, "_").trim();
  return s.slice(0, 40) || "字幕";
}

/** 复制洗稿文稿到剪贴板。 */
async function copyRewrite(text: string, kind = "洗稿文稿") {
  if (!text.trim()) {
    toast.info(`暂无可复制的${kind}`);
    return;
  }
  try {
    await navigator.clipboard.writeText(text);
    toast.success(`已复制${kind}`);
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  }
}

/** 生成并保存 SRT 字幕；lang 决定保存哪个文件（en=原文 / zh=中文译文），各自独立导出。 */
async function exportSrt(opts: {
  lang: "en" | "zh";
  rewrite?: unknown;
  transcript?: string;
  segments?: unknown;
  title?: string;
  duration?: number;
}) {
  const hasRewrite = rewriteFullText(opts.rewrite).trim().length > 0;
  if (!hasRewrite && !(opts.transcript ?? "").trim()) {
    toast.info("暂无可导出的文案");
    return;
  }
  const loadingId = toast.loading("正在生成字幕…");
  try {
    const res = await invoke<{
      cue_count: number;
      srt_en: string;
      srt_zh: string;
      translated: boolean;
      title: string;
      message?: string;
    }>("ai_rewrite_build_srt", {
      rewrite: hasRewrite ? opts.rewrite : null,
      transcript: opts.transcript ?? "",
      segments: opts.segments ?? null,
      title: opts.title ?? "",
      duration: opts.duration ?? 0,
    });
    toast.dismiss(loadingId);

    const baseName = safeFileName(opts.title ?? "字幕");
    if (opts.lang === "zh") {
      if (!res.translated || !res.srt_zh.trim()) {
        toast.warning(res.message || "中文翻译未生成，无法导出中文字幕");
        return;
      }
      const zhPath = await save({
        defaultPath: `${baseName}_zh.srt`,
        filters: [{ name: "SRT 字幕（中文）", extensions: ["srt"] }],
      });
      if (zhPath) {
        await writeTextFile(zhPath, res.srt_zh);
        toast.success(`已导出中文字幕（${res.cue_count} 条）`);
      }
    } else {
      if (!res.srt_en.trim()) {
        toast.warning("没有可导出的字幕内容");
        return;
      }
      const enPath = await save({
        defaultPath: `${baseName}_en.srt`,
        filters: [{ name: "SRT 字幕（原文）", extensions: ["srt"] }],
      });
      if (enPath) {
        await writeTextFile(enPath, res.srt_en);
        toast.success(`已导出原文/英文字幕（${res.cue_count} 条）`);
      }
    }
  } catch (e) {
    toast.dismiss(loadingId);
    toast.error(e instanceof Error ? e.message : String(e));
  }
}

export function AiRewritePage({
  open: _open,
  onOpenChange: _onOpenChange,
  subPage: _subPage,
  embedded = false,
}: AiRewritePageProps) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<"run" | "tasks" | "assistant">("run");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [taskFlash, setTaskFlash] = useState<{ count: number } | null>(null);
  const reduceMotion = useReducedMotion();

  // 「开始分析」→ 任务列表的小人飞行动画
  const runBtnWrap = useRef<HTMLDivElement>(null);
  const tasksTabRef = useRef<HTMLDivElement>(null);
  const flySeq = useRef(0);
  const runKick = useAnimationControls();
  const [flyer, setFlyer] = useState<{
    x0: number;
    y0: number;
    x1: number;
    y1: number;
    key: number;
  } | null>(null);
  const throwRunner = useCallback(() => {
    const from = runBtnWrap.current?.getBoundingClientRect();
    const to = tasksTabRef.current?.getBoundingClientRect();
    if (!from || !to || reduceMotion) return;
    flySeq.current += 1;
    setFlyer({
      x0: from.left + from.width / 2 - 14,
      y0: from.top + from.height / 2 - 14,
      x1: to.left + to.width / 2 - 14,
      y1: to.top + to.height / 2 - 14,
      key: flySeq.current,
    });
  }, [reduceMotion]);

  const [status, setStatus] = useState<AiStatus | null>(null);
  const [loadingStatus, setLoadingStatus] = useState(false);
  const [statusError, setStatusError] = useState<string | null>(null);

  const [customLinks, setCustomLinks] = useState("");
  const [tasks, setTasks] = useState<TaskItem[]>([]);
  const [summary, setSummary] = useState<{
    total: number;
    done: number;
    failed: number;
  } | null>(null);

  /** 持久化全部任务到本地磁盘（含进行中/中断/失败/已完成），应用重启时完整恢复。 */
  const persistTasks = useCallback(async (next: TaskItem[]) => {
    try {
      const serializable = next.map((t) => ({
        ...t,
        // 日志截断到 500 条，避免磁盘膨胀
        logs: t.logs.slice(-500),
        persisted: true,
      }));
      await invoke("ai_rewrite_save_tasks", { tasks: serializable });
    } catch (e) {
      console.error("持久化任务列表失败", e);
    }
  }, []);

  /** 防抖持久化：每次 setTasks 后 1s 统一落盘一次，避免每条进度都写磁盘。 */
  const persistTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const schedulePersist = useCallback(
    (next: TaskItem[]) => {
      if (persistTimer.current) clearTimeout(persistTimer.current);
      persistTimer.current = setTimeout(() => {
        void persistTasks(next);
      }, 1000);
    },
    [persistTasks],
  );

  /** 持有 tasks 最新值，供组件卸载时强制落盘使用。 */
  const tasksRef = useRef<TaskItem[]>([]);
  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  /** 页面进入时把本地已落盘的任务加载回列表。
   *  上次应用退出时还在 running 的任务标记为 interrupted，UI 展示为「中断」。 */
  const loadPersistedTasks = useCallback(async () => {
    try {
      const arr = await invoke<unknown[]>("ai_rewrite_load_tasks");
      if (!Array.isArray(arr) || arr.length === 0) return;
      const loaded: TaskItem[] = arr
        .map((raw) => raw as Partial<TaskItem>)
        .filter((t) => t && (t.key || t.url))
        .map((t) => {
          const item = { ...(t as TaskItem), persisted: true };
          // 上次运行中断的任务：标记为 interrupted，避免 UI 一直显示"处理中"
          if (item.status === "running" || item.status === "pending") {
            return {
              ...item,
              status: "interrupted" as const,
              stage: "error" as TaskStage,
              message: "任务在上次会话中中断",
            };
          }
          return item;
        });
      // 排序：未完成（interrupted）在前，按原始插入顺序；已完成在后
      loaded.sort((a, b) => {
        const aUnfinished = a.status === "interrupted" ? 0 : 1;
        const bUnfinished = b.status === "interrupted" ? 0 : 1;
        return aUnfinished - bUnfinished;
      });
      setTasks(loaded);
    } catch (e) {
      console.error("加载持久化任务失败", e);
    }
  }, []);

  useEffect(() => {
    void loadPersistedTasks();
  }, [loadPersistedTasks]);

  /** 删除单个任务（从列表移除并重新落盘）。 */
  const deleteTask = useCallback(
    (key: string) => {
      setTasks((prev) => {
        const next = prev.filter((t) => t.key !== key && t.url !== key);
        void persistTasks(next);
        return next;
      });
      toast.success("已删除任务");
    },
    [persistTasks],
  );

  /** 清空整个任务列表（含本地落盘文件）。 */
  const clearTasks = useCallback(async () => {
    if (tasks.length === 0) return;
    const ok = await confirm(
      `确定要清空全部 ${tasks.length} 条任务吗？此操作不可撤销。`,
      { kind: "warning", okLabel: "清空", cancelLabel: "取消" },
    ).catch(() => false);
    if (!ok) return;
    setTasks([]);
    setSummary(null);
    try {
      await invoke("ai_rewrite_clear_tasks");
      toast.success("已清空任务列表");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  }, [tasks]);

  const hasFinishedTasks = useMemo(
    () =>
      tasks.some(
        (t) =>
          t.status === "done" ||
          t.status === "error" ||
          t.status === "interrupted",
      ),
    [tasks],
  );

  const refreshStatus = useCallback(async () => {
    setLoadingStatus(true);
    setStatusError(null);
    try {
      const s = await invoke<AiStatus>("ai_rewrite_status");
      setStatus(s);
    } catch (e) {
      setStatus(null);
      setStatusError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingStatus(false);
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  const matchTask = useCallback(
    (prev: TaskItem[], key?: string, url?: string): number => {
      if (url) {
        const byUrl = prev.findIndex((t) => t.url && t.url === url);
        if (byUrl >= 0) return byUrl;
      }
      if (key) {
        const byKey = prev.findIndex((t) => t.key === key);
        if (byKey >= 0) return byKey;
      }
      return -1;
    },
    [],
  );

  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    listen<ProgressPayload>("ai-rewrite:progress", (event) => {
      const p = event.payload;
      // AI 助手后台发起的任务不在当前列表里：用事件负载补建条目并切到任务列表，
      // 保证发起即可见（Rust 端同样已落盘，重开页面也能看到）。
      const known = matchTask(tasksRef.current, p.task_key, p.source_url) >= 0;
      setTasks((prev) => {
        const at = matchTask(prev, p.task_key, p.source_url);
        if (at < 0) {
          const item: TaskItem = {
            key: p.task_key,
            title: p.title || p.source_url || p.task_key,
            url: p.source_url || p.task_key,
            platform: p.platform,
            stage: (p.stage as TaskStage) || "pending",
            percent: typeof p.percent === "number" ? p.percent : 0,
            message: p.message ?? "",
            speedText: p.stage === "download" ? (p.speed_text ?? "") : "",
            logs: [],
            status:
              p.stage === "done"
                ? "done"
                : p.stage === "error"
                  ? "error"
                  : "running",
          };
          const next = [...prev, item];
          schedulePersist(next);
          return next;
        }
        const next = [...prev];
        const cur = next[at];
        next[at] = {
          ...cur,
          title: cur.title || p.title || "",
          platform: cur.platform || p.platform || cur.platform,
          stage: (p.stage as TaskStage) || cur.stage,
          percent: typeof p.percent === "number" ? p.percent : cur.percent,
          message: p.message ?? cur.message,
          speedText: p.stage === "download" ? (p.speed_text ?? "") : "",
          status:
            p.stage === "done"
              ? "done"
              : p.stage === "error"
                ? "error"
                : "running",
        };
        schedulePersist(next);
        return next;
      });
      if (!known) {
        setTab("tasks");
        setTaskFlash({ count: 1 });
        window.setTimeout(() => setTaskFlash(null), 900);
      }
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    listen<LogPayload>("ai-rewrite:log", (event) => {
      const l = event.payload;
      setTasks((prev) => {
        const at = matchTask(prev, l.task_key, l.source_url);
        if (at < 0) return prev;
        const next = [...prev];
        const cur = next[at];
        const logs = [
          ...cur.logs,
          { time: l.time, level: l.level, message: l.message },
        ].slice(-300);
        next[at] = { ...cur, logs };
        schedulePersist(next);
        return next;
      });
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    listen<ContentPayload>("ai-rewrite:content", (event) => {
      const c = event.payload;
      setTasks((prev) => {
        const at = matchTask(prev, c.task_key, c.source_url);
        if (at < 0) return prev;
        const next = [...prev];
        const cur = next[at];
        const patch: Partial<TaskItem> = {};
        if (c.field === "transcript") patch.transcript = String(c.value ?? "");
        else if (c.field === "transcript_segments")
          patch.transcriptSegments = c.value;
        else if (c.field === "transcript_zh")
          patch.transcriptZh = String(c.value ?? "");
        else if (c.field === "analyze") patch.analyze = c.value;
        else if (c.field === "rewrite_ref")
          patch.rewriteRef = c.value as { label?: string; points?: RefPoints };
        else if (c.field === "rewrite") patch.rewrite = c.value;
        else if (c.field === "score") patch.scoreJson = c.value;
        next[at] = { ...cur, ...patch };
        schedulePersist(next);
        return next;
      });
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});

    return () => {
      for (const fn of unlisteners) fn();
      // 组件卸载前强制 flush：清掉 pending timer，立即落盘最新状态
      if (persistTimer.current) clearTimeout(persistTimer.current);
      void persistTasks(tasksRef.current);
    };
  }, [matchTask, schedulePersist, persistTasks]);

  const customCount = useMemo(
    () => customLinks.split(/\r?\n/).filter((s) => s.trim()).length,
    [customLinks],
  );
  const totalCount = customCount;

  const runningCount = useMemo(
    () =>
      tasks.filter((t) => t.status === "running" || t.status === "pending")
        .length,
    [tasks],
  );

  /** 往粘贴框追加本地路径（去重、换行拼接） */
  const appendLocalPaths = useCallback((paths: string[]) => {
    const list = paths.filter(Boolean);
    if (list.length === 0) return;
    setCustomLinks((prev) => {
      const have = new Set(
        prev
          .split(/\r?\n/)
          .map((s) => s.trim())
          .filter(Boolean),
      );
      const add = list.filter((p) => !have.has(p));
      if (add.length === 0) return prev;
      const base = prev.trimEnd();
      return base ? `${base}\n${add.join("\n")}` : add.join("\n");
    });
  }, []);

  const pickVideoFiles = useCallback(async () => {
    try {
      const res = await open({
        multiple: true,
        filters: [
          {
            name: "视频文件",
            extensions: [
              "mp4",
              "mkv",
              "mov",
              "avi",
              "webm",
              "flv",
              "ts",
              "m4v",
              "wmv",
              "mpg",
              "mpeg",
              "3gp",
            ],
          },
        ],
      });
      appendLocalPaths(Array.isArray(res) ? res : res ? [res] : []);
    } catch {
      // 用户取消
    }
  }, [appendLocalPaths]);

  const pickVideoDir = useCallback(async () => {
    try {
      const res = await open({ directory: true });
      if (res) appendLocalPaths([res]);
    } catch {
      // 用户取消
    }
  }, [appendLocalPaths]);

  /** 把视频/文件夹直接拖进粘贴框（Tauri 原生拖放，落点限定在粘贴卡内） */
  const dropZoneRef = useRef<HTMLDivElement | null>(null);
  const [dragOver, setDragOver] = useState(false);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "leave") {
          setDragOver(false);
          return;
        }
        // Tauri 给物理像素坐标，DOM getBoundingClientRect 是逻辑像素
        const rect = dropZoneRef.current?.getBoundingClientRect();
        const scale = window.devicePixelRatio || 1;
        const inside =
          !!rect &&
          p.position.x / scale >= rect.left &&
          p.position.x / scale <= rect.right &&
          p.position.y / scale >= rect.top &&
          p.position.y / scale <= rect.bottom;
        if (p.type === "enter" || p.type === "over") {
          setDragOver(inside);
          return;
        }
        // drop：只收视频文件和文件夹
        setDragOver(false);
        if (!inside) return;
        const paths = p.paths.filter(
          (f) => !/\.[^./\\]+$/.test(f) || VIDEO_EXT.test(f),
        );
        if (paths.length === 0) return;
        appendLocalPaths(paths);
        toast.info(`已添加 ${paths.length} 个本地视频/文件夹`);
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, [appendLocalPaths]);

  /** 合并后端运行结果到任务列表（批量跑与单任务重试共用）；失败时保留已到达的阶段，便于看到卡在哪一步。
   *  overwrite=true（重新洗稿）：成功的新结果覆盖旧的 分析/洗稿/打分，不再保留旧值 */
  const applyRunResults = useCallback(
    (results: RunResult[], overwrite?: boolean) => {
      setTasks((prev) => {
        const merged = prev.map((t) => ({ ...t }));
        for (const r of results || []) {
          let at = -1;
          if (r.source_url)
            at = merged.findIndex((t) => t.url === r.source_url);
          if (at < 0 && r.video_id)
            at = merged.findIndex((t) => t.key === r.video_id);
          if (at < 0) continue;
          const cur = merged[at];
          const ow = overwrite && r.success;
          merged[at] = {
            ...cur,
            stage: r.success ? "done" : cur.stage,
            status: r.success ? "done" : "error",
            percent: 100,
            speedText: "",
            message: r.success
              ? r.reused
                ? "复用历史结果"
                : "完成"
              : r.error || "失败",
            error: r.success ? undefined : r.error,
            result: r.success ? r : undefined,
            transcript:
              (ow ? r.transcript : cur.transcript) ??
              (r.transcript as string | undefined),
            transcriptSegments:
              (ow ? r.transcript_segments : cur.transcriptSegments) ??
              r.transcript_segments,
            analyze: (ow ? r.analyze : cur.analyze) ?? r.analyze,
            rewrite: (ow ? r.rewrite : cur.rewrite) ?? r.rewrite,
            scoreJson: (ow ? r.score_json : cur.scoreJson) ?? r.score_json,
          };
        }
        void persistTasks(merged);
        return merged;
      });
    },
    [persistTasks],
  );

  const startRun = useCallback(
    async (mode?: "transcript") => {
      const items: Record<string, unknown>[] = [];
      const initTasks: TaskItem[] = [];
      const seen = new Set<string>();
      const push = (
        item: Record<string, unknown>,
        title: string,
        platform?: string,
      ) => {
        const url = String(item.source_url ?? "");
        if (!url || seen.has(url)) return;
        seen.add(url);
        if (mode) item.mode = mode;
        items.push(item);
        initTasks.push({
          key: url,
          title: title || url,
          url,
          platform,
          stage: "pending",
          percent: 0,
          message: "等待中",
          speedText: "",
          logs: [],
          status: "pending",
        });
      };
      for (const line of customLinks.split(/\r?\n/)) {
        const u = line.trim();
        if (!u) continue;
        const lower = u.toLowerCase();
        const pf =
          lower.includes("youtube.com") || lower.includes("youtu.be")
            ? "youtube"
            : lower.includes("tiktok.com")
              ? "tiktok"
              : lower.includes("douyin.com")
                ? "douyin"
                : "";
        push({ source_url: u, title: u, platform: pf }, u, pf || undefined);
      }
      if (items.length === 0) {
        toast.info("请先粘贴链接或本地视频路径");
        return;
      }
      // 保留已有任务（含已落盘的历史任务），追加本次新任务
      setTasks((prev) => [...prev, ...initTasks]);
      setSummary(null);
      setTaskFlash({ count: initTasks.length });
      window.setTimeout(() => setTaskFlash(null), 900);
      void runKick.start(
        { scale: [1, 0.85, 1.12, 1], rotate: [0, -5, 4, 0] },
        { duration: reduceMotion ? 0 : 0.45, ease: "easeOut" },
      );
      throwRunner();
      try {
        const res = await invoke<{
          total: number;
          done: number;
          failed: number;
          results: RunResult[];
        }>("ai_rewrite_run_links", { items, concurrency: 3 });
        setSummary({ total: res.total, done: res.done, failed: res.failed });
        applyRunResults(res.results);
        toast.success(`分析完成：成功 ${res.done}，失败 ${res.failed}`);
        void refreshStatus();
      } catch (e) {
        toast.error(e instanceof Error ? e.message : String(e));
      }
    },
    [
      customLinks,
      refreshStatus,
      applyRunResults,
      runKick,
      throwRunner,
      reduceMotion,
    ],
  );

  /** 单任务重试：从失败处继续（云端会复用已完成的转写/分析/洗稿，不重复计费） */
  const retryTask = useCallback(
    async (task: TaskItem) => {
      const cur = tasksRef.current.find((t) => t.key === task.key);
      if (!cur || (cur.status !== "error" && cur.status !== "interrupted"))
        return;
      setTasks((prev) =>
        prev.map((t) =>
          t.key === task.key
            ? {
                ...t,
                stage: "pending",
                status: "pending",
                percent: 0,
                message: "重试中",
                speedText: "",
                logs: [],
                error: undefined,
                result: undefined,
              }
            : t,
        ),
      );
      try {
        const res = await invoke<{
          total: number;
          done: number;
          failed: number;
          results: RunResult[];
        }>("ai_rewrite_run_links", {
          items: [{ source_url: task.url }],
          concurrency: 1,
        });
        applyRunResults(res.results);
        const r = res.results?.[0];
        if (r?.success) {
          setSummary((s) =>
            s
              ? { ...s, done: s.done + 1, failed: Math.max(0, s.failed - 1) }
              : s,
          );
          toast.success("重试成功");
        } else {
          toast.error(`重试失败：${r?.error || "未知错误"}`);
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setTasks((prev) =>
          prev.map((t) =>
            t.key === task.key
              ? {
                  ...t,
                  stage: "error",
                  status: "error",
                  message: msg,
                  error: msg,
                }
              : t,
          ),
        );
        toast.error(`重试失败：${msg}`);
      }
    },
    [applyRunResults],
  );

  /** 重新洗稿：跳过云端「已完成」整体复用，重跑爆点分析/洗稿/打分（转写复用），最终结果覆盖旧结果 */
  const rerunRewrite = useCallback(
    async (task: TaskItem) => {
      const cur = tasksRef.current.find((t) => t.key === task.key);
      if (!cur || cur.status === "pending" || cur.status === "running") return;
      setTasks((prev) =>
        prev.map((t) =>
          t.key === task.key
            ? {
                ...t,
                stage: "analyze",
                status: "running",
                percent: 45,
                message: "重新洗稿中",
                speedText: "",
                logs: [],
                error: undefined,
                // 转写与其译文不会变，保留；分析及之后的结果清除，等重跑结果覆盖
                analyze: undefined,
                rewrite: undefined,
                scoreJson: undefined,
                rewriteRef: undefined,
                result: undefined,
              }
            : t,
        ),
      );
      try {
        const res = await invoke<{
          total: number;
          done: number;
          failed: number;
          results: RunResult[];
        }>("ai_rewrite_run_links", {
          items: [{ source_url: task.url, force_rerun: true }],
          concurrency: 1,
        });
        applyRunResults(res.results, true);
        const r = res.results?.[0];
        if (r?.success) {
          toast.success("已重新洗稿，最终结果已覆盖");
        } else {
          toast.error(`重新洗稿失败：${r?.error || "未知错误"}`);
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setTasks((prev) =>
          prev.map((t) =>
            t.key === task.key
              ? {
                  ...t,
                  stage: "error",
                  status: "error",
                  message: msg,
                  error: msg,
                }
              : t,
          ),
        );
        toast.error(`重新洗稿失败：${msg}`);
      }
    },
    [applyRunResults],
  );

  const configReady = status?.config_ready === true;

  return (
    <div
      className={cn(
        "flex min-h-0 flex-1 flex-col gap-4 overflow-hidden",
        embedded ? "p-4" : "p-6",
      )}
    >
      {/* 标题 + 状态 */}
      <div className="flex shrink-0 items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <LuSparkles className="size-5 text-primary" />
          <h1 className="text-lg font-semibold">AI 洗稿</h1>
          <span className="text-xs text-muted-foreground">
            本地下载转写，服务器只管密钥与计费
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-xs"
            onClick={() => setSettingsOpen(true)}
          >
            <LuSettings2 className="size-3.5" />
            接口设置
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-xs"
            onClick={() => void refreshStatus()}
            disabled={loadingStatus}
          >
            {loadingStatus ? (
              <LuLoader className="size-3.5 animate-spin" />
            ) : (
              <LuRefreshCw className="size-3.5" />
            )}
            刷新状态
          </Button>
        </div>
      </div>

      {/* 状态卡 */}
      <div className="flex shrink-0 flex-wrap items-center gap-2 text-xs">
        <StatusBadge
          ok={!!status}
          label={
            status
              ? `已连接 · ${status.server_time ?? ""}`
              : statusError
                ? `连接失败：${statusError}`
                : "未连接"
          }
        />
        <StatusBadge
          ok={configReady}
          label={configReady ? "密钥已就绪" : "服务器未配置 AI 密钥"}
        />
      </div>

      {/* 标签页 */}
      <div className="flex shrink-0 items-center gap-2">
        <TabButton active={tab === "run"} onClick={() => setTab("run")}>
          <LuSparkles className="size-3.5" />
          洗稿操作
        </TabButton>
        <TabButton
          active={tab === "assistant"}
          onClick={() => setTab("assistant")}
        >
          <LuBot className="size-3.5" />
          {t("aiAssistant.title")}
        </TabButton>
        <motion.div
          ref={tasksTabRef}
          className="relative"
          animate={taskFlash ? { scale: [1, 1.16, 1] } : { scale: 1 }}
          transition={{ duration: reduceMotion ? 0 : 0.5, ease: "easeOut" }}
        >
          <TabButton active={tab === "tasks"} onClick={() => setTab("tasks")}>
            <LuHistory className="size-3.5" />
            任务列表
            {tasks.length > 0 && (
              <motion.span
                key={tasks.length}
                initial={{ scale: 0.4, opacity: 0 }}
                animate={{ scale: 1, opacity: 1 }}
                transition={{ type: "spring", stiffness: 520, damping: 18 }}
                className={cn(
                  "ml-0.5 rounded-full px-1.5 text-[10px] font-semibold",
                  runningCount > 0
                    ? "bg-primary/15 text-primary"
                    : "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
                )}
              >
                {runningCount > 0 ? runningCount : tasks.length}
              </motion.span>
            )}
          </TabButton>
          <AnimatePresence>
            {taskFlash && (
              <motion.span
                key="drop"
                initial={{ opacity: 0, y: -22, scale: 0.6 }}
                animate={{
                  opacity: [0, 1, 1, 0],
                  y: 4,
                  scale: [0.6, 1, 1, 0.5],
                }}
                exit={{ opacity: 0 }}
                transition={{
                  duration: reduceMotion ? 0 : 0.8,
                  ease: "easeOut",
                }}
                className="pointer-events-none absolute left-1/2 top-0 z-30 -translate-x-1/2 whitespace-nowrap rounded-full border bg-card px-2 py-0.5 text-[10px] font-medium shadow-sm"
              >
                +{taskFlash.count}
              </motion.span>
            )}
          </AnimatePresence>
        </motion.div>
      </div>

      <AnimatePresence mode="wait" initial={false}>
        {tab === "assistant" ? (
          <motion.div
            key="assistant"
            initial={{ opacity: 0, x: 24 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: 24 }}
            transition={{ duration: reduceMotion ? 0 : 0.22 }}
            className="flex min-h-0 flex-1 flex-col overflow-hidden"
          >
            <AiAssistantPage bare />
          </motion.div>
        ) : tab === "run" ? (
          <motion.div
            key="run"
            initial={{ opacity: 0, x: -16 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -16 }}
            transition={{ duration: reduceMotion ? 0 : 0.18 }}
            className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden"
          >
            <ScrollArea className="min-h-0 flex-1">
              <div className="space-y-3 pr-2">
                {/* 粘贴链接 / 本地视频 */}
                <div
                  ref={dropZoneRef}
                  className={cn(
                    "rounded-lg border bg-card p-4 transition-colors",
                    dragOver &&
                      "border-primary bg-primary/5 ring-1 ring-primary/40",
                  )}
                >
                  <div className="mb-3 flex items-center gap-2 text-sm font-medium">
                    <StepNo n={1} />
                    粘贴链接 / 本地视频
                    <span className="flex-1" />
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7 gap-1 text-xs"
                      onClick={() => void pickVideoFiles()}
                    >
                      <LuPaperclip className="size-3.5" />
                      选择视频
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-7 gap-1 text-xs"
                      onClick={() => void pickVideoDir()}
                    >
                      <LuFolderOpen className="size-3.5" />
                      选择文件夹
                    </Button>
                  </div>
                  <Textarea
                    value={customLinks}
                    onChange={(e) => setCustomLinks(e.target.value)}
                    placeholder={
                      "每行一个：视频链接、本地视频文件路径或文件夹，例如：\nhttps://www.tiktok.com/@xxx/video/123\nD:\\videos\\movie.mp4\nD:\\videos  （文件夹自动展开其中全部视频）\n也可直接把视频文件拖进这个输入框"
                    }
                    className="min-h-[440px] resize-y text-sm"
                  />
                  <div className="mt-1.5 text-xs text-muted-foreground">
                    已粘贴 {customCount} 条；本地文件不调下载与数据接口，仅转写
                    + 洗稿；支持把视频拖进上方输入框
                  </div>
                </div>
              </div>
            </ScrollArea>

            {/* 底部操作条 */}
            <div className="flex shrink-0 items-center gap-3 rounded-lg border bg-card px-4 py-3">
              <motion.div ref={runBtnWrap} animate={runKick}>
                <Button
                  size="sm"
                  className="h-9 gap-1.5 text-sm"
                  onClick={() => void startRun()}
                  disabled={!status || totalCount === 0}
                >
                  <LuSparkles className="size-4" />
                  开始分析
                </Button>
              </motion.div>
              <Button
                size="sm"
                variant="outline"
                className="h-9 gap-1.5 text-sm"
                onClick={() => void startRun("transcript")}
                disabled={!status || totalCount === 0}
              >
                <LuFileText className="size-4" />
                提取文案
              </Button>
              <span className="text-xs text-muted-foreground">
                本次将分析 {totalCount} 条；任务将加入「任务列表」
              </span>
            </div>
          </motion.div>
        ) : (
          <motion.div
            key="tasks"
            initial={{ opacity: 0, x: 24 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: 24 }}
            transition={{ duration: reduceMotion ? 0 : 0.22 }}
            className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden"
          >
            <ScrollArea className="min-h-0 flex-1">
              <div className="space-y-3 pr-2">
                {tasks.length === 0 ? (
                  <div className="rounded-md border border-dashed px-3 py-16 text-center text-xs text-muted-foreground">
                    暂无任务，去「洗稿操作」粘贴链接后点击「开始分析」
                  </div>
                ) : (
                  <div className="space-y-3">
                    <div className="flex flex-wrap items-center gap-3 text-sm">
                      <span className="font-medium">任务列表</span>
                      <span className="text-xs text-muted-foreground">
                        {summary
                          ? `共 ${summary.total} · 成功 ${summary.done} · 失败 ${summary.failed}`
                          : `共 ${tasks.length} 条，处理中…`}
                      </span>
                      {hasFinishedTasks && (
                        <Button
                          variant="outline"
                          size="sm"
                          className="ml-auto h-7 gap-1 text-[11px] text-destructive"
                          onClick={() => void clearTasks()}
                        >
                          清空列表
                        </Button>
                      )}
                    </div>
                    {tasks.map((t) => (
                      <TaskCard
                        key={t.key}
                        task={t}
                        onDelete={() => deleteTask(t.key || t.url)}
                        onRetry={() => void retryTask(t)}
                        onRerun={() => void rerunRewrite(t)}
                      />
                    ))}
                  </div>
                )}
              </div>
            </ScrollArea>
          </motion.div>
        )}
      </AnimatePresence>

      <AiRewriteSettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
      />

      {/* 被扔进任务列表的小人：从「开始分析」按钮沿抛物线飞向任务列表 Tab */}
      {flyer &&
        createPortal(
          <motion.div
            key={flyer.key}
            className="pointer-events-none fixed left-0 top-0 z-[9999] text-primary drop-shadow-md"
            initial={{
              x: flyer.x0,
              y: flyer.y0,
              scale: 1,
              rotate: 0,
              opacity: 1,
            }}
            animate={{
              x: flyer.x1,
              y: [flyer.y0, Math.min(flyer.y0, flyer.y1) - 70, flyer.y1],
              scale: [1, 0.9, 0.45],
              rotate: [0, 500],
              opacity: [1, 1, 0],
            }}
            transition={{
              duration: 0.85,
              ease: "linear",
              y: { times: [0, 0.42, 1], ease: ["easeOut", "easeIn"] },
              scale: { times: [0, 0.42, 1] },
              opacity: { times: [0, 0.72, 1] },
            }}
            onAnimationComplete={() => setFlyer(null)}
          >
            <svg
              width="28"
              height="28"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden
            >
              <circle
                cx="12"
                cy="4.6"
                r="2.4"
                fill="currentColor"
                stroke="none"
              />
              <path d="M12 9.5v5.5" />
              <path d="M12 11 7.2 7.8" />
              <path d="M12 11l4.8-3.2" />
              <path d="M12 15 8.6 20.5" />
              <path d="M12 15l3.4 5.5" />
            </svg>
          </motion.div>,
          document.body,
        )}
    </div>
  );
}

/* ============ 任务卡片（阶段步骤条 + 进度/速度 + 日志 + 结果） ============ */

function TaskCard({
  task,
  onDelete,
  onRetry,
  onRerun,
}: {
  task: TaskItem;
  onDelete?: () => void;
  onRetry?: () => void;
  onRerun?: () => void;
}) {
  const [showLogs, setShowLogs] = useState(false);
  const order = stageOrder(task.stage);
  const isDone = task.status === "done";
  const isError = task.status === "error";
  const isInterrupted = task.status === "interrupted";

  const scoreVal = (() => {
    const s = (task.scoreJson ?? task.result?.score_json ?? {}) as Record<
      string,
      unknown
    >;
    return typeof s.score === "number" ? s.score : undefined;
  })();

  const detailData: DetailData = {
    title: task.result?.title ?? task.title,
    platform: task.result?.platform ?? task.platform,
    transcript:
      task.transcript ?? (task.result?.transcript as string | undefined),
    transcriptSegments:
      task.transcriptSegments ?? task.result?.transcript_segments,
    analyze: task.analyze ?? task.result?.analyze,
    rewriteRef:
      task.rewriteRef ??
      (task.result?.rewrite_ref as DetailData["rewriteRef"] | undefined),
    transcriptZh:
      task.transcriptZh ?? (task.result?.transcript_zh as string | undefined),
    rewrite: task.rewrite ?? task.result?.rewrite,
    score: task.scoreJson ?? task.result?.score_json,
  };
  const hasDetail =
    task.transcript !== undefined ||
    task.analyze !== undefined ||
    task.rewrite !== undefined ||
    task.scoreJson !== undefined ||
    !!task.result;

  const badge = isDone
    ? {
        text: "完成",
        cls: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
      }
    : isInterrupted
      ? { text: "中断", cls: "bg-amber-500/10 text-amber-600" }
      : isError
        ? { text: "失败", cls: "bg-destructive/10 text-destructive" }
        : {
            text: STAGE_LABEL[task.stage] ?? "处理中",
            cls: "bg-primary/10 text-primary",
          };

  return (
    <div className="rounded-lg border bg-card p-4">
      {/* 头部 */}
      <div className="flex items-center gap-2">
        {isError || isInterrupted ? (
          <LuCircleX
            className={cn(
              "size-4 shrink-0",
              isInterrupted ? "text-amber-500" : "text-destructive",
            )}
          />
        ) : isDone ? (
          <LuCircleCheck className="size-4 shrink-0 text-emerald-500" />
        ) : (
          <LuLoader className="size-4 shrink-0 animate-spin text-primary" />
        )}
        <span
          className="min-w-0 flex-1 truncate text-sm font-medium"
          title={task.title}
        >
          {task.title || task.url}
        </span>
        {task.platform && (
          <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
            {platformLabel(task.platform)}
          </span>
        )}
        {scoreVal !== undefined && (
          <span
            className={cn(
              "shrink-0 text-xs font-semibold",
              scoreColor(scoreVal),
            )}
          >
            {scoreVal} 分
          </span>
        )}
        <span
          className={cn(
            "shrink-0 rounded px-1.5 py-0.5 text-[10px]",
            badge.cls,
          )}
        >
          {badge.text}
        </span>
        {(isDone || isInterrupted || isError) && onRerun && (
          <button
            type="button"
            className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary"
            title="重新洗稿：重跑爆点分析 / 洗稿 / 打分（转写复用，最终结果覆盖旧结果）"
            onClick={() => void onRerun()}
          >
            <LuRefreshCw className="size-3.5" />
          </button>
        )}
        {(isError || isInterrupted) && onRetry && (
          <button
            type="button"
            className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary"
            title={`从失败处重试（已完成步骤自动复用，不重复计费）`}
            onClick={() => void onRetry()}
          >
            <LuRotateCcw className="size-3.5" />
          </button>
        )}
        {onDelete && (
          <button
            type="button"
            className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
            title="删除此任务"
            onClick={() => void onDelete()}
          >
            <LuTrash2 className="size-3.5" />
          </button>
        )}
      </div>

      {/* 步骤条 */}
      <div className="mt-3 flex items-center">
        {PIPELINE_STAGES.map((s, i) => {
          const stepDone = order > i;
          const active = order === i && !isDone && !isError;
          const failedHere = isError && task.stage === s.key;
          return (
            <div
              key={s.key}
              className="flex flex-1 items-center last:flex-none"
            >
              {failedHere && onRetry ? (
                <button
                  type="button"
                  onClick={() => void onRetry()}
                  title="点击重试这一步（已完成步骤自动复用，不重复计费）"
                  className="flex shrink-0 cursor-pointer items-center gap-1 rounded bg-destructive/10 px-1.5 py-0.5 text-[10px] font-medium text-destructive transition-colors hover:bg-destructive/20"
                >
                  <LuCircleX className="size-3" />
                  {s.label}
                  <LuRotateCcw className="size-3" />
                </button>
              ) : (
                <div
                  className={cn(
                    "flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[10px]",
                    stepDone
                      ? "text-emerald-600 dark:text-emerald-400"
                      : active
                        ? "bg-primary/10 font-medium text-primary"
                        : "text-muted-foreground",
                  )}
                >
                  {stepDone ? (
                    <LuCircleCheck className="size-3" />
                  ) : (
                    <span className="size-1.5 rounded-full bg-current" />
                  )}
                  {s.label}
                </div>
              )}
              {i < PIPELINE_STAGES.length - 1 && (
                <div
                  className={cn(
                    "mx-1 h-px flex-1",
                    stepDone ? "bg-emerald-500/50" : "bg-border",
                  )}
                />
              )}
            </div>
          );
        })}
      </div>

      {/* 进度条 */}
      <div className="mt-2 flex items-center gap-2">
        <Progress value={task.percent} className="h-1.5 flex-1" />
        <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
          {Math.round(task.percent)}%
        </span>
      </div>

      {/* 当前状态 + 速度 + 日志开关 */}
      <div className="mt-1.5 flex items-center gap-2 text-xs">
        <span
          className={cn(isError ? "text-destructive" : "text-muted-foreground")}
        >
          {task.message}
        </span>
        {task.speedText && (
          <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-foreground">
            {task.speedText}
          </span>
        )}
        <button
          type="button"
          className="ml-auto shrink-0 text-[10px] text-primary hover:underline"
          onClick={() => setShowLogs((v) => !v)}
        >
          {showLogs ? "收起日志" : `日志 (${task.logs.length})`}
        </button>
      </div>

      {/* 日志 */}
      {showLogs && (
        <div className="mt-2 max-h-48 overflow-auto rounded-md bg-muted/50 p-2 font-mono text-[10px] leading-relaxed">
          {task.logs.length === 0 ? (
            <div className="text-muted-foreground">暂无日志</div>
          ) : (
            task.logs.map((l, i) => (
              <div
                key={i}
                className={cn(
                  "whitespace-pre-wrap break-all",
                  l.level === "error"
                    ? "text-destructive"
                    : l.level === "warn"
                      ? "text-orange-500"
                      : l.level === "debug"
                        ? "text-muted-foreground"
                        : "text-foreground",
                )}
              >
                <span className="text-muted-foreground">{l.time} </span>
                {l.message}
              </div>
            ))
          )}
        </div>
      )}

      {/* 产物：走到哪一步就展示哪一步（转写/分析/洗稿/打分逐步出现） */}
      {hasDetail && (
        <div className="mt-3 border-t pt-3">
          {task.result?.reused && (
            <div className="mb-2 inline-flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
              <LuHistory className="size-3" />
              复用历史结果（未重复计费）
            </div>
          )}
          {!isDone && !isError && (
            <div className="mb-2 inline-flex items-center gap-1 text-[10px] text-primary">
              <LuLoader className="size-3 animate-spin" />
              实时生成中，内容会逐步补全
            </div>
          )}
          <DetailView data={detailData} defaultOpen />
        </div>
      )}
    </div>
  );
}

/* ============ 详情展示（转写 / 爆点分析 / 洗稿文案 / 打分） ============ */

function DetailView({
  data,
  defaultOpen = false,
}: {
  data: DetailData;
  defaultOpen?: boolean;
}) {
  const analyze = (data.analyze ?? {}) as Record<string, unknown>;
  const rewrite = (data.rewrite ?? {}) as Record<string, unknown>;
  const score = (data.score ?? {}) as Record<string, unknown>;
  const [showZh, setShowZh] = useState(false);

  const structure = asArray(analyze.structure) as Record<string, unknown>[];
  const points = toList(analyze.points);
  const keywords = toList(analyze.keywords);
  const authorship = analyze.authorship as Record<string, unknown> | undefined;
  const authorshipReasons = asArray(authorship?.reasons);
  const body = bodyParas(rewrite.body);
  const tags = toList(rewrite.tags);
  const kept = toList(rewrite.kept_points);
  const borrowed = toList(rewrite.borrowed);
  const dimensions = asArray(score.dimensions) as Record<string, unknown>[];
  const suggestions = toList(score.suggestions);
  const scoreVal =
    typeof score.score === "number" ? (score.score as number) : undefined;

  const fullText = [rewrite.hook, ...body, rewrite.cta]
    .filter(Boolean)
    .join("\n");

  return (
    <details open={defaultOpen} className="group space-y-3">
      <summary className="flex cursor-pointer list-none items-center gap-2 text-sm font-medium">
        <span className="min-w-0 truncate">{data.title || "（无标题）"}</span>
        <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
          {platformLabel(data.platform)}
        </span>
        {data.play_count !== undefined && (
          <span className="shrink-0 text-[10px] text-muted-foreground">
            ▶ {formatCount(data.play_count)}
          </span>
        )}
        {scoreVal !== undefined && (
          <span
            className={cn(
              "shrink-0 text-xs font-semibold",
              scoreColor(scoreVal),
            )}
          >
            {scoreVal} 分
          </span>
        )}
        <span className="ml-auto shrink-0 text-[10px] text-muted-foreground group-open:hidden">
          展开
        </span>
        <span className="ml-auto hidden shrink-0 text-[10px] text-muted-foreground group-open:inline">
          收起
        </span>
      </summary>

      <div className="space-y-3">
        {/* 原始信息 */}
        <div className="flex flex-wrap items-center gap-3 text-[11px] text-muted-foreground">
          {data.created_at && <span>分析时间：{data.created_at}</span>}
          {data.source_url && (
            <button
              type="button"
              className="flex items-center gap-1 text-primary hover:underline"
              onClick={() => void openUrl(data.source_url as string)}
            >
              <LuExternalLink className="size-3" />
              原视频链接
            </button>
          )}
        </div>

        {/* 数据链路：每个环节的输入来自哪里 */}
        <div className="rounded-md border border-border/60 bg-muted/30 px-3 py-2 text-[11px] leading-relaxed text-muted-foreground">
          <div className="mb-0.5 font-medium text-foreground">数据链路</div>
          <div>
            ① 转写文案 ← 平台原生字幕（时间轴对齐）/ 语音识别； ② 爆点分析 ←
            转写文案 + 播放量/发布时间（库内缺失时实时拉取，按次计费）； ③ 洗稿
            ← 爆点分析 + 转写文案 + 爆款参考；④ 评分 ← 洗稿文案。
          </div>
        </div>

        <Section title="① 转写文案" tone="slate">
          {data.transcript ? (
            <div className="space-y-2">
              <p className="whitespace-pre-wrap text-xs leading-relaxed">
                {data.transcript}
              </p>
              {data.transcriptZh && (
                <div>
                  <button
                    type="button"
                    className="text-[11px] text-primary hover:underline"
                    onClick={() => setShowZh((v) => !v)}
                  >
                    {showZh ? "收起中文翻译" : "展开中文翻译"}
                  </button>
                  {showZh && (
                    <div className="mt-1.5 rounded-md bg-muted/50 px-2.5 py-2">
                      <p className="whitespace-pre-wrap text-xs leading-relaxed text-foreground">
                        {data.transcriptZh}
                      </p>
                    </div>
                  )}
                </div>
              )}
            </div>
          ) : (
            <Empty />
          )}
        </Section>

        <Section title="② 爆点分析" tone="violet">
          <div className="space-y-2 text-xs">
            <KV k="作品" v={analyze.work} />
            <KV
              k="集数"
              v={
                typeof analyze.episode === "number" && analyze.episode > 0
                  ? `第 ${analyze.episode} 集`
                  : ""
              }
            />
            <KV k="赛道" v={analyze.category} />
            <KV k="开头钩子" v={analyze.hook} />
            <KV k="主导情绪" v={analyze.emotion} />
            <KV k="可复用套路" v={analyze.reusable} />
            <KV k="一句话总结" v={analyze.summary} />
            {typeof authorship?.verdict === "string" && (
              <div>
                <div className="flex items-center gap-1.5">
                  <span className="text-muted-foreground">
                    原稿来源倾向（仅供参考）：
                  </span>
                  <span
                    className={cn(
                      "font-medium",
                      authorship.verdict === "ai"
                        ? "text-warning"
                        : authorship.verdict === "human"
                          ? "text-success"
                          : "text-muted-foreground",
                    )}
                  >
                    {authorship.verdict === "ai"
                      ? "AI 痕迹偏重"
                      : authorship.verdict === "human"
                        ? "人工痕迹偏重"
                        : "无法判断"}
                  </span>
                </div>
                {authorshipReasons.length > 0 && (
                  <ul className="mt-1 list-disc space-y-0.5 pl-4">
                    {authorshipReasons.map((r, i) => (
                      <li key={i}>{String(r)}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}
            {keywords.length > 0 && (
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="text-muted-foreground">关键词：</span>
                {keywords.map((k, i) => (
                  <span
                    key={i}
                    className="rounded bg-muted px-1.5 py-0.5 text-[10px]"
                  >
                    {k}
                  </span>
                ))}
              </div>
            )}
            {structure.length > 0 && (
              <div>
                <div className="mb-1 text-muted-foreground">结构拆解：</div>
                <ol className="space-y-1">
                  {structure.map((s, i) => (
                    <li key={i} className="flex gap-2">
                      <span className="shrink-0 font-medium text-primary">
                        {String(s.stage ?? "")}
                      </span>
                      <span>{String(s.desc ?? "")}</span>
                    </li>
                  ))}
                </ol>
              </div>
            )}
            {points.length > 0 && (
              <div>
                <div className="mb-1 text-muted-foreground">爆点清单：</div>
                <ul className="list-disc space-y-1 pl-4">
                  {points.map((p, i) => (
                    <li key={i}>{p}</li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        </Section>

        <Section title="③ 洗稿文案" tone="blue">
          <div className="space-y-2 text-xs">
            {data.rewriteRef?.label || data.rewriteRef?.points ? (
              <div className="rounded bg-blue-500/10 px-2 py-1.5 text-[11px] text-foreground">
                <div>
                  本次参考网感（公司爆点库）：
                  {data.rewriteRef.label || "已注入"}
                  —— 爆点库随每次分析/评分自动积累
                </div>
                {(() => {
                  const pts = data.rewriteRef.points ?? {};
                  const layers = (
                    Object.keys(REF_LAYER_LABEL) as (keyof RefPoints)[]
                  ).filter((k) => (pts[k] ?? []).length > 0);
                  if (layers.length === 0) return null;
                  return (
                    <details className="mt-1">
                      <summary className="cursor-pointer select-none text-muted-foreground hover:text-foreground">
                        查看参考的爆点库内容
                      </summary>
                      <div className="mt-1 space-y-1.5">
                        {layers.map((k) => (
                          <div key={k}>
                            <div className="font-medium text-muted-foreground">
                              {REF_LAYER_LABEL[k]}
                            </div>
                            <ul className="list-disc space-y-0.5 pl-4">
                              {(pts[k] ?? []).map((line, i) => (
                                <li key={i}>{line}</li>
                              ))}
                            </ul>
                          </div>
                        ))}
                      </div>
                    </details>
                  );
                })()}
              </div>
            ) : (
              <div className="text-[11px] text-muted-foreground">
                本次未注入参考爆点（爆点库为空或拉取失败）
              </div>
            )}
            {borrowed.length > 0 && (
              <div>
                <div className="mb-1 text-muted-foreground">
                  本文借鉴的网感点：
                </div>
                <ul className="list-disc space-y-0.5 pl-4">
                  {borrowed.map((b, i) => (
                    <li key={i}>{b}</li>
                  ))}
                </ul>
              </div>
            )}
            <KV k="标题" v={rewrite.title} />
            <KV k="前3秒钩子" v={rewrite.hook} />
            {body.length > 0 && (
              <div>
                <div className="mb-1 text-muted-foreground">正文：</div>
                <div className="space-y-1.5">
                  {body.map((p, i) => (
                    <p key={i} className="whitespace-pre-wrap leading-relaxed">
                      {p}
                    </p>
                  ))}
                </div>
              </div>
            )}
            <KV k="结尾引导" v={rewrite.cta} />
            {kept.length > 0 && (
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="text-muted-foreground">保留爆点：</span>
                {kept.map((k, i) => (
                  <span
                    key={i}
                    className="rounded bg-muted px-1.5 py-0.5 text-[10px]"
                  >
                    {k}
                  </span>
                ))}
              </div>
            )}
            {tags.length > 0 && (
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="text-muted-foreground">标签：</span>
                {tags.map((t, i) => (
                  <span
                    key={i}
                    className="rounded bg-muted px-1.5 py-0.5 text-[10px]"
                  >
                    {t}
                  </span>
                ))}
              </div>
            )}
            {(fullText || data.transcript) && (
              <div className="mt-1 flex flex-wrap gap-2">
                {fullText && (
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-7 gap-1.5 text-[11px]"
                    onClick={() => void copyRewrite(fullText)}
                  >
                    <LuCopy className="size-3.5" />
                    复制洗稿文稿
                  </Button>
                )}
                {data.transcript && (
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-7 gap-1.5 text-[11px]"
                    onClick={() =>
                      void copyRewrite(data.transcript!, "转写文案")
                    }
                  >
                    <LuCopy className="size-3.5" />
                    复制转写文案
                  </Button>
                )}
                <Button
                  variant="outline"
                  size="sm"
                  className="h-7 gap-1.5 text-[11px]"
                  onClick={() =>
                    void exportSrt({
                      lang: "en",
                      rewrite: data.rewrite,
                      transcript: data.transcript,
                      segments: data.transcriptSegments,
                      title: data.title,
                    })
                  }
                >
                  <LuFileDown className="size-3.5" />
                  导出英文字幕(SRT)
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-7 gap-1.5 text-[11px]"
                  onClick={() =>
                    void exportSrt({
                      lang: "zh",
                      rewrite: data.rewrite,
                      transcript: data.transcript,
                      segments: data.transcriptSegments,
                      title: data.title,
                    })
                  }
                >
                  <LuFileDown className="size-3.5" />
                  导出中文字幕(SRT)
                </Button>
              </div>
            )}
          </div>
        </Section>

        <Section title="④ 文案打分" tone="amber">
          {scoreVal === undefined && dimensions.length === 0 ? (
            <Empty />
          ) : (
            <div className="space-y-3 text-xs">
              <div className="flex items-center gap-3">
                <span
                  className={cn("text-2xl font-bold", scoreColor(scoreVal))}
                >
                  {scoreVal ?? "—"}
                </span>
                {typeof score.level === "string" && (
                  <Badge variant="secondary" className="text-[10px]">
                    {score.level}
                  </Badge>
                )}
              </div>
              {typeof score.summary === "string" && score.summary !== "" && (
                <p className="text-muted-foreground">{score.summary}</p>
              )}
              {dimensions.length > 0 && (
                <div className="space-y-2">
                  {dimensions.map((d, i) => {
                    const ds =
                      typeof d.score === "number" ? (d.score as number) : 0;
                    return (
                      <div key={i} className="space-y-1">
                        <div className="flex items-center justify-between">
                          <span>{String(d.name ?? "")}</span>
                          <span className={cn("font-medium", scoreColor(ds))}>
                            {ds}
                          </span>
                        </div>
                        <Progress value={ds} className="h-1.5" />
                        {typeof d.comment === "string" && d.comment !== "" && (
                          <div className="text-[10px] text-muted-foreground">
                            {d.comment}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
              {suggestions.length > 0 && (
                <div>
                  <div className="mb-1 text-muted-foreground">改进建议：</div>
                  <ul className="list-disc space-y-1 pl-4">
                    {suggestions.map((s, i) => (
                      <li key={i}>{s}</li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          )}
        </Section>
      </div>
    </details>
  );
}

function Section({
  title,
  tone,
  children,
}: {
  title: string;
  tone: "slate" | "violet" | "blue" | "amber";
  children: ReactNode;
}) {
  const toneClass: Record<string, string> = {
    slate: "border-l-slate-400",
    violet: "border-l-violet-400",
    blue: "border-l-blue-400",
    amber: "border-l-amber-400",
  };
  return (
    <div
      className={cn(
        "rounded-md border border-l-4 bg-background/60 px-3 py-2",
        toneClass[tone],
      )}
    >
      <div className="mb-1.5 text-xs font-semibold">{title}</div>
      {children}
    </div>
  );
}

function KV({ k, v }: { k: string; v: unknown }) {
  const text =
    typeof v === "string" ? v : v === undefined || v === null ? "" : String(v);
  if (!text) return null;
  return (
    <div className="flex gap-2">
      <span className="shrink-0 text-muted-foreground">{k}：</span>
      <span className="whitespace-pre-wrap">{text}</span>
    </div>
  );
}

function Empty() {
  return <span className="text-xs text-muted-foreground">（无）</span>;
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
        active
          ? "bg-primary/10 text-primary"
          : "text-muted-foreground hover:bg-muted",
      )}
    >
      {children}
    </button>
  );
}

function StepNo({ n }: { n: number }) {
  return (
    <span className="grid size-5 place-items-center rounded-full bg-primary/10 text-[11px] font-semibold text-primary">
      {n}
    </span>
  );
}

function StatusBadge({ ok, label }: { ok: boolean; label: string }) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md px-2 py-1",
        ok
          ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
          : "bg-destructive/10 text-destructive",
      )}
    >
      <span
        className={cn(
          "size-1.5 rounded-full",
          ok ? "bg-emerald-500" : "bg-destructive",
        )}
      />
      {label}
    </span>
  );
}
