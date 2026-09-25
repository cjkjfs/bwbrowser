"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  LuCheck,
  LuDownload,
  LuFolderOpen,
  LuLink,
  LuLoader,
  LuPause,
  LuPlay,
  LuRefreshCw,
  LuRotateCcw,
  LuSettings,
  LuTrash2,
  LuX,
} from "react-icons/lu";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import {
  type DownloadTask,
  useVideoDownload,
} from "@/hooks/use-video-download";
import { dismissToast, showLaunchProgressToast } from "@/lib/toast-utils";

interface VideoDownloadPageProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  subPage?: boolean;
  embedded?: boolean;
}

export function VideoDownloadPage({
  open,
  onOpenChange,
  subPage,
  embedded = false,
}: VideoDownloadPageProps) {
  const {
    tasks,
    settings,
    toolStatus,
    lastCookieTime,
    isRefreshingCookie,
    addDownload,
    addBatchDownload,
    cancelTask,
    pauseTask,
    resumeTask,
    retryTask,
    deleteTask,
    clearFinished,
    pauseAll,
    retryAll,
    startPending,
    deleteAll,
    updateSettings,
    loadTasks,
    downloadYtDlp,
    downloadFfmpeg,
    checkTools,
    isDownloadingTools,
    toolsExist,
    pasteAndDownload,
    refreshCookie,
    openDir,
    openFile,
    proxyList,
  } = useVideoDownload();

  const [urlInput, setUrlInput] = useState("");
  const [activeTab, setActiveTab] = useState("tasks");
  const [downloadingTools, setDownloadingTools] = useState<string[]>([]);
  const [toolDownloadProgress, setToolDownloadProgress] = useState<
    Record<string, number>
  >({});
  const [toolDownloadInfoMap, setToolDownloadInfoMap] = useState<
    Record<string, { downloaded: number; total: number }>
  >({});
  const [updatingTool, setUpdatingTool] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);
  const autoDownloadingRef = useRef(false);
  const downloadingToolsRef = useRef<string[]>([]);
  const toolProgressRef = useRef<Record<string, number>>({});
  const toolInfoRef = useRef<
    Record<string, { downloaded: number; total: number }>
  >({});
  const toolCheckedRef = useRef(false);

  // 监听工具下载进度
  useEffect(() => {
    const unlisten = listen<{
      tool: string;
      progress: number;
      downloaded: number;
      total: number;
    }>("video-download:tool-download-progress", (event) => {
      const { tool, progress, downloaded, total } = event.payload;

      toolProgressRef.current[tool] = progress;
      toolInfoRef.current[tool] = { downloaded, total };

      setToolDownloadProgress((prev) => ({ ...prev, [tool]: progress }));
      setToolDownloadInfoMap((prev) => ({
        ...prev,
        [tool]: { downloaded, total },
      }));

      // 每个工具用独立的 toast 提示框，显示各自的下载进度
      const active = downloadingToolsRef.current;
      if (active.includes(tool)) {
        const toastId = `tool-download-progress-${tool}`;
        const label =
          tool === "yt-dlp" ? "yt-dlp" : tool === "ffmpeg" ? "ffmpeg" : tool;
        const downloadedMB = (downloaded / 1024 / 1024).toFixed(1);
        const totalMB = total > 0 ? (total / 1024 / 1024).toFixed(1) : "?";
        showLaunchProgressToast(
          toastId,
          `正在下载 ${label}...`,
          progress,
          `${downloadedMB} MB / ${totalMB} MB`,
        );
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 监听工具更新状态
  useEffect(() => {
    const unlisten = listen<{ status: string; message: string }>(
      "video-download:tool-update-status",
      (event) => {
        setUpdateStatus(event.payload.message);
        if (event.payload.status === "done") {
          setUpdatingTool(false);
          toast.success(event.payload.message);
          setTimeout(() => setUpdateStatus(null), 3000);
        }
      },
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // 更新下载工具：yt-dlp 和 ffmpeg 并行更新，各自独立 toast 提示框显示进度
  const handleUpdateTool = useCallback(async () => {
    if (updatingTool) return;
    setUpdatingTool(true);
    setUpdateStatus("正在更新工具...");

    const active = ["yt-dlp", "ffmpeg", "ffprobe"];
    downloadingToolsRef.current = active;
    setDownloadingTools(active);
    // yt-dlp 和 ffmpeg 立即显示进度框；ffprobe 随 ffmpeg 下载完成后由后端事件触发
    ["yt-dlp", "ffmpeg"].forEach((t) => {
      setToolDownloadProgress((prev) => ({ ...prev, [t]: 0 }));
      showLaunchProgressToast(
        `tool-download-progress-${t}`,
        `正在更新 ${t}...`,
        0,
        "准备中...",
      );
    });
    // ffprobe 初始提示框（等待 ffmpeg 解压后填充）
    showLaunchProgressToast(
      "tool-download-progress-ffprobe",
      "正在更新 ffprobe...",
      0,
      "等待 ffmpeg 解压...",
    );

    const results = await Promise.all([
      downloadYtDlp().then(
        () => true,
        (e) => {
          console.error("yt-dlp 更新失败:", e);
          return false;
        },
      ),
      downloadFfmpeg().then(
        () => true,
        (e) => {
          console.error("ffmpeg 更新失败:", e);
          return false;
        },
      ),
    ]);

    downloadingToolsRef.current = [];
    toolProgressRef.current = {};
    toolInfoRef.current = {};
    active.forEach((t) => dismissToast(`tool-download-progress-${t}`));
    setDownloadingTools([]);
    setToolDownloadInfoMap({});

    const allOk = results.every((r) => r);
    if (allOk) {
      setUpdateStatus(null);
      toast.success("工具更新完成");
    } else {
      const failed = active.filter((_, i) => !results[i]);
      toast.error(`${failed.join("、")} 更新失败`, {
        description: "请检查网络连接后重试",
      });
      setUpdateStatus(null);
    }
    setUpdatingTool(false);
  }, [updatingTool, downloadYtDlp, downloadFfmpeg]);

  // 自动下载工具的公共函数（支持并发下载多个工具以节省时间）
  const autoDownloadTools = useCallback(
    async (missing: string[]) => {
      const isFirstTime = !toolStatus?.yt_dlp && !toolStatus?.ffmpeg;

      try {
        const needYtDlp = missing.includes("yt-dlp");
        const needFfmpeg =
          missing.includes("ffmpeg") || missing.includes("ffprobe");

        const active: string[] = [];
        const tasks: Promise<boolean>[] = [];

        // 缺失的工具并行下载，每个工具独立 toast 提示框显示各自进度
        if (needYtDlp) {
          active.push("yt-dlp");
          tasks.push(
            downloadYtDlp().then(
              () => true,
              (e) => {
                console.error("yt-dlp 下载失败:", e);
                return false;
              },
            ),
          );
        }
        if (needFfmpeg) {
          active.push("ffmpeg");
          // ffprobe 随 ffmpeg 一起下载（同一 zip），加入 active 让进度监听器接受其完成事件
          active.push("ffprobe");
          tasks.push(
            downloadFfmpeg().then(
              () => true,
              (e) => {
                console.error("ffmpeg 下载失败:", e);
                return false;
              },
            ),
          );
        }

        if (active.length === 0) return true;

        downloadingToolsRef.current = active;
        setDownloadingTools(active);
        // yt-dlp 和 ffmpeg 立即显示进度框；ffprobe 随 ffmpeg 解压完成后由后端事件触发
        const immediate = active.filter((t) => t !== "ffprobe");
        immediate.forEach((t) => {
          setToolDownloadProgress((prev) => ({ ...prev, [t]: 0 }));
          showLaunchProgressToast(
            `tool-download-progress-${t}`,
            `${isFirstTime ? "首次配置：" : ""}正在下载 ${t}...`,
            0,
            "准备中...",
          );
        });
        if (active.includes("ffprobe")) {
          showLaunchProgressToast(
            "tool-download-progress-ffprobe",
            `${isFirstTime ? "首次配置：" : ""}正在下载 ffprobe...`,
            0,
            "等待 ffmpeg 解压...",
          );
        }

        const results = await Promise.all(tasks);
        const allOk = results.every((r) => r);
        if (!allOk) throw new Error("部分工具下载失败");

        downloadingToolsRef.current = [];
        toolProgressRef.current = {};
        toolInfoRef.current = {};
        active.forEach((t) => dismissToast(`tool-download-progress-${t}`));
        await checkTools();

        toast.success("工具下载完成", {
          description: isFirstTime
            ? "首次配置完成，已准备好开始下载视频"
            : "已准备好开始下载视频",
        });

        return true;
      } catch (e) {
        console.error("自动下载工具失败:", e);
        downloadingToolsRef.current = [];
        toolProgressRef.current = {};
        toolInfoRef.current = {};
        // 关闭所有可能残留的工具 toast
        ["yt-dlp", "ffmpeg", "ffprobe"].forEach((t) =>
          dismissToast(`tool-download-progress-${t}`),
        );
        toast.error("工具下载失败", {
          description: "请在设置中手动点击下载按钮",
        });
        return false;
      } finally {
        setDownloadingTools([]);
        setToolDownloadInfoMap({});
        autoDownloadingRef.current = false;
      }
    },
    [
      toolStatus?.yt_dlp,
      toolStatus?.ffmpeg,
      downloadYtDlp,
      downloadFfmpeg,
      checkTools,
    ],
  );

  // 确保工具状态已加载，如果缺失则自动下载
  const _ensureToolsReady = useCallback(async () => {
    if (autoDownloadingRef.current) return false;

    let status = toolStatus;
    if (!status) {
      status = await checkTools();
    }

    const missing: string[] = [];
    if (!status?.yt_dlp) missing.push("yt-dlp");
    if (!status?.ffmpeg) missing.push("ffmpeg");

    if (missing.length === 0) return true;

    autoDownloadingRef.current = true;
    const ok = await autoDownloadTools(missing);
    if (ok) {
      startPending();
    }
    return ok;
  }, [toolStatus, checkTools, autoDownloadTools, startPending]);

  // 监听缺少工具事件，自动下载
  useEffect(() => {
    if (!open) return;

    const unlisten = listen<string[]>(
      "video-download:tools-missing",
      async (event) => {
        if (autoDownloadingRef.current) return;
        autoDownloadingRef.current = true;

        const missing = event.payload;
        const ok = await autoDownloadTools(missing);
        if (ok) {
          startPending();
        }
      },
    );

    return () => {
      unlisten.then((f) => f());
    };
  }, [open, autoDownloadTools, startPending]);

  // 后端在 CDP 不可用时通过此事件触发前端打开 VPS 浏览器
  useEffect(() => {
    if (!open) return;
    const unlisten = listen("video-download:auto-open-vps", () => {
      invoke("bwbrowser_open_vps_login").catch((e) =>
        console.error("[video-download] 自动打开 VPS 浏览器失败:", e),
      );
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [open]);

  // 页面打开时检查工具文件是否存在，不存在则自动下载
  useEffect(() => {
    if (!open) {
      toolCheckedRef.current = false;
      return;
    }
    if (toolCheckedRef.current) return;

    let cancelled = false;

    (async () => {
      // 用轻量文件存在性检查代替 checkTools，不调版本检测，不阻塞下载流程
      const exist = await toolsExist();

      console.log(
        "[video-download] 工具文件检查:",
        exist ? "全部存在" : "有缺失",
      );

      if (cancelled) return;

      if (!exist) {
        // 文件缺失，确定缺哪些，触发自动下载
        const missing: string[] = [];
        // 只在缺失时才调 checkTools 获取详细状态（此时不阻塞，因为下载还没开始）
        const status = await checkTools();
        if (cancelled || !status) return;
        if (!status.yt_dlp) missing.push("yt-dlp");
        if (!status.ffmpeg) missing.push("ffmpeg");
        if (!status.ffprobe) missing.push("ffprobe");

        if (missing.length === 0) {
          toolCheckedRef.current = true;
          return;
        }

        toolCheckedRef.current = true;
        if (autoDownloadingRef.current) return;

        // 检查后端是否已经在下载工具（页面切换后重新打开时避免重复下载）
        const backendDownloading = await isDownloadingTools();
        if (cancelled) return;

        if (backendDownloading) {
          console.log("[video-download] 后端已在下载工具，等待完成");
          autoDownloadingRef.current = true;
          // 轮询等待后端下载完成
          const poll = async () => {
            while (!cancelled) {
              await new Promise((r) => setTimeout(r, 2000));
              if (cancelled) return;
              const still = await isDownloadingTools();
              if (!still) {
                await checkTools();
                const hasWaiting = tasks.some(
                  (t) => t.status === "waiting" || t.status === "paused",
                );
                if (hasWaiting) startPending();
                autoDownloadingRef.current = false;
                return;
              }
            }
          };
          poll();
          return;
        }

        autoDownloadingRef.current = true;
        const ok = await autoDownloadTools(missing);

        if (cancelled || !ok) return;

        const hasWaiting = tasks.some(
          (t) => t.status === "waiting" || t.status === "paused",
        );
        if (hasWaiting) {
          startPending();
        }
      } else {
        toolCheckedRef.current = true;
        // 工具文件都存在，后台异步刷新版本信息（不阻塞下载）
        checkTools();
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [
    open,
    toolsExist,
    checkTools,
    isDownloadingTools,
    autoDownloadTools,
    startPending,
    tasks,
  ]);

  // 统计
  const stats = useMemo(() => {
    const downloading = tasks.filter((t) => t.status === "downloading").length;
    const waiting = tasks.filter((t) => t.status === "waiting").length;
    const paused = tasks.filter((t) => t.status === "paused").length;
    const completed = tasks.filter((t) => t.status === "completed").length;
    const failed = tasks.filter((t) => t.status === "error").length;
    // 全局下载速度（所有下载中任务的已下载字节之和的简化估算）
    const totalDownloaded = tasks
      .filter((t) => t.status === "downloading")
      .reduce((sum, t) => sum + t.downloaded, 0);
    return { downloading, waiting, paused, completed, failed, totalDownloaded };
  }, [tasks]);

  // 把并发下载的多个工具进度聚合为单个对象，供任务行展示
  const combinedToolInfo = useMemo(() => {
    if (downloadingTools.length === 0) return null;
    let downloaded = 0;
    let total = 0;
    for (const t of downloadingTools) {
      const info = toolDownloadInfoMap[t];
      if (info) {
        downloaded += info.downloaded;
        total += info.total;
      }
    }
    return {
      tool:
        downloadingTools.length > 1 ? "yt-dlp + ffmpeg" : downloadingTools[0],
      downloaded,
      total,
    };
  }, [downloadingTools, toolDownloadInfoMap]);

  // 开始下载
  const handleStartDownload = useCallback(async () => {
    const urls = (urlInput.match(/https?:\/\/[^\s'"<>`\]]+/g) || [])
      .map((u) => u.trim())
      .filter((u) => u.length > 0);

    if (urls.length === 0) {
      toast.error("请输入有效的视频链接");
      return;
    }

    let added = false;
    if (urls.length === 1) {
      const id = await addDownload(urls[0]);
      if (id) {
        added = true;
        setUrlInput("");
      }
    } else {
      const ids = await addBatchDownload(urls);
      if (ids.length > 0) {
        added = true;
        setUrlInput("");
      }
    }

    if (added) {
      toast.success(
        urls.length === 1
          ? "已添加到下载队列"
          : `已添加 ${urls.length} 个下载任务`,
      );
    }
  }, [urlInput, addDownload, addBatchDownload]);

  // 选择下载目录
  const handleChooseDir = useCallback(async () => {
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "选择下载目录",
      });
      if (selected && typeof selected === "string") {
        updateSettings({ ...settings, download_dir: selected });
      }
    } catch (e) {
      console.error("选择目录失败:", e);
    }
  }, [settings, updateSettings]);

  // 打开下载目录
  const handleOpenDir = useCallback(async () => {
    if (settings.download_dir) {
      try {
        await openDir(settings.download_dir);
      } catch (e) {
        console.error("打开目录失败:", e);
      }
    }
  }, [settings.download_dir, openDir]);

  // 取消任务
  const handleCancel = useCallback(
    async (task: DownloadTask) => {
      await cancelTask(task.id);
      toast.info("已取消下载");
    },
    [cancelTask],
  );

  // 切换 Cookie 设置
  const toggleCookieFromBrowser = useCallback(() => {
    updateSettings({
      ...settings,
      cookie_from_browser: !settings.cookie_from_browser,
    });
  }, [settings, updateSettings]);

  // 切换粘贴自动下载
  const toggleAutoPaste = useCallback(() => {
    updateSettings({
      ...settings,
      auto_paste_download: !settings.auto_paste_download,
    });
  }, [settings, updateSettings]);

  // 切换并发数
  const handleConcurrentChange = useCallback(
    (value: string) => {
      updateSettings({ ...settings, max_concurrent: parseInt(value, 10) });
    },
    [settings, updateSettings],
  );

  const urlCount = useMemo(() => {
    return urlInput.split("\n").filter((u) => u.trim().startsWith("http"))
      .length;
  }, [urlInput]);

  const content = (
    <Tabs
      defaultValue="tasks"
      value={activeTab}
      onValueChange={setActiveTab}
      className="flex-1 flex flex-col min-h-0"
    >
      <TabsList
        className={`w-full ${(subPage || embedded) && "!bg-transparent !p-0 !h-auto !rounded-none justify-start gap-4 border-b pb-3 mb-0"}`}
      >
        <TabsTrigger
          value="tasks"
          className={`flex items-center gap-2 ${(subPage || embedded) && "!flex-none !rounded-none !bg-transparent !shadow-none data-[state=active]:!bg-transparent data-[state=active]:!text-foreground data-[state=active]:!shadow-none text-muted-foreground hover:text-foreground !px-1 !py-1 text-xs"}`}
        >
          <LuDownload className="w-4 h-4" />
          任务列表
          {stats.downloading > 0 && (
            <Badge variant="secondary" className="ml-1 text-[10px] h-4 px-1.5">
              {stats.downloading}
            </Badge>
          )}
        </TabsTrigger>
        <TabsTrigger
          value="settings"
          className={`flex items-center gap-2 ${(subPage || embedded) && "!flex-none !rounded-none !bg-transparent !shadow-none data-[state=active]:!bg-transparent data-[state=active]:!text-foreground data-[state=active]:!shadow-none text-muted-foreground hover:text-foreground !px-1 !py-1 text-xs"}`}
        >
          <LuSettings className="w-4 h-4" />
          下载设置
        </TabsTrigger>
      </TabsList>

      <TabsContent
        value="tasks"
        className="flex-1 flex flex-col mt-4 gap-4 min-h-0"
      >
        {/* 链接输入区 */}
        <div className="bg-card border rounded-lg p-4 shrink-0">
          <div className="flex items-center gap-2 mb-2">
            <LuLink className="w-4 h-4 text-muted-foreground" />
            <span className="text-sm font-medium">粘贴链接下载</span>
          </div>
          <p className="text-xs text-muted-foreground mb-3">
            支持 YouTube、抖音、B站、Twitter 等 1000+ 网站，每行一个链接
          </p>
          <Textarea
            placeholder="粘贴视频链接，每行一个&#10;例如：&#10;https://www.youtube.com/watch?v=xxx&#10;https://www.douyin.com/video/xxx"
            value={urlInput}
            onChange={(e) => setUrlInput(e.target.value)}
            className="h-24 resize-none font-mono text-xs"
          />
          <div className="flex items-center justify-between mt-3">
            <div className="flex items-center gap-3">
              <span className="text-xs text-muted-foreground">
                {urlCount} 个链接
              </span>
              <button
                type="button"
                role="switch"
                aria-checked={settings.auto_paste_download}
                onClick={toggleAutoPaste}
                className={`relative inline-flex h-4 w-7 items-center rounded-full transition-colors ${
                  settings.auto_paste_download ? "bg-primary" : "bg-muted"
                }`}
                title="开启后复制视频链接自动添加到下载队列"
              >
                <span
                  className={`inline-block h-3 w-3 transform rounded-full bg-white transition-transform ${
                    settings.auto_paste_download
                      ? "translate-x-3.5"
                      : "translate-x-0.5"
                  }`}
                />
              </button>
              <span className="text-xs text-muted-foreground">
                复制自动下载
              </span>
            </div>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={async () => {
                  const count = await pasteAndDownload();
                  if (count > 0) {
                    toast.success(`已从剪贴板添加 ${count} 个任务`);
                  } else {
                    toast.info("剪贴板没有视频链接");
                  }
                }}
              >
                粘贴并下载
              </Button>
              <Button onClick={handleStartDownload} size="sm">
                <LuPlay className="w-4 h-4 mr-1" />
                开始下载
              </Button>
            </div>
          </div>
        </div>

        {/* 下载统计 */}
        <div className="grid grid-cols-5 gap-2 shrink-0">
          <div className="bg-card border rounded-lg p-3 text-center">
            <div className="text-xl font-bold text-primary">
              {stats.downloading}
            </div>
            <div className="text-xs text-muted-foreground">下载中</div>
          </div>
          <div className="bg-card border rounded-lg p-3 text-center">
            <div className="text-xl font-bold">{stats.waiting}</div>
            <div className="text-xs text-muted-foreground">等待中</div>
          </div>
          <div className="bg-card border rounded-lg p-3 text-center">
            <div className="text-xl font-bold text-warning">{stats.paused}</div>
            <div className="text-xs text-muted-foreground">已暂停</div>
          </div>
          <div className="bg-card border rounded-lg p-3 text-center">
            <div className="text-xl font-bold text-success">
              {stats.completed}
            </div>
            <div className="text-xs text-muted-foreground">已完成</div>
          </div>
          <div className="bg-card border rounded-lg p-3 text-center">
            <div className="text-xl font-bold text-destructive">
              {stats.failed}
            </div>
            <div className="text-xs text-muted-foreground">失败</div>
          </div>
        </div>

        {/* 任务列表 */}
        <div className="flex items-center justify-between shrink-0">
          <div className="flex items-center gap-2">
            {updatingTool && updateStatus && (
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <LuLoader className="w-3.5 h-3.5 animate-spin" />
                <span>{updateStatus}</span>
                {combinedToolInfo && (
                  <span>
                    {(combinedToolInfo.downloaded / 1024 / 1024).toFixed(1)} /{" "}
                    {combinedToolInfo.total > 0
                      ? (combinedToolInfo.total / 1024 / 1024).toFixed(1)
                      : "?"}{" "}
                    MB
                  </span>
                )}
              </div>
            )}
            {downloadingTools.length > 0 && combinedToolInfo && (
              <Progress
                value={
                  combinedToolInfo.total > 0
                    ? (combinedToolInfo.downloaded / combinedToolInfo.total) *
                      100
                    : 0
                }
                className="w-24 h-2"
              />
            )}
            <Button
              variant="outline"
              size="sm"
              onClick={handleUpdateTool}
              disabled={updatingTool}
              title="更新 yt-dlp"
            >
              {updatingTool ? (
                <LuLoader className="w-4 h-4 mr-1 animate-spin" />
              ) : (
                <LuRefreshCw className="w-4 h-4 mr-1" />
              )}
              {updatingTool ? "更新中" : "更新工具"}
            </Button>
            <Button variant="ghost" size="sm" onClick={loadTasks}>
              <LuRefreshCw className="w-4 h-4 mr-1" />
              刷新
            </Button>
          </div>
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              onClick={pauseAll}
              disabled={stats.downloading + stats.waiting === 0}
              title="全部暂停"
            >
              <LuPause className="w-3.5 h-3.5 mr-1" />
              全部暂停
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={retryAll}
              disabled={stats.failed + stats.paused === 0}
              title="全部重试"
            >
              <LuRotateCcw className="w-3.5 h-3.5 mr-1" />
              全部重试
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={clearFinished}
              disabled={stats.completed + stats.failed === 0}
              title="清除已完成"
            >
              <LuTrash2 className="w-3.5 h-3.5 mr-1" />
              清除已完成
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                if (confirm("确定要删除全部任务吗？")) {
                  deleteAll();
                }
              }}
              disabled={tasks.length === 0}
              className="text-destructive hover:text-destructive"
              title="全部删除"
            >
              <LuX className="w-3.5 h-3.5 mr-1" />
              全部删除
            </Button>
            <Button variant="ghost" size="sm" onClick={handleOpenDir}>
              <LuFolderOpen className="w-4 h-4 mr-1" />
              打开目录
            </Button>
          </div>
        </div>

        <ScrollArea className="flex-1 min-h-0 border rounded-lg">
          {tasks.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-16 text-muted-foreground">
              <LuDownload className="w-12 h-12 mb-3 opacity-30" />
              <p className="text-sm">暂无下载任务</p>
              <p className="text-xs mt-1">粘贴视频链接开始下载</p>
            </div>
          ) : (
            <div className="divide-y">
              {tasks.map((task, idx) => {
                // 第一个等待中的任务显示工具下载进度
                const showToolProgress =
                  downloadingTools.length > 0 &&
                  combinedToolInfo &&
                  task.status === "waiting" &&
                  idx === tasks.findIndex((t) => t.status === "waiting");
                return (
                  <TaskRow
                    key={task.id}
                    task={task}
                    onPause={() => pauseTask(task.id)}
                    onResume={() => resumeTask(task.id)}
                    onRetry={() => retryTask(task.id)}
                    onDelete={() => deleteTask(task.id)}
                    onCancel={() => handleCancel(task)}
                    onOpenFile={openFile}
                    onOpenDir={openDir}
                    toolDownloadInfo={
                      showToolProgress ? combinedToolInfo : null
                    }
                  />
                );
              })}
            </div>
          )}
        </ScrollArea>
      </TabsContent>

      <TabsContent
        value="settings"
        className="flex-1 mt-4 space-y-6 min-h-0 overflow-auto"
      >
        <div className="bg-card border rounded-lg p-4 space-y-5">
          <h3 className="text-sm font-medium mb-2">下载设置</h3>

          {/* 下载目录 */}
          <div className="space-y-2">
            <Label>下载目录</Label>
            <div className="flex gap-2">
              <Input
                value={settings.download_dir}
                readOnly
                placeholder="默认下载目录"
                className="flex-1"
              />
              <Button variant="outline" onClick={handleChooseDir}>
                <LuFolderOpen className="w-4 h-4 mr-1" />
                选择
              </Button>
            </div>
          </div>

          {/* 最大分辨率 */}
          <div className="space-y-2">
            <Label>最大分辨率</Label>
            <Select
              value={settings.max_height.toString()}
              onValueChange={(v) =>
                updateSettings({ ...settings, max_height: parseInt(v, 10) })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="0">最高画质</SelectItem>
                <SelectItem value="2160">4K (2160p)</SelectItem>
                <SelectItem value="1440">2K (1440p)</SelectItem>
                <SelectItem value="1080">1080p</SelectItem>
                <SelectItem value="720">720p</SelectItem>
                <SelectItem value="480">480p</SelectItem>
                <SelectItem value="360">360p</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {/* 并发数 */}
          <div className="space-y-2">
            <Label>同时下载数</Label>
            <Select
              value={settings.max_concurrent.toString()}
              onValueChange={handleConcurrentChange}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="0">无限制</SelectItem>
                <SelectItem value="1">1 个</SelectItem>
                <SelectItem value="2">2 个</SelectItem>
                <SelectItem value="3">3 个</SelectItem>
                <SelectItem value="5">5 个</SelectItem>
                <SelectItem value="10">10 个</SelectItem>
                <SelectItem value="20">20 个</SelectItem>
                <SelectItem value="50">50 个</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {/* 下载代理 */}
          <div className="space-y-2">
            <Label>下载代理</Label>
            <Select
              value={settings.proxy_id ?? "direct"}
              onValueChange={(v) =>
                updateSettings({
                  ...settings,
                  proxy_id: v === "direct" ? null : v,
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="direct">直连（不使用代理）</SelectItem>
                {proxyList.length === 0 && (
                  <SelectItem value="no-proxies" disabled>
                    暂无代理，请先添加代理
                  </SelectItem>
                )}
                {proxyList.map((p) => (
                  <SelectItem key={p.id} value={p.id}>
                    {p.name} ({p.proxy_type}://{p.host}:{p.port})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {settings.proxy_id && (
              <p className="text-xs text-muted-foreground">
                视频下载将通过所选代理进行
              </p>
            )}
          </div>

          {/* 使用浏览器 Cookie */}
          <div className="space-y-2">
            <div className="flex items-center justify-between py-2">
              <div className="space-y-0.5">
                <Label>使用浏览器 Cookie</Label>
                <p className="text-xs text-muted-foreground">
                  从爆文浏览器导出 Cookie，自动登录会员视频网站
                </p>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={settings.cookie_from_browser}
                onClick={toggleCookieFromBrowser}
                className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                  settings.cookie_from_browser ? "bg-primary" : "bg-muted"
                }`}
              >
                <span
                  className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
                    settings.cookie_from_browser
                      ? "translate-x-4"
                      : "translate-x-0.5"
                  }`}
                />
              </button>
            </div>
            {settings.cookie_from_browser && (
              <div className="flex items-center justify-between pl-1">
                <div className="text-xs text-muted-foreground">
                  {lastCookieTime > 0
                    ? `上次获取: ${formatCookieTime(lastCookieTime)}`
                    : "尚未获取 Cookie"}
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={isRefreshingCookie}
                  onClick={async () => {
                    const ok = await refreshCookie();
                    if (ok) {
                      toast.success("Cookie 已刷新");
                    } else {
                      toast.error("Cookie 刷新失败");
                    }
                  }}
                >
                  <LuRefreshCw
                    className={`w-3.5 h-3.5 mr-1 ${isRefreshingCookie ? "animate-spin" : ""}`}
                  />
                  {isRefreshingCookie ? "刷新中..." : "刷新 Cookie"}
                </Button>
              </div>
            )}
          </div>
        </div>

        {/* 下载工具状态 */}
        <div className="bg-card border rounded-lg p-4 space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">下载工具</h3>
            <Button variant="ghost" size="sm" onClick={checkTools}>
              <LuRefreshCw className="w-3.5 h-3.5 mr-1" />
              刷新
            </Button>
          </div>

          <div className="space-y-3">
            {/* yt-dlp */}
            <div className="p-2 bg-muted/30 rounded-md space-y-2">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                  <div
                    className={`w-2 h-2 rounded-full ${
                      toolStatus?.yt_dlp ? "bg-success" : "bg-destructive"
                    }`}
                  />
                  <div>
                    <div className="text-sm font-medium">yt-dlp</div>
                    <div className="text-xs text-muted-foreground">
                      {toolStatus?.yt_dlp
                        ? toolStatus.yt_dlp_version
                          ? `v${toolStatus.yt_dlp_version}`
                          : "已就绪"
                        : "未安装"}
                    </div>
                  </div>
                </div>
                {toolStatus?.yt_dlp ? (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={downloadingTools.includes("yt-dlp")}
                    onClick={async () => {
                      setToolDownloadProgress((prev) => ({
                        ...prev,
                        "yt-dlp": 0,
                      }));
                      setDownloadingTools(["yt-dlp"]);
                      const ok = await downloadYtDlp();
                      setDownloadingTools([]);
                      if (ok) {
                        toast.success("yt-dlp 更新成功");
                        checkTools();
                      } else {
                        toast.error("yt-dlp 更新失败");
                      }
                    }}
                  >
                    {downloadingTools.includes("yt-dlp") ? "更新中..." : "更新"}
                  </Button>
                ) : (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={downloadingTools.includes("yt-dlp")}
                    onClick={async () => {
                      setToolDownloadProgress((prev) => ({
                        ...prev,
                        "yt-dlp": 0,
                      }));
                      setDownloadingTools(["yt-dlp"]);
                      const ok = await downloadYtDlp();
                      setDownloadingTools([]);
                      if (ok) {
                        toast.success("yt-dlp 下载完成");
                        checkTools();
                      } else {
                        toast.error("yt-dlp 下载失败");
                      }
                    }}
                  >
                    {downloadingTools.includes("yt-dlp") ? "下载中..." : "下载"}
                  </Button>
                )}
              </div>
              {downloadingTools.includes("yt-dlp") && (
                <div className="space-y-1">
                  <Progress
                    value={toolDownloadProgress["yt-dlp"] || 0}
                    className="h-1.5"
                  />
                  <div className="text-xs text-muted-foreground text-right">
                    {(toolDownloadProgress["yt-dlp"] || 0).toFixed(1)}%
                  </div>
                </div>
              )}
            </div>

            {/* ffmpeg */}
            <div className="p-2 bg-muted/30 rounded-md space-y-2">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                  <div
                    className={`w-2 h-2 rounded-full ${
                      toolStatus?.ffmpeg ? "bg-success" : "bg-destructive"
                    }`}
                  />
                  <div>
                    <div className="text-sm font-medium">ffmpeg</div>
                    <div className="text-xs text-muted-foreground">
                      {toolStatus?.ffmpeg
                        ? toolStatus.ffmpeg_version
                          ? `v${toolStatus.ffmpeg_version}`
                          : "已就绪"
                        : "未安装（视频合并需要）"}
                    </div>
                  </div>
                </div>
                {toolStatus?.ffmpeg ? (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={downloadingTools.includes("ffmpeg")}
                    onClick={async () => {
                      setToolDownloadProgress((prev) => ({
                        ...prev,
                        ffmpeg: 0,
                      }));
                      setDownloadingTools(["ffmpeg"]);
                      const ok = await downloadFfmpeg();
                      setDownloadingTools([]);
                      if (ok) {
                        toast.success("ffmpeg 更新成功");
                        checkTools();
                      } else {
                        toast.error("ffmpeg 更新失败");
                      }
                    }}
                  >
                    {downloadingTools.includes("ffmpeg") ? "更新中..." : "更新"}
                  </Button>
                ) : (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={downloadingTools.includes("ffmpeg")}
                    onClick={async () => {
                      setToolDownloadProgress((prev) => ({
                        ...prev,
                        ffmpeg: 0,
                      }));
                      setDownloadingTools(["ffmpeg"]);
                      const ok = await downloadFfmpeg();
                      setDownloadingTools([]);
                      if (ok) {
                        toast.success("ffmpeg 下载完成");
                        checkTools();
                      } else {
                        toast.error("ffmpeg 下载失败");
                      }
                    }}
                  >
                    {downloadingTools.includes("ffmpeg") ? "下载中..." : "下载"}
                  </Button>
                )}
              </div>
              {downloadingTools.includes("ffmpeg") && (
                <div className="space-y-1">
                  <Progress
                    value={toolDownloadProgress.ffmpeg || 0}
                    className="h-1.5"
                  />
                  <div className="text-xs text-muted-foreground text-right">
                    {(toolDownloadProgress.ffmpeg || 0).toFixed(1)}%
                  </div>
                </div>
              )}
            </div>

            {/* ffprobe */}
            <div className="flex items-center justify-between p-2 bg-muted/30 rounded-md">
              <div className="flex items-center gap-3">
                <div
                  className={`w-2 h-2 rounded-full ${
                    toolStatus?.ffprobe ? "bg-success" : "bg-destructive"
                  }`}
                />
                <div>
                  <div className="text-sm font-medium">ffprobe</div>
                  <div className="text-xs text-muted-foreground">
                    {toolStatus?.ffprobe
                      ? toolStatus.ffprobe_version
                        ? `v${toolStatus.ffprobe_version}`
                        : "已就绪"
                      : "未安装"}
                  </div>
                </div>
              </div>
              <div className="text-xs text-muted-foreground">
                随 ffmpeg 一起更新
              </div>
            </div>
          </div>

          <p className="text-xs text-muted-foreground">
            工具保存在应用数据目录的 video-tools 文件夹下
          </p>
        </div>

        <div className="bg-card border rounded-lg p-4 text-xs text-muted-foreground space-y-2">
          <h3 className="text-sm font-medium text-foreground mb-2">关于</h3>
          <p>• 基于 yt-dlp，支持 1000+ 视频网站</p>
          <p>• 自动使用爆文浏览器的登录 Cookie</p>
          <p>• 视频默认保存为 MP4 格式</p>
        </div>
      </TabsContent>
    </Tabs>
  );

  if (embedded) {
    return (
      <div className="flex h-full min-h-0 w-full flex-col gap-0 overflow-hidden p-6">
        {content}
      </div>
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className={`max-w-4xl flex flex-col ${subPage ? "!bg-transparent !border-none !shadow-none" : ""}`}
      >
        <DialogHeader className={subPage ? "hidden" : ""}>
          <DialogTitle className="flex items-center gap-2">
            <LuDownload className="w-5 h-5" />
            视频下载
          </DialogTitle>
        </DialogHeader>

        {content}
      </DialogContent>
    </Dialog>
  );
}

function formatCookieTime(timestamp: number): string {
  if (!timestamp) return "从未获取";
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const diff = now.getTime() - date.getTime();
  const minutes = Math.floor(diff / 60000);
  const hours = Math.floor(diff / 3600000);

  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  if (hours < 24) return `${hours} 小时前`;
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function TaskRow({
  task,
  onPause,
  onResume,
  onRetry,
  onDelete,
  onCancel,
  onOpenFile,
  onOpenDir,
  toolDownloadInfo,
}: {
  task: DownloadTask;
  onPause: () => void;
  onResume: () => void;
  onRetry: () => void;
  onDelete: () => void;
  onCancel: () => void;
  onOpenFile: (path: string) => void;
  onOpenDir: (path: string) => void;
  toolDownloadInfo: {
    tool: string;
    downloaded: number;
    total: number;
  } | null;
}) {
  const statusConfig: Record<
    string,
    {
      label: string;
      variant: "secondary" | "default" | "destructive" | "outline";
      icon: typeof LuPlay;
    }
  > = {
    waiting: { label: "等待中", variant: "secondary", icon: LuPlay },
    downloading: { label: "下载中", variant: "default", icon: LuPlay },
    paused: { label: "已暂停", variant: "secondary", icon: LuPause },
    completed: { label: "已完成", variant: "default", icon: LuCheck },
    error: { label: "失败", variant: "destructive", icon: LuX },
    cancelled: { label: "已取消", variant: "outline", icon: LuX },
  };

  const config = statusConfig[task.status] || statusConfig.waiting;
  const StatusIcon = config.icon;

  const formatSize = (bytes: number) => {
    if (bytes >= 1024 * 1024 * 1024)
      return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
    if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(2)} KB`;
    return `${bytes} B`;
  };

  const formatDuration = (seconds: number) => {
    if (seconds < 60) return `${Math.floor(seconds)} 秒`;
    if (seconds < 3600)
      return `${Math.floor(seconds / 60)} 分 ${Math.floor(seconds % 60)} 秒`;
    return `${Math.floor(seconds / 3600)} 时 ${Math.floor((seconds % 3600) / 60)} 分`;
  };

  const remaining =
    task.total > task.downloaded ? task.total - task.downloaded : 0;

  return (
    <div className="p-3 hover:bg-muted/30 transition-colors flex items-start gap-3">
      {task.thumbnail && (
        <button
          type="button"
          className={
            task.filename
              ? "shrink-0 cursor-pointer hover:opacity-80 transition-opacity"
              : "shrink-0 cursor-default"
          }
          onClick={() => {
            if (task.filename) onOpenFile(task.filename);
          }}
          disabled={!task.filename}
        >
          <img
            src={task.thumbnail}
            alt=""
            className="w-24 h-14 object-cover rounded border bg-muted/50"
            loading="lazy"
            referrerPolicy="no-referrer"
          />
        </button>
      )}
      <div className="flex-1 min-w-0">
        <div className="flex items-start justify-between gap-3 mb-2">
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 mb-1">
              <Badge
                variant={config.variant}
                className="text-[10px] h-4 px-1.5 font-normal"
              >
                <StatusIcon className="w-3 h-3 mr-0.5" />
                {config.label}
              </Badge>
              {task.speed_text && task.status === "downloading" && (
                <span className="text-xs text-muted-foreground">
                  {task.speed_text}
                </span>
              )}
              {task.total > 0 && task.status === "downloading" && (
                <span className="text-xs text-muted-foreground">
                  剩余 {formatSize(remaining)}
                </span>
              )}
            </div>
            <p
              className="text-sm font-medium truncate"
              title={task.title || task.filename || task.url}
            >
              {task.title || task.filename || task.url}
            </p>
            {!task.title && task.filename && (
              <p
                className="text-xs text-muted-foreground truncate"
                title={task.url}
              >
                {task.url}
              </p>
            )}
            {task.error_msg && (
              <p className="text-xs text-destructive mt-1 line-clamp-2">
                {task.error_msg}
              </p>
            )}
          </div>
          <div className="flex items-center gap-1 shrink-0">
            {task.status === "downloading" && (
              <Button variant="ghost" size="sm" onClick={onPause} title="暂停">
                <LuPause className="w-4 h-4" />
              </Button>
            )}
            {task.status === "paused" && (
              <Button variant="ghost" size="sm" onClick={onResume} title="继续">
                <LuPlay className="w-4 h-4" />
              </Button>
            )}
            {task.status === "error" && (
              <Button variant="ghost" size="sm" onClick={onRetry} title="重试">
                <LuRotateCcw className="w-4 h-4" />
              </Button>
            )}
            {(task.status === "waiting" ||
              task.status === "downloading" ||
              task.status === "paused") && (
              <Button variant="ghost" size="sm" onClick={onCancel} title="取消">
                <LuX className="w-4 h-4" />
              </Button>
            )}
            <Button variant="ghost" size="sm" onClick={onDelete} title="删除">
              <LuTrash2 className="w-4 h-4" />
            </Button>
          </div>
        </div>

        {(task.status === "downloading" ||
          task.status === "paused" ||
          task.status === "waiting") && (
          <div className="mt-2 space-y-1.5">
            {toolDownloadInfo ? (
              <>
                <Progress
                  value={
                    toolDownloadInfo.total > 0
                      ? (toolDownloadInfo.downloaded / toolDownloadInfo.total) *
                        100
                      : 0
                  }
                  className="h-2"
                />
                <div className="flex items-center justify-between text-xs text-muted-foreground">
                  <span className="font-medium flex items-center gap-1">
                    <LuDownload className="w-3 h-3" />
                    正在下载 {toolDownloadInfo.tool}
                  </span>
                  <span>
                    {(toolDownloadInfo.downloaded / 1024 / 1024).toFixed(1)} MB
                    /{" "}
                    {toolDownloadInfo.total > 0
                      ? (toolDownloadInfo.total / 1024 / 1024).toFixed(1)
                      : "?"}{" "}
                    MB
                  </span>
                </div>
              </>
            ) : task.total > 0 ? (
              <>
                <Progress value={task.progress} className="h-2" />
                <div className="flex items-center justify-between text-xs text-muted-foreground">
                  <span className="font-medium">
                    {task.progress.toFixed(1)}%
                  </span>
                  <span>
                    {formatSize(task.downloaded)} / {formatSize(task.total)}
                  </span>
                  {task.speed_text && <span>{task.speed_text}</span>}
                </div>
              </>
            ) : (
              <div className="flex items-center gap-2 text-xs text-muted-foreground">
                <LuLoader className="w-3.5 h-3.5 animate-spin" />
                <span>
                  {task.status === "waiting"
                    ? "等待下载..."
                    : task.status === "paused"
                      ? "已暂停"
                      : task.status_text || "正在解析视频信息..."}
                </span>
              </div>
            )}
          </div>
        )}

        {task.status === "completed" && (
          <div className="space-y-2">
            <div className="h-0.5 rounded-full bg-success/30 overflow-hidden">
              <div className="h-full bg-success w-full" />
            </div>
            <div className="flex items-center gap-4 text-xs text-muted-foreground">
              {task.total > 0 && <span>{formatSize(task.total)}</span>}
              {task.resolution && <span>{task.resolution}</span>}
              {task.peak_speed_text && <span>峰值 {task.peak_speed_text}</span>}
              {task.started_at && task.finished_at && (
                <span>
                  用时 {formatDuration(task.finished_at - task.started_at)}
                </span>
              )}
              {task.filename && (
                <>
                  <span className="flex-1" />
                  <button
                    type="button"
                    className="text-primary hover:underline flex items-center gap-1"
                    onClick={() => {
                      if (task.filename) onOpenFile(task.filename);
                    }}
                  >
                    打开视频
                  </button>
                  <button
                    type="button"
                    className="text-primary hover:underline flex items-center gap-1"
                    onClick={() => {
                      if (task.filename) onOpenDir(task.filename);
                    }}
                  >
                    <LuFolderOpen className="w-3 h-3" />
                    打开目录
                  </button>
                </>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
