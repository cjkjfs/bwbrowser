"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { motion, useReducedMotion } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FiWifi } from "react-icons/fi";
import { GoGear } from "react-icons/go";
import {
  LuClapperboard,
  LuCloud,
  LuDownload,
  LuGlobe,
  LuMonitorSmartphone,
  LuMusic,
  LuPuzzle,
  LuSparkles,
  LuUser,
  LuVideo,
} from "react-icons/lu";
import { confirmDialog } from "@/components/ui/confirm-dialog";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { useBwbrowserPermissions } from "@/hooks/use-bwbrowser-permissions";
import { useInputModality } from "@/hooks/use-input-modality";
import { MOTION_SPRING_POSITION } from "@/lib/motion";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import { Logo } from "./icons/logo";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";

export type AppPage =
  | "profiles"
  | "proxies"
  | "cloudAccounts"
  | "extensions"
  | "groups"
  | "cookieBot"
  | "agent"
  | "vpns"
  | "settings"
  | "integrations"
  | "account"
  | "userManagement"
  | "remoteManagement"
  | "videoDownload"
  | "audioProcessor"
  | "videoMatcher"
  | "doubaoVideo"
  | "aiRewrite"
  | "import"
  | "shortcuts"
  | "trash";

// 中文菜单项（直接硬编码中文，不依赖 i18n）
const RAIL_LABELS_ZH: Record<AppPage, string> = {
  profiles: "环境管理",
  proxies: "代理中心",
  cloudAccounts: "云端账号",
  extensions: "扩展管理",
  groups: "分组管理",
  cookieBot: "Cookie 养号",
  agent: "智能助手",
  vpns: "VPN 管理",
  settings: "系统设置",
  integrations: "集成服务",
  account: "账号中心",
  userManagement: "用户管理",
  remoteManagement: "远程管理",
  videoDownload: "视频下载",
  audioProcessor: "音频处理",
  videoMatcher: "视频工具",
  doubaoVideo: "豆包视频",
  aiRewrite: "AI 洗稿",
  import: "导入配置",
  shortcuts: "快捷键",
  trash: "回收站",
};

interface RailNavProps {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
  /**
   * A remote session is running right now. The Cookie Bot item carries a dot so
   * the state is legible from every other page — an overnight job you cannot
   * see from where you are standing may as well not be observable at all.
   */
  cookieBotRunning?: boolean;
}

/** Shared-element indicator that slides between the active rail items. */
function ActiveIndicator() {
  const reduceMotion = useReducedMotion();
  const inputModality = useInputModality();
  const animate = !reduceMotion && inputModality === "pointer";
  return (
    <motion.span
      aria-hidden="true"
      initial={false}
      layoutId={animate ? "rail-indicator" : undefined}
      transition={animate ? MOTION_SPRING_POSITION : { duration: 0 }}
      className="absolute inset-y-2 left-[-8px] w-[3px] rounded-full bg-gradient-to-b from-primary via-primary/70 to-primary/30 shadow-[0_0_10px_-1px_var(--primary)]"
    />
  );
}

interface RailItem {
  page: AppPage;
  Icon: React.ComponentType<{ className?: string }>;
  labelKey: string;
}

const TOP_ITEMS: RailItem[] = [
  { page: "profiles", Icon: LuUser, labelKey: "rail.profiles" },
  { page: "proxies", Icon: FiWifi, labelKey: "rail.network" },
  { page: "cloudAccounts", Icon: LuGlobe, labelKey: "rail.cloudAccounts" },
  {
    page: "remoteManagement",
    Icon: LuMonitorSmartphone,
    labelKey: "rail.remoteManagement",
  },
  { page: "videoDownload", Icon: LuDownload, labelKey: "rail.videoDownload" },
  { page: "audioProcessor", Icon: LuMusic, labelKey: "rail.audioProcessor" },
  { page: "videoMatcher", Icon: LuClapperboard, labelKey: "rail.videoMatcher" },
  { page: "doubaoVideo", Icon: LuVideo, labelKey: "rail.doubaoVideo" },
  { page: "aiRewrite", Icon: LuSparkles, labelKey: "rail.aiRewrite" },
  { page: "extensions", Icon: LuPuzzle, labelKey: "rail.extensions" },
  { page: "account", Icon: LuCloud, labelKey: "rail.account" },
];

const ADMIN_ONLY_PAGES: AppPage[] = [];

const ADMIN_ROLES = ["manager", "admin", "super_admin"];

