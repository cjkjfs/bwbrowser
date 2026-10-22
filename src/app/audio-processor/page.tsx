// app/audio-processor/page.tsx - 音频处理页面
"use client";

import { AudioSilenceRemover } from "@/components/audio-silence-remover";

export default function AudioProcessorPage() {
  return (
    <div className="container mx-auto py-8">
      <AudioSilenceRemover />
    </div>
  );
}
