// video-tools-merge.tsx - 快速合并：多视频按顺序拼接（CutMatch FastMerger 移植）

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useRef, useState } from "react";
import {
  LuArrowDown,
  LuArrowUp,
  LuLoaderCircle,
  LuPlus,
  LuX,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

interface ToolsProgress {
  stage: string;
  percent: number;
  message: string;
}

export function QuickMergeTab() {
  const [files, setFiles] = useState<string[]>([]);
  const [method, setMethod] = useState<"auto" | "lossless" | "reencode">(
    "auto",
  );
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<ToolsProgress | null>(null);
  const [outputPath, setOutputPath] = useState("");
  const dragIndex = useRef<number | null>(null);

  const pickFiles = async () => {
    const selected = await open({
      multiple: true,
      filters: [
        {
          name: "Video Files",
          extensions: ["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "ts"],
        },
      ],
    });
    if (selected) {
      const list = Array.isArray(selected) ? selected : [selected];
      setFiles((prev) => [...prev, ...(list as string[])]);
      setOutputPath("");
    }
  };

  // 选择文件夹：递归加入文件夹下所有视频（后端按文件名排序）
  const pickFolder = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (!selected) return;
    try {
      const list = await invoke<string[]>("video_tools_list_folder_videos", {
        folder: selected,
      });
      setFiles((prev) => [...prev, ...list]);
      setOutputPath("");
    } catch (error) {
      alert(`${error}`);
    }
  };

  const removeFile = (idx: number) => {
    setFiles((prev) => prev.filter((_, i) => i !== idx));
  };

  const moveFile = (idx: number, dir: -1 | 1) => {
    setFiles((prev) => {
      const next = [...prev];
      const target = idx + dir;
      if (target < 0 || target >= next.length) return prev;
      [next[idx], next[target]] = [next[target], next[idx]];
      return next;
    });
  };

  // 拖拽排序
  const onDropAt = (idx: number) => {
    const from = dragIndex.current;
    dragIndex.current = null;
    if (from === null || from === idx) return;
    setFiles((prev) => {
      const next = [...prev];
      const [moved] = next.splice(from, 1);
      next.splice(idx, 0, moved);
      return next;
    });
  };

  const defaultOutput = () => {
    if (files.length === 0) return "";
    const dir = files[0].split(/[/\\]/).slice(0, -1).join("/");
    const sep = files[0].includes("\\") ? "\\" : "/";
    return `${dir}${sep}merged_${Date.now() % 100000}.mp4`;
  };

  const handleMerge = async () => {
    if (files.length < 2) return;
    setRunning(true);
    setOutputPath("");
    setProgress({ stage: "merge", percent: 0, message: "准备合并..." });
    try {
      const out = outputPath.trim() || defaultOutput();
      const path = await invoke<string>("video_tools_merge", {
        files,
        outputPath: out,
        method,
      });
      setOutputPath(path);
      alert(`合并完成:\n${path}`);
    } catch (error) {
      console.error("合并失败:", error);
      alert(`合并失败: ${error}`);
    } finally {
      setRunning(false);
      setTimeout(() => setProgress(null), 2000);
    }
  };

  return (
    <div className="space-y-5 max-w-[1400px]">
      <div>
        <h2 className="text-xl font-bold mb-1">快速合并</h2>
        <p className="text-sm text-muted-foreground">
          多个视频按顺序拼接成一个文件。参数一致时无损秒合，不一致自动重编码。
        </p>
      </div>

      {/* 文件列表 */}
      <div className="space-y-2">
        {files.map((f, idx) => (
          <div
            key={`${f}-${idx}`}
            draggable
            role="listitem"
            onDragStart={() => (dragIndex.current = idx)}
            onDragOver={(e) => e.preventDefault()}
            onDrop={() => onDropAt(idx)}
            className={cn(
              "flex items-center gap-2 p-2.5 bg-secondary rounded-lg border border-border cursor-grab",
            )}
          >
            <span className="text-xs font-medium text-muted-foreground w-6 shrink-0">
              {idx + 1}
            </span>
            <span className="text-sm truncate flex-1">{f}</span>
            <button
              type="button"
              onClick={() => moveFile(idx, -1)}
              disabled={idx === 0}
              className="p-1 text-muted-foreground hover:text-foreground disabled:opacity-30"
              title="上移"
            >
              <LuArrowUp className="h-3.5 w-3.5" />
            </button>
            <button
              type="button"
              onClick={() => moveFile(idx, 1)}
              disabled={idx === files.length - 1}
              className="p-1 text-muted-foreground hover:text-foreground disabled:opacity-30"
              title="下移"
            >
              <LuArrowDown className="h-3.5 w-3.5" />
            </button>
            <button
              type="button"
              onClick={() => removeFile(idx)}
              className="p-1 text-muted-foreground hover:text-destructive"
              title="移除"
            >
              <LuX className="h-3.5 w-3.5" />
            </button>
          </div>
        ))}
        <Button
          variant="outline"
          size="sm"
          onClick={pickFiles}
          className="w-full border-dashed"
        >
          <LuPlus className="mr-1 h-3.5 w-3.5" />
          添加视频（可多选，拖拽排序）
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={pickFolder}
          className="w-full border-dashed"
        >
          添加文件夹（递归加入全部视频）
        </Button>
      </div>

      {/* 合并方式 */}
      {files.length >= 2 && (
        <div className="space-y-1">
          <span className="text-sm font-medium">合并方式</span>
          <div className="flex gap-2">
            {(
              [
                ["auto", "自动（推荐）"],
                ["lossless", "无损快速"],
                ["reencode", "重编码兼容"],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                onClick={() => setMethod(key)}
                className={cn(
                  "px-3 py-1.5 text-xs rounded",
                  method === key
                    ? "bg-primary text-primary-foreground"
                    : "bg-secondary-foreground/10 hover:bg-secondary-foreground/20",
                )}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* 合并按钮 */}
      <Button
        onClick={handleMerge}
        disabled={files.length < 2 || running}
        className="w-full"
      >
        {running ? (
          <>
            <LuLoaderCircle className="mr-2 h-4 w-4 animate-spin" />
            合并中...
          </>
        ) : (
          `合并 ${files.length} 个视频`
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

      {outputPath && (
        <p className="text-xs text-success font-mono break-all">{outputPath}</p>
      )}
    </div>
  );
}
