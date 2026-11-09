// video-tools-copywriting.tsx - 文案：视频/链接转录出文案 + 逐行中文翻译
//
// 布局（上下两个功能区，均为左原文 / 中翻译按钮 / 右译文）：
//   上：转录区 —— 拉入视频或粘贴链接 → 转录文案（每段一行）→ 翻译 → 译文
//   下：自定义区 —— 手动粘贴文案（每行一段）→ 翻译 → 中文翻译
//
// 转录复用 AI 洗稿流水线的 transcript 模式（ai_rewrite_run_links + mode=transcript），
// 翻译复用 AI 洗稿的模型配置（ai_rewrite_translate_lines，行数一一对应）。
//
// 人工审核支持：
//   - 逐行编辑（行编辑器 LineListEditor）：单击联动高亮、双击行内编辑、多行粘贴自动分行、
//     上下键移动选中、hover 删除行、底部添加一行
//   - 左右两侧共享同一 activeIndex：点左侧某行，右侧对应译文同步亮起
//   - 数据持久化：copywriting_load_state / copywriting_save_state（按登录账号隔离），800ms 防抖
//   - 宽滚动条：滚动容器套 .wide-scrollbar（覆盖全局 thin 样式）

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  LuArrowRight,
  LuBookMarked,
  LuChevronDown,
  LuClipboardList,
  LuCopy,
  LuFileDown,
  LuImport,
  LuLanguages,
  LuLink,
  LuLoaderCircle,
  LuPaperclip,
  LuPlus,
  LuReplace,
  LuSearch,
  LuTrash2,
  LuX,
} from "react-icons/lu";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

const VIDEO_EXT = /\.(mp4|mkv|webm|mov|avi|flv|ts|m4v|wmv|3gp|mp3|m4a|wav)$/i;

interface TransProgress {
  percent: number;
  message: string;
}

interface RunResult {
  source_url?: string;
  success?: boolean;
  error?: string;
  transcript?: string;
  transcript_segments?: unknown;
}

/** 解说词出处识别结果（copywriting_identify_source 返回） */
interface IdentifySourceResult {
  found: boolean;
  type: string;
  title: string;
  season: number;
  episode: number;
  /** 该集核心剧情段落概括（动漫/剧集给出具体集数时），非剧集或缺集数时为空 */
  breakdown?: string;
  evidence: string;
  confidence: number;
  raw?: string;
}

/** 把转录结果（segments 或整段文本）拆成每段一行 */
function transcriptToLines(r: RunResult): string[] {
  const segs = Array.isArray(r.transcript_segments)
    ? r.transcript_segments
    : [];
  const fromSegs = segs
    .map((s) => {
      const t =
        typeof s === "object" && s !== null
          ? (s as Record<string, unknown>).text
          : s;
      return String(t ?? "")
        .replace(/\s+/g, " ")
        .trim();
    })
    .filter(Boolean);
  if (fromSegs.length > 0) return fromSegs;
  return String(r.transcript ?? "")
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter(Boolean);
}

/** 出处识别：来源类型 → 中文标签 */
function typeLabel(t: string): string {
  switch (t) {
    case "movie":
      return "电影";
    case "tv":
      return "剧集";
    case "anime":
      return "动漫";
    default:
      return "";
  }
}

