import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

export type DownloadStatus =
  | "waiting"
  | "downloading"
  | "paused"
  | "completed"
  | "error"
  | "cancelled";

export interface DownloadTask {
  id: string;
  url: string;
  title: string | null;
  status: DownloadStatus;
  progress: number;
  speed_text: string;
  peak_speed_text: string;
  downloaded: number;
  total: number;
  filename: string | null;
  resolution: string | null;
  error_msg: string | null;
  status_text: string | null;
  started_at: number | null;
  finished_at: number | null;
  thumbnail: string | null;
}

export interface DownloadSettings {
  download_dir: string;
  max_concurrent: number;
  max_height: number;
  auto_paste_download: boolean;
  cookie_from_browser: boolean;
  proxy_id: string | null;
}

export interface ProxyItem {
  id: string;
  name: string;
  proxy_type: string;
  host: string;
  port: number;
}

export interface ToolStatus {
  yt_dlp: boolean;
  ffmpeg: boolean;
  ffprobe: boolean;
  yt_dlp_path: string | null;
  ffmpeg_path: string | null;
  ffprobe_path: string | null;
  yt_dlp_version: string | null;
  ffmpeg_version: string | null;
  ffprobe_version: string | null;
}

const defaultSettings: DownloadSettings = {
  download_dir: "",
  max_concurrent: 10,
  max_height: 0,
  auto_paste_download: false,
  cookie_from_browser: true,
  proxy_id: null,
};

