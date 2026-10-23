// video-tools-page.tsx - 视频工具页：视频匹配 / 快速合并 / 快速拆分

import { useState } from "react";
import {
  AnimatedTabs,
  AnimatedTabsList,
  AnimatedTabsTrigger,
} from "@/components/ui/animated-tabs";
import { VideoMatchTab } from "@/components/video-matcher";
import { QuickMergeTab } from "@/components/video-tools-merge";
import { QuickSplitTab } from "@/components/video-tools-split";
import { cn } from "@/lib/utils";

export function VideoToolsPage() {
  // 三个 Tab 的内容全部常驻挂载（forceMount 语义），只用 hidden 切换显隐——
  // Radix TabsContent 默认卸载非活动内容，会导致播放器/任务队列状态丢失
  const [active, setActive] = useState("match");
  return (
    <div className="flex min-h-0 flex-1 flex-col p-6">
      <AnimatedTabs
        value={active}
        onValueChange={setActive}
        className="flex min-h-0 flex-1 flex-col"
      >
        <AnimatedTabsList className="mb-4 shrink-0">
          <AnimatedTabsTrigger value="match">视频匹配</AnimatedTabsTrigger>
          <AnimatedTabsTrigger value="merge">快速合并</AnimatedTabsTrigger>
          <AnimatedTabsTrigger value="split">快速拆分</AnimatedTabsTrigger>
        </AnimatedTabsList>
        <div
          className={cn(
            "min-h-0 flex-1 overflow-y-auto",
            active !== "match" && "hidden",
          )}
        >
          <VideoMatchTab />
        </div>
        <div
          className={cn(
            "min-h-0 flex-1 overflow-y-auto",
            active !== "merge" && "hidden",
          )}
        >
          <QuickMergeTab />
        </div>
        <div
          className={cn(
            "min-h-0 flex-1 overflow-y-auto",
            active !== "split" && "hidden",
          )}
        >
          <QuickSplitTab />
        </div>
      </AnimatedTabs>
    </div>
  );
}