export function QuickCopywritingTab() {
  // ---- 转录区状态（行数组）----
  const [linkInput, setLinkInput] = useState("");
  const [sources, setSources] = useState<string[]>([]);
  const [transcribing, setTranscribing] = useState(false);
  const [progress, setProgress] = useState<TransProgress | null>(null);
  const [transcript, setTranscript] = useState<string[]>([]);
  const [transcriptZh, setTranscriptZh] = useState<string[]>([]);

  // ---- 自定义文案区状态（行数组）----
  const [custom, setCustom] = useState<string[]>([]);
  const [customZh, setCustomZh] = useState<string[]>([]);

  // ---- 翻译状态 ----
  const [translatingTop, setTranslatingTop] = useState(false);
  const [translatingBottom, setTranslatingBottom] = useState(false);
  /** 翻译真实进度：{done, total}，null = 无（后端逐行完成事件驱动） */
  const [translateProgress, setTranslateProgress] = useState<{
    done: number;
    total: number;
  } | null>(null);
  /** 左右两栏行文字字号（px），中间列 A-/A+ 调节，持久化 */
  const [rowFontSize, setRowFontSize] = useState(12);
  const [dragOver, setDragOver] = useState(false);

  // ---- 底部功能区（自定义文案/中文翻译）默认收起，点「+」展开 ----
  const [showCustomLeft, setShowCustomLeft] = useState(false);
  const [showCustomRight, setShowCustomRight] = useState(false);

  // ---- 翻译目标语言 + 不可译词高亮（行号 → 词列表） ----
  const [topTargetLang, setTopTargetLang] = useState("zh");
  const [bottomTargetLang, setBottomTargetLang] = useState("zh");
  const [transcriptUntranslatable, setTranscriptUntranslatable] = useState<
    Record<number, string[]>
  >({});
  const [customUntranslatable, setCustomUntranslatable] = useState<
    Record<number, string[]>
  >({});

  // ---- 不可译词表管理对话框 ----
  const [termsOpen, setTermsOpen] = useState(false);
  const [terms, setTerms] = useState<GlossaryEntry[]>([]);
  const [newTerm, setNewTerm] = useState("");
  const [newTermZh, setNewTermZh] = useState("");
  /** 正处于"编辑对照译文"状态的原文词（null = 无） */
  const [editingTerm, setEditingTerm] = useState<string | null>(null);
  const [editZhValue, setEditZhValue] = useState("");
  /** 词表搜索关键词（过滤原文/对照译文，大小写不敏感） */
  const [termSearch, setTermSearch] = useState("");
  /** 批量粘贴区：展开状态 + 文本 */
  const [batchOpen, setBatchOpen] = useState(false);
  const [batchText, setBatchText] = useState("");

  // ---- 解说词出处识别（转录文案 → 影视/动漫出处）----
  const [identifying, setIdentifying] = useState(false);
  const [identifyResult, setIdentifyResult] =
    useState<IdentifySourceResult | null>(null);
  const [identifyOpen, setIdentifyOpen] = useState(false);
  /** 识别已等待秒数（仅识别中计时，用户可感知“还没卡住”） */
  const [identifyWaitSec, setIdentifyWaitSec] = useState(0);

  // 识别中每秒刷新已等待秒数；非识别中立即归零
  useEffect(() => {
    if (!identifying) {
      setIdentifyWaitSec(0);
      return;
    }
    setIdentifyWaitSec(0);
    const t = setInterval(() => {
      setIdentifyWaitSec((s) => s + 1);
    }, 1000);
    return () => clearInterval(t);
  }, [identifying]);

  // ---- 持久化：注入状态 + 防抖保存 ----
  const loadedRef = useRef(false);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 翻译实时上屏上下文：当前正在翻译的行映射 + 译文 setter（进度事件驱动） */
  const translateCtxRef = useRef<{
    idxMap: number[];
    setOut: React.Dispatch<React.SetStateAction<string[]>>;
  } | null>(null);

  useEffect(() => {
    invoke<{
      transcript?: string[];
      transcriptZh?: string[];
      custom?: string[];
      customZh?: string[];
      transcriptUntranslatable?: Record<string, string[]>;
      customUntranslatable?: Record<string, string[]>;
      rowFontSize?: number;
    }>("copywriting_load_state", {})
      .then((res) => {
        const d = res ?? {};
        if (Array.isArray(d.transcript)) setTranscript(d.transcript);
        if (Array.isArray(d.transcriptZh)) setTranscriptZh(d.transcriptZh);
        if (Array.isArray(d.custom)) setCustom(d.custom);
        if (Array.isArray(d.customZh)) setCustomZh(d.customZh);
        if (d.transcriptUntranslatable)
          setTranscriptUntranslatable(
            normalizeHighlight(d.transcriptUntranslatable),
          );
        if (d.customUntranslatable)
          setCustomUntranslatable(normalizeHighlight(d.customUntranslatable));
        if (
          typeof d.rowFontSize === "number" &&
          d.rowFontSize >= 10 &&
          d.rowFontSize <= 22
        ) {
          setRowFontSize(d.rowFontSize);
        }
      })
      .catch(() => {
        // 文件缺失或损坏：忽略，按全新状态开始
      })
      .finally(() => {
        loadedRef.current = true;
      });
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
    };
  }, []);

  useEffect(() => {
    if (!loadedRef.current) return;
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      invoke("copywriting_save_state", {
        state: {
          transcript,
          transcriptZh,
          custom,
          customZh,
          transcriptUntranslatable,
          customUntranslatable,
          rowFontSize,
        },
      }).catch(() => {
        // 保存失败不打断编辑，下次变更会重试
      });
    }, 800);
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
    };
  }, [
    transcript,
    transcriptZh,
    custom,
    customZh,
    transcriptUntranslatable,
    customUntranslatable,
    rowFontSize,
  ]);

  // ---- 添加来源（链接 or 本地路径）----
  const addSources = useCallback((list: string[]) => {
    setSources((prev) => {
      const next = [...prev];
      for (const s of list) {
        const v = s.trim();
        if (v && !next.includes(v)) next.push(v);
      }
      return next;
    });
  }, []);

  const addLink = () => {
    const v = linkInput.trim();
    if (!v) return;
    addSources([v]);
    setLinkInput("");
  };

  const pickFiles = async () => {
    const selected = await open({
      multiple: true,
      filters: [
        {
          name: "Video / Audio",
          extensions: [
            "mp4",
            "mkv",
            "webm",
            "mov",
            "avi",
            "flv",
            "ts",
            "m4v",
            "wmv",
            "3gp",
            "mp3",
            "m4a",
            "wav",
          ],
        },
      ],
    });
    if (!selected) return;
    const list = Array.isArray(selected) ? selected : [selected];
    addSources(list as string[]);
  };

  // ---- 转录（复用 AI 洗稿 transcript 模式）----
  const runTranscribe = async () => {
    const all = [...sources];
    const link = linkInput.trim();
    if (link) all.push(link);
    if (all.length === 0) {
      toast.error("请先拉入视频文件或粘贴视频链接");
      return;
    }
    setTranscribing(true);
    setProgress({ percent: 0, message: "准备中…" });
    try {
      const res = await invoke<{ results?: RunResult[] }>(
        "ai_rewrite_run_links",
        {
          items: all.map((source_url) => ({
            source_url,
            mode: "transcript",
            title: "",
          })),
          wait: true,
        },
      );
      const results = res?.results ?? [];

      // 按加入顺序拼接：精确匹配 source_url；本地文件夹由后端展开成文件路径，
      // 用「路径前缀」归到文件夹来源下；剩余未匹配的追加在最后。
      const used = new Set<number>();
      const lines: string[] = [];
      const errors: string[] = [];
      const collect = (matchFn: (r: RunResult, i: number) => boolean) => {
        results.forEach((r, i) => {
          if (used.has(i) || !matchFn(r, i)) return;
          used.add(i);
          if (!r.success) {
            errors.push(
              `${shortName(r.source_url ?? "")}：${r.error ?? "转录失败"}`,
            );
            return;
          }
          lines.push(...transcriptToLines(r));
        });
      };
      for (const src of all) {
        const isDir = !VIDEO_EXT.test(src);
        collect((r) => {
          const u = (r.source_url ?? "").trim();
          return (
            u === src || (isDir && u.startsWith(src.replace(/[\\/]+$/, "")))
          );
        });
      }
      collect((_r, i) => !used.has(i));

      if (lines.length === 0) {
        throw new Error(
          errors.length > 0
            ? errors.join("；")
            : "未获得转录结果（视频可能没有可识别的语音）",
        );
      }
      setTranscript((prev) => [...prev, ...lines]);
      // 行号可能变化，重置旧的不可译词高亮
      setTranscriptUntranslatable({});
      if (errors.length > 0) {
        toast.warning(`部分转录失败：${errors.join("；")}`);
      } else {
        toast.success(`转录完成（${lines.length} 行）`);
      }
    } catch (e) {
      toast.error(`转录失败：${e}`);
    } finally {
      setTranscribing(false);
      setProgress(null);
    }
  };

  // ---- 转录进度事件 ----
  useEffect(() => {
    if (!transcribing) return;
    let un: (() => void) | undefined;
    listen<{ percent: number; message: string }>(
      "ai-rewrite:progress",
      (ev) => {
        setProgress({
          percent: ev.payload.percent ?? 0,
          message: ev.payload.message ?? "",
        });
      },
    ).then((fn) => (un = fn));
    return () => un?.();
  }, [transcribing]);

  // ---- 识别解说词出处（影视/动漫）----
  const identifySource = async () => {
    const lines = transcript.map((s) => s.trim()).filter((s) => s.length > 0);
    if (lines.length === 0) {
      toast.error("转录文案为空，无法识别出处");
      return;
    }
    setIdentifying(true);
    setIdentifyResult(null);
    setIdentifyOpen(true);
    try {
      const res = await invoke<IdentifySourceResult>(
        "copywriting_identify_source",
        { lines },
      );
      setIdentifyResult(res);
    } catch (e) {
      toast.error(`识别出处失败：${e}`);
      setIdentifyOpen(false);
    } finally {
      setIdentifying(false);
    }
  };

  // ---- 翻译进度事件（后端逐行完成时上报真实 done/total + 译文实时上屏） ----
  useEffect(() => {
    let un: (() => void) | undefined;
    listen<{
      done: number;
      total: number;
      line?: number;
      translation?: string;
    }>("copywriting:translate-progress", (ev) => {
      setTranslateProgress({
        done: ev.payload.done ?? 0,
        total: ev.payload.total ?? 0,
      });
      // 逐行实时上屏：后端行号 → 原始行号 → 更新对应译文行
      const ctx = translateCtxRef.current;
      const { line, translation } = ev.payload;
      if (!ctx || line === undefined || !translation) return;
      const orig = ctx.idxMap[line];
      if (orig === undefined) return;
      ctx.setOut((prev) => prev.map((x, i) => (i === orig ? translation : x)));
    }).then((fn) => (un = fn));
    return () => un?.();
  }, []);

  // ---- 拖放视频/文件夹（Tauri 原生拖放，整个 tab 区域可落）----
  useEffect(() => {
    let un: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "leave") {
          setDragOver(false);
          return;
        }
        if (p.type === "enter" || p.type === "over") {
          setDragOver(true);
          return;
        }
        setDragOver(false);
        const paths = p.paths.filter(
          (f) => !/\.[^./\\]+$/.test(f) || VIDEO_EXT.test(f),
        );
        if (paths.length === 0) return;
        addSources(paths);
        toast.info(`已添加 ${paths.length} 个视频/文件夹，点击「开始转录」`);
      })
      .then((fn) => (un = fn));
    return () => un?.();
  }, [addSources]);

  // ---- 逐行翻译（带目标语言选择与不可译词返回） ----
  const translateLines = async (
    source: string[],
    setOut: React.Dispatch<React.SetStateAction<string[]>>,
    setLoading: (v: boolean) => void,
    targetLang: string,
    setUntranslatable: (v: Record<number, string[]>) => void,
  ) => {
    // 过滤空行，同时记录 cleaned 索引 → 原始行索引的映射
    const idxMap: number[] = [];
    source.forEach((s, i) => {
      if (makeRowText(s).length > 0) idxMap.push(i);
    });
    if (idxMap.length === 0) {
      toast.error("左侧没有可翻译的内容");
      return;
    }
    setLoading(true);
    setTranslateProgress({ done: 0, total: idxMap.length });
    // 注册实时上屏上下文：进度事件里每翻完一行立即更新对应行
    translateCtxRef.current = { idxMap, setOut };
    try {
      const res = await invoke<{
        translations: string[];
        partial: boolean;
        note: string;
        untranslatable?: { line: number; term: string }[];
        new_terms?: string[];
      }>("ai_rewrite_translate_lines", {
        lines: idxMap.map((i) => makeRowText(source[i])),
        targetLang,
      });
      // cleaned 结果展开回原始行数组（空行保留空），行号严格对齐
      const translations = res.translations ?? [];
      const full: string[] = source.map((_, i) => {
        const ci = idxMap.indexOf(i);
        return ci >= 0 ? (translations[ci] ?? "") : "";
      });
      setOut(full);
      // 不可译词：cleaned 行号映射回原始行号
      const map: Record<number, string[]> = {};
      for (const it of res.untranslatable ?? []) {
        const orig = idxMap[it.line];
        if (orig === undefined) continue;
        (map[orig] ??= []).push(it.term);
      }
      // 对照译文高亮：已配置固定对照（t 非空）的词，若其对照译文出现在该行译文里则高亮
      try {
        const glossary = await invoke<GlossaryEntry[]>("translate_terms_list");
        const fixed = (glossary ?? []).filter((g) => g.t.length > 0);
        if (fixed.length > 0) {
          (res.translations ?? []).forEach((trans, idx) => {
            const orig = idxMap[idx];
            if (orig === undefined) return;
            for (const g of fixed) {
              if (trans.includes(g.t)) {
                (map[orig] ??= []).push(g.t);
              }
            }
          });
        }
      } catch {
        // 词表读取失败不影响翻译结果
      }
      setUntranslatable(map);
      if (res.new_terms && res.new_terms.length > 0) {
        toast.info(
          `AI 发现 ${res.new_terms.length} 个不可译词，已自动加入词表`,
        );
      }
      if (res.note) {
        toast.warning(res.note);
      } else {
        toast.success("翻译完成");
      }
    } catch (e) {
      toast.error(`翻译失败：${e}`);
    } finally {
      setLoading(false);
      setTranslateProgress(null);
      translateCtxRef.current = null;
    }
  };

  // ---- 不可译词表管理 ----
  const openTerms = useCallback(async () => {
    setTermsOpen(true);
    try {
      const list = await invoke<GlossaryEntry[]>("translate_terms_list");
      setTerms(list ?? []);
    } catch {
      setTerms([]);
    }
  }, []);

  const addTerm = async () => {
    const s = newTerm.trim();
    if (!s) return;
    try {
      const list = await invoke<GlossaryEntry[]>("translate_terms_add", {
        terms: [{ s, t: newTermZh.trim() }],
      });
      setTerms(list ?? []);
      setNewTerm("");
      setNewTermZh("");
    } catch (e) {
      toast.error(`添加失败：${e}`);
    }
  };

  const removeTerm = async (term: string) => {
    try {
      const list = await invoke<GlossaryEntry[]>("translate_terms_remove", {
        term,
      });
      setTerms(list ?? []);
    } catch (e) {
      toast.error(`删除失败：${e}`);
    }
  };

  /** 批量导入：每行一条，支持 `原文==对照` 或 `原文=对照` 或 `原文：对照` 或仅原文 */
  const batchAddTerms = async () => {
    const raw = batchText.trim();
    if (!raw) {
      toast.error("请先粘贴内容");
      return;
    }
    const entries: GlossaryEntry[] = [];
    for (const line of raw.split(/[\r\n]+/)) {
      const l = line.trim();
      if (!l) continue;
      // 分隔符：== 、=、：、:
      const m = l.match(/^(.*?)(?:==|=|：|:)(.*)$/);
      if (m) {
        const s = m[1].trim();
        const t = m[2].trim();
        if (s) entries.push({ s, t });
      } else if (l) {
        entries.push({ s: l, t: "" });
      }
    }
    if (entries.length === 0) {
      toast.error("没有可导入的词条");
      return;
    }
    try {
      const list = await invoke<GlossaryEntry[]>("translate_terms_add", {
        terms: entries,
      });
      setTerms(list ?? []);
      setBatchText("");
      toast.success(`已批量导入 ${entries.length} 条`);
    } catch (e) {
      toast.error(`批量导入失败：${e}`);
    }
  };

  /** 手动修改某个词条的对照译文 */
  const updateTermZh = async (s: string, zh: string) => {
    try {
      const list = await invoke<GlossaryEntry[]>("translate_terms_add", {
        terms: [{ s, t: zh }],
      });
      setTerms(list ?? []);
    } catch (e) {
      toast.error(`修改失败：${e}`);
    }
  };

  const copyText = async (text: string, label: string) => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      toast.success(`${label}已复制`);
    } catch {
      toast.error("复制失败");
    }
  };

  /**
   * 一键替换：把右侧译文里所有「已配置固定对照」的原文词替换成对照译文。
   * 同时把出现在译文里的对照译文加入高亮（保持高亮）。
   */
  const applyGlossaryReplace = async (
    trans: string[],
    setTrans: (v: string[]) => void,
    setHL: (v: Record<number, string[]>) => void,
  ) => {
    if (trans.every((t) => t.length === 0)) {
      toast.error("右侧没有译文可替换");
      return;
    }
    try {
      const glossary = await invoke<GlossaryEntry[]>("translate_terms_list");
      const fixed = (glossary ?? []).filter((g) => g.t.length > 0);
      if (fixed.length === 0) {
        toast.info("词表里还没有「原文↔对照」选项，先在词表里添加");
        return;
      }
      const newTrans = trans.map((line) => {
        let s = line;
        for (const g of fixed) {
          if (s.toLocaleLowerCase().includes(g.s.toLocaleLowerCase())) {
            s = replaceAllCaseInsensitive(s, g.s, g.t);
          }
        }
        return s;
      });
      setTrans(newTrans);
      // 高亮：对照译文出现在新译文行里的词加入高亮
      const hl: Record<number, string[]> = {};
      newTrans.forEach((line, idx) => {
        for (const g of fixed) {
          if (line.includes(g.t)) (hl[idx] ??= []).push(g.t);
        }
      });
      setHL(hl);
      toast.success(`已替换 ${fixed.length} 个对照词`);
    } catch (e) {
      toast.error(`替换失败：${e}`);
    }
  };

  const exportTxt = async (text: string, name: string) => {
    if (!text) return;
    const path = await save({
      defaultPath: `${name}.txt`,
      filters: [{ name: "Text", extensions: ["txt"] }],
    });
    if (!path) return;
    await writeTextFile(path, text);
    toast.success(`已导出：${path}`);
  };

  // 词表搜索过滤（原文/对照译文都参与匹配，大小写不敏感）
  const keyword = termSearch.trim().toLocaleLowerCase();
  const filteredTerms =
    keyword.length > 0
      ? terms.filter(
          (e) =>
            e.s.toLocaleLowerCase().includes(keyword) ||
            e.t.toLocaleLowerCase().includes(keyword),
        )
      : terms;

  return (
    <div
      className={cn(
        "flex h-full min-h-0 flex-col gap-3 rounded-lg border p-3",
        dragOver && "border-primary bg-primary/5 ring-2 ring-primary/30",
      )}
    >
      {/* 顶部：来源输入 */}
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <div className="relative min-w-56 flex-1">
          <LuLink className="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={linkInput}
            onChange={(e) => setLinkInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !transcribing) {
                addLink();
              }
            }}
            placeholder="粘贴视频链接（TikTok / YouTube / 抖音…），或把视频文件直接拖进来"
            className="pl-8"
            disabled={transcribing}
          />
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={addLink}
          disabled={transcribing || !linkInput.trim()}
        >
          <LuPlus className="size-4" />
          添加链接
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={pickFiles}
          disabled={transcribing}
        >
          <LuPaperclip className="size-4" />
          选视频
        </Button>
        <Button
          size="sm"
          onClick={runTranscribe}
          disabled={transcribing || (sources.length === 0 && !linkInput.trim())}
        >
          {transcribing ? (
            <LuLoaderCircle className="size-4 animate-spin" />
          ) : (
            <LuLanguages className="size-4" />
          )}
          {transcribing ? "转录中…" : "开始转录"}
        </Button>
        {sources.length > 0 && (
          <span className="text-xs text-muted-foreground">
            共 {sources.length} 个来源
          </span>
        )}
      </div>

      {/* 来源 chips */}
      {sources.length > 0 && (
        <div className="flex shrink-0 flex-wrap gap-1.5">
          {sources.map((s, i) => (
            <span
              key={s}
              className="inline-flex max-w-72 items-center gap-1 rounded-md border bg-muted/50 px-2 py-0.5 text-xs"
            >
              <span className="truncate" title={s}>
                {shortName(s)}
              </span>
              <button
                type="button"
                className="text-muted-foreground hover:text-foreground"
                onClick={() =>
                  setSources((prev) => prev.filter((_, idx) => idx !== i))
                }
                disabled={transcribing}
              >
                <LuX className="size-3" />
              </button>
            </span>
          ))}
          <button
            type="button"
            className="inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-xs text-muted-foreground hover:text-destructive"
            onClick={() => setSources([])}
            disabled={transcribing}
          >
            <LuTrash2 className="size-3" />
            清空
          </button>
        </div>
      )}

      {/* 转录进度 */}
      {transcribing && progress && (
        <div className="flex shrink-0 items-center gap-2">
          <Progress value={progress.percent} className="h-1.5 flex-1" />
          <span className="w-72 truncate text-xs text-muted-foreground">
            {progress.message}
          </span>
        </div>
      )}

      {/* 上：转录文案 / 译文 */}
      <PairEditor
        leftLabel="转录文案（每段一行）"
        leftValue={transcript}
        onLeftChange={setTranscript}
        rightLabel="译文"
        rightValue={transcriptZh}
        onRightChange={setTranscriptZh}
        translating={translatingTop}
        translateProgress={translateProgress}
        fontSize={rowFontSize}
        onFontSizeChange={(delta) =>
          setRowFontSize((v) => Math.min(20, Math.max(10, v + delta)))
        }
        onTranslate={() =>
          translateLines(
            transcript,
            setTranscriptZh,
            setTranslatingTop,
            topTargetLang,
            setTranscriptUntranslatable,
          )
        }
        onCopyLeft={() => copyText(transcript.join("\n"), "转录文案")}
        onCopyRight={() => copyText(transcriptZh.join("\n"), "译文")}
        onExportLeft={() => exportTxt(transcript.join("\n"), "转录文案")}
        onExportRight={() => exportTxt(transcriptZh.join("\n"), "译文")}
        onClearLeft={() => {
          setTranscript([]);
          setTranscriptUntranslatable({});
        }}
        onClearRight={() => {
          setTranscriptZh([]);
          setTranscriptUntranslatable({});
        }}
        targetLang={topTargetLang}
        onTargetLangChange={setTopTargetLang}
        untranslatable={transcriptUntranslatable}
        onOpenTerms={openTerms}
        identifying={identifying}
        onIdentify={identifySource}
        onApplyGlossary={() =>
          applyGlossaryReplace(
            transcriptZh,
            setTranscriptZh,
            setTranscriptUntranslatable,
          )
        }
      />

      {/* 下：自定义文案 / 中文翻译（默认收起，左下/右下按钮展开） */}
      <PairEditor
        leftLabel="自定义文案（每行一段）"
        leftValue={custom}
        onLeftChange={setCustom}
        rightLabel="中文翻译"
        rightValue={customZh}
        onRightChange={setCustomZh}
        translating={translatingBottom}
        translateProgress={translateProgress}
        fontSize={rowFontSize}
        onFontSizeChange={(delta) =>
          setRowFontSize((v) => Math.min(20, Math.max(10, v + delta)))
        }
        onTranslate={() =>
          translateLines(
            custom,
            setCustomZh,
            setTranslatingBottom,
            bottomTargetLang,
            setCustomUntranslatable,
          )
        }
        onCopyLeft={() => copyText(custom.join("\n"), "文案")}
        onCopyRight={() => copyText(customZh.join("\n"), "中文翻译")}
        onExportLeft={() => exportTxt(custom.join("\n"), "文案")}
        onExportRight={() => exportTxt(customZh.join("\n"), "中文翻译")}
        onClearLeft={() => setCustom([])}
        onClearRight={() => {
          setCustomZh([]);
          setCustomUntranslatable({});
        }}
        targetLang={bottomTargetLang}
        onTargetLangChange={setBottomTargetLang}
        untranslatable={customUntranslatable}
        onOpenTerms={openTerms}
        onApplyGlossary={() =>
          applyGlossaryReplace(customZh, setCustomZh, setCustomUntranslatable)
        }
        leftOpen={showCustomLeft}
        rightOpen={showCustomRight}
        onLeftOpenChange={setShowCustomLeft}
        onRightOpenChange={setShowCustomRight}
      />

      {/* 不可译词表管理 */}
      <Dialog open={termsOpen} onOpenChange={setTermsOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>不可译词表</DialogTitle>
            <DialogDescription>
              设置「原文 ↔ 固定译文」对照：翻译时遇到原文会自动转成对照译文
              （对照留空 = 保留原文不翻译）。AI 发现的不可译词会自动补充。
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-1.5">
            <button
              type="button"
              onClick={() => setBatchOpen((v) => !v)}
              className="flex w-full items-center justify-between rounded-md border border-dashed px-2 py-1 text-xs text-muted-foreground hover:border-primary/60 hover:text-primary"
            >
              <span className="inline-flex items-center gap-1">
                <LuClipboardList className="size-3.5" />
                批量粘贴导入
              </span>
              <LuChevronDown
                className={`size-3.5 transition-transform ${
                  batchOpen ? "rotate-180" : ""
                }`}
              />
            </button>
            {batchOpen && (
              <div className="grid gap-1.5">
                <Textarea
                  value={batchText}
                  onChange={(e) => setBatchText(e.target.value)}
                  placeholder={
                    "每行一条，支持几种格式：\n章鱼哥==zhangyuge\n章鱼哥=zhangyuge\n章鱼哥：zhangyuge\n仅原文（保留不翻译）"
                  }
                  className="min-h-[120px] text-xs"
                />
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={batchAddTerms}
                  disabled={!batchText.trim()}
                >
                  <LuImport className="size-4" />
                  批量导入
                </Button>
              </div>
            )}
          </div>
          <div className="relative">
            <LuSearch className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={termSearch}
              onChange={(e) => setTermSearch(e.target.value)}
              placeholder="搜索不可译的词（原文或对照译文）"
              className="pl-8"
            />
          </div>
          <div className="flex gap-2">
            <Input
              value={newTerm}
              onChange={(e) => setNewTerm(e.target.value)}
              placeholder="原文（如：章鱼哥）"
              className="flex-1"
            />
            <Input
              value={newTermZh}
              onChange={(e) => setNewTermZh(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addTerm();
                }
              }}
              placeholder="对照译文（如：zhangyuge）"
              className="flex-1"
            />
            <Button size="sm" onClick={addTerm} disabled={!newTerm.trim()}>
              <LuPlus className="size-4" />
              添加
            </Button>
          </div>
          <div className="wide-scrollbar grid max-h-[55vh] grid-cols-1 gap-1.5 overflow-y-auto sm:grid-cols-2">
            {terms.length === 0 && (
              <span className="text-xs text-muted-foreground">
                暂无词条，翻译时 AI 会自动发现并补充
              </span>
            )}
            {terms.length > 0 && filteredTerms.length === 0 && (
              <span className="text-xs text-muted-foreground">
                没有匹配「{termSearch.trim()}」的词条
              </span>
            )}
            {filteredTerms.map((e) => (
              <div
                key={e.s}
                className="flex items-center gap-1.5 rounded-md border bg-muted/50 px-2 py-1 text-xs"
              >
                <span className="truncate font-medium" title={e.s}>
                  {e.s}
                </span>
                <LuArrowRight className="size-3 shrink-0 text-muted-foreground" />
                {editingTerm === e.s ? (
                  <Input
                    autoFocus
                    value={editZhValue}
                    onChange={(ev) => setEditZhValue(ev.target.value)}
                    onKeyDown={(ev) => {
                      if (ev.key === "Enter") {
                        ev.preventDefault();
                        updateTermZh(e.s, editZhValue);
                        setEditingTerm(null);
                      } else if (ev.key === "Escape") {
                        ev.preventDefault();
                        setEditingTerm(null);
                      }
                    }}
                    onBlur={() => setEditingTerm(null)}
                    className="h-6 min-w-0 flex-1 bg-background px-1 text-xs"
                    placeholder="对照译文（留空=保留原文）"
                  />
                ) : (
                  <button
                    type="button"
                    onClick={() => {
                      setEditingTerm(e.s);
                      setEditZhValue(e.t);
                    }}
                    className={`min-w-0 flex-1 truncate rounded-sm px-1 py-0.5 text-left ${
                      e.t
                        ? "text-primary hover:bg-primary/10"
                        : "text-muted-foreground hover:bg-muted"
                    }`}
                    title={e.t ? `点击修改对照「${e.t}」` : "点击设置对照译文"}
                  >
                    {e.t.length > 0 ? e.t : "点击设置对照"}
                  </button>
                )}
                <button
                  type="button"
                  className="shrink-0 text-muted-foreground hover:text-destructive"
                  onClick={() => removeTerm(e.s)}
                  title="删除"
                >
                  <LuX className="size-3" />
                </button>
              </div>
            ))}
          </div>
        </DialogContent>
      </Dialog>

      {/* 解说词出处识别结果 */}
      <Dialog open={identifyOpen} onOpenChange={setIdentifyOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>解说词出处识别</DialogTitle>
            <DialogDescription>
              根据转录文案判断这段解说词可能出自哪部影视或动漫。
            </DialogDescription>
          </DialogHeader>
          {identifyResult ? (
            <div className="flex flex-col gap-3">
              {identifyResult.found ? (
                <div className="flex items-center gap-3 rounded-md border bg-muted/40 p-3">
                  <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span className="text-lg font-bold leading-tight text-primary">
                      {identifyResult.title || "未识别到确切作品"}
                    </span>
                    <span className="text-xs text-muted-foreground">
                      {typeLabel(identifyResult.type)}
                      {identifyResult.season > 0 &&
                        ` · 第 ${identifyResult.season} 季`}
                      {identifyResult.episode > 0 &&
                        ` · 第 ${identifyResult.episode} ${
                          identifyResult.type === "anime" ? "话" : "集"
                        }`}
                      {identifyResult.confidence > 0 &&
                        ` · 置信度 ${Math.round(
                          identifyResult.confidence * 100,
                        )}%`}
                    </span>
                  </div>
                </div>
              ) : (
                <div className="rounded-md border bg-muted/40 p-3 text-sm text-muted-foreground">
                  未能依据这段文案确定出处，请确认文案是否完整，或补充更多上下文后再试。
                </div>
              )}
              {identifyResult.found && identifyResult.breakdown && (
                <div className="rounded-md border border-primary/30 bg-primary/5 p-3">
                  <span className="mb-1 block text-xs font-semibold text-primary">
                    剧情段落（本集）
                  </span>
                  <span className="text-sm leading-relaxed text-foreground">
                    {identifyResult.breakdown}
                  </span>
                </div>
              )}
              {identifyResult.evidence && (
                <div className="text-sm leading-relaxed">
                  <span className="font-medium text-foreground">
                    判断依据：{" "}
                  </span>
                  <span className="text-muted-foreground">
                    {identifyResult.evidence}
                  </span>
                </div>
              )}
              <div className="flex justify-end">
                <Button
                  variant="outline"
                  onClick={() => setIdentifyOpen(false)}
                >
                  关闭
                </Button>
              </div>
            </div>
          ) : (
            <div className="flex flex-col gap-3">
              <div className="overflow-hidden rounded-full bg-muted">
                <div className="h-1.5 w-1/3 rounded-full bg-primary/70 animate-progress-indeterminate" />
              </div>
              <div className="flex items-center justify-between text-xs text-muted-foreground">
                <span className="inline-flex items-center gap-1.5">
                  <LuLoaderCircle className="size-3.5 animate-spin" />
                  正在解读角色与剧情线索…
                </span>
                <span>已等待 {identifyWaitSec}s</span>
              </div>
              <div className="text-center text-[11px] text-muted-foreground">
                识别是单次联机请求，通常 5~15 秒，请稍候
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

// ============================================================
// 左右一对行编辑器 + 中间翻译按钮（共享 activeIndex 联动高亮）
// ============================================================

interface PairEditorProps {
  leftLabel: string;
  leftValue: string[];
  onLeftChange: (v: string[]) => void;
  rightLabel: string;
  rightValue: string[];
  onRightChange: (v: string[]) => void;
  translating: boolean;
  /** 翻译真实进度（done/total），null 时显示流动指示条 */
  translateProgress?: { done: number; total: number } | null;
  /** 左右两栏行文字字号（px）与调节回调（中间列 A-/A+） */
  fontSize?: number;
  onFontSizeChange?: (delta: number) => void;
  onTranslate: () => void;
  onCopyLeft: () => void;
  onCopyRight: () => void;
  onExportLeft: () => void;
  onExportRight: () => void;
  /** 提供时左面板标题栏显示「清空」按钮 */
  onClearLeft?: () => void;
  /** 提供时右面板标题栏显示「清空」按钮 */
  onClearRight?: () => void;
  /** 目标语言（auto/zh/en/fr/...）与变更回调（中间下拉框） */
  targetLang: string;
  onTargetLangChange: (v: string) => void;
  /** 不可译词高亮：原始行号 → 词列表（右侧译文渲染用） */
  untranslatable: Record<number, string[]>;
  /** 打开不可译词表管理对话框 */
  onOpenTerms: () => void;
  /** 一键替换：把右侧译文里已配置对照的词替换成对照译文并保持高亮 */
  onApplyGlossary: () => void;
  /** 左/右面板是否展开（默认展开；底部功能区可收起隐藏） */
  leftOpen?: boolean;
  rightOpen?: boolean;
  onLeftOpenChange?: (open: boolean) => void;
  onRightOpenChange?: (open: boolean) => void;
  /** 正在识别出处（左栏标题栏「识别出处」按钮 loading 态） */
  identifying?: boolean;
  /** 提供时左栏标题栏显示「识别出处」按钮 */
  onIdentify?: () => void;
}

// ============================================================
// 滚动对齐辅助：像素级镜像 —— 同一行号在左右两栏处于相同垂直位置
// ============================================================

/**
 * 把 source 容器的第 idx 行与 target 容器的第 idx 行调到同一垂直位置：
 * 计算 source 行当前距其容器顶部的像素偏移（可为负=部分滚出），
 * 再调整 target.scrollTop，使 target 同序号行停在同样的偏移处。
 * 这样左栏行在中间，右栏同行也在中间——真正的“对齐”，而不是钉到顶部。
 */
function alignRowOffset(source: HTMLElement, target: HTMLElement, idx: number) {
  const sRow = source.querySelector<HTMLElement>(`[data-row="${idx}"]`);
  if (!sRow) return;
  const tRow =
    target.querySelector<HTMLElement>(`[data-row="${idx}"]`) ??
    [...target.querySelectorAll<HTMLElement>("[data-row]")].pop();
  if (!tRow) return;
  const sRect = source.getBoundingClientRect();
  const tRect = target.getBoundingClientRect();
  const sourceOffset = sRow.getBoundingClientRect().top - sRect.top;
  const targetOffset = tRow.getBoundingClientRect().top - tRect.top;
  target.scrollTop += targetOffset - sourceOffset;
}

/** 取容器当前可见顶行序号（含部分滚出顶部的行） */
function topVisibleRowIndex(el: HTMLElement): number {
  const rows = Array.from(el.querySelectorAll<HTMLElement>("[data-row]"));
  if (rows.length === 0) return -1;
  const containerTop = el.getBoundingClientRect().top;
  for (let j = 0; j < rows.length; j++) {
    if (rows[j].getBoundingClientRect().top >= containerTop - 1) return j;
  }
  return rows.length - 1;
}

function PairEditor(props: PairEditorProps) {
  const { leftValue, rightValue } = props;
  const leftOpen = props.leftOpen ?? true;
  const rightOpen = props.rightOpen ?? true;
  // activeIndex：当前选中行（两侧同号行一起高亮）
  const [activeIndex, setActiveIndex] = useState(-1);

  // 两块滚动面板的 DOM 元素（面板收起时为 null）
  const leftEl = useRef<HTMLElement | null>(null);
  const rightEl = useRef<HTMLElement | null>(null);

  // 行数变小时修正 activeIndex，避免越界
  useEffect(() => {
    setActiveIndex((a) => {
      if (a < 0) return -1;
      const max = Math.max(leftValue.length, rightValue.length);
      return a < max ? a : max > 0 ? max - 1 : -1;
    });
  }, [leftValue.length, rightValue.length]);

  // 左栏滚动停止后（120ms 防抖）→ 右栏镜像对齐到同一垂直位置。
  // 单向驱动：右栏滚动绝不影响左栏，左栏位置永不被改。
  // 依赖含展开标志：面板收起/展开会挂载/卸载 DOM，需重新绑定监听。
  useEffect(() => {
    const l = leftEl.current;
    if (!l) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const onLeftScroll = () => {
      const r = rightEl.current;
      if (!r?.isConnected) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        const rr = rightEl.current;
        if (!rr?.isConnected) return;
        const idx = topVisibleRowIndex(l);
        if (idx >= 0) alignRowOffset(l, rr, idx);
      }, 120);
    };
    l.addEventListener("scroll", onLeftScroll, { passive: true });
    return () => {
      l.removeEventListener("scroll", onLeftScroll);
      if (timer) clearTimeout(timer);
    };
  }, []);

  // 某一侧展开后，自动与左栏当前顶行对齐（仅两侧都在时执行）
  useEffect(() => {
    if (!leftOpen || !rightOpen) return;
    const t = setTimeout(() => {
      const l = leftEl.current;
      const r = rightEl.current;
      if (!l || !r?.isConnected) return;
      const idx = topVisibleRowIndex(l);
      if (idx >= 0) alignRowOffset(l, r, idx);
    }, 60);
    return () => clearTimeout(t);
  }, [leftOpen, rightOpen]);

  // 点击左侧某行 → 左栏不动（保持点击位置），右栏同一行镜像到相同垂直位置
  const onLeftActivate = useCallback((i: number) => {
    setActiveIndex(i);
    const l = leftEl.current;
    const r = rightEl.current;
    if (!l || !r?.isConnected) return;
    alignRowOffset(l, r, i);
  }, []);

  // 点击右侧某行 → 右栏不动，左栏同一行镜像到相同垂直位置（对称对齐）
  const onRightActivate = useCallback((i: number) => {
    setActiveIndex(i);
    const l = leftEl.current;
    const r = rightEl.current;
    if (!l || !r || !l.isConnected) return;
    alignRowOffset(r, l, i);
  }, []);

  // 双侧都收起：一条细横条，左下/右下各一个展开按钮
  if (!leftOpen && !rightOpen) {
    return (
      <div className="flex shrink-0 items-center justify-between rounded-lg border border-dashed px-1.5 py-1">
        <button
          type="button"
          onClick={() => props.onLeftOpenChange?.(true)}
          className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
          title={`展开${props.leftLabel}`}
        >
          <LuPlus className="size-3.5" />
          {props.leftLabel}
        </button>
        <button
          type="button"
          onClick={() => props.onRightOpenChange?.(true)}
          className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
          title={`展开${props.rightLabel}`}
        >
          {props.rightLabel}
          <LuPlus className="size-3.5" />
        </button>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 gap-2">
      {leftOpen ? (
        <LineListEditor
          label={props.leftLabel}
          value={props.leftValue}
          onChange={props.onLeftChange}
          activeIndex={activeIndex}
          onActivate={onLeftActivate}
          onCopy={props.onCopyLeft}
          onExport={props.onExportLeft}
          onClear={props.onClearLeft}
          onCollapse={
            props.onLeftOpenChange
              ? () => props.onLeftOpenChange?.(false)
              : undefined
          }
          fontSize={props.fontSize}
          registerScrollEl={(el) => (leftEl.current = el)}
          identifying={props.identifying}
          onIdentify={props.onIdentify}
        />
      ) : (
        <CollapsedStrip
          label={props.leftLabel}
          onExpand={() => props.onLeftOpenChange?.(true)}
        />
      )}
      <div className="flex w-28 shrink-0 flex-col items-stretch justify-center gap-1.5">
        <Button
          size="sm"
          variant="secondary"
          className="h-11 flex-col gap-1"
          onClick={props.onTranslate}
          disabled={props.translating || props.leftValue.length === 0}
        >
          {props.translating ? (
            <LuLoaderCircle className="size-4 animate-spin" />
          ) : (
            <LuLanguages className="size-4" />
          )}
          <span className="text-xs font-medium">
            {props.translating ? "翻译中…" : "开始翻译"}
          </span>
        </Button>
        {props.translating && (
          <div className="flex w-28 flex-col items-center gap-1">
            {props.translateProgress && props.translateProgress.total > 0 ? (
              <>
                <Progress
                  value={
                    (props.translateProgress.done /
                      props.translateProgress.total) *
                    100
                  }
                  className="h-1.5 w-full"
                />
                <span className="text-[10px] leading-none text-muted-foreground">
                  {props.translateProgress.done}/{props.translateProgress.total}
                </span>
              </>
            ) : (
              <div className="w-full overflow-hidden rounded-full bg-muted">
                <div className="h-0.5 w-1/3 rounded-full bg-primary/70 animate-progress-indeterminate" />
              </div>
            )}
          </div>
        )}
        <div
          className="rounded-lg border bg-background p-1.5"
          title="选择翻译目标语言"
        >
          <span className="mb-1 block text-center text-[10px] font-semibold leading-none text-muted-foreground">
            译为
          </span>
          <Select
            value={props.targetLang}
            onValueChange={props.onTargetLangChange}
          >
            <SelectTrigger className="h-7 w-full gap-1 border bg-background px-2 text-xs shadow-none">
              <SelectValue placeholder="语言" />
            </SelectTrigger>
            <SelectContent>
              {LANG_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value} className="text-xs">
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <Button
          size="sm"
          variant="secondary"
          className="h-11 flex-col gap-1"
          onClick={props.onApplyGlossary}
          disabled={props.translating}
          title="把右侧译文里已配置对照的词一键替换成对照译文（保持高亮）"
        >
          <LuReplace className="size-4" />
          <span className="text-xs font-medium">一键替换</span>
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="h-11 flex-col gap-1"
          onClick={props.onOpenTerms}
          disabled={props.translating}
          title="术语对照表（AI 发现的词自动补充）"
        >
          <LuBookMarked className="size-4" />
          <span className="text-xs font-medium">不可译词表</span>
        </Button>
        <div
          className="mt-1 flex items-center justify-center gap-1 rounded-lg border bg-background p-1"
          title="左右两栏文字大小"
        >
          <button
            type="button"
            onClick={() => props.onFontSizeChange?.(-1)}
            disabled={props.translating || (props.fontSize ?? 12) <= 10}
            className="flex h-6 w-7 items-center justify-center rounded-md text-xs font-semibold text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-40"
            title="缩小文字"
          >
            A-
          </button>
          <span className="min-w-6 text-center text-[10px] leading-none text-muted-foreground">
            {props.fontSize ?? 12}
          </span>
          <button
            type="button"
            onClick={() => props.onFontSizeChange?.(1)}
            disabled={props.translating || (props.fontSize ?? 12) >= 20}
            className="flex h-6 w-7 items-center justify-center rounded-md text-xs font-bold text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-40"
            title="放大文字"
          >
            A+
          </button>
        </div>
      </div>
      {rightOpen ? (
        <LineListEditor
          label={props.rightLabel}
          value={props.rightValue}
          onChange={props.onRightChange}
          activeIndex={activeIndex}
          onActivate={onRightActivate}
          onCopy={props.onCopyRight}
          onExport={props.onExportRight}
          onClear={props.onClearRight}
          untranslatable={props.untranslatable}
          onCollapse={
            props.onRightOpenChange
              ? () => props.onRightOpenChange?.(false)
              : undefined
          }
          fontSize={props.fontSize}
          registerScrollEl={(el) => (rightEl.current = el)}
        />
      ) : (
        <CollapsedStrip
          label={props.rightLabel}
          onExpand={() => props.onRightOpenChange?.(true)}
        />
      )}
    </div>
  );
}

/** 收起态的窄竖条：点击展开对应面板（垂直文字 + 加号图标） */
function CollapsedStrip({
  label,
  onExpand,
}: {
  label: string;
  onExpand: () => void;
}) {
  const short = label.replace(/（.*?）/g, "");
  return (
    <button
      type="button"
      onClick={onExpand}
      title={`展开${short}`}
      className="flex w-9 shrink-0 flex-col items-center justify-center gap-2 rounded-md border border-dashed text-muted-foreground hover:border-primary/60 hover:bg-primary/5 hover:text-primary"
    >
      <LuPlus className="size-4" />
      <span className="text-[10px] leading-none [writing-mode:vertical-rl]">
        {short}
      </span>
    </button>
  );
}

// ============================================================
// 逐行编辑器：行号 / 单击联动高亮 / 双击行内编辑 / 粘贴分行 / 上下键导航 / hover 删除 / 添加行
// ============================================================

interface LineListEditorProps {
  label: string;
  value: string[];
  onChange: (v: string[]) => void;
  activeIndex: number;
  onActivate: (i: number) => void;
  onCopy: () => void;
  onExport: () => void;
  /** 提供时在标题栏显示「清空」按钮（一键清空所有行） */
  onClear?: () => void;
  /** 不可译词高亮：行号 → 词列表（仅显示态渲染，编辑态为纯文本） */
  untranslatable?: Record<number, string[]>;
  /** 提供时在标题栏显示「收起」按钮（点击隐藏本面板） */
  onCollapse?: () => void;
  /** 行文字字号（px），未提供用默认 text-xs */
  fontSize?: number;
  /** 把自身滚动容器注册给父级（用于左栏驱动右栏对齐） */
  registerScrollEl: (el: HTMLElement | null) => void;
  /** 正在识别出处（显示 loading 图标） */
  identifying?: boolean;
  /** 提供时标题栏显示「识别出处」按钮（针对本栏文本做影视/动漫出处识别） */
  onIdentify?: () => void;
}

function LineListEditor({
  label,
  value,
  onChange,
  activeIndex,
  onActivate,
  onCopy,
  onExport,
  onClear,
  untranslatable,
  onCollapse,
  fontSize,
  registerScrollEl,
  identifying,
  onIdentify,
}: LineListEditorProps) {
  const [editIndex, setEditIndex] = useState<number | null>(null);
  const [editText, setEditText] = useState("");
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const editInputRef = useRef<HTMLInputElement | null>(null);

  // 把滚动容器注册给父级，供左右拖动比例联动使用
  useEffect(() => {
    registerScrollEl(scrollRef.current);
    return () => registerScrollEl(null);
  }, [registerScrollEl]);

  // 编辑态挂载后自动聚焦并全选，替代 autoFocus 属性（避免 biome noAutofocus 告警）
  useEffect(() => {
    if (editIndex === null) return;
    const el = editInputRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, [editIndex]);

  const startEditAt = (i: number, initial?: string) => {
    const v = initial ?? value[i] ?? "";
    setEditText(v);
    setEditIndex(i);
  };

  /** 提交行内编辑；empty 则删除该行；moveNext 为 Enter 前进到下一行 */
  const commitEdit = (moveNext: boolean) => {
    if (editIndex === null) return;
    const i = editIndex;
    const val = makeRowText(editText);
    let next: string[];
    if (val.length === 0) {
      next = value.filter((_, idx) => idx !== i);
    } else {
      next = value.map((x, idx) => (idx === i ? val : x));
    }
    onChange(next);
    setEditIndex(null);
    setEditText("");
    if (moveNext) {
      const ni = val.length > 0 ? i + 1 : i;
      const target = Math.min(ni, next.length - 1);
      if (next[target] !== undefined) {
        startEditAt(target);
        onActivate(target);
      }
    }
  };

  /** 容器层面拦截粘贴：非编辑态下按换行拆成多行插入选中行之后 */
  const handlePaste = (e: React.ClipboardEvent) => {
    if (editIndex !== null) return;
    const text = e.clipboardData.getData("text");
    const pieces = text
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (pieces.length === 0) return;
    e.preventDefault();
    let next = [...value];
    if (activeIndex < 0 || activeIndex >= next.length) {
      next = next.concat(pieces);
      onChange(next);
      onActivate(next.length - 1);
    } else {
      const insertAt = activeIndex + 1;
      next.splice(insertAt, 0, ...pieces);
      onChange(next);
      onActivate(insertAt + pieces.length - 1);
    }
  };

  const handleContainerKeyDown = (e: React.KeyboardEvent) => {
    if (editIndex !== null) return;
    if (e.key === "ArrowDown" && activeIndex >= 0) {
      e.preventDefault();
      const ni = Math.min(activeIndex + 1, value.length - 1);
      onActivate(ni);
      scrollRef.current?.querySelector(`[data-row="${ni}"]`)?.scrollIntoView({
        block: "nearest",
      });
    } else if (e.key === "ArrowUp" && activeIndex > 0) {
      e.preventDefault();
      const ni = Math.max(activeIndex - 1, 0);
      onActivate(ni);
      scrollRef.current?.querySelector(`[data-row="${ni}"]`)?.scrollIntoView({
        block: "nearest",
      });
    }
  };

  const addRow = () => {
    const next = [...value, ""];
    onChange(next);
    const i = next.length - 1;
    onActivate(i);
    startEditAt(i);
  };

  const isActive = (i: number) => i === activeIndex;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-1">
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium text-muted-foreground">
          {label}
        </span>
        <div className="flex items-center gap-1">
          {onIdentify && (
            <Button
              size="sm"
              variant="ghost"
              className="h-6 gap-1 px-2"
              onClick={onIdentify}
              disabled={identifying || value.length === 0}
              title="识别这段文案出自哪部影视/动漫的哪一集"
            >
              {identifying ? (
                <LuLoaderCircle className="size-3.5 animate-spin" />
              ) : (
                <LuSearch className="size-3.5" />
              )}
              <span className="text-xs">识别出处</span>
            </Button>
          )}
          <Button
            size="sm"
            variant="ghost"
            className="size-6 p-0"
            onClick={onCopy}
            disabled={value.length === 0}
            title="复制"
          >
            <LuCopy className="size-3.5" />
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="size-6 p-0"
            onClick={onExport}
            disabled={value.length === 0}
            title="导出 TXT"
          >
            <LuFileDown className="size-3.5" />
          </Button>
          {onClear && (
            <Button
              size="sm"
              variant="ghost"
              className="size-6 p-0"
              onClick={onClear}
              disabled={value.length === 0}
              title="清空全部行"
            >
              <LuTrash2 className="size-3.5" />
            </Button>
          )}
          {onCollapse && (
            <Button
              size="sm"
              variant="ghost"
              className="size-6 p-0"
              onClick={onCollapse}
              title="收起面板"
            >
              <LuChevronDown className="size-3.5" />
            </Button>
          )}
        </div>
      </div>

      <div
        ref={scrollRef}
        tabIndex={0}
        role="listbox"
        aria-label={label}
        onKeyDown={handleContainerKeyDown}
        onPaste={handlePaste}
        className="wide-scrollbar min-h-0 flex-1 overflow-y-auto rounded-md border bg-muted/20 px-1 py-1 outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
      >
        {value.length === 0 && (
          <div className="px-2 py-3 text-center text-xs text-muted-foreground/70">
            左侧为空，粘贴内容（Ctrl+V 自动分行）或点击下方「添加一行」
          </div>
        )}
        {value.map((line, i) => {
          const active = isActive(i);
          const editing = editIndex === i;
          return (
            <div
              key={i}
              role="option"
              aria-selected={active}
              tabIndex={-1}
              data-row={i}
              onClick={() => {
                if (editIndex !== null) return;
                onActivate(i);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  startEditAt(i);
                }
              }}
              onDoubleClick={(e) => {
                e.preventDefault();
                startEditAt(i);
              }}
              className={cn(
                "group flex cursor-pointer items-start gap-2 rounded-md px-2 py-[3px]",
                active
                  ? "border border-primary bg-primary/10"
                  : "border border-transparent hover:bg-muted",
              )}
            >
              <span className="w-6 shrink-0 pt-px text-right text-[10px] leading-5 text-muted-foreground select-none">
                {i + 1}
              </span>
              {editing ? (
                <input
                  ref={editInputRef}
                  value={editText}
                  onChange={(e) => setEditText(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      commitEdit(true);
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      commitEdit(false);
                    }
                  }}
                  onBlur={() => commitEdit(false)}
                  style={
                    fontSize
                      ? { fontSize, lineHeight: `${fontSize * 1.6}px` }
                      : undefined
                  }
                  className="w-full min-w-0 flex-1 rounded-sm border border-primary bg-background px-1 py-0 font-mono leading-5 outline-none"
                />
              ) : (
                <span
                  style={
                    fontSize
                      ? { fontSize, lineHeight: `${fontSize * 1.6}px` }
                      : undefined
                  }
                  className="min-w-0 flex-1 break-words whitespace-pre-wrap font-mono text-foreground"
                >
                  {line.length > 0
                    ? renderHighlighted(line, untranslatable?.[i] ?? [])
                    : "\u00A0"}
                </span>
              )}
              {!editing && (
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation();
                    onChange(value.filter((_, idx) => idx !== i));
                  }}
                  className="ml-auto shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-destructive"
                  title="删除该行"
                >
                  <LuX className="size-3.5" />
                </button>
              )}
            </div>
          );
        })}
        <button
          type="button"
          onClick={addRow}
          className="mt-1 flex w-full items-center justify-center gap-1 rounded-md border border-dashed px-2 py-1 text-xs text-muted-foreground hover:border-primary hover:text-primary"
        >
          <LuPlus className="size-3.5" />
          添加一行
        </button>
      </div>
    </div>
  );
}