export function useVideoDownload() {
  const [tasks, setTasks] = useState<DownloadTask[]>([]);
  const [settings, setSettings] = useState<DownloadSettings>(defaultSettings);
  const [toolStatus, setToolStatus] = useState<ToolStatus | null>(null);
  const [lastCookieTime, setLastCookieTime] = useState<number>(0);
  const [proxyList, setProxyList] = useState<ProxyItem[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isCheckingTools, setIsCheckingTools] = useState(false);
  const [isRefreshingCookie, setIsRefreshingCookie] = useState(false);
  const unlistenRef = useRef<Array<() => void>>([]);

  // 加载任务列表
  const loadTasks = useCallback(async () => {
    try {
      const result = await invoke<DownloadTask[]>("video_download_list_tasks");
      setTasks(result);
    } catch (e) {
      console.error("[video_download] 加载任务失败:", e);
    }
  }, []);

  // 加载设置
  const loadSettings = useCallback(async () => {
    try {
      const result = await invoke<DownloadSettings>(
        "video_download_get_settings",
      );
      setSettings(result);
    } catch (e) {
      console.error("[video_download] 加载设置失败:", e);
    }
  }, []);

  // 加载最后 cookie 获取时间
  const loadLastCookieTime = useCallback(async () => {
    try {
      const result = await invoke<number>(
        "video_download_get_last_cookie_time",
      );
      setLastCookieTime(result);
    } catch (e) {
      console.error("[video_download] 获取 cookie 时间失败:", e);
    }
  }, []);

  // 加载代理列表
  const loadProxies = useCallback(async () => {
    try {
      const result = await invoke<ProxyItem[]>("video_download_list_proxies");
      setProxyList(result);
    } catch (e) {
      console.error("[video_download] 加载代理列表失败:", e);
    }
  }, []);

  // 刷新 cookie
  const refreshCookie = useCallback(async (): Promise<boolean> => {
    setIsRefreshingCookie(true);
    try {
      const result = await invoke<number>("video_download_refresh_cookie");
      setLastCookieTime(result);
      return true;
    } catch (e) {
      console.error("[video_download] 刷新 cookie 失败:", e);
      return false;
    } finally {
      setIsRefreshingCookie(false);
    }
  }, []);

  // 打开目录（在资源管理器中显示）
  const openDir = useCallback(async (path: string) => {
    try {
      await invoke("video_download_open_dir", { path });
    } catch (e) {
      console.error("[video_download] 打开目录失败:", e);
    }
  }, []);

  // 用默认程序打开文件
  const openFile = useCallback(async (path: string) => {
    try {
      await invoke("video_download_open_file", { path });
    } catch (e) {
      console.error("[video_download] 打开文件失败:", e);
    }
  }, []);

  // 打开任务日志文件
  const openTaskLog = useCallback(async (taskId: string) => {
    try {
      const path = await invoke<string>("video_download_get_task_log_path", {
        taskId,
      });
      await invoke("video_download_open_file", { path });
    } catch (e) {
      console.error("[video_download] 打开日志失败:", e);
    }
  }, []);

  // 打开日志目录
  const openLogDir = useCallback(async () => {
    try {
      const path = await invoke<string>("video_download_get_log_dir");
      await invoke("video_download_open_dir", { path });
    } catch (e) {
      console.error("[video_download] 打开日志目录失败:", e);
    }
  }, []);

  // 轻量检查工具文件是否存在（不调版本检测，不阻塞下载流程）
  const toolsExist = useCallback(async () => {
    try {
      return await invoke<boolean>("video_download_tools_exist");
    } catch (e) {
      console.error("[video_download] tools_exist 检查失败:", e);
      return false;
    }
  }, []);

  // 添加下载任务
  const addDownload = useCallback(
    async (url: string): Promise<string | null> => {
      try {
        const id = await invoke<string>("video_download_add", { url });
        await loadTasks();
        return id;
      } catch (e) {
        console.error("[video_download] 添加任务失败:", e);
        return null;
      }
    },
    [loadTasks],
  );

  // 批量添加
  const addBatchDownload = useCallback(
    async (urls: string[]): Promise<string[]> => {
      try {
        const ids = await invoke<string[]>("video_download_add_batch", {
          urls,
        });
        await loadTasks();
        return ids;
      } catch (e) {
        console.error("[video_download] 批量添加失败:", e);
        return [];
      }
    },
    [loadTasks],
  );

  // 取消任务
  const cancelTask = useCallback(
    async (taskId: string) => {
      try {
        await invoke("video_download_cancel", { taskId });
        await loadTasks();
      } catch (e) {
        console.error("[video_download] 取消任务失败:", e);
      }
    },
    [loadTasks],
  );

  // 暂停任务
  const pauseTask = useCallback(
    async (taskId: string) => {
      try {
        await invoke("video_download_pause", { taskId });
        await loadTasks();
      } catch (e) {
        console.error("[video_download] 暂停任务失败:", e);
      }
    },
    [loadTasks],
  );

  // 继续任务
  const resumeTask = useCallback(
    async (taskId: string) => {
      try {
        await invoke("video_download_resume", { taskId });
        await loadTasks();
      } catch (e) {
        console.error("[video_download] 继续任务失败:", e);
      }
    },
    [loadTasks],
  );

  // 重试任务
  const retryTask = useCallback(
    async (taskId: string) => {
      try {
        await invoke("video_download_retry", { taskId });
        await loadTasks();
      } catch (e) {
        console.error("[video_download] 重试任务失败:", e);
      }
    },
    [loadTasks],
  );

  // 删除任务
  const deleteTask = useCallback(
    async (taskId: string) => {
      try {
        await invoke("video_download_delete", { taskId });
        await loadTasks();
      } catch (e) {
        console.error("[video_download] 删除任务失败:", e);
      }
    },
    [loadTasks],
  );

  // 清除已完成任务
  const clearFinished = useCallback(async () => {
    try {
      await invoke("video_download_clear_finished");
      await loadTasks();
    } catch (e) {
      console.error("[video_download] 清除失败:", e);
    }
  }, [loadTasks]);

  // 全部暂停
  const pauseAll = useCallback(async () => {
    try {
      await invoke("video_download_pause_all");
      await loadTasks();
    } catch (e) {
      console.error("[video_download] 全部暂停失败:", e);
    }
  }, [loadTasks]);

  // 全部重试
  const retryAll = useCallback(async () => {
    try {
      await invoke("video_download_retry_all");
      await loadTasks();
    } catch (e) {
      console.error("[video_download] 全部重试失败:", e);
    }
  }, [loadTasks]);

  // 启动等待中的下载（工具就绪后调用）
  const startPending = useCallback(async () => {
    try {
      await invoke("video_download_start_pending");
      await loadTasks();
    } catch (e) {
      console.error("[video_download] 启动等待下载失败:", e);
    }
  }, [loadTasks]);

  // 全部删除
  const deleteAll = useCallback(async () => {
    try {
      await invoke("video_download_delete_all");
      await loadTasks();
    } catch (e) {
      console.error("[video_download] 全部删除失败:", e);
    }
  }, [loadTasks]);

  // 更新设置
  const updateSettings = useCallback(async (newSettings: DownloadSettings) => {
    try {
      await invoke("video_download_update_settings", { settings: newSettings });
      setSettings(newSettings);
    } catch (e) {
      console.error("[video_download] 更新设置失败:", e);
    }
  }, []);

  // 设置 yt-dlp 路径
  const setYtDlpPath = useCallback(async (path: string) => {
    try {
      await invoke("video_download_set_yt_dlp_path", { path });
    } catch (e) {
      console.error("[video_download] 设置 yt-dlp 路径失败:", e);
    }
  }, []);

  // 监听进度事件
  useEffect(() => {
    let cancelled = false;

    const setupListeners = async () => {
      // 任务开始下载
      const unlistenStarted = await listen(
        "video-download:task-started",
        (event) => {
          const taskId = event.payload as string;
          if (cancelled) return;
          setTasks((prev) =>
            prev.map((t) =>
              t.id === taskId
                ? {
                    ...t,
                    status: "downloading" as const,
                    started_at: Date.now() / 1000,
                  }
                : t,
            ),
          );
        },
      );

      // 状态更新（解析阶段反馈）
      const unlistenStatus = await listen("video-download:status", (event) => {
        const payload = event.payload as {
          taskId: string;
          statusText: string;
          status: string;
        };
        if (cancelled) return;
        setTasks((prev) =>
          prev.map((t) =>
            t.id === payload.taskId
              ? {
                  ...t,
                  status_text: payload.statusText,
                  status: payload.status as DownloadStatus,
                }
              : t,
          ),
        );
      });

      // 进度更新
      const unlistenProgress = await listen(
        "video-download:progress",
        (event) => {
          const payload = event.payload as {
            taskId: string;
            progress: number;
            speedText: string;
            peakSpeedText: string;
            downloaded: number;
            total: number;
            status: string;
          };
          if (cancelled) return;
          setTasks((prev) =>
            prev.map((t) =>
              t.id === payload.taskId
                ? {
                    ...t,
                    progress: payload.progress,
                    speed_text: payload.speedText,
                    peak_speed_text: payload.peakSpeedText,
                    downloaded: payload.downloaded,
                    total: payload.total,
                    status: payload.status as DownloadStatus,
                  }
                : t,
            ),
          );
        },
      );

      // 任务完成
      const unlistenCompleted = await listen(
        "video-download:task-completed",
        (event) => {
          const task = event.payload as DownloadTask;
          if (cancelled) return;
          setTasks((prev) =>
            prev.map((t) => (t.id === task.id ? { ...t, ...task } : t)),
          );
        },
      );

      // 任务错误
      const unlistenError = await listen(
        "video-download:task-error",
        (event) => {
          const task = event.payload as DownloadTask;
          if (cancelled) return;
          setTasks((prev) =>
            prev.map((t) => (t.id === task.id ? { ...t, ...task } : t)),
          );
        },
      );

      // 任务添加
      const unlistenAdded = await listen("video-download:task-added", () => {
        if (cancelled) return;
        loadTasks();
      });

      // Cookie 刷新
      const unlistenCookieRefreshed = await listen(
        "video-download:cookie-refreshed",
        (event) => {
          if (cancelled) return;
          setLastCookieTime(event.payload as number);
        },
      );

      // 任务删除
      const unlistenTaskDeleted = await listen(
        "video-download:task-deleted",
        () => {
          if (cancelled) return;
          loadTasks();
        },
      );


      // 任务元数据更新（解析到缩略图/标题/清晰度时，实时刷新列表，让预览图及时出现）
      const unlistenTaskUpdated = await listen("video-download:task-updated", () => {
        if (cancelled) return;
        loadTasks();
      });

      unlistenRef.current = [
        unlistenStarted,
        unlistenProgress,
        unlistenStatus,
        unlistenCompleted,
        unlistenError,
        unlistenAdded,
        unlistenCookieRefreshed,
        unlistenTaskDeleted,
        unlistenTaskUpdated,
      ];
    };

    setupListeners();

    return () => {
      cancelled = true;
      unlistenRef.current.forEach((fn) => fn());
      unlistenRef.current = [];
    };
  }, [loadTasks]);

  // 初始加载
  useEffect(() => {
    setIsLoading(true);
    Promise.all([
      loadTasks(),
      loadSettings(),
      loadLastCookieTime(),
      loadProxies(),
    ]).finally(() => setIsLoading(false));
  }, [loadTasks, loadSettings, loadLastCookieTime, loadProxies]);

  // 检查工具状态
  const checkTools = useCallback(async (): Promise<ToolStatus | null> => {
    setIsCheckingTools(true);
    try {
      const result = await invoke<ToolStatus>("video_download_check_tools");
      setToolStatus(result);
      return result;
    } catch (e) {
      console.error("[video_download] 检查工具失败:", e);
      return null;
    } finally {
      setIsCheckingTools(false);
    }
  }, []);

  // 检查后端是否正在下载工具
  const isDownloadingTools = useCallback(async (): Promise<boolean> => {
    try {
      return await invoke<boolean>("video_download_is_downloading_tools");
    } catch {
      return false;
    }
  }, []);

  // 下载 yt-dlp
  const downloadYtDlp = useCallback(async (): Promise<boolean> => {
    try {
      await invoke("video_download_download_yt_dlp");
      await checkTools();
      return true;
    } catch (e) {
      console.error("[video_download] 下载 yt-dlp 失败:", e);
      return false;
    }
  }, [checkTools]);

  // 更新工具：先 yt-dlp -U，失败从 GitHub 下载
  const updateTool = useCallback(async (): Promise<boolean> => {
    try {
      await invoke<string>("video_download_update_tool");
      await checkTools();
      return true;
    } catch (e) {
      console.error("[video_download] 更新工具失败:", e);
      return false;
    }
  }, [checkTools]);

  // 下载 ffmpeg
  const downloadFfmpeg = useCallback(async (): Promise<boolean> => {
    try {
      await invoke("video_download_download_ffmpeg");
      await checkTools();
      return true;
    } catch (e) {
      console.error("[video_download] 下载 ffmpeg 失败:", e);
      return false;
    }
  }, [checkTools]);

  // 从剪贴板读取并下载
  const pasteAndDownload = useCallback(async (): Promise<number> => {
    try {
      const { readText } = await import("@tauri-apps/plugin-clipboard-manager");
      const text = await readText();
      if (!text) return 0;

      const urls = (text.match(/https?:\/\/[^\s'"<>`\]]+/g) || [])
        .map((u) => u.trim())
        .filter((u) => u.length > 0);

      if (urls.length === 0) return 0;

      const ids = await addBatchDownload(urls);
      return ids.length;
    } catch (e) {
      console.error("[video_download] 粘贴下载失败:", e);
      return 0;
    }
  }, [addBatchDownload]);

  // 注：剪贴板自动监控由后端 Rust 线程实现（200ms 轮询）
  // 前端只保留手动粘贴下载功能
  // 设置开关通过 updateSettings 传到后端，后端自动启停监控

  // 初始用轻量文件检查，不调版本检测，不阻塞下载流程
  useEffect(() => {
    toolsExist().then((exist) => {
      if (!exist) checkTools();
    });
  }, [toolsExist, checkTools]);

  return {
    tasks,
    settings,
    toolStatus,
    lastCookieTime,
    proxyList,
    isLoading,
    isCheckingTools,
    isRefreshingCookie,
    loadTasks,
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
    setYtDlpPath,
    checkTools,
    isDownloadingTools,
    toolsExist,
    downloadYtDlp,
    updateTool,
    downloadFfmpeg,
    pasteAndDownload,
    refreshCookie,
    loadLastCookieTime,
    loadProxies,
    openDir,
    openFile,
    openTaskLog,
    openLogDir,
  };
}
