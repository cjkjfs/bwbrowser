"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { FaExternalLinkAlt, FaTimes } from "react-icons/fa";
import { LuCheckCheck, LuPackageOpen } from "react-icons/lu";
import { Button } from "@/components/ui/button";
import type { AppUpdateInfo } from "@/types";
import { RippleButton } from "./ui/ripple";

/** Seconds between the "update ready" toast appearing and the automatic restart. */
const AUTO_RESTART_SECONDS = 5;
/** How often to retry the restart while browser windows are still open. */
const BROWSER_RETRY_INTERVAL_MS = 60_000;

interface AppUpdateToastProps {
  updateInfo: AppUpdateInfo;
  onRestart: () => Promise<{ profilesRunning: boolean }>;
  onDismiss: () => void;
  updateReady?: boolean;
  /** When the update is ready, restart automatically after a short countdown. */
  autoRestart?: boolean;
  /** Forced update: countdown cannot be cancelled or dismissed. */
  forceUpdate?: boolean;
}

export function AppUpdateToast({
  updateInfo,
  onRestart,
  onDismiss,
  updateReady = false,
  autoRestart = false,
  forceUpdate = false,
}: AppUpdateToastProps) {
  const { t } = useTranslation();
  const [hasPendingInstaller, setHasPendingInstaller] = useState(false);
  // Auto-restart countdown in seconds; null means manual mode (no auto restart).
  const [countdown, setCountdown] = useState<number | null>(
    updateReady && autoRestart ? AUTO_RESTART_SECONDS : null,
  );
  // Restart is blocked because browser windows are still open — keep retrying
  // until they are all closed, then the next attempt applies the update.
  const [waitingBrowsers, setWaitingBrowsers] = useState(false);

  // Windows 的 exe/msi 更新会 defer 安装包等待重启时执行——存在待安装包时
  // 才显示「手动安装」，其他更新方式（覆盖式/包管理器）没有安装包可打开。
  useEffect(() => {
    if (!updateReady) return;
    let cancelled = false;
    invoke<string | null>("get_pending_update_installer")
      .then((path) => {
        if (!cancelled) setHasPendingInstaller(Boolean(path));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [updateReady]);

  const attemptRestart = useCallback(async () => {
    const result = await onRestart();
    if (result?.profilesRunning) {
      setWaitingBrowsers(true);
    } else {
      // Restart issued (app is exiting) or failed with an error that
      // onRestart already surfaced — either way stop the automatic flow.
      setWaitingBrowsers(false);
    }
    setCountdown(null);
  }, [onRestart]);

  // Countdown tick.
  useEffect(() => {
    if (countdown === null || countdown <= 0) return;
    const timer = setTimeout(
      () => setCountdown((c) => (c === null ? null : c - 1)),
      1000,
    );
    return () => clearTimeout(timer);
  }, [countdown]);

  // Fire the restart when the countdown reaches zero.
  useEffect(() => {
    if (countdown !== 0) return;
    void attemptRestart();
  }, [countdown, attemptRestart]);

  // While browser windows are open, retry periodically — the moment the last
  // window closes, the next attempt succeeds and the update applies itself.
  useEffect(() => {
    if (!waitingBrowsers) return;
    const timer = setInterval(
      () => void attemptRestart(),
      BROWSER_RETRY_INTERVAL_MS,
    );
    return () => clearInterval(timer);
  }, [waitingBrowsers, attemptRestart]);

  const handleManualInstall = async () => {
    try {
      await invoke("open_pending_update_installer");
    } catch (e) {
      console.error("open installer failed:", e);
    }
  };

  const handleRestartClick = async () => {
    await attemptRestart();
  };

  const cancelAutoRestart = () => {
    setCountdown(null);
    setWaitingBrowsers(false);
  };

  const handleViewRelease = () => {
    if (updateInfo.release_page_url) {
      const event = new CustomEvent("url-open-request", {
        detail: updateInfo.release_page_url,
      });
      window.dispatchEvent(event);
    }
  };

  const autoRestartActive = countdown !== null || waitingBrowsers;

  return (
    <div className="flex w-full max-w-md items-start rounded-lg border border-border bg-card p-4 text-card-foreground shadow-lg">
      <div className="mt-0.5 mr-3">
        <LuCheckCheck className="size-5 shrink-0" />
      </div>

      <div className="min-w-0 flex-1">
        <div className="flex items-start justify-between gap-2">
          <div className="flex flex-col gap-1">
            <span className="text-sm font-semibold text-foreground">
              {updateReady
                ? autoRestartActive
                  ? t("appUpdate.toast.autoRestarting")
                  : t("appUpdate.toast.updateReady")
                : updateInfo.repo_update
                  ? "Update available via package manager"
                  : t("appUpdate.toast.manualDownloadRequired")}
            </span>
            <div className="text-xs text-muted-foreground">
              {updateInfo.current_version} → {updateInfo.new_version}
            </div>
          </div>

          {!forceUpdate && (
            <Button
              variant="ghost"
              size="sm"
              onClick={onDismiss}
              className="size-6 shrink-0 p-0"
            >
              <FaTimes className="size-3" />
            </Button>
          )}
        </div>

        <div className="mt-3 flex items-center gap-2">
          {updateReady ? (
            <>
              <RippleButton
                onClick={() => void handleRestartClick()}
                size="sm"
                className="flex items-center gap-2 text-xs"
              >
                <LuCheckCheck className="size-3" />
                {countdown !== null && countdown > 0
                  ? t("appUpdate.toast.autoRestartCountdown", {
                      seconds: countdown,
                    })
                  : t("appUpdate.toast.restartNow")}
              </RippleButton>
              {!forceUpdate && countdown !== null && countdown > 0 && (
                <RippleButton
                  variant="outline"
                  onClick={cancelAutoRestart}
                  size="sm"
                  className="flex items-center gap-2 text-xs"
                >
                  {t("appUpdate.toast.cancelAutoRestart")}
                </RippleButton>
              )}
              {!forceUpdate && hasPendingInstaller && (
                <RippleButton
                  variant="outline"
                  onClick={() => void handleManualInstall()}
                  size="sm"
                  className="flex items-center gap-2 text-xs"
                >
                  <LuPackageOpen className="size-3" />
                  {t("appUpdate.toast.installManually")}
                </RippleButton>
              )}
            </>
          ) : (
            !updateInfo.repo_update &&
            updateInfo.manual_update_required && (
              <RippleButton
                onClick={handleViewRelease}
                size="sm"
                className="flex items-center gap-2 text-xs"
              >
                <FaExternalLinkAlt className="size-3" />
                {t("appUpdate.toast.viewRelease")}
              </RippleButton>
            )
          )}
          {!autoRestartActive && !forceUpdate && (
            <RippleButton
              variant="outline"
              onClick={onDismiss}
              size="sm"
              className="text-xs"
            >
              {t("appUpdate.toast.later")}
            </RippleButton>
          )}
        </div>

        {waitingBrowsers && (
          <div className="mt-2 text-xs text-muted-foreground">
            {t("appUpdate.toast.waitingBrowsers")}
          </div>
        )}
      </div>
    </div>
  );
}