function shortName(s: string): string {
  if (/^https?:\/\//i.test(s)) {
    try {
      const u = new URL(s);
      return `${u.hostname.replace(/^www\./, "")}…`;
    } catch {
      return s.slice(0, 40);
    }
  }
  const base = s.split(/[\\/]/).pop() ?? s;
  return base.length > 36 ? `${base.slice(0, 33)}…` : base;
}

/** 词表条目：`s` 原文，`t` 对照译文（空 = 保留原文不翻译） */
interface GlossaryEntry {
  s: string;
  t: string;
}

/** 大小写不敏感的全量替换（保留 to 原样） */
function replaceAllCaseInsensitive(
  text: string,
  from: string,
  to: string,
): string {
  if (!from) return text;
  const lowerText = text.toLocaleLowerCase();
  const lowerFrom = from.toLocaleLowerCase();
  let result = "";
  let last = 0;
  for (
    let idx = lowerText.indexOf(lowerFrom);
    idx !== -1;
    idx = lowerText.indexOf(lowerFrom, last)
  ) {
    result += text.slice(last, idx) + to;
    last = idx + from.length;
  }
  result += text.slice(last);
  return result;
}

/**
 * 高亮 map（行号→词列表）从持久化恢复时：key 是字符串，转成数字并去重。
 * 顺带合并该行可能的重复词、清空空数组。
 */
