// audio-silence-remover.tsx - 音频气口清除器组件

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";

interface SilenceSegment {
  start: number;
  end: number;
  duration: number;
}

interface ProcessStats {
  originalDuration: number;
  removedDuration: number;
  finalDuration: number;
  keptSegments: number;
}

interface ProcessParams {
  thresholdDb: number;
  keepBefore: number;
  keepAfter: number;
  minSilenceDuration: number;
  pace: string;
}

export function AudioSilenceRemover() {
  const [audioFile, setAudioFile] = useState<string | null>(null);
  const [audioName, setAudioName] = useState<string>("");
  const [processing, setProcessing] = useState(false);
  const [stats, setStats] = useState<ProcessStats | null>(null);
  const [silenceSegments, setSilenceSegments] = useState<SilenceSegment[]>([]);
  const [originalWaveform, setOriginalWaveform] = useState<string | null>(null);
  const [resultWaveform, setResultWaveform] = useState<string | null>(null);
  const [resultPath, setResultPath] = useState<string>("");
  const [params, setParams] = useState<ProcessParams>({
    thresholdDb: -28,
    keepBefore: 0.1,
    keepAfter: 0.1,
    minSilenceDuration: 0.35,
    pace: "medium",
  });
  // 参数面板收缩状态（低分辨率下收起参数保证开始处理按钮可见）
  const [paramsOpen, setParamsOpen] = useState(false);

  const audioRef = useRef<HTMLAudioElement>(null);
  const resultAudioRef = useRef<HTMLAudioElement>(null);
  const [originalProgress, setOriginalProgress] = useState(0);
  const [resultProgress, setResultProgress] = useState(0);

  // 加载原音频波形
  useEffect(() => {
    async function loadWaveform() {
      if (!audioFile) {
        setOriginalWaveform(null);
        return;
      }
      try {
        const b64 = await invoke<string>("audio_get_waveform", {
          inputPath: audioFile,
        });
        setOriginalWaveform(`data:image/png;base64,${b64}`);
      } catch (error) {
        console.error("Failed to get waveform:", error);
        setOriginalWaveform(null);
      }
    }
    loadWaveform();
  }, [audioFile]);

  // 参数变化自动保存
  const updateParams = useCallback((next: ProcessParams) => {
    setParams(next);
    invoke("audio_silence_save_settings", { settings: next }).catch((error) =>
      console.error("保存音频设置失败:", error),
    );
  }, []);

  // 过滤参数持久化：启动时读取保存的参数
  useEffect(() => {
    invoke<ProcessParams>("audio_silence_get_settings")
      .then((saved) => {
        setParams(saved);
      })
      .catch((error) => console.error("读取音频设置失败:", error));
  }, []);

  // 播放进度经 onTimeUpdate 驱动波形时间轴（元素随文件切换重新挂载，props 绑定最可靠）
  const handleOriginalTime = () => {
    const audio = audioRef.current;
    if (audio?.duration) {
      setOriginalProgress(audio.currentTime / audio.duration);
    }
  };

  const handleResultTime = () => {
    const audio = resultAudioRef.current;
    if (audio?.duration) {
      setResultProgress(audio.currentTime / audio.duration);
    }
  };

  const handleSelectFile = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: "Audio Files",
          extensions: ["mp3", "wav", "m4a", "ogg", "flac", "aac", "wma"],
        },
      ],
    });

    if (selected) {
      const path = selected as string;
      setAudioFile(path);
      const fileName = path.split(/[/\\]/).pop() || "";
      setAudioName(fileName);
      setStats(null);
      setSilenceSegments([]);
      setResultWaveform(null);
      setResultPath("");
    }
  };

  // Tauri 原生拖放：WebView2 里 HTML5 drop 拿不到文件路径，必须走 onDragDropEvent
  const dropZoneRef = useRef<HTMLDivElement | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const applyDroppedPath = useCallback((path: string) => {
    setAudioFile(path);
    setAudioName(path.split(/[/\\]/).pop() || "");
    setStats(null);
    setSilenceSegments([]);
    setResultWaveform(null);
    setResultPath("");
  }, []);
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
          /\.(mp3|wav|m4a|ogg|flac|aac|wma)$/i.test(f),
        );
        if (path) applyDroppedPath(path);
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      unlisten?.();
    };
  }, [applyDroppedPath]);

  const detectSilence = async () => {
    if (!audioFile) return;

    try {
      const segments = await invoke<SilenceSegment[]>("audio_detect_silence", {
        inputPath: audioFile,
        thresholdDb: params.thresholdDb,
        minDuration: params.minSilenceDuration,
      });
      setSilenceSegments(segments);
    } catch (error) {
      console.error("Failed to detect silence:", error);
      alert(`检测失败: ${error}`);
    }
  };

  const processAudio = async () => {
    if (!audioFile) return;

    setProcessing(true);
    setStats(null);
    setResultWaveform(null);
    try {
      // 输出到原音频所在目录，自动命名 原名_时间戳.wav
      const dir = audioFile
        .split(/[/\\]/)
        .slice(0, -1)
        .join(audioFile.includes("\\") ? "\\" : "/");
      const baseName = audioName.replace(/\.[^.]+$/, "");
      const stamp = new Date().toISOString().replace(/[-:T]/g, "").slice(0, 14); // YYYYMMDDHHmmss
      const outputPath = `${dir}/${baseName}_${stamp}.wav`;

      const result = await invoke<ProcessStats>("audio_remove_silence", {
        inputPath: audioFile,
        outputPath,
        params,
      });

      setStats(result);
      setResultPath(outputPath);

      // 加载处理后的波形
      try {
        const b64 = await invoke<string>("audio_get_waveform", {
          inputPath: outputPath,
        });
        setResultWaveform(`data:image/png;base64,${b64}`);
      } catch (error) {
        console.error("Failed to get result waveform:", error);
      }
    } catch (error) {
      console.error("Failed to process audio:", error);
      alert(`处理失败: ${error}`);
    } finally {
      setProcessing(false);
    }
  };

  // 慢/中等/快 预设直接套用一组过滤参数，自定义则保留手动值
  const applyPace = (pace: typeof params.pace) => {
    if (pace === "slow") {
      updateParams({
        pace,
        thresholdDb: -35,
        keepBefore: 0.25,
        keepAfter: 0.25,
        minSilenceDuration: 0.5,
      });
    } else if (pace === "medium") {
      updateParams({
        pace,
        thresholdDb: -28,
        keepBefore: 0.12,
        keepAfter: 0.12,
        minSilenceDuration: 0.35,
      });
    } else if (pace === "fast") {
      updateParams({
        pace,
        thresholdDb: -22,
        keepBefore: 0.05,
        keepAfter: 0.05,
        minSilenceDuration: 0.2,
      });
    } else {
      updateParams({ ...params, pace: "custom" });
    }
  };

  const formatDuration = (seconds: number) => {
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}:${secs.toString().padStart(2, "0")}`;
  };

  return (
    <div className="p-6 space-y-6 max-w-[1400px]">
      <div>
        <h2 className="text-2xl font-bold mb-2">气口清除器</h2>
        <p className="text-muted-foreground">
          自动剪掉录音中的停顿与静音 · 全程在本地浏览器处理，音频不会上传
        </p>
      </div>

      {/* 文件上传区域 */}
      <div
        ref={dropZoneRef}
        role="button"
        tabIndex={0}
        className={`border-2 border-dashed rounded-lg p-8 text-center cursor-pointer transition-colors ${
          dragOver
            ? "border-primary bg-primary/5"
            : "border-border hover:border-primary"
        }`}
        onClick={handleSelectFile}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            handleSelectFile();
          }
        }}
      >
        {audioFile ? (
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              <span className="text-2xl">🎵</span>
              <span className="font-medium">{audioName}</span>
            </div>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                setAudioFile(null);
                setAudioName("");
                setStats(null);
                setOriginalProgress(0);
              }}
              className="text-destructive hover:underline"
            >
              清除
            </button>
          </div>
        ) : (
          <div>
            <p className="text-lg mb-2">点击选择或拖入音频文件</p>
            <p className="text-sm text-muted-foreground">
              支持 mp3 / wav / m4a / ogg / flac 等浏览器可解码格式
            </p>
          </div>
        )}
      </div>

      {/* 音频预览 */}
      {audioFile && (
        <div className="space-y-2">
          {/* 原音频波段 */}
          <div className="space-y-1">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium">原音频波形</span>
              <span className="text-xs text-muted-foreground">{audioName}</span>
            </div>
            {originalWaveform ? (
              <div className="relative w-full h-20 rounded border border-border overflow-hidden bg-card">
                {/* biome-ignore lint/performance/noImgElement: dynamic base64 waveform preview */}
                <img
                  src={originalWaveform}
                  alt="原音频波形"
                  className="w-full h-full object-cover"
                />
                <div className="absolute inset-0 rounded" aria-hidden="true">
                  <div className="absolute left-0 right-0 top-1/2 h-px bg-border/50" />
                </div>
                {/* 播放进度竖线 */}
                <div
                  className="absolute top-0 bottom-0 w-0.5 bg-primary"
                  style={{ left: `${originalProgress * 100}%` }}
                  aria-hidden="true"
                />
              </div>
            ) : (
              <div className="w-full h-12 bg-secondary rounded flex items-center justify-center text-sm text-muted-foreground">
                波形加载中...
              </div>
            )}
          </div>

          {/* biome-ignore lint/a11y/useMediaCaption: audio preview player doesn't require captions */}
          <audio
            ref={audioRef}
            controls
            className="w-full"
            onTimeUpdate={handleOriginalTime}
          >
            <source src={convertFileSrc(audioFile)} />
          </audio>
        </div>
      )}

      {/* 处理结果波形 */}
      {resultWaveform && (
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium text-success">处理后波形</span>
            <span className="text-xs text-muted-foreground">
              {resultPath.split(/[/\\]/).pop()}
            </span>
          </div>
          <div className="relative w-full h-20 rounded border border-border overflow-hidden bg-card">
            {/* biome-ignore lint/performance/noImgElement: dynamic base64 waveform preview */}
            <img
              src={resultWaveform}
              alt="处理后波形"
              className="w-full h-full object-cover"
            />
            <div className="absolute inset-0 rounded" aria-hidden="true">
              <div className="absolute left-0 right-0 top-1/2 h-px bg-border/50" />
            </div>
            {/* 播放进度竖线 */}
            <div
              className="absolute top-0 bottom-0 w-0.5 bg-primary"
              style={{ left: `${resultProgress * 100}%` }}
              aria-hidden="true"
            />
          </div>
          {/* 处理后音频试听 */}
          {/* biome-ignore lint/a11y/useMediaCaption: audio preview player doesn't require captions */}
          <audio
            ref={resultAudioRef}
            controls
            className="w-full"
            onTimeUpdate={handleResultTime}
          >
            <source src={convertFileSrc(resultPath)} />
          </audio>
        </div>
      )}

      {/* 过滤参数（可收缩，低分辨率下收起保证操作按钮可见） */}
      <div className="space-y-3">
        <button
          type="button"
          onClick={() => setParamsOpen(!paramsOpen)}
          className="flex w-full items-center justify-between text-left"
        >
          <h3 className="font-semibold">剪辑节奏与过滤参数</h3>
          <span className="text-xs text-muted-foreground">
            {paramsOpen ? "收起 ▴" : "展开 ▾"}
          </span>
        </button>
        {paramsOpen && (
          <>
            {/* 剪辑节奏 */}
            <div className="space-y-3">
              <h3 className="font-semibold">剪辑节奏</h3>
              <div className="grid grid-cols-4 gap-2">
                {(["slow", "medium", "fast", "custom"] as const).map((pace) => (
                  <button
                    key={pace}
                    type="button"
                    onClick={() => applyPace(pace)}
                    className={`py-2 px-4 rounded ${
                      params.pace === pace
                        ? "bg-primary text-primary-foreground"
                        : "bg-secondary text-secondary-foreground hover:bg-secondary/80"
                    }`}
                  >
                    {pace === "slow"
                      ? "慢"
                      : pace === "medium"
                        ? "中等"
                        : pace === "fast"
                          ? "快"
                          : "自定义"}
                  </button>
                ))}
              </div>
              {params.pace === "custom" && (
                <p className="text-sm text-muted-foreground">
                  手动设置静音阈值和片段前后保留时长
                </p>
              )}
            </div>

            {/* 过滤参数 */}
            <div className="space-y-4">
              <h3 className="font-semibold">过滤参数</h3>

              {/* 音量阈值 */}
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <label htmlFor="threshold-db" className="text-sm">
                    如果音量小于
                  </label>
                  <input
                    id="threshold-db"
                    type="number"
                    value={params.thresholdDb}
                    onChange={(e) =>
                      updateParams({
                        ...params,
                        thresholdDb: parseFloat(e.target.value),
                      })
                    }
                    className="w-20 px-2 py-1 border rounded"
                    step="1"
                    min="-70"
                    max="20"
                  />
                  <span className="text-sm">dB，则视为静音</span>
                </div>
                <input
                  type="range"
                  min="-70"
                  max="20"
                  step="1"
                  value={params.thresholdDb}
                  onChange={(e) =>
                    updateParams({
                      ...params,
                      thresholdDb: parseFloat(e.target.value),
                    })
                  }
                  className="w-full"
                  aria-label="音量阈值"
                />
                <div className="flex justify-between text-xs text-muted-foreground">
                  <span>-70 dB</span>
                  <span>20 dB</span>
                </div>
              </div>

              {/* 片段前保留 */}
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <label htmlFor="keep-before" className="text-sm">
                    每个「有用片段」前，保留
                  </label>
                  <input
                    id="keep-before"
                    type="number"
                    value={params.keepBefore}
                    onChange={(e) =>
                      updateParams({
                        ...params,
                        keepBefore: parseFloat(e.target.value),
                      })
                    }
                    className="w-20 px-2 py-1 border rounded"
                    step="0.1"
                    min="0"
                    max="2"
                  />
                  <span className="text-sm">秒</span>
                </div>
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.1"
                  value={params.keepBefore}
                  onChange={(e) =>
                    updateParams({
                      ...params,
                      keepBefore: parseFloat(e.target.value),
                    })
                  }
                  className="w-full"
                  aria-label="片段前保留时长"
                />
                <div className="flex justify-between text-xs text-muted-foreground">
                  <span>0 秒</span>
                  <span>2 秒</span>
                </div>
              </div>

              {/* 片段后保留 */}
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <label htmlFor="keep-after" className="text-sm">
                    每个「有用片段」后，保留
                  </label>
                  <input
                    id="keep-after"
                    type="number"
                    value={params.keepAfter}
                    onChange={(e) =>
                      updateParams({
                        ...params,
                        keepAfter: parseFloat(e.target.value),
                      })
                    }
                    className="w-20 px-2 py-1 border rounded"
                    step="0.1"
                    min="0"
                    max="2"
                  />
                  <span className="text-sm">秒</span>
                </div>
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.1"
                  value={params.keepAfter}
                  onChange={(e) =>
                    updateParams({
                      ...params,
                      keepAfter: parseFloat(e.target.value),
                    })
                  }
                  className="w-full"
                  aria-label="片段后保留时长"
                />
                <div className="flex justify-between text-xs text-muted-foreground">
                  <span>0 秒</span>
                  <span>2 秒</span>
                </div>
              </div>

              {/* 停顿阈值 */}
              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <label htmlFor="min-silence" className="text-sm">
                    停顿短于
                  </label>
                  <input
                    id="min-silence"
                    type="number"
                    value={params.minSilenceDuration}
                    onChange={(e) =>
                      updateParams({
                        ...params,
                        minSilenceDuration: parseFloat(e.target.value),
                      })
                    }
                    className="w-20 px-2 py-1 border rounded"
                    step="0.05"
                    min="0.05"
                    max="3"
                  />
                  <span className="text-sm">秒时不剪</span>
                </div>
                <input
                  type="range"
                  min="0.05"
                  max="3"
                  step="0.05"
                  value={params.minSilenceDuration}
                  onChange={(e) =>
                    updateParams({
                      ...params,
                      minSilenceDuration: parseFloat(e.target.value),
                    })
                  }
                  className="w-full"
                  aria-label="最小静音时长"
                />
                <div className="flex justify-between text-xs text-muted-foreground">
                  <span>0.05 秒</span>
                  <span>3 秒</span>
                </div>
                <p className="text-xs text-muted-foreground">
                  值越小剪得越狠，越接近"快"节奏；避免设得过小导致语句衔接生硬
                </p>
              </div>
            </div>
          </>
        )}
      </div>

      {/* 操作按钮 */}
      <div className="flex gap-3">
        <button
          type="button"
          onClick={detectSilence}
          disabled={!audioFile || processing}
          className="px-4 py-2 bg-secondary text-secondary-foreground rounded hover:bg-secondary/80 disabled:opacity-50"
        >
          检测静音
        </button>
        <button
          type="button"
          onClick={processAudio}
          disabled={!audioFile || processing}
          className="px-4 py-2 bg-primary text-primary-foreground rounded hover:bg-primary/90 disabled:opacity-50"
        >
          {processing ? "处理中..." : "开始处理"}
        </button>
      </div>

      {/* 统计信息 */}
      {stats && (
        <div className="grid grid-cols-4 gap-4 p-4 bg-secondary rounded-lg">
          <div className="text-center">
            <div className="text-2xl font-bold">
              {formatDuration(stats.originalDuration)}
            </div>
            <div className="text-sm text-muted-foreground">原时长</div>
          </div>
          <div className="text-center">
            <div className="text-2xl font-bold text-destructive">
              -{formatDuration(stats.removedDuration)}
            </div>
            <div className="text-sm text-muted-foreground">剪掉</div>
          </div>
          <div className="text-center">
            <div className="text-2xl font-bold text-success">
              {formatDuration(stats.finalDuration)}
            </div>
            <div className="text-sm text-muted-foreground">剪后时长</div>
          </div>
          <div className="text-center">
            <div className="text-2xl font-bold">{stats.keptSegments}</div>
            <div className="text-sm text-muted-foreground">保留片段</div>
          </div>
        </div>
      )}

      {/* 波形预览 (简化版本) */}
      {silenceSegments.length > 0 && (
        <div className="space-y-2">
          <h3 className="font-semibold">
            检测到的静音段 ({silenceSegments.length} 个)
          </h3>
          <div className="max-h-40 overflow-y-auto space-y-1">
            {silenceSegments.map((seg, idx) => (
              <div key={idx} className="flex items-center gap-2 text-sm">
                <span className="text-muted-foreground w-20">
                  {formatDuration(seg.start)} - {formatDuration(seg.end)}
                </span>
                <span className="text-destructive">
                  {seg.duration.toFixed(2)} 秒
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
