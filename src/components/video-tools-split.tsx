// video-tools-split.tsx - 快速拆分：标记关键帧 → 按段导出（CutMatch SplitWorker 移植）

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  LuLoaderCircle,
  LuPause,
  LuPlay,
  LuPlus,
  LuScissors,
  LuTrash2,
  LuX,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

interface ToolsProgress {
  stage: string;
  percent: number;
  message: string;
}

const fmt = (s: number) => {
  const m = Math.floor(s / 60);
  const sec = s % 60;
  return `${m.toString().padStart(2, "0")}:${sec.toFixed(1).padStart(4, "0")}`;
};

export function QuickSplitTab() {
  const [videoPath, setVideoPath] = useState("");
  const [duration, setDuration] = useState(0);
  const [keyframes, setKeyframes] = useState<number[]>([]);
  const [currentTime, setCurrentTime] = useState(0);
  const [isPlaying, setIsPlaying] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const dropZoneRef = useRef<HTMLButtonElement | null>(null);
  const [outputDir, setOutputDir] = useState("");
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<ToolsProgress | null>(null);
  const [results, setResults] = useState<string[]>([]);
  const [splitDone, setSplitDone] = useState("");

  const videoRef = useRef<HTMLVideoElement>(null);

  // 加载视频（选文件与拖放共用）
  const applyDropped = useCallback(async (path: string) => {
    setVideoPath(path);
    setKeyframes([]);
    setResults([]);
    setProgress(null);
    try {
      const dur = await invoke<number>("video_tools_get_duration", {
        videoPath: path,
      });
      setDuration(dur);
    } catch (error) {
      console.error("获取时长失败:", error);
      setDuration(0);
    }
  }, []);

  const pickVideo = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: "Video Files",
          extensions: ["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "ts"],
        },
      ],
    });
    if (selected) {
      const path = selected as string;
      await applyDropped(path);
      /* original inline loader removed */
      try {
        const dur = await invoke<number>("video_tools_get_duration", {
          videoPath: path,
        });
        setDuration(dur);
      } catch (error) {
        console.error("获取时长失败:", error);
        setDuration(0);
      }
    }
  };

  // Tauri 原生拖放：拖入视频直接加载
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "leave") {
          setDragOver(false);
          return;
        }
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
        setDragOver(false);
        if (!inside) return;
        const path = p.paths.find((f) =>
          /\.(mp4|mkv|avi|mov|wmv|flv|webm|ts|m4v)$/i.test(f),
        );
        if (path) {
          void applyDropped(path);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      unlisten?.();
    };
  }, [applyDropped]);

  const clearVideo = () => {
    setVideoPath("");
    setDuration(0);
    setKeyframes([]);
    setResults([]);
  };

  // 播放器 seek 到指定时间
  const seekTo = (t: number) => {
    if (videoRef.current) {
      videoRef.current.currentTime = t;
    }
    setCurrentTime(t);
  };

  const addKeyframe = () => {
    const t = videoRef.current?.currentTime ?? currentTime;
    if (keyframes.some((k) => Math.abs(k - t) < 0.05)) return;
    setKeyframes((prev) => [...prev, t].sort((a, b) => a - b));
  };

  const removeKeyframe = (idx: number) => {
    setKeyframes((prev) => prev.filter((_, i) => i !== idx));
  };

  const updateKeyframe = (idx: number, value: number) => {
    setKeyframes((prev) =>
      prev.map((k, i) => (i === idx ? value : k)).sort((a, b) => a - b),
    );
  };

  // 关键帧 → 预览分段（与后端一致）
  const previewSegments = () => {
    const points = [...keyframes].sort((a, b) => a - b);
    const segs: Array<[number, number]> = [];
    let prev = 0;
    for (const t of points) {
      if (t > prev + 0.1) segs.push([prev, t]);
      prev = t;
    }
    if (duration > prev + 0.1) segs.push([prev, duration]);
    return segs;
  };

  const pickOutputDir = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected) setOutputDir(selected as string);
  };

  const handleSplit = async () => {
    if (!videoPath) return;
    const segs = previewSegments();
    if (segs.length < 2) return;
    setRunning(true);
    setResults([]);
    setProgress({ stage: "split", percent: 0, message: "准备拆分..." });
    try {
      const dir = outputDir || videoPath.split(/[/\\]/).slice(0, -1).join("/");
      const out = await invoke<string[]>("video_tools_split", {
        videoPath,
        keyframes,
        outputDir: dir,
      });
      setResults(out);
      setSplitDone(`拆分完成，共 ${out.length} 段`);
    } catch (error) {
      console.error("拆分失败:", error);
      alert(`拆分失败: ${error}`);
    } finally {
      setRunning(false);
      setTimeout(() => setProgress(null), 2000);
    }
  };

  const segs = videoPath ? previewSegments() : [];

  return (
    <div className="space-y-5 max-w-[1400px]">
      <div>
        <h2 className="text-xl font-bold mb-1">快速拆分</h2>
        <p className="text-sm text-muted-foreground">
          播放视频并在需要拆分的位置添加关键帧标记，按标记切成多段导出。
        </p>
      </div>

      {/* 视频选择 */}
      {videoPath ? (
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-sm truncate flex-1">{videoPath}</span>
            <button
              type="button"
              onClick={clearVideo}
              className="p-1 text-muted-foreground hover:text-destructive"
              title="更换视频"
            >
              <LuX className="h-4 w-4" />
            </button>
          </div>
          <div className="relative">
            {/* biome-ignore lint/a11y/useMediaCaption: video preview */}
            <video
              ref={videoRef}
              src={convertFileSrc(videoPath)}
              className="w-full rounded-lg bg-black max-h-[62vh] border-2 border-border shadow-lg"
              onTimeUpdate={(e) => setCurrentTime(e.currentTarget.currentTime)}
              onPlay={() => setIsPlaying(true)}
              onPause={() => setIsPlaying(false)}
              onClick={() => {
                // 点击画面切换播放/暂停
                const v = videoRef.current;
                if (v) {
                  if (v.paused) void v.play();
                  else v.pause();
                }
              }}
            />
          </div>
          {/* 自定义播放控制栏：关键帧竖线直接标在播放进度条上 */}
          {duration > 0 && (
            <div className="flex items-center gap-2 px-1">
              <button
                type="button"
                onClick={() => {
                  const v = videoRef.current;
                  if (v) {
                    if (v.paused) void v.play();
                    else v.pause();
                  }
                }}
                className="p-1 text-foreground hover:text-primary"
                title="播放/暂停"
              >
                {isPlaying ? (
                  <LuPause className="h-4 w-4" />
                ) : (
                  <LuPlay className="h-4 w-4" />
                )}
              </button>
              <span className="text-xs text-muted-foreground font-mono shrink-0">
                {fmt(currentTime)} / {fmt(duration)}
              </span>
              {/* 进度条：关键帧竖线 + 拖拽跳转 */}
              <div
                role="slider"
                tabIndex={0}
                aria-label="播放进度（含关键帧标记）"
                aria-valuemin={0}
                aria-valuemax={Math.round(duration)}
                aria-valuenow={Math.round(currentTime)}
                className="relative flex-1 h-6 cursor-pointer select-none group"
                onPointerDown={(e) => {
                  // 按下即跳转并支持拖拽
                  const rect = e.currentTarget.getBoundingClientRect();
                  const onMove = (ev: PointerEvent) => {
                    const t = Math.min(
                      duration,
                      Math.max(
                        0,
                        ((ev.clientX - rect.left) / rect.width) * duration,
                      ),
                    );
                    seekTo(t);
                  };
                  onMove(e.nativeEvent as PointerEvent);
                  const onUp = () => {
                    window.removeEventListener("pointermove", onMove);
                    window.removeEventListener("pointerup", onUp);
                  };
                  window.addEventListener("pointermove", onMove);
                  window.addEventListener("pointerup", onUp);
                }}
                onDoubleClick={(e) => {
                  // 双击进度条 = 在该位置添加关键帧
                  const rect = e.currentTarget.getBoundingClientRect();
                  const t = ((e.clientX - rect.left) / rect.width) * duration;
                  if (!keyframes.some((k) => Math.abs(k - t) < 0.05)) {
                    setKeyframes((prev) => [...prev, t].sort((a, b) => a - b));
                  }
                }}
                onKeyDown={(e) => {
                  if (e.key === "ArrowRight") seekTo(currentTime + 0.1);
                  if (e.key === "ArrowLeft")
                    seekTo(Math.max(0, currentTime - 0.1));
                }}
              >
                {/* 轨道（单条全宽，避免双条相向排布的抖动） */}
                <div className="absolute top-1/2 -translate-y-1/2 left-0 right-0 h-1.5 bg-muted rounded" />
                {/* 已播放进度（覆盖在轨道上） */}
                <div
                  className="absolute top-1/2 -translate-y-1/2 left-0 h-1.5 bg-primary rounded"
                  style={{
                    width: `${Math.min(100, (currentTime / duration) * 100)}%`,
                  }}
                />
                {/* 关键帧竖线（骑在进度条上，贯穿上下） */}
                {keyframes.map((k, i) => (
                  <div
                    key={`${k}-${i}`}
                    className="absolute top-0 bottom-0 w-0.5 bg-warning"
                    style={{ left: `${(k / duration) * 100}%` }}
                    title={`关键帧 ${i + 1}: ${fmt(k)}（列表中可微调/删除）`}
                  />
                ))}
                {/* 播放头 */}
                <div
                  className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-3 h-3 rounded-full bg-primary border-2 border-background shadow"
                  style={{ left: `${(currentTime / duration) * 100}%` }}
                />
              </div>
              {/* 添加关键帧按钮（当前播放位置） */}
              <button
                type="button"
                onClick={addKeyframe}
                className="p-1 text-primary hover:text-primary/70 shrink-0"
                title={`在 ${fmt(currentTime)} 添加关键帧`}
              >
                <LuPlus className="h-4 w-4" />
              </button>
              <button
                type="button"
                onClick={() => {
                  // 删除最近的关键帧
                  if (keyframes.length === 0) return;
                  let nearest = 0;
                  for (let i = 1; i < keyframes.length; i++) {
                    if (
                      Math.abs(keyframes[i] - currentTime) <
                      Math.abs(keyframes[nearest] - currentTime)
                    ) {
                      nearest = i;
                    }
                  }
                  setKeyframes((prev) => prev.filter((_, i) => i !== nearest));
                }}
                className="p-1 text-muted-foreground hover:text-destructive shrink-0"
                title="删除最近的关键帧"
              >
                <LuTrash2 className="h-4 w-4" />
              </button>
            </div>
          )}
        </div>
      ) : (
        <Button
          ref={dropZoneRef}
          variant="outline"
          onClick={pickVideo}
          className={cn(
            "w-full h-24 border-dashed text-sm",
            dragOver
              ? "border-primary bg-primary/5 text-primary"
              : "text-muted-foreground",
          )}
        >
          <LuPlay className="mr-2 h-4 w-4" />
          点击选择或拖入视频文件
        </Button>
      )}

      {/* 关键帧列表 */}
      {keyframes.length > 0 && (
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium">
              关键帧 ({keyframes.length})
            </span>
            <button
              type="button"
              onClick={() => setKeyframes([])}
              className="text-xs text-muted-foreground hover:text-destructive"
            >
              <LuTrash2 className="inline mr-1 h-3 w-3" />
              清空
            </button>
          </div>
          <div className="space-y-1.5">
            {keyframes.map((k, idx) => (
              <div key={`${k}-${idx}`} className="flex items-center gap-2">
                <span className="text-xs text-muted-foreground w-8">
                  #{idx + 1}
                </span>
                <Input
                  type="number"
                  value={Number(k.toFixed(2))}
                  step="0.1"
                  min="0.1"
                  max={duration || undefined}
                  onChange={(e) =>
                    updateKeyframe(idx, parseFloat(e.target.value) || 0)
                  }
                  className="h-7 w-28 text-xs"
                />
                <span className="text-xs text-muted-foreground">秒</span>
                <button
                  type="button"
                  onClick={() => seekTo(k)}
                  className="text-xs text-primary hover:underline"
                >
                  跳转
                </button>
                <button
                  type="button"
                  onClick={() => removeKeyframe(idx)}
                  className="ml-auto p-1 text-muted-foreground hover:text-destructive"
                  title="删除"
                >
                  <LuX className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 分段预览 */}
      {segs.length >= 2 && (
        <div className="space-y-1">
          <span className="text-sm font-medium">
            分段预览 ({segs.length} 段)
          </span>
          <div className="text-xs text-muted-foreground space-y-0.5">
            {segs.map(([s, e], i) => (
              <div key={i}>
                第 {i + 1} 段：{fmt(s)} → {fmt(e)}（{(e - s).toFixed(1)} 秒）
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 输出目录 */}
      {videoPath && (
        <div className="space-y-1">
          <span className="text-sm font-medium">输出目录</span>
          <div className="flex gap-2">
            <Input
              value={outputDir}
              readOnly
              placeholder={
                videoPath
                  ? videoPath.split(/[/\\]/).slice(0, -1).join("/") ||
                    "原视频所在目录"
                  : "选择视频后自动填充"
              }
              className="h-8 text-xs font-mono"
            />
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                const dir =
                  outputDir || videoPath.split(/[/\\]/).slice(0, -1).join("/");
                if (dir) void openPath(dir);
              }}
              title="打开输出文件夹"
            >
              打开文件夹
            </Button>
            <Button size="sm" variant="outline" onClick={pickOutputDir}>
              选择
            </Button>
          </div>
        </div>
      )}

      {/* 拆分按钮 */}
      <Button
        onClick={handleSplit}
        disabled={!videoPath || segs.length < 2 || running}
        className="w-full"
      >
        {running ? (
          <>
            <LuLoaderCircle className="mr-2 h-4 w-4 animate-spin" />
            拆分中...
          </>
        ) : (
          <>
            <LuScissors className="mr-2 h-4 w-4" />
            开始拆分为 {Math.max(segs.length, 2)} 段
          </>
        )}
      </Button>

      {/* 进度 */}
      {progress && (
        <div className="space-y-1">
          <div className="flex justify-between text-sm">
            <span className="text-muted-foreground">{progress.message}</span>
            <span className="text-muted-foreground">{progress.percent}%</span>
          </div>
          <div className="w-full h-2 bg-secondary rounded overflow-hidden">
            <div
              className="h-full bg-primary transition-all"
              style={{ width: `${progress.percent}%` }}
            />
          </div>
        </div>
      )}

      {splitDone && (
        <p className="text-sm font-medium text-success">✓ {splitDone}</p>
      )}
      {results.length > 0 && (
        <div className="space-y-1">
          <span className="text-sm font-medium text-success">输出文件</span>
          {results.map((r) => (
            <p
              key={r}
              className="text-xs font-mono break-all text-muted-foreground"
            >
              {r}
            </p>
          ))}
        </div>
      )}
    </div>
  );
}
