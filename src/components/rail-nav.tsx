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
  LuDoorOpen,
  LuDownload,
  LuGlobe,
  LuMonitorSmartphone,
  LuMusic,
  LuPuzzle,
  LuSparkles,
  LuUser,
  LuUsers,
  LuVideo,
} from "react-icons/lu";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { useBwbrowserPermissions } from "@/hooks/use-bwbrowser-permissions";
import { useInputModality } from "@/hooks/use-input-modality";
import { launchBwbrowserClone } from "@/lib/bwbrowser-physics";
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

const CLICK_THRESHOLD = 5;
const CLICK_WINDOW_MS = 2000;
const LOGO_HIDDEN_KEY = "bwbrowser-logo-hidden";

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

function useLogoEasterEgg({
  currentPage,
}: {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
}) {
  const reduceMotion = useReducedMotion();
  const inputModality = useInputModality();
  const playfulMotion = !reduceMotion && inputModality === "pointer";
  const clickTimestamps = useRef<number[]>([]);
  const [isPressed, setIsPressed] = useState(false);
  const [wobbleKey, setWobbleKey] = useState(0);
  /** Wiggles earned by the cheat code, which arrives by keyboard by nature. */
  const [cheerKey, setCheerKey] = useState(0);
  const [isFalling, setIsFalling] = useState(false);
  /**
   * Click count toward the bounce trigger while the user is on the profiles
   * page. Capped at 4: each click here grows the logo by 25%, so step 4 has
   * doubled the original size. Click 5 fires `triggerFall` and resets.
   */
  const [growStep, setGrowStep] = useState(0);
  const resetTimeoutRef = useRef<number | null>(null);
  const [isHidden, setIsHidden] = useState(() => {
    try {
      return sessionStorage.getItem(LOGO_HIDDEN_KEY) === "1";
    } catch {
      return false;
    }
  });
  const logoRef = useRef<HTMLButtonElement>(null);
  const cancelFallRef = useRef<(() => void) | null>(null);

  const triggerFall = useCallback(() => {
    const el = logoRef.current;
    if (!el || isFalling) return;
    setIsFalling(true);

    cancelFallRef.current = launchBwbrowserClone(el, {
      onExit: () => {
        try {
          sessionStorage.setItem(LOGO_HIDDEN_KEY, "1");
        } catch {
          // ignore — sessionStorage unavailable in some Tauri WebViews
        }
        setIsHidden(true);
        setIsFalling(false);
      },
    });
  }, [isFalling]);

  useEffect(() => {
    return () => {
      cancelFallRef.current?.();
    };
  }, []);

  const handleClick = useCallback(() => {
    if (isFalling || isHidden) return;

    // 如果不在 profiles 页面，不触发彩蛋动画
    if (currentPage !== "profiles") {
      return;
    }

    if (!playfulMotion) return;

    const now = Date.now();
    clickTimestamps.current = clickTimestamps.current.filter(
      (t) => now - t < CLICK_WINDOW_MS,
    );
    clickTimestamps.current.push(now);

    if (clickTimestamps.current.length >= CLICK_THRESHOLD) {
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
      triggerFall();
    } else {
      setGrowStep(
        Math.min(clickTimestamps.current.length, CLICK_THRESHOLD - 1),
      );
      setWobbleKey((k) => k + 1);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
      }
      resetTimeoutRef.current = window.setTimeout(() => {
        clickTimestamps.current = [];
        setGrowStep(0);
        resetTimeoutRef.current = null;
      }, CLICK_WINDOW_MS);
    }
  }, [currentPage, isFalling, isHidden, triggerFall, playfulMotion]);

  // Leaving the profiles page mid-streak cancels growth so we never end up
  // with an outsized logo when the user returns later.
  useEffect(() => {
    if (currentPage !== "profiles" || !playfulMotion) {
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
    }
  }, [currentPage, playfulMotion]);

  // The cheat code (see useKonamiCode) is received with the same wiggle a
  // click earns, so the bwbrowser visibly takes the credit. It is typed, so the
  // pointer-only gate on click wiggles cannot apply; only reduced motion does.
  useEffect(() => {
    if (reduceMotion) return;
    const onCheatCode = () => setCheerKey((k) => k + 1);
    window.addEventListener("bwbrowser-cheat-code", onCheatCode);
    return () => {
      window.removeEventListener("bwbrowser-cheat-code", onCheatCode);
    };
  }, [reduceMotion]);

  useEffect(() => {
    if (!reduceMotion) return;
    cancelFallRef.current?.();
    cancelFallRef.current = null;
    setIsFalling(false);
    setIsPressed(false);
    if (logoRef.current) logoRef.current.style.visibility = "";
  }, [reduceMotion]);

  useEffect(() => {
    return () => {
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
      }
    };
  }, []);

  return {
    logoRef,
    isPressed,
    setIsPressed,
    wobbleKey,
    cheerKey,
    isFalling,
    isHidden,
    growStep,
    handleClick,
    playfulMotion,
  };
}

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
  { page: "userManagement", Icon: LuUsers, labelKey: "rail.userManagement" },
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
    canManageUsers,
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
    if (item.page === "userManagement" && !canManageUsers) return false;
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
  const {
    logoRef,
    isPressed,
    setIsPressed,
    wobbleKey,
    cheerKey,
    isFalling,
    isHidden,
    growStep,
    handleClick,
    playfulMotion,
  } = useLogoEasterEgg({ currentPage, onNavigate });

  // ===== 爆文库入口 =====
  // 应用版本号（侧边栏底部显示 v2.4.8 之类）
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    invoke<{ app_version: string }>("get_system_info")
      .then((info) => setAppVersion(info.app_version))
      .catch(() => {});
  }, []);
  const [vpsLaunchProgress, setVpsLaunchProgress] = useState<number | null>(
    null,
  );

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
  const handleDoorOpen = useCallback(() => {
    if (doorOpening) return;
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

  // ===== 爆文库入口（2026-11-20 重构） =====
  // 查询当前用户（按归属人）绑定的爆文库账号：有绑定 → 左上角显示「爆文库」按钮，
  // 点击 = 启动绑定账号（完整账号启动链路）并打开 yacm.xin；未绑定 → 按钮不显示。
  const [vpsBind, setVpsBind] = useState<{
    accountId: number;
    accountName: string;
  } | null>(null);
  const refreshVpsBind = useCallback(() => {
    invoke<{
      success: boolean;
      bound?: boolean;
      account?: { id: number; account_name?: string } | null;
    }>("bwbrowser_vps_bind_info")
      .then((d) => {
        if (d.success && d.bound && d.account) {
          setVpsBind({
            accountId: d.account.id,
            accountName: d.account.account_name ?? "",
          });
        } else {
          setVpsBind(null);
        }
      })
      .catch(() => setVpsBind(null));
  }, []);
  useEffect(() => {
    void refreshVpsBind();
    const unlisten = listen("cloud-auth-changed", () => {
      // 登录/登出/切换账号后重查绑定
      window.setTimeout(refreshVpsBind, 500);
    });
    // 编辑账号里开/关「绑定爆文库」后立即刷新左上角入口显隐
    const unlistenBind = listen("vps-bind-changed", () => {
      void refreshVpsBind();
    });
    return () => {
      void unlisten.then((fn) => fn());
      void unlistenBind.then((fn) => fn());
    };
  }, [refreshVpsBind]);
  const handleVpsLogoLeftClick = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();

      // 触发 logo 彩蛋动画
      handleClick();

      // 未绑定爆文库账号：仅保留彩蛋，不启动（此时「爆文库」文字不显示）
      if (!vpsBind) return;

      const accountId = vpsBind.accountId;
      let unlistenProgress: (() => void) | undefined;

      setVpsLaunchProgress(0);

      // 监听后端真实启动进度（绑定账号走 account-launch-progress，按 account_id 过滤）
      import("@tauri-apps/api/event")
        .then(({ listen }) =>
          listen<{ account_id: number; pct: number; label: string }>(
            "account-launch-progress",
            (event) => {
              if (event.payload.account_id !== accountId) return;
              const pct = Math.min(100, Math.max(0, event.payload.pct));
              setVpsLaunchProgress(pct);
            },
          ),
        )
        .then((fn) => {
          unlistenProgress = fn;
        })
        .catch((e) => {
          console.warn("failed to listen launch progress", e);
        });

      invoke("bwbrowser_open_vps_login")
        .then(() => {
          setVpsLaunchProgress(100);
          setTimeout(() => {
            setVpsLaunchProgress(null);
            showSuccessToast("爆文库已启动");
          }, 500);
        })
        .catch((err: unknown) => {
          setVpsLaunchProgress(null);
          const msg = err instanceof Error ? err.message : String(err);
          showErrorToast(`启动失败: ${msg}`);
        })
        .finally(() => {
          unlistenProgress?.();
        });
    },
    [handleClick, vpsBind],
  );

  // 侧边栏宽度：w-44 = 176px，加上图标和文字
  const RAIL_WIDTH = "w-48";

  return (
    <nav
      className={cn(
        "relative flex shrink-0 flex-col gap-1 border-r border-border bg-background py-2",
        RAIL_WIDTH,
      )}
    >
      {!isHidden ? (
        <div className="px-3 pb-1">
          <button
            type="button"
            className="group flex w-full items-center gap-2 rounded-md px-1 py-1 text-left transition-colors hover:bg-accent/50"
            onClick={handleVpsLogoLeftClick}
            onContextMenu={(e) => e.preventDefault()}
            title={
              vpsBind
                ? `启动爆文库（${vpsBind.accountName || `账号 #${vpsBind.accountId}`}）`
                : undefined
            }
          >
            <span
              ref={logoRef as React.RefObject<HTMLSpanElement>}
              className="grid size-7 shrink-0 place-items-center rounded-md bg-transparent text-foreground select-none"
              onPointerDown={() => {
                setIsPressed(true);
              }}
              onPointerUp={() => {
                setIsPressed(false);
              }}
              onPointerLeave={() => {
                setIsPressed(false);
              }}
            >
              <span
                style={{
                  transform: !playfulMotion
                    ? "none"
                    : isPressed
                      ? `scale(${(1 + growStep * 0.25) * 0.9})`
                      : `scale(${1 + growStep * 0.25})`,
                }}
                className="inline-grid place-items-center transition-transform duration-300 ease-out motion-reduce:transition-none"
              >
                <span
                  key={`${wobbleKey}:${cheerKey}`}
                  className={cn(
                    "inline-grid place-items-center",
                    !isFalling &&
                      !isPressed &&
                      ((playfulMotion && wobbleKey > 0) || cheerKey > 0) &&
                      "animate-[wiggle_0.3s_ease-in-out]",
                  )}
                >
                  <Logo className="size-5 will-change-transform" />
                </span>
              </span>
            </span>
            {vpsBind && (
              <span className="bg-gradient-to-r from-primary via-foreground to-primary bg-clip-text text-[15px] font-bold text-transparent transition-all duration-300 group-hover:drop-shadow-[0_0_6px_var(--primary)]">
                爆文库
              </span>
            )}
            {doorCanOpen && (
              <Tooltip delayDuration={300}>
                <TooltipTrigger asChild>
                  <motion.button
                    type="button"
                    aria-label="远程开门"
                    title="远程开门"
                    onClick={handleDoorOpen}
                    disabled={doorOpening}
                    whileTap={playfulMotion ? { scale: 0.85 } : undefined}
                    className={cn(
                      "ml-auto flex size-7 shrink-0 items-center justify-center rounded-full transition-all duration-200",
                      doorOpening
                        ? "bg-emerald-500/20 text-emerald-600"
                        : "bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20 hover:shadow-[0_0_10px_rgba(34,197,94,0.35)]",
                    )}
                  >
                    <LuDoorOpen
                      className={cn(
                        "size-4 transition-transform duration-200",
                        doorOpening && "animate-pulse",
                      )}
                    />
                  </motion.button>
                </TooltipTrigger>
                <TooltipContent side="bottom">远程开门</TooltipContent>
              </Tooltip>
            )}
            {vpsLaunchProgress !== null && (
              <span className="ml-auto flex items-center gap-1.5">
                <span className="relative h-1 w-10 overflow-hidden rounded-full bg-muted">
                  <span
                    className="absolute left-0 top-0 h-full rounded-full bg-primary transition-all duration-200"
                    style={{ width: `${vpsLaunchProgress}%` }}
                  />
                </span>
                <span className="text-[10px] font-medium tabular-nums text-muted-foreground">
                  {Math.floor(vpsLaunchProgress)}%
                </span>
              </span>
            )}
          </button>
        </div>
      ) : (
        <div className="size-7 shrink-0" />
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