/** 底部用户信息卡片：显示头像、姓名、公司、套餐等 */
export function UserInfoCard() {
  const { user } = useBwbrowserAuth();
  if (!user) return null;

  const displayName = user.realName || user.email || "";
  const initial = displayName.charAt(0).toUpperCase() || "?";
  const roleLabel = user.teamRole
    ? {
        member: "组员",
        leader: "组长",
        supervisor: "主管",
        manager: "经理",
        admin: "管理员",
        super_admin: "超级管理员",
      }[user.teamRole as string] || user.teamRole
    : "";
  const profilesCount = user.stats?.profiles_count ?? 0;
  const proxiesCount = user.stats?.proxies_count ?? 0;

  return (
    <div className="mx-2 mb-1 shrink-0 rounded-lg border border-border bg-card/50 p-2.5">
      <div className="flex items-center gap-2">
        {user.avatar ? (
          <span
            role="img"
            aria-label={displayName}
            className="size-7 shrink-0 rounded-full bg-cover bg-center"
            style={{ backgroundImage: `url(${user.avatar})` }}
          />
        ) : (
          <span className="grid size-7 shrink-0 place-items-center rounded-full bg-primary text-xs font-bold text-primary-foreground">
            {initial}
          </span>
        )}
        <div className="flex min-w-0 flex-col">
          <span className="truncate text-xs font-semibold text-foreground">
            {displayName}
          </span>
          {roleLabel && (
            <span className="text-[10px] text-muted-foreground">
              {roleLabel}
            </span>
          )}
        </div>
      </div>
      <div className="mt-2 space-y-0.5 text-[10px] text-muted-foreground">
        {user.teamName && (
          <div className="flex items-center justify-between">
            <span>公司</span>
            <span className="truncate text-foreground/80" title={user.teamName}>
              {user.teamName}
            </span>
          </div>
        )}
        {user.planName && (
          <div className="flex items-center justify-between">
            <span>套餐</span>
            <span className="text-foreground/80">{user.planName}</span>
          </div>
        )}
        {profilesCount > 0 && (
          <div className="flex items-center justify-between">
            <span>环境</span>
            <span className="tabular-nums text-foreground/80">
              {profilesCount}
            </span>
          </div>
        )}
        {proxiesCount > 0 && (
          <div className="flex items-center justify-between">
            <span>代理</span>
            <span className="tabular-nums text-foreground/80">
              {proxiesCount}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

export function RailNav({
  currentPage,
  onNavigate,
  cookieBotRunning = false,
}: RailNavProps) {
  const { t } = useTranslation();
  const { user } = useBwbrowserAuth();
  const {
    canDownloadVideos,
    canAiRewrite,
    canManageRemote,
    canVideoTools,
    canDoubaoVideo,
    canAudioProcessor,
    canExtensions,
    canProfileManagement,
    canProxyCenter,
    canCloudAccount,
    canCloudAccounts,
  } = useBwbrowserPermissions();
  const isAdmin = !!user?.teamRole && ADMIN_ROLES.includes(user.teamRole);

  // AI 洗稿进行中任务数：直接跟踪后端运行状态（任务运行时菜单显示「AI 洗稿(N)」且图标转动）。
  // 后端任务在页面切走后仍继续运行，所以这里不依赖洗稿页面挂载，导航栏自己监听事件 + 启动时问一次后端。
  const [aiRewriteActive, setAiRewriteActive] = useState(0);
  const aiRewriteLiveKeys = useRef<Set<string>>(new Set());
  const syncAiRewriteActive = useCallback(() => {
    setAiRewriteActive(aiRewriteLiveKeys.current.size);
  }, []);
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void invoke<{
      tasks?: Array<{ task_key?: string; stage?: string }>;
    }>("ai_rewrite_running_tasks")
      .then((res) => {
        for (const r of res.tasks ?? []) {
          if (
            r.task_key &&
            !["done", "error", "interrupted"].includes(r.stage ?? "")
          ) {
            aiRewriteLiveKeys.current.add(r.task_key);
          }
        }
        syncAiRewriteActive();
      })
      .catch(() => {});
    void listen<{
      task_key?: string;
      stage?: string;
    }>("ai-rewrite:progress", (e) => {
      const key = e.payload?.task_key;
      if (!key) return;
      const terminal = ["done", "error", "interrupted"].includes(
        e.payload?.stage ?? "",
      );
      if (terminal) aiRewriteLiveKeys.current.delete(key);
      else aiRewriteLiveKeys.current.add(key);
      syncAiRewriteActive();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
    };
  }, [syncAiRewriteActive]);

  const visibleTopItems = TOP_ITEMS.filter((item) => {
    if (ADMIN_ONLY_PAGES.includes(item.page) && !isAdmin) return false;
    if (item.page === "remoteManagement" && !canManageRemote) return false;
    if (item.page === "videoDownload" && !canDownloadVideos) return false;
    if (item.page === "aiRewrite" && !canAiRewrite) return false;
    if (item.page === "videoMatcher" && !canVideoTools) return false;
    if (item.page === "doubaoVideo" && !canDoubaoVideo) return false;
    if (item.page === "audioProcessor" && !canAudioProcessor) return false;
    if (item.page === "extensions" && !canExtensions) return false;
    if (item.page === "profiles" && !canProfileManagement) return false;
    if (item.page === "proxies" && !canProxyCenter) return false;
    if (item.page === "cloudAccounts" && !canCloudAccounts) return false;
    if (item.page === "account" && !canCloudAccount) return false;
    return true;
  });
  // 导航项入场/微交互动画开关（指针输入 + 未开启系统减弱动效时才播放）。
  const reduceMotion = useReducedMotion();
  const inputModality = useInputModality();
  const playfulMotion = !reduceMotion && inputModality === "pointer";

  // ===== 爆文库入口 =====
  // 应用版本号（侧边栏底部显示 v2.4.8 之类）
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    invoke<{ app_version: string }>("get_system_info")
      .then((info) => setAppVersion(info.app_version))
      .catch(() => {});
  }, []);
  // ===== 门禁远程开门（2026-11-20） =====
  // 仅当登录用户有开门资格（can_open=1）时才显示按钮；未登录/无资格/接口异常一律静默隐藏
  const [doorCanOpen, setDoorCanOpen] = useState(false);
  const [doorOpening, setDoorOpening] = useState(false);
  const refreshDoorPerm = useCallback(() => {
    invoke<{ ok: boolean; can_open?: number }>("bwbrowser_door_info")
      .then((d) => setDoorCanOpen(d.ok && d.can_open === 1))
      .catch(() => setDoorCanOpen(false));
  }, []);
  useEffect(() => {
    void refreshDoorPerm();
    const unlisten = listen("cloud-auth-changed", () => {
      // 登录/登出/切换账号后重查资格
      window.setTimeout(refreshDoorPerm, 500);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refreshDoorPerm]);
  const handleDoorOpen = useCallback(async () => {
    if (doorOpening) return;
    // 点开门先弹确认，防误触；全局确认对话框（与应用 UI 一致的 shadcn 风格）
    const ok = await confirmDialog(
      "确定要远程开门吗？\n\n点击「确认开门」后门将立即打开。",
      {
        title: "远程开门",
        kind: "info",
        okLabel: "确认开门",
        cancelLabel: "取消",
      },
    );
    if (!ok) return;
    setDoorOpening(true);
    invoke<{ ok: boolean; msg?: string }>("bwbrowser_door_open")
      .then((d) => {
        if (d.ok) {
          showSuccessToast(d.msg || "🚪 开门成功");
        } else {
          showErrorToast(d.msg || "开门失败");
        }
      })
      .catch((e) => showErrorToast(String(e)))
      .finally(() => setDoorOpening(false));
  }, [doorOpening]);

  // 侧边栏宽度：w-44 = 176px，加上图标和文字
  const RAIL_WIDTH = "w-48";

  return (
    <nav
      className={cn(
        "relative flex shrink-0 flex-col gap-1 border-r border-border bg-background py-2",
        RAIL_WIDTH,
      )}
    >
      {/* 左上角：远程开门（仅 doorCanOpen 时渲染，无权限则导航项自动上移补位） */}
      {doorCanOpen && (
        <div className="px-3 pb-1">
          <button
            type="button"
            className="group flex w-full items-center gap-2 rounded-md px-1 py-1 text-left transition-colors hover:bg-accent/50 disabled:opacity-60"
            onClick={() => void handleDoorOpen()}
            disabled={doorOpening}
            aria-label="远程开门"
          >
            <span className="grid size-7 shrink-0 place-items-center rounded-md bg-transparent text-foreground select-none">
              <Logo className="size-5" />
            </span>
            <span
              className={cn(
                "text-[15px] font-bold transition-all duration-300",
                doorOpening
                  ? "animate-pulse text-emerald-500"
                  : "text-emerald-600 group-hover:drop-shadow-[0_0_6px_rgba(34,197,94,0.45)]",
              )}
            >
              {doorOpening ? "开门中…" : "远程开门"}
            </span>
          </button>
        </div>
      )}

      <div className="my-1 mx-3 h-px shrink-0 bg-border" />

      <div className="flex min-h-0 w-full flex-1 flex-col gap-1 overflow-y-auto px-2 scrollbar-none [-ms-overflow-style:none] [&::-webkit-scrollbar]:hidden">
        {visibleTopItems.map(({ page, Icon, labelKey }, index) => {
          const active = currentPage === page;
          return (
            <Tooltip key={page} delayDuration={300}>
              <TooltipTrigger asChild>
                <motion.button
                  type="button"
                  initial={playfulMotion ? { opacity: 0, x: -10 } : false}
                  animate={{ opacity: 1, x: 0 }}
                  transition={
                    playfulMotion
                      ? {
                          delay: index * 0.035,
                          type: "spring",
                          stiffness: 400,
                          damping: 30,
                        }
                      : { duration: 0 }
                  }
                  onClick={() => {
                    onNavigate(page);
                  }}
                  aria-label={t(labelKey)}
                  aria-current={active ? "page" : undefined}
                  className={cn(
                    "group relative flex w-full items-center gap-3 rounded-lg px-2.5 py-2.5 transition-all duration-200",
                    active
                      ? "bg-gradient-to-r from-blue-500/25 via-blue-500/10 to-transparent text-foreground"
                      : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
                  )}
                >
                  {active && <ActiveIndicator />}
                  <Icon
                    className={cn(
                      "size-[18px] shrink-0 transition-transform duration-200 group-hover:scale-110 group-active:scale-95",
                      active &&
                        "text-primary drop-shadow-[0_0_5px_var(--primary)]",
                      page === "aiRewrite" &&
                        aiRewriteActive > 0 &&
                        "animate-spin",
                    )}
                  />
                  <span
                    key={active ? "on" : "off"}
                    className={cn(
                      "truncate text-[15px] font-semibold tracking-wide",
                      active &&
                        "animate-rail-text-pop bg-gradient-to-r from-primary to-foreground bg-clip-text text-transparent",
                    )}
                  >
                    {page === "aiRewrite" && aiRewriteActive > 0
                      ? `${RAIL_LABELS_ZH[page]}(${aiRewriteActive})`
                      : RAIL_LABELS_ZH[page]}
                  </span>
                  {page === "cookieBot" && cookieBotRunning && (
                    <span
                      aria-hidden="true"
                      className="ml-auto size-2 shrink-0 rounded-full bg-success shadow-[0_0_6px_var(--success)]"
                    />
                  )}
                </motion.button>
              </TooltipTrigger>
              <TooltipContent side="right">
                {page === "cookieBot" && cookieBotRunning
                  ? t("rail.cookieBotRunning")
                  : t(labelKey)}
              </TooltipContent>
            </Tooltip>
          );
        })}
      </div>

      {/* 底部区域：设置 */}
      <div className="flex flex-col gap-1 px-2">
        <Tooltip delayDuration={300}>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={() => {
                onNavigate("settings");
              }}
              aria-label={t("rail.settings")}
              aria-current={currentPage === "settings" ? "page" : undefined}
              className={cn(
                "group relative flex w-full items-center gap-3 rounded-lg px-2.5 py-2.5 transition-all duration-200",
                currentPage === "settings"
                  ? "bg-gradient-to-r from-blue-500/25 via-blue-500/10 to-transparent text-foreground"
                  : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
              )}
            >
              {currentPage === "settings" && <ActiveIndicator />}
              <GoGear
                className={cn(
                  "size-[18px] shrink-0 transition-transform duration-200 group-hover:scale-110 group-active:scale-95",
                  currentPage === "settings" &&
                    "text-primary drop-shadow-[0_0_5px_var(--primary)]",
                )}
              />
              <span
                key={currentPage === "settings" ? "on" : "off"}
                className={cn(
                  "truncate text-[15px] font-semibold tracking-wide",
                  currentPage === "settings" &&
                    "animate-rail-text-pop bg-gradient-to-r from-primary to-foreground bg-clip-text text-transparent",
                )}
              >
                系统设置
              </span>
              {appVersion && (
                <span
                  className="ml-auto shrink-0 text-[10px] tabular-nums text-muted-foreground/70"
                  title={`BW Browser v${appVersion}`}
                >
                  v{appVersion}
                </span>
              )}
            </button>
          </TooltipTrigger>
          <TooltipContent side="right">{t("rail.settings")}</TooltipContent>
        </Tooltip>
      </div>
    </nav>
  );
}