function normalizeHighlight(
  raw: Record<string, string[]> | undefined,
): Record<number, string[]> {
  const out: Record<number, string[]> = {};
  if (!raw) return out;
  for (const [k, arr] of Object.entries(raw)) {
    const idx = Number(k);
    if (!Number.isInteger(idx) || idx < 0) continue;
    const uniq = Array.from(new Set((arr ?? []).filter(Boolean)));
    if (uniq.length > 0) out[idx] = uniq;
  }
  return out;
}

/** 目标语言选项（auto = 智能判断：含中文→英，否则→中） */
const LANG_OPTIONS: { value: string; label: string }[] = [
  { value: "zh", label: "中文" },
  { value: "en", label: "英语" },
  { value: "fr", label: "法语" },
  { value: "es", label: "西班牙语" },
  { value: "de", label: "德语" },
  { value: "ru", label: "俄语" },
  { value: "ja", label: "日语" },
  { value: "ko", label: "韩语" },
  { value: "pt", label: "葡萄牙语" },
];

/**
 * 行文本高亮渲染：把命中不可译词表的片段包上 <mark>。
 * 多词命中区间合并，避免嵌套高亮。
 */
function renderHighlighted(text: string, terms: string[]): React.ReactNode {
  if (terms.length === 0 || text.length === 0) return text;
  const lower = text.toLowerCase();
  const ranges: Array<[number, number]> = [];
  for (const term of terms) {
    const t = term.toLowerCase();
    if (!t) continue;
    let from = 0;
    for (;;) {
      const pos = lower.indexOf(t, from);
      if (pos < 0) break;
      ranges.push([pos, pos + t.length]);
      from = pos + t.length;
    }
  }
  if (ranges.length === 0) return text;
  ranges.sort((a, b) => a[0] - b[0] || b[1] - a[1]);
  const merged: Array<[number, number]> = [];
  for (const r of ranges) {
    const last = merged[merged.length - 1];
    if (last && r[0] <= last[1]) {
      last[1] = Math.max(last[1], r[1]);
    } else {
      merged.push([r[0], r[1]]);
    }
  }
  const out: React.ReactNode[] = [];
  let cursor = 0;
  merged.forEach(([s, e], k) => {
    if (s > cursor) out.push(text.slice(cursor, s));
    out.push(
      <mark
        key={k}
        className="rounded bg-amber-200/80 px-0.5 text-inherit dark:bg-amber-500/30"
      >
        {text.slice(s, e)}
      </mark>,
    );
    cursor = e;
  });
  if (cursor < text.length) out.push(text.slice(cursor));
  return out;
}

/** 行文本规整：trim 首尾，collapse 空白（保留换行统一为单行语义） */
function makeRowText(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}
