"use client";

import { useTranslation } from "react-i18next";
import { LuCheckCheck } from "react-icons/lu";
import type { AppUpdateProgress } from "@/types";

interface AppUpdateProgressToastProps {
  updateReady: boolean;
  progress: AppUpdateProgress | null;
  version: string;
}

export function AppUpdateProgressToast({
  updateReady,
  progress,
  version,
}: AppUpdateProgressToastProps) {
  const { t } = useTranslation();

  return (
    <div className="flex w-full max-w-md items-start gap-3 rounded-lg border border-border bg-card p-4 text-card-foreground shadow-lg">
      <div className="mt-0.5">
        {updateReady ? (
          <LuCheckCheck className="size-5 text-success" />
        ) : (
          <div className="size-5 animate-spin rounded-full border-2 border-border border-t-primary" />
        )}
      </div>

      <div className="min-w-0 flex-1">
        <span className="block text-sm font-semibold text-foreground">
          {updateReady
            ? t("appUpdate.toast.autoRestarting")
            : t("appUpdate.toast.forceUpdating")}
        </span>
        <div className="mt-1 text-xs text-muted-foreground">
          {version}
          {!updateReady && progress?.percentage
            ? ` · ${progress.percentage}%`
            : null}
          {!updateReady && progress?.speed ? ` · ${progress.speed}` : null}
        </div>
      </div>
    </div>
  );
}
