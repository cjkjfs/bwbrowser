"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { LuCloud, LuLogOut, LuRefreshCw, LuUser } from "react-icons/lu";
import {
  formatDate,
  formatHours,
  RemoteHoursMeter,
} from "@/components/cookie-bot-shared";
import { LoadingButton } from "@/components/loading-button";
import { TeamUsagePanel } from "@/components/team-usage-panel";
import {
  AnimatedTabs,
  AnimatedTabsContent,
  AnimatedTabsList,
  AnimatedTabsTrigger,
} from "@/components/ui/animated-tabs";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { useCloudAuth } from "@/hooks/use-cloud-auth";
import { cookieBotScopeFor, useCookieBot } from "@/hooks/use-cookie-bot";
import { translateBackendError } from "@/lib/backend-errors";
import { effectivePlanOf, isTeamOwner } from "@/lib/entitlements";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import type { SyncSettings } from "@/types";

interface AccountPageProps {
  isOpen: boolean;
  onClose: () => void;
  subPage?: boolean;
  onOpenSignIn: () => void;
}

type ConnectionStatus = "unknown" | "testing" | "connected" | "error";

export function AccountPage({
  isOpen,
  onClose,
  subPage,
  onOpenSignIn,
}: AccountPageProps) {
  const { t } = useTranslation();
  const {
    user,
    isLoggedIn,
    isLoading: isCloudLoading,
    logout,
    refreshProfile,
  } = useCloudAuth();
  const {
    user: bwbrowserUser,
    logout: bwbrowserLogout,
    refreshProfile: bwbrowserRefresh,
  } = useBwbrowserAuth();
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [isLoggingOut, setIsLoggingOut] = useState(false);

  // Remote hours are plan truth, so they belong here rather than only next to
  // the controls that spend them. Until this landed, `remote-sessions/quota`
  // had no caller anywhere and a customer's first sight of their allowance was
  // a refused launch.
  // 远程时长（Cookie Bot）仅 Donut 付费套餐使用，bwbrowser 体系不显示
  const remoteHoursVisible = false;
  const showTeamUsage = remoteHoursVisible && isTeamOwner(user);
  // A member's own row says "free" because the owner pays. The plan the seat
  // is served under is the one the customer expects to read here, and the
  // billing period slot names the seat instead, since a seat has no period.
  const effectivePlan = effectivePlanOf(user);
  const _isTeamSeat = user != null && effectivePlan !== user.plan;
  const seatRole =
    user?.teamRole === "owner"
      ? t("sync.team.roleOwner")
      : user?.teamRole === "admin"
        ? t("sync.team.roleAdmin")
        : t("sync.team.roleMember");
  const _seatLabel = user?.teamName
    ? t("account.teamSeat", { role: seatRole, team: user.teamName })
    : t("account.teamSeatUnnamed", { role: seatRole });
  const { quota, isLoading: isQuotaLoading } = useCookieBot(
    remoteHoursVisible,
    cookieBotScopeFor(user),
  );
  const [activeTab, setActiveTab] = useState("account");

  // Signing out (or losing the team) removes the tab while it is the selected
  // one, which would leave the page showing an empty panel with no trigger to
  // click back to.
  useEffect(() => {
    if (!showTeamUsage && activeTab === "team-usage") setActiveTab("account");
  }, [showTeamUsage, activeTab]);

  // Self-hosted server state. Loaded once when the dialog opens and persisted
  // via `save_sync_settings` so the rest of the app picks up the new URL/token
  // from `SettingsManager`.
  const [serverUrl, setServerUrl] = useState("");
  const [token, setToken] = useState("");
  const [_showToken, _setShowToken] = useState(false);
  const [_isSavingSelfHosted, setIsSavingSelfHosted] = useState(false);
  const [_isTestingConnection, setIsTestingConnection] = useState(false);
  const [_connectionStatus, setConnectionStatus] =
    useState<ConnectionStatus>("unknown");

  const _hasConfig = Boolean(serverUrl && token);
  // Self-hosted and cloud are mutually exclusive — both share the same sync
  // engine and a profile can't be sync'd to two backends. The tab trigger is
  // disabled here AND the backend rejects mixed state (see `save_sync_settings`
  // / `cloud_logout`), so even if someone bypasses the UI we don't end up
  // with split-brain.
  const _selfHostedDisabled = isLoggedIn || isCloudLoading;

  const handleRefresh = async () => {
    setIsRefreshing(true);
    try {
      await refreshProfile();
      showSuccessToast(t("account.refreshed"));
    } catch (e) {
      showErrorToast(String(e));
    } finally {
      setIsRefreshing(false);
    }
  };

  const handleLogout = async () => {
    setIsLoggingOut(true);
    try {
      await logout();
      await loadSelfHostedSettings();
      showSuccessToast(t("account.loggedOut"));
      onClose();
      onOpenSignIn();
    } catch (e) {
      showErrorToast(String(e));
    } finally {
      setIsLoggingOut(false);
    }
  };

  const loadSelfHostedSettings = useCallback(async () => {
    try {
      const settings = await invoke<SyncSettings>("get_sync_settings");
      setServerUrl(settings.sync_server_url ?? "");
      setToken(settings.sync_token ?? "");
      setConnectionStatus(
        settings.sync_server_url && settings.sync_token ? "unknown" : "unknown",
      );
    } catch (error) {
      console.error("Failed to load sync settings:", error);
    }
  }, []);

  useEffect(() => {
    if (isOpen) {
      void loadSelfHostedSettings();
    }
  }, [isOpen, loadSelfHostedSettings]);

  const _handleTestConnection = useCallback(async () => {
    if (!serverUrl) {
      showErrorToast(t("sync.config.serverUrlRequired"));
      return;
    }
    setIsTestingConnection(true);
    setConnectionStatus("testing");
    try {
      const healthUrl = `${serverUrl.replace(/\/$/, "")}/health`;
      const response = await fetch(healthUrl);
      if (response.ok) {
        setConnectionStatus("connected");
        showSuccessToast(t("sync.config.connectionSuccess"));
      } else {
        setConnectionStatus("error");
        showErrorToast(t("sync.config.serverError"));
      }
    } catch {
      setConnectionStatus("error");
      showErrorToast(t("sync.config.connectFailed"));
    } finally {
      setIsTestingConnection(false);
    }
  }, [serverUrl, t]);

  const _handleSaveSelfHosted = useCallback(async () => {
    setIsSavingSelfHosted(true);
    try {
      await invoke<SyncSettings>("save_sync_settings", {
        syncServerUrl: serverUrl || null,
        syncToken: token || null,
      });
      try {
        await invoke("restart_sync_service");
      } catch (e) {
        console.error("Failed to restart sync service:", e);
      }
      showSuccessToast(t("sync.config.settingsSaved"));
    } catch (error) {
      console.error("Failed to save sync settings:", error);
      // Use the structured backend-error translator so the cloud-vs-self-
      // hosted mutex (`SELF_HOSTED_REQUIRES_LOGOUT`) shows a clear message
      // instead of the generic "save failed" toast.
      showErrorToast(translateBackendError(t as never, error));
    } finally {
      setIsSavingSelfHosted(false);
    }
  }, [serverUrl, token, t]);

  const _handleDisconnectSelfHosted = useCallback(async () => {
    setIsSavingSelfHosted(true);
    try {
      await invoke<SyncSettings>("save_sync_settings", {
        syncServerUrl: null,
        syncToken: null,
      });
      try {
        await invoke("restart_sync_service");
      } catch (e) {
        console.error("Failed to restart sync service:", e);
      }
      setServerUrl("");
      setToken("");
      setConnectionStatus("unknown");
      showSuccessToast(t("sync.config.disconnected"));
    } catch (error) {
      console.error("Failed to disconnect:", error);
      showErrorToast(t("sync.config.disconnectFailed"));
    } finally {
      setIsSavingSelfHosted(false);
    }
  }, [t]);

  return (
    <Dialog open={isOpen} onOpenChange={onClose} subPage={subPage}>
      <DialogContent className="flex max-h-[calc(100vh-5rem)] max-w-3xl flex-col">
        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className={cn(subPage && "mx-auto w-full max-w-4xl")}>
            <AnimatedTabs value={activeTab} onValueChange={setActiveTab}>
              <AnimatedTabsList>
                <AnimatedTabsTrigger value="account">
                  {t("account.tabs.account")}
                </AnimatedTabsTrigger>
                {showTeamUsage && (
                  <AnimatedTabsTrigger value="team-usage">
                    {t("account.tabs.teamUsage")}
                  </AnimatedTabsTrigger>
                )}
              </AnimatedTabsList>

              <AnimatedTabsContent value="account" className="mt-4">
                <div className="flex flex-col gap-4">
                  {/* 统一用户信息卡片：bwbrowser + Donut 合并 */}
                  {(bwbrowserUser || (isLoggedIn && user)) && (
                    <div className="rounded-lg border border-border bg-card p-4">
                      <div className="flex items-center gap-3">
                        {/* bwbrowser 头像 */}
                        {bwbrowserUser ? (
                          bwbrowserUser.avatar ? (
                            <span
                              role="img"
                              aria-label={
                                bwbrowserUser.realName || bwbrowserUser.email
                              }
                              className="size-12 shrink-0 rounded-full bg-cover bg-center border-2 border-primary/20"
                              style={{
                                backgroundImage: `url(${bwbrowserUser.avatar})`,
                              }}
                            />
                          ) : (
                            <span className="grid size-12 shrink-0 place-items-center rounded-full bg-primary text-base font-bold text-primary-foreground">
                              {(
                                bwbrowserUser.realName ||
                                bwbrowserUser.email ||
                                "?"
                              )
                                .charAt(0)
                                .toUpperCase()}
                            </span>
                          )
                        ) : (
                          <div className="grid size-12 shrink-0 place-items-center rounded-full bg-accent text-accent-foreground">
                            <LuUser className="size-6" />
                          </div>
                        )}
                        <div className="min-w-0 flex-1">
                          <h2 className="truncate text-base font-semibold">
                            {bwbrowserUser?.realName ||
                              bwbrowserUser?.email ||
                              user?.email ||
                              "未登录"}
                          </h2>
                          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
                            {bwbrowserUser?.teamRole && (
                              <span>
                                {{
                                  member: "组员",
                                  leader: "组长",
                                  supervisor: "主管",
                                  manager: "经理",
                                  admin: "管理员",
                                  super_admin: "超级管理员",
                                }[bwbrowserUser.teamRole as string] ||
                                  bwbrowserUser.teamRole}
                              </span>
                            )}
                            {bwbrowserUser?.teamName && (
                              <>
                                <span className="text-border">·</span>
                                <span
                                  className="truncate"
                                  title={bwbrowserUser.teamName}
                                >
                                  {bwbrowserUser.teamName}
                                </span>
                              </>
                            )}
                          </div>
                        </div>
                      </div>

                      {/* 字段网格 */}
                      <div className="mt-4 grid grid-cols-2 gap-2 text-xs">
                        {bwbrowserUser?.planName && (
                          <div className="rounded-md border border-border bg-muted/40 px-3 py-2">
                            <p className="text-[10px] tracking-wide text-muted-foreground uppercase">
                              套餐
                            </p>
                            <p className="mt-0.5 font-medium">
                              {bwbrowserUser.planName}
                            </p>
                          </div>
                        )}
                        {typeof bwbrowserUser?.stats?.profiles_count ===
                          "number" && (
                          <div className="rounded-md border border-border bg-muted/40 px-3 py-2">
                            <p className="text-[10px] tracking-wide text-muted-foreground uppercase">
                              环境
                            </p>
                            <p className="mt-0.5 tabular-nums font-medium">
                              {bwbrowserUser.stats.profiles_count}
                            </p>
                          </div>
                        )}
                        {typeof bwbrowserUser?.stats?.proxies_count ===
                          "number" && (
                          <div className="rounded-md border border-border bg-muted/40 px-3 py-2">
                            <p className="text-[10px] tracking-wide text-muted-foreground uppercase">
                              代理
                            </p>
                            <p className="mt-0.5 tabular-nums font-medium">
                              {bwbrowserUser.stats.proxies_count}
                            </p>
                          </div>
                        )}
                        {typeof bwbrowserUser?.stats?.accounts_count ===
                          "number" && (
                          <div className="rounded-md border border-border bg-muted/40 px-3 py-2">
                            <p className="text-[10px] tracking-wide text-muted-foreground uppercase">
                              账号
                            </p>
                            <p className="mt-0.5 tabular-nums font-medium">
                              {bwbrowserUser.stats.accounts_count}
                            </p>
                          </div>
                        )}
                      </div>
                    </div>
                  )}

                  {remoteHoursVisible && (
                    // A headline block, not one field among six: the allowance
                    // is the number a customer needs before a launch is
                    // refused, which is the only way they ever saw it before.
                    <div className="rounded-md border border-border bg-muted/40 px-3 py-2.5">
                      <div className="flex items-baseline justify-between gap-3">
                        <p className="text-[10px] tracking-wide text-muted-foreground uppercase">
                          {t("cookieBot.hours.label")}
                        </p>
                        {formatDate(quota?.period_end) && (
                          <p className="text-xs tabular-nums text-muted-foreground">
                            {t("cookieBot.hours.resets", {
                              date: formatDate(quota?.period_end),
                            })}
                          </p>
                        )}
                      </div>
                      <p className="mt-1 text-lg leading-none font-semibold tabular-nums">
                        {quota ? formatHours(quota.remaining_hours) : "—"}
                        <span className="ml-1 text-sm font-normal text-muted-foreground">
                          {t("cookieBot.hours.remainingOf", {
                            total: quota
                              ? formatHours(quota.granted_hours)
                              : "—",
                          })}
                        </span>
                      </p>
                      <RemoteHoursMeter
                        quota={quota}
                        isLoading={isQuotaLoading}
                        variant="inline"
                        className="mt-2"
                      />
                      <div className="mt-2 flex items-baseline justify-between gap-3">
                        <p className="text-xs tabular-nums text-muted-foreground">
                          {t("cookieBot.hours.used", {
                            used: quota ? formatHours(quota.used_hours) : "—",
                            total: quota
                              ? formatHours(quota.granted_hours)
                              : "—",
                          })}
                        </p>
                        {showTeamUsage && (
                          <button
                            type="button"
                            onClick={() => {
                              setActiveTab("team-usage");
                            }}
                            className="text-xs text-muted-foreground underline underline-offset-2 transition-colors duration-100 hover:text-foreground"
                          >
                            {t("account.viewTeamUsage")}
                          </button>
                        )}
                      </div>
                    </div>
                  )}

                  <div className="mt-2 flex flex-wrap gap-2">
                    {bwbrowserUser ? (
                      <>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={async () => {
                            setIsRefreshing(true);
                            try {
                              await bwbrowserRefresh();
                              showSuccessToast("已刷新");
                            } catch (e) {
                              showErrorToast(String(e));
                            } finally {
                              setIsRefreshing(false);
                            }
                          }}
                          disabled={isRefreshing}
                          className="h-8 gap-1.5 text-xs"
                        >
                          <LuRefreshCw className="size-3" />
                          刷新
                        </Button>
                        <LoadingButton
                          size="sm"
                          variant="destructive"
                          isLoading={isLoggingOut}
                          disabled={isRefreshing}
                          onClick={() => void handleLogout()}
                          className="h-8 gap-1.5 text-xs"
                        >
                          <LuLogOut className="size-3" />
                          退出登录
                        </LoadingButton>
                      </>
                    ) : isLoggedIn ? (
                      <>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => {
                            void handleRefresh();
                          }}
                          disabled={isRefreshing}
                          className="h-8 gap-1.5 text-xs"
                        >
                          <LuRefreshCw className="size-3" />
                          {t("account.refresh")}
                        </Button>
                        <LoadingButton
                          size="sm"
                          variant="destructive"
                          isLoading={isLoggingOut}
                          disabled={isRefreshing}
                          onClick={() => {
                            void handleLogout();
                          }}
                          className="h-8 gap-1.5 text-xs"
                        >
                          <LuLogOut className="size-3" />
                          {t("account.logout")}
                        </LoadingButton>
                      </>
                    ) : (
                      <Button
                        size="sm"
                        onClick={onOpenSignIn}
                        className="h-8 gap-1.5 text-xs"
                      >
                        <LuCloud className="size-3" />
                        {t("account.signIn")}
                      </Button>
                    )}
                  </div>
                </div>
              </AnimatedTabsContent>

              {showTeamUsage && (
                <AnimatedTabsContent value="team-usage" className="mt-4">
                  <TeamUsagePanel quota={quota} />
                </AnimatedTabsContent>
              )}
            </AnimatedTabs>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
