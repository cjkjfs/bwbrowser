// video-matcher.tsx - 视频画面匹配组件（pHash MVP + 设置）

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

interface MatchSegment {
  sourceStart: number;
  sourceEnd: number;
  targetStart: number;
  targetEnd: number;
  similarity: number;
  matchedFrames: number;
}

interface MatcherSettings {
  cacheDir: string;
  extractor: string;
  intervalSec: number;
  threshold: number;
  minFrames: number;
}

interface MatcherProgress {
  stage: string;
  percent: number;
  message: string;
}

interface MatcherTask {
  id: string;
  sourcePath: string;
  targetPath: string;
  status: "waiting" | "running" | "completed" | "failed" | "cancelled";
  percent: number;
  message: string;
  segmentCount: number;
  createdAt: number;
  finishedAt: number | null;
  segments: MatchSegment[] | null;
}

interface ModelStatus {
  id: string;
  name: string;
  featureDim: number;
  approxSizeMb: number;
  imgSize: number;
  exists: boolean;
  path: string;
  sizeBytes: number;
  downloadUrl: string;
}

export function VideoMatchTab() {
  const [sourceVideo, setSourceVideo] = useState<string>(""); // 对标视频
  const [targetVideo, setTargetVideo] = useState<string>(""); // 原视频
  const [segments, setSegments] = useState<MatchSegment[]>([]);
  const [threshold, setThreshold] = useState(0.7);
  const [interval, setInterval] = useState(1.0);
  const [showSettings, setShowSettings] = useState(false);
  const [settings, setSettings] = useState<MatcherSettings | null>(null);
  const [defaultCacheDir, setDefaultCacheDir] = useState("");
  const [cacheCleared, setCacheCleared] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [draftPath, setDraftPath] = useState("");
  const [models, setModels] = useState<ModelStatus[]>([]);
  const [device, setDevice] = useState<string>("unknown");
  const [tasks, setTasks] = useState<MatcherTask[]>([]);
  // 每个模型的下载状态: modelId -> { percent, message }
  const [modelDownloads, setModelDownloads] = useState<
    Record<string, { percent: number; message: string }>
  >({});

  const sourceVideoRef = useRef<HTMLVideoElement>(null);
  const targetVideoRef = useRef<HTMLVideoElement>(null);

  // 加载设置 + 预热推理会话（让设备徽章立即可见）
  useEffect(() => {
    invoke("video_matcher_warmup").catch(() => {});
    // 预热是后台任务，轮询设备状态直到初始化完成（最多 30 秒）
    let pollId: number | undefined;
    const poll = () => {
      void (async () => {
        try {
          const d: string = await invoke("video_matcher_device");
          setDevice(d);
          if (d !== "unknown" && pollId) {
            clearInterval(pollId);
            pollId = undefined;
          }
        } catch {
          /* ignore */
        }
      })();
    };
    pollId = window.setInterval(poll, 1500);
    setTimeout(() => {
      if (pollId) clearInterval(pollId);
    }, 30000);
    async function loadSettings() {
      try {
        const s = await invoke<MatcherSettings>("video_matcher_get_settings");
        setSettings(s);
        setThreshold(s.threshold);
        setInterval(s.intervalSec);
        const def = await invoke<string>("video_matcher_default_cache_dir");
        setDefaultCacheDir(def);
        const ms = await invoke<ModelStatus[]>("video_matcher_model_status");
        setModels(ms);
        const d = await invoke<string>("video_matcher_device");
        setDevice(d);
      } catch (error) {
        console.error("加载设置失败:", error);
      }
    }
    loadSettings();
  }, []);

  const refreshDevice = useCallback(async () => {
    try {
      setDevice(await invoke<string>("video_matcher_device"));
    } catch (error) {
      console.error("查询设备失败:", error);
    }
  }, []);

  const refreshModels = useCallback(async () => {
    try {
      const ms = await invoke<ModelStatus[]>("video_matcher_model_status");
      setModels(ms);
    } catch (error) {
      console.error("刷新模型状态失败:", error);
    }
  }, []);

  const changeExtractor = async (extractor: string) => {
    if (!settings) return;
    const newSettings = { ...settings, extractor };
    setSettings(newSettings);
    setSegments([]);
    try {
      await invoke("video_matcher_save_settings", { settings: newSettings });
      await refreshModels();
    } catch (error) {
      console.error("切换模型失败:", error);
    }
  };

  const deleteModel = async (modelId: string) => {
    if (!confirm("确定删除该模型文件？下次使用会重新下载。")) return;
    try {
      await invoke("video_matcher_delete_model", { modelId });
      await refreshModels();
    } catch (error) {
      alert(`删除失败: ${error}`);
    }
  };

  // 监听匹配进度事件（模型下载走 model_download:<id> 分支，其余进主进度条）
  useEffect(() => {
    const unlisten = listen<MatcherProgress>("video-matcher-progress", (e) => {
      const p = e.payload;
      if (p.stage.startsWith("model_download:")) {
        const id = p.stage.slice("model_download:".length);
        setModelDownloads((prev) => ({
          ...prev,
          [id]: { percent: p.percent, message: p.message },
        }));
        if (p.percent >= 100) {
          refreshModels();
          // 完成后清掉该模型的下载态
          setTimeout(() => {
            setModelDownloads((prev) => {
              const next = { ...prev };
              delete next[id];
              return next;
            });
          }, 2000);
        }
        return;
      }
      if (p.stage === "done") {
        refreshDevice();
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refreshDevice, refreshModels]);

  const downloadModel = async (modelId: string) => {
    setModelDownloads((prev) => ({
      ...prev,
      [modelId]: { percent: 0, message: "准备下载..." },
    }));
    try {
      await invoke("video_matcher_download_model", { modelId });
    } catch (error) {
      const msg = String(error);
      if (!msg.includes("取消")) {
        alert(`下载失败: ${msg}`);
      }
      setModelDownloads((prev) => {
        const next = { ...prev };
        delete next[modelId];
        return next;
      });
    }
  };

  const cancelModelDownload = async (modelId: string) => {
    try {
      await invoke("video_matcher_cancel_download", { modelId });
    } catch (error) {
      console.error("取消失败:", error);
    }
  };

  const saveSettings = async (newSettings: MatcherSettings) => {
    setSettings(newSettings);
    try {
      await invoke("video_matcher_save_settings", { settings: newSettings });
    } catch (error) {
      console.error("保存设置失败:", error);
    }
  };

  const handleClearCache = async () => {
    try {
      const dir = settings?.cacheDir || undefined;
      const count = await invoke<number>("video_matcher_clear_cache", {
        cacheDir: dir,
      });
      setCacheCleared(true);
      setTimeout(() => setCacheCleared(false), 2000);
      alert(`已清除 ${count} 个缓存文件`);
    } catch (error) {
      alert(`清除缓存失败: ${error}`);
    }
  };

  const pickCacheDir = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && settings) {
      await saveSettings({ ...settings, cacheDir: selected as string });
    }
  };

  const pickVideo = async (setter: (path: string) => void) => {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: "Video Files",
          extensions: ["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm"],
        },
      ],
    });
    if (selected) {
      setter(selected as string);
      setSegments([]);
    }
  };

  // 把当前选择加入匹配任务队列（可连续添加多个，后端串行执行）
  // 只选原视频、未选对标 = 预缓存任务：仅提前提取原视频特征入缓存
  const handleAddTask = async () => {
    if (!targetVideo) return;
    try {
      await invoke("video_matcher_add_task", {
        sourcePath: sourceVideo,
        targetPath: targetVideo,
      });
      await invoke("video_matcher_run_queue");
      await refreshTasks();
      // 加入队列后清空选择，方便继续添加下一对
      setSourceVideo("");
      setTargetVideo("");
      setSegments([]);
    } catch (error) {
      alert(`添加任务失败: ${error}`);
    }
  };

  const refreshTasks = useCallback(async () => {
    try {
      const t = await invoke<MatcherTask[]>("video_matcher_list_tasks");
      setTasks(t);
    } catch (error) {
      console.error("刷新任务失败:", error);
    }
  }, []);

  // 订阅任务列表变化与单任务进度
  useEffect(() => {
    const un1 = listen("video-matcher-tasks-changed", () => {
      void refreshTasks();
    });
    const un2 = listen<MatcherProgress>("video-matcher-task-progress", () => {
      void refreshTasks();
    });
    void refreshTasks();
    return () => {
      un1.then((fn) => fn());
      un2.then((fn) => fn());
    };
  }, [refreshTasks]);

  const cancelTask = async (taskId: string) => {
    try {
      await invoke("video_matcher_cancel_task", { taskId });
      await refreshTasks();
    } catch (error) {
      alert(`取消失败: ${error}`);
    }
  };

  const deleteTask = async (taskId: string) => {
    try {
      await invoke("video_matcher_delete_task", { taskId });
      await refreshTasks();
    } catch (error) {
      alert(`删除失败: ${error}`);
    }
  };

  const clearFinishedTasks = async () => {
    try {
      await invoke("video_matcher_clear_finished_tasks");
      await refreshTasks();
    } catch (error) {
      alert(`清理失败: ${error}`);
    }
  };

  // 当前展示结果的任务 id；再次点击同一任务 = 收起
  const [resultTaskId, setResultTaskId] = useState<string | null>(null);

  const loadTaskResult = (task: MatcherTask) => {
    if (resultTaskId === task.id) {
      // 收起
      setResultTaskId(null);
      setSegments([]);
      setDraftPath("");
      return;
    }
    if (task.segments) {
      setResultTaskId(task.id);
      setSegments(task.segments);
      setSourceVideo(task.sourcePath);
      setTargetVideo(task.targetPath);
      setDraftPath("");
    }
  };

  const handleExportDraft = async () => {
    if (!sourceVideo || segments.length === 0) return;

    setExporting(true);
    try {
      const outputDir = await open({
        directory: true,
        multiple: false,
        title: "选择剪映草稿保存目录",
      });
      if (!outputDir) {
        setExporting(false);
        return;
      }

      // 对标视频真实总时长（播放器已加载）；无播放器信息时退回最大 sourceEnd。
      // 用「最大 sourceEnd」会让草稿在对标尾部无匹配时提前结束（看起来快进播完了）
      const refDuration = Math.max(
        sourceVideoRef.current?.duration ?? 0,
        segments.reduce((max, s) => Math.max(max, s.sourceEnd), 0),
      );

      const path = await invoke<string>("video_export_jianying_draft", {
        segments,
        sourceVideo: targetVideo, // 原视频是素材
        outputDir,
        // 草稿名 = 原视频文件名（不含扩展名），后端自动加时间戳
        draftName:
          targetVideo
            .split(/[/\\]/)
            .pop()
            ?.replace(/\.[^.]+$/, "") ?? "导出",
        referenceDuration: refDuration,
      });
      setDraftPath(path);
      alert(`草稿已导出:\n${path}\n\n在剪映首页即可看到该草稿。`);
    } catch (error) {
      console.error("导出失败:", error);
      alert(`导出失败: ${error}`);
    } finally {
      setExporting(false);
    }
  };

  const handleExportPr = async () => {
    if (!targetVideo || segments.length === 0) return;

    setExporting(true);
    try {
      const outputDir = await open({
        directory: true,
        multiple: false,
        title: "选择 PR 工程保存目录",
      });
      if (!outputDir) {
        setExporting(false);
        return;
      }

      // 对标视频真实总时长（同剪映导出：尾部无匹配段时草稿不应提前结束）
      const refDuration = Math.max(
        sourceVideoRef.current?.duration ?? 0,
        segments.reduce((max, s) => Math.max(max, s.sourceEnd), 0),
      );

      const path = await invoke<string>("video_export_pr_xml", {
        segments,
        sourceVideo: targetVideo,
        outputDir,
        draftName:
          targetVideo
            .split(/[/\\]/)
            .pop()
            ?.replace(/\.[^.]+$/, "") ?? "PR导出",
        referenceDuration: refDuration,
      });
      setDraftPath(path);
      alert(`PR 工程已导出:
${path}

在 Premiere Pro 中导入该 XML 即可查看片段。`);
    } catch (error) {
      console.error("导出失败:", error);
      alert(`导出失败: ${error}`);
    } finally {
      setExporting(false);
    }
  };

  const formatTime = (seconds: number) => {
    const m = Math.floor(seconds / 60);
    const s = Math.floor(seconds % 60);
    return `${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  };

  const seekVideo = (
    ref: React.RefObject<HTMLVideoElement | null>,
    time: number,
  ) => {
    if (ref.current) {
      ref.current.currentTime = time;
      ref.current.play();
    }
  };

  return (
    <div className="p-6 space-y-6">
      <div className="flex items-start justify-between">
        <div>
          <div className="flex items-center gap-3">
            <h2 className="text-2xl font-bold mb-2">视频工具</h2>
            {/* 推理设备徽章：一眼可见 GPU/CPU */}
            <span
              className={cn(
                "px-2 py-0.5 text-xs font-medium rounded",
                device === "gpu"
                  ? "bg-success/10 text-success"
                  : device === "cpu"
                    ? "bg-warning/10 text-warning"
                    : "bg-muted text-muted-foreground",
              )}
              title="ONNX 推理设备（首次匹配后确定）"
            >
              {device === "gpu"
                ? "⚡ GPU 加速"
                : device === "cpu"
                  ? "CPU 推理"
                  : "设备初始化中…"}
            </span>
          </div>
          <p className="text-muted-foreground">
            从原视频中寻找与对标视频画面相同的片段
          </p>
        </div>
        <button
          type="button"
          onClick={() => setShowSettings(!showSettings)}
          className="px-3 py-1.5 text-sm bg-secondary text-secondary-foreground rounded hover:bg-secondary/80"
        >
          {showSettings ? "关闭设置" : "⚙ 设置"}
        </button>
      </div>

      {/* 设置面板 */}
      {showSettings && settings && (
        <div className="p-4 bg-secondary rounded-lg space-y-4">
          <h3 className="font-semibold">匹配设置</h3>

          {/* 缓存目录 */}
          <div className="space-y-1">
            <span className="text-sm font-medium">缓存目录</span>
            <p className="text-xs text-muted-foreground">
              缓存抽帧特征，二次匹配同一视频时速度大幅提升。留空使用默认：
            </p>
            <p className="text-xs text-muted-foreground font-mono">
              {settings.cacheDir || defaultCacheDir}
            </p>
            <div className="flex gap-2 mt-1">
              <button
                type="button"
                onClick={pickCacheDir}
                className="px-3 py-1 text-xs bg-primary text-primary-foreground rounded hover:bg-primary/90"
              >
                选择目录
              </button>
              {settings.cacheDir && (
                <button
                  type="button"
                  onClick={() => saveSettings({ ...settings, cacheDir: "" })}
                  className="px-3 py-1 text-xs bg-secondary-foreground/10 rounded hover:bg-secondary-foreground/20"
                >
                  恢复默认
                </button>
              )}
              <button
                type="button"
                onClick={handleClearCache}
                className="px-3 py-1 text-xs bg-destructive/10 text-destructive rounded hover:bg-destructive/20"
              >
                {cacheCleared ? "已清除" : "清除缓存"}
              </button>
            </div>
          </div>

          {/* 抽帧间隔 */}
          <div className="space-y-1">
            <span className="text-sm font-medium">
              抽帧间隔: {settings.intervalSec.toFixed(1)} 秒
            </span>
            <select
              value={settings.intervalSec}
              onChange={(e) => {
                const v = parseFloat(e.target.value);
                setSettings({ ...settings, intervalSec: v });
                setInterval(v);
                saveSettings({ ...settings, intervalSec: v });
              }}
              className="w-full px-2 py-1 border rounded bg-background"
              aria-label="抽帧间隔"
            >
              <option value="0.033">
                0.033 秒（逐帧 · CutMatch 同款 · 最精确最慢）
              </option>
              <option value="0.1">0.1 秒（高精度）</option>
              <option value="0.2">0.2 秒（推荐 · 精度速度平衡）</option>
              <option value="0.5">0.5 秒（较快）</option>
              <option value="1">1 秒（最快 · 精度低）</option>
              <option value="2">2 秒</option>
              <option value="3">3 秒</option>
            </select>
            <p className="text-xs text-muted-foreground">
              0.033 = 逐帧（CutMatch
              同款，最精确但最慢）；修改后需重新提取特征缓存
            </p>
          </div>

          {/* 相似度阈值 */}
          <div className="space-y-1">
            <span className="text-sm font-medium">
              默认相似度阈值: {settings.threshold.toFixed(2)}
            </span>
            <input
              type="range"
              min="0.4"
              max="0.95"
              step="0.05"
              value={Math.min(0.75, Math.max(0.35, settings.threshold))}
              onChange={(e) => {
                const v = parseFloat(e.target.value);
                setSettings({ ...settings, threshold: v });
                setThreshold(v);
              }}
              onMouseUp={() => settings && saveSettings(settings)}
              onTouchEnd={() => settings && saveSettings(settings)}
              className="w-full"
              aria-label="相似度阈值"
            />
          </div>

          {/* 最少匹配帧数 */}
          <div className="space-y-1">
            <span className="text-sm font-medium">
              最少连续匹配帧数: {settings.minFrames}
            </span>
            <input
              type="range"
              min="1"
              max="10"
              step="1"
              value={settings.minFrames}
              onChange={(e) => {
                const v = parseInt(e.target.value, 10);
                setSettings({ ...settings, minFrames: v });
              }}
              onMouseUp={() => settings && saveSettings(settings)}
              onTouchEnd={() => settings && saveSettings(settings)}
              className="w-full"
              aria-label="最少匹配帧数"
            />
            <p className="text-xs text-muted-foreground">
              越大过滤掉越多的误匹配短片段
            </p>
          </div>

          {/* 特征模型 */}
          <div className="space-y-1">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium">特征提取模型</span>
              <span className="text-xs text-muted-foreground">
                推理设备：{" "}
                {device === "gpu"
                  ? "GPU 加速"
                  : device === "cpu"
                    ? "CPU"
                    : "未初始化"}
              </span>
            </div>
            <div className="space-y-2">
              {models.map((m) => {
                const dl = modelDownloads[m.id];
                const active = settings.extractor === m.id;
                return (
                  <div
                    key={m.id}
                    className={`p-2.5 rounded border ${
                      active ? "border-primary bg-primary/5" : "border-border"
                    }`}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="text-sm font-medium">{m.name}</span>
                          <span className="text-xs text-muted-foreground">
                            {m.featureDim} 维 · 约 {m.approxSizeMb} MB
                          </span>
                          {m.exists && (
                            <span className="px-1.5 py-0.5 text-xs rounded bg-success/10 text-success">
                              已就绪
                            </span>
                          )}
                          {active && (
                            <span className="px-1.5 py-0.5 text-xs rounded bg-primary/10 text-primary">
                              使用中
                            </span>
                          )}
                        </div>
                      </div>
                      <div className="flex gap-1.5 items-center shrink-0">
                        <button
                          type="button"
                          onClick={() => changeExtractor(m.id)}
                          disabled={active}
                          className={`px-2.5 py-1 text-xs rounded ${
                            active
                              ? "bg-muted text-muted-foreground cursor-default"
                              : "bg-primary text-primary-foreground hover:bg-primary/90"
                          }`}
                        >
                          {active ? "使用中" : "切换"}
                        </button>
                        {!m.exists && !dl && (
                          <button
                            type="button"
                            onClick={() => downloadModel(m.id)}
                            className="px-2.5 py-1 text-xs bg-secondary-foreground/10 rounded hover:bg-secondary-foreground/20"
                          >
                            下载
                          </button>
                        )}
                        {dl && (
                          <button
                            type="button"
                            onClick={() => cancelModelDownload(m.id)}
                            className="px-2.5 py-1 text-xs bg-destructive/10 text-destructive rounded hover:bg-destructive/20"
                          >
                            停止
                          </button>
                        )}
                        {m.exists && (
                          <button
                            type="button"
                            onClick={() => deleteModel(m.id)}
                            className="px-2 py-1 text-xs text-destructive hover:underline"
                          >
                            删除
                          </button>
                        )}
                      </div>
                    </div>
                    {/* 单模型下载进度条 */}
                    {dl && (
                      <div className="mt-2 space-y-1">
                        <div className="flex justify-between text-xs text-muted-foreground">
                          <span>{dl.message}</span>
                          <span>{dl.percent}%</span>
                        </div>
                        <div className="w-full h-1.5 bg-secondary rounded overflow-hidden">
                          <div
                            className="h-full bg-primary transition-all"
                            style={{ width: `${dl.percent}%` }}
                          />
                        </div>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
            <p className="text-xs text-muted-foreground">
              切换模型后特征缓存不通用，会重新提取
            </p>
          </div>
        </div>
      )}

      {/* 视频选择 */}
      <div className="grid grid-cols-2 gap-4">
        {/* 对标视频 */}
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium">对标视频（参考）</span>
            {sourceVideo && (
              <button
                type="button"
                onClick={() => {
                  setSourceVideo("");
                  setSegments([]);
                }}
                className="text-xs text-muted-foreground hover:text-destructive"
              >
                移除
              </button>
            )}
          </div>
          <div
            role="button"
            tabIndex={0}
            className="border-2 border-dashed border-border rounded-lg p-6 text-center cursor-pointer hover:border-primary transition-colors"
            onClick={() => pickVideo(setSourceVideo)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                pickVideo(setSourceVideo);
              }
            }}
          >
            {sourceVideo ? (
              <div className="space-y-2">
                <p className="font-medium text-sm truncate">
                  {sourceVideo.split(/[/\\]/).pop()}
                </p>
                {/* biome-ignore lint/a11y/useMediaCaption: video preview */}
                <video
                  ref={sourceVideoRef}
                  src={convertFileSrc(sourceVideo)}
                  controls
                  className="w-full rounded border-2 border-border shadow-lg bg-black"
                  style={{ maxHeight: "38vh" }}
                />
              </div>
            ) : (
              <p className="text-muted-foreground">点击选择对标视频</p>
            )}
          </div>
        </div>

        {/* 原视频 */}
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium">原视频（素材）</span>
            {targetVideo && (
              <button
                type="button"
                onClick={() => {
                  setTargetVideo("");
                  setSegments([]);
                }}
                className="text-xs text-muted-foreground hover:text-destructive"
              >
                移除
              </button>
            )}
          </div>
          <div
            role="button"
            tabIndex={0}
            className="border-2 border-dashed border-border rounded-lg p-6 text-center cursor-pointer hover:border-primary transition-colors"
            onClick={() => pickVideo(setTargetVideo)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                pickVideo(setTargetVideo);
              }
            }}
          >
            {targetVideo ? (
              <div className="space-y-2">
                <p className="font-medium text-sm truncate">
                  {targetVideo.split(/[/\\]/).pop()}
                </p>
                {/* biome-ignore lint/a11y/useMediaCaption: video preview */}
                <video
                  ref={targetVideoRef}
                  src={convertFileSrc(targetVideo)}
                  controls
                  className="w-full rounded border-2 border-border shadow-lg bg-black"
                  style={{ maxHeight: "38vh" }}
                />
              </div>
            ) : (
              <p className="text-muted-foreground">点击选择原视频</p>
            )}
          </div>
          <p className="text-xs text-muted-foreground">
            只选原视频不选对标，点「加入匹配」=
            提前缓存该视频特征（可连续加入多集，后台排队提取）
          </p>
        </div>
      </div>

      {/* 参数设置 */}
      <div className="space-y-4">
        <h3 className="font-semibold">匹配参数</h3>
        <div className="grid grid-cols-2 gap-4">
          <div className="space-y-2">
            <label htmlFor="threshold" className="text-sm">
              相似度阈值: {threshold.toFixed(2)}
            </label>
            <input
              id="threshold"
              type="range"
              min="0.4"
              max="0.95"
              step="0.05"
              value={threshold}
              onChange={(e) => setThreshold(parseFloat(e.target.value))}
              className="w-full"
            />
            <p className="text-xs text-muted-foreground">
              DINOv2 余弦相似度：同画面不同比例/压缩通常在 0.6-0.9，建议
              0.5-0.70；过高会漏匹配
            </p>
          </div>
          <div className="space-y-2">
            <label htmlFor="interval" className="text-sm">
              抽帧间隔: {interval.toFixed(1)} 秒
            </label>
            <select
              id="interval"
              value={interval}
              onChange={(e) => setInterval(parseFloat(e.target.value))}
              className="w-full px-2 py-1 border rounded bg-background"
            >
              <option value="0.033">0.033 秒（逐帧 · 最精确最慢）</option>
              <option value="0.1">0.1 秒（高精度）</option>
              <option value="0.2">0.2 秒（推荐）</option>
              <option value="0.5">0.5 秒（较快）</option>
              <option value="1">1 秒（最快 · 精度低）</option>
              <option value="2">2 秒</option>
              <option value="3">3 秒</option>
            </select>
            <p className="text-xs text-muted-foreground">越小越精确但越慢</p>
          </div>
        </div>
      </div>

      {/* 操作按钮：加入队列（可连续添加多个任务；仅选原视频 = 预缓存） */}
      <button
        type="button"
        onClick={handleAddTask}
        disabled={!targetVideo}
        className="w-full px-4 py-2 bg-primary text-primary-foreground rounded hover:bg-primary/90 disabled:opacity-50"
      >
        {sourceVideo ? "加入匹配队列" : "加入队列（预缓存原视频）"}
      </button>

      {/* 任务列表 */}
      {tasks.length > 0 && (
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <h3 className="font-semibold">匹配任务 ({tasks.length})</h3>
            <button
              type="button"
              onClick={clearFinishedTasks}
              className="text-xs text-muted-foreground hover:text-foreground"
            >
              清理已完成
            </button>
          </div>
          <div className="space-y-2 max-h-80 overflow-y-auto">
            {tasks.map((t, i) => (
              <div key={t.id} className="p-3 bg-secondary rounded-lg space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2 min-w-0">
                    <span className="text-xs font-medium text-muted-foreground shrink-0">
                      #{i + 1}
                    </span>
                    <span
                      className={`px-1.5 py-0.5 text-xs rounded shrink-0 ${
                        t.status === "completed"
                          ? "bg-success/10 text-success"
                          : t.status === "running"
                            ? "bg-primary/10 text-primary"
                            : t.status === "failed"
                              ? "bg-destructive/10 text-destructive"
                              : t.status === "cancelled"
                                ? "bg-muted text-muted-foreground"
                                : "bg-secondary-foreground/10 text-muted-foreground"
                      }`}
                    >
                      {t.status === "waiting"
                        ? "排队中"
                        : t.status === "running"
                          ? "匹配中"
                          : t.status === "completed"
                            ? "完成"
                            : t.status === "failed"
                              ? "失败"
                              : "已取消"}
                    </span>
                    <span className="text-xs truncate">
                      {t.sourcePath.split(/[/\\]/).pop()} ↔{" "}
                      {t.targetPath.split(/[/\\]/).pop()}
                    </span>
                  </div>
                  <div className="flex gap-1 shrink-0">
                    {t.status === "completed" && t.segments && (
                      <button
                        type="button"
                        onClick={() => loadTaskResult(t)}
                        className={cn(
                          "px-2 py-1 text-xs rounded",
                          resultTaskId === t.id
                            ? "bg-muted text-muted-foreground hover:bg-muted/80"
                            : "bg-primary/10 text-primary hover:bg-primary/20",
                        )}
                      >
                        {resultTaskId === t.id ? "收起" : "查看结果"} (
                        {t.segmentCount})
                      </button>
                    )}
                    {t.status === "completed" && t.segments && (
                      <button
                        type="button"
                        onClick={() => {
                          loadTaskResult(t);
                          void handleExportDraft();
                        }}
                        className="px-2 py-1 text-xs rounded bg-success/10 text-success hover:bg-success/20"
                      >
                        导出剪映草稿
                      </button>
                    )}
                    {(t.status === "waiting" || t.status === "running") && (
                      <button
                        type="button"
                        onClick={() => cancelTask(t.id)}
                        className="px-2 py-1 text-xs bg-destructive/10 text-destructive rounded hover:bg-destructive/20"
                      >
                        取消
                      </button>
                    )}
                    {t.status !== "running" && (
                      <button
                        type="button"
                        onClick={() => deleteTask(t.id)}
                        className="px-2 py-1 text-xs text-muted-foreground hover:text-destructive"
                      >
                        删除
                      </button>
                    )}
                  </div>
                </div>
                {(t.status === "running" || t.status === "waiting") && (
                  <div className="space-y-1">
                    <div className="flex justify-between text-xs text-muted-foreground">
                      <span>{t.message}</span>
                      <span>{t.percent}%</span>
                    </div>
                    <div className="w-full h-1.5 bg-background rounded overflow-hidden">
                      <div
                        className="h-full bg-primary transition-all"
                        style={{ width: `${t.percent}%` }}
                      />
                    </div>
                  </div>
                )}
                {t.status === "failed" && (
                  <p className="text-xs text-destructive">{t.message}</p>
                )}
                {t.status === "completed" && (
                  <p className="text-xs text-success">{t.message}</p>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 匹配结果 */}
      {segments.length > 0 && (
        <div className="space-y-3">
          <h3 className="font-semibold">匹配结果 ({segments.length} 个片段)</h3>
          <div className="space-y-2 max-h-96 overflow-y-auto">
            {segments.map((seg, idx) => (
              <div
                key={idx}
                className="flex items-center gap-3 p-3 bg-secondary rounded-lg"
              >
                <span className="text-sm font-medium w-8">#{idx + 1}</span>
                <div className="flex-1 grid grid-cols-2 gap-4 text-sm">
                  <div>
                    <span className="text-muted-foreground">对标:</span>{" "}
                    {formatTime(seg.sourceStart)} - {formatTime(seg.sourceEnd)}
                  </div>
                  <div>
                    <span className="text-muted-foreground">原片:</span>{" "}
                    {formatTime(seg.targetStart)} - {formatTime(seg.targetEnd)}
                  </div>
                </div>
                <div className="text-right">
                  <div className="text-sm font-medium">
                    {(seg.similarity * 100).toFixed(1)}%
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {seg.matchedFrames} 帧
                  </div>
                </div>
                <div className="flex gap-1">
                  <button
                    type="button"
                    onClick={() => seekVideo(sourceVideoRef, seg.sourceStart)}
                    className="px-2 py-1 text-xs bg-secondary-foreground/10 rounded hover:bg-secondary-foreground/20"
                  >
                    对标
                  </button>
                  <button
                    type="button"
                    onClick={() => seekVideo(targetVideoRef, seg.targetStart)}
                    className="px-2 py-1 text-xs bg-secondary-foreground/10 rounded hover:bg-secondary-foreground/20"
                  >
                    原片
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 导出剪映草稿 / PR 草稿 */}
      {segments.length > 0 && (
        <div className="space-y-2">
          <div className="grid grid-cols-2 gap-3">
            <button
              type="button"
              onClick={handleExportDraft}
              disabled={exporting}
              className="px-4 py-2 bg-success text-success-foreground rounded hover:bg-success/90 disabled:opacity-50"
            >
              {exporting ? "导出中..." : "导出剪映草稿"}
            </button>
            <button
              type="button"
              onClick={handleExportPr}
              disabled={exporting}
              className="px-4 py-2 bg-primary text-primary-foreground rounded hover:bg-primary/90 disabled:opacity-50"
            >
              {exporting ? "导出中..." : "导出 PR 草稿"}
            </button>
          </div>
          {draftPath && (
            <p className="text-xs text-success font-mono break-all">
              {draftPath}
            </p>
          )}
          <p className="text-xs text-muted-foreground">
            剪映：片段按对标时间轴排列、未匹配留空、素材复制进草稿目录；PR：导出
            Premiere XML，导入后在项目面板查看片段。
          </p>
        </div>
      )}

      {segments.length === 0 && (sourceVideo || targetVideo) && (
        <p className="text-center text-muted-foreground text-sm">
          选择两个视频并点击「开始匹配」
        </p>
      )}
    </div>
  );
}
