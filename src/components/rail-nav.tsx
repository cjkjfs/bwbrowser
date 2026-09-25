"use client";

import { invoke } from "@tauri-apps/api/core";
import { motion, useReducedMotion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { FiWifi } from "react-icons/fi";
import { GoGear } from "react-icons/go";
import {
  LuChevronRight,
  LuCloud,
  LuCookie,
  LuDownload,
  LuGlobe,
  LuInfo,
  LuNetwork,
  LuPuzzle,
  LuSearch,
  LuTrash2,
  LuUser,
  LuUsers,
  LuX,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { useBwbrowserPermissions } from "@/hooks/use-bwbrowser-permissions";
import { useInputModality } from "@/hooks/use-input-modality";
import { useProxyEvents } from "@/hooks/use-proxy-events";
import { launchBwbrowserClone } from "@/lib/bwbrowser-physics";
import { MOTION_SPRING_POSITION } from "@/lib/motion";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import type { StoredProxy } from "@/types";
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
  | "videoDownload"
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
  videoDownload: "视频下载",
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
  /** Import the local Chrome login data into the currently logged-in account. */
  onImportChromeLogin?: () => void;
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
      className="absolute inset-y-1.5 left-[-7px] w-[2px] rounded-full bg-foreground"
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
  { page: "videoDownload", Icon: LuDownload, labelKey: "rail.videoDownload" },
  { page: "extensions", Icon: LuPuzzle, labelKey: "rail.extensions" },
  { page: "account", Icon: LuCloud, labelKey: "rail.account" },
];

const ADMIN_ONLY_PAGES: AppPage[] = ["profiles", "proxies"];

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
  onImportChromeLogin,
}: RailNavProps) {
  const { t } = useTranslation();
  const { user } = useBwbrowserAuth();
  const { canManageUsers, canDownloadVideos } = useBwbrowserPermissions();
  const isAdmin = !!user?.teamRole && ADMIN_ROLES.includes(user.teamRole);
  const visibleTopItems = TOP_ITEMS.filter((item) => {
    if (ADMIN_ONLY_PAGES.includes(item.page) && !isAdmin) return false;
    if (item.page === "userManagement" && !canManageUsers) return false;
    if (item.page === "videoDownload" && !canDownloadVideos) return false;
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

  // ===== VPS 登录相关 =====
  const { storedProxies } = useProxyEvents();
  const [vpsProxyDialogOpen, setVpsProxyDialogOpen] = useState(false);
  const [vpsContextMenuOpen, setVpsContextMenuOpen] = useState(false);
  const [vpsContextMenuPos, setVpsContextMenuPos] = useState({ x: 0, y: 0 });
  const [cookieSubmenuOpen, setCookieSubmenuOpen] = useState(false);
  const cookieSubmenuRef = useRef<HTMLDivElement>(null);
  const [vpsProxySearch, setVpsProxySearch] = useState("");
  const [vpsSelectedProxyId, setVpsSelectedProxyId] = useState<string | null>(
    null,
  );
  const [vpsSavingProxy, setVpsSavingProxy] = useState(false);
  const vpsMenuTriggerRef = useRef<HTMLButtonElement>(null);
  const [vpsLaunchProgress, setVpsLaunchProgress] = useState<number | null>(
    null,
  );

  const filteredVpsProxies = useMemo(() => {
    const q = vpsProxySearch.trim().toLowerCase();
    if (!q) return storedProxies;
    return storedProxies.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.proxy_settings.host.toLowerCase().includes(q) ||
        p.proxy_settings.proxy_type.toLowerCase().includes(q),
    );
  }, [storedProxies, vpsProxySearch]);

  const handleOpenVpsProxyDialog = useCallback(async () => {
    // 先打开对话框，再异步加载数据
    setVpsProxySearch("");
    setVpsProxyDialogOpen(true);
    try {
      const info = await invoke<{ proxy_id: string | null } | null>(
        "bwbrowser_get_vps_profile_info",
      );
      setVpsSelectedProxyId(info?.proxy_id ?? null);
    } catch {
      setVpsSelectedProxyId(null);
    }
  }, []);

  const handleSaveVpsProxy = useCallback(async () => {
    setVpsSavingProxy(true);
    try {
      await invoke("bwbrowser_set_vps_proxy", {
        proxyId: vpsSelectedProxyId,
      });
      showSuccessToast("代理设置成功");
      setVpsProxyDialogOpen(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      showErrorToast(`设置代理失败: ${msg}`);
    } finally {
      setVpsSavingProxy(false);
    }
  }, [vpsSelectedProxyId]);

  const handleDeleteVpsData = useCallback(() => {
    const ok = window.confirm(
      "确定删除 VPS 登录的本地数据？下次打开将重新创建。",
    );
    if (!ok) return;
    invoke("bwbrowser_delete_vps_data")
      .then(() => {
        showSuccessToast("已删除本地数据");
      })
      .catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : String(err);
        showErrorToast(`删除失败: ${msg}`);
      });
  }, []);

  const handleDeleteCloudCookies = useCallback(() => {
    const ok = window.confirm("确定删除云端保存的爆文库 cookie？");
    if (!ok) return;
    invoke("bwbrowser_update_bwbrowser_cookies", { cookies: "" })
      .then(() => {
        showSuccessToast("云端 cookie 已删除");
      })
      .catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : String(err);
        showErrorToast(`删除失败: ${msg}`);
      });
  }, []);

  const [cookieInfoDialogOpen, setCookieInfoDialogOpen] = useState(false);
  const [cookieInfo, setCookieInfo] = useState<{
    count: number;
    size: number;
    updatedAt: string;
  } | null>(null);
  const [cookieInfoLoading, setCookieInfoLoading] = useState(false);

  const handleViewCloudCookieInfo = useCallback(() => {
    setCookieInfoLoading(true);
    setCookieInfo(null);
    setCookieInfoDialogOpen(true);
    invoke<string | null>("bwbrowser_get_bwbrowser_cookies")
      .then((cookies) => {
        if (
          !cookies ||
          cookies === "null" ||
          cookies === "[]" ||
          cookies.trim().length === 0
        ) {
          setCookieInfo({ count: 0, size: 0, updatedAt: "—" });
          return;
        }
        try {
          const arr = JSON.parse(cookies);
          const count = Array.isArray(arr) ? arr.length : 0;
          setCookieInfo({
            count,
            size: cookies.length,
            updatedAt: new Date().toLocaleString(),
          });
        } catch {
          setCookieInfo({ count: -1, size: cookies.length, updatedAt: "—" });
        }
      })
      .catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : String(err);
        showErrorToast(`获取失败: ${msg}`);
        setCookieInfo({ count: -1, size: 0, updatedAt: "—" });
      })
      .finally(() => {
        setCookieInfoLoading(false);
      });
  }, []);

  const handleVpsLogoLeftClick = useCallback(
    (e: React.MouseEvent) => {
      // 左键：打开 VPS 登录（不打开菜单）
      e.preventDefault();

      // 触发 logo 彩蛋动画
      handleClick();

      let unlistenProgress: (() => void) | undefined;

      setVpsLaunchProgress(0);

      // 监听后端真实启动进度，替代模拟动画
      import("@tauri-apps/api/event")
        .then(({ listen }) =>
          listen<{ pct: number; label: string }>(
            "vps-launch-progress",
            (event) => {
              const pct = Math.min(100, Math.max(0, event.payload.pct));
              setVpsLaunchProgress(pct);
            },
          ),
        )
        .then((fn) => {
          unlistenProgress = fn;
        })
        .catch((e) => {
          console.warn("failed to listen vps launch progress", e);
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
    [handleClick],
  );

  const handleVpsLogoContextMenu = useCallback((e: React.MouseEvent) => {
    // 右键：打开菜单（不触发 VPS 登录）
    e.preventDefault();
    e.stopPropagation();
    setVpsContextMenuPos({ x: e.clientX, y: e.clientY });
    setVpsContextMenuOpen(true);
  }, []);

  // 点击外部关闭右键菜单
  useEffect(() => {
    if (!vpsContextMenuOpen) return;
    const close = () => setVpsContextMenuOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("scroll", close, true);
    };
  }, [vpsContextMenuOpen]);

  // 侧边栏宽度：w-44 = 176px，加上图标和文字
  const RAIL_WIDTH = "w-48";

  return (
    <>
      <nav
        className={cn(
          "relative flex shrink-0 flex-col gap-1 border-r border-border bg-background py-2",
          RAIL_WIDTH,
        )}
      >
        {!isHidden ? (
          <div className="px-3 pb-1">
            <button
              ref={vpsMenuTriggerRef}
              type="button"
              className="flex w-full items-center gap-2 rounded-md px-1 py-1 text-left transition-colors hover:bg-accent/50"
              onClick={handleVpsLogoLeftClick}
              onContextMenu={handleVpsLogoContextMenu}
              title="左键打开爆文库，右键设置代理"
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
              <span className="text-sm font-semibold text-foreground">
                爆文浏览器
              </span>
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
          {visibleTopItems.map(({ page, Icon, labelKey }) => {
            const active = currentPage === page;
            return (
              <Tooltip key={page} delayDuration={300}>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    onClick={() => {
                      onNavigate(page);
                    }}
                    aria-label={t(labelKey)}
                    aria-current={active ? "page" : undefined}
                    className={cn(
                      "relative flex w-full items-center gap-3 rounded-md px-2 py-2 transition-colors duration-100",
                      active
                        ? "bg-accent text-accent-foreground"
                        : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                    )}
                  >
                    {active && <ActiveIndicator />}
                    <Icon className="size-4 shrink-0" />
                    <span className="truncate text-sm font-medium">
                      {RAIL_LABELS_ZH[page]}
                    </span>
                    {page === "cookieBot" && cookieBotRunning && (
                      <span
                        aria-hidden="true"
                        className="ml-auto size-2 shrink-0 rounded-full bg-success"
                      />
                    )}
                  </button>
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
                  "relative flex w-full items-center gap-3 rounded-md px-2 py-2 transition-colors duration-100",
                  currentPage === "settings"
                    ? "bg-accent text-accent-foreground"
                    : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                )}
              >
                {currentPage === "settings" && <ActiveIndicator />}
                <GoGear className="size-4 shrink-0" />
                <span className="truncate text-sm font-medium">系统设置</span>
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">{t("rail.settings")}</TooltipContent>
          </Tooltip>
        </div>
      </nav>

      {/* VPS 右键菜单 - Portal 到 body，避免父级 transform 影响 fixed 定位 */}
      {vpsContextMenuOpen &&
        typeof document !== "undefined" &&
        createPortal(
          <div
            role="menu"
            className="fixed z-[10000] w-44 rounded-md border border-border bg-popover p-1 shadow-lg"
            style={{ left: vpsContextMenuPos.x, top: vpsContextMenuPos.y }}
            onContextMenu={(e) => e.preventDefault()}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <button
              type="button"
              className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-popover-foreground transition-colors hover:bg-accent focus:bg-accent focus:outline-none"
              onClick={() => {
                setVpsContextMenuOpen(false);
                onImportChromeLogin?.();
              }}
            >
              <LuDownload className="h-3.5 w-3.5 text-muted-foreground" />
              导入登录数据
            </button>
            <div className="my-1 h-px bg-border" />
            <button
              type="button"
              className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-popover-foreground transition-colors hover:bg-accent focus:bg-accent focus:outline-none"
              onClick={() => {
                setVpsContextMenuOpen(false);
                handleOpenVpsProxyDialog();
              }}
            >
              <LuNetwork className="h-3.5 w-3.5 text-muted-foreground" />
              设置代理
            </button>
            <div className="my-1 h-px bg-border" />
            <div
              role="group"
              className="relative"
              ref={cookieSubmenuRef}
              onMouseEnter={() => setCookieSubmenuOpen(true)}
              onMouseLeave={() => setCookieSubmenuOpen(false)}
            >
              <button
                type="button"
                className="flex w-full items-center justify-between gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-popover-foreground transition-colors hover:bg-accent focus:bg-accent focus:outline-none"
              >
                <span className="flex items-center gap-2">
                  <LuCookie className="h-3.5 w-3.5 text-muted-foreground" />
                  cookie 相关
                </span>
                <LuChevronRight className="h-3 w-3 text-muted-foreground" />
              </button>
              {cookieSubmenuOpen && (
                <div
                  role="menu"
                  className="absolute left-full top-0 ml-1 w-48 rounded-md border border-border bg-popover p-1 shadow-lg"
                >
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-popover-foreground transition-colors hover:bg-accent focus:bg-accent focus:outline-none"
                    onClick={() => {
                      setVpsContextMenuOpen(false);
                      setCookieSubmenuOpen(false);
                      handleViewCloudCookieInfo();
                    }}
                  >
                    <LuInfo className="h-3.5 w-3.5 text-muted-foreground" />
                    查看云端 cookie 信息
                  </button>
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-popover-foreground transition-colors hover:bg-accent focus:bg-accent focus:outline-none"
                    onClick={() => {
                      setVpsContextMenuOpen(false);
                      setCookieSubmenuOpen(false);
                      handleDeleteCloudCookies();
                    }}
                  >
                    <LuCloud className="h-3.5 w-3.5 text-muted-foreground" />
                    删除云端 cookie
                  </button>
                  <button
                    type="button"
                    className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-destructive transition-colors hover:bg-accent focus:bg-accent focus:outline-none"
                    onClick={() => {
                      setVpsContextMenuOpen(false);
                      setCookieSubmenuOpen(false);
                      handleDeleteVpsData();
                    }}
                  >
                    <LuTrash2 className="h-3.5 w-3.5" />
                    删除本地 cookie 和数据
                  </button>
                </div>
              )}
            </div>
          </div>,
          document.body,
        )}

      {/* 云端 cookie 信息对话框 */}
      <Dialog
        open={cookieInfoDialogOpen}
        onOpenChange={setCookieInfoDialogOpen}
      >
        <DialogContent className="w-[420px] max-w-[90vw]">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <LuCookie className="h-4 w-4 text-primary" />
              云端 cookie 信息
            </DialogTitle>
          </DialogHeader>
          <div className="py-2">
            {cookieInfoLoading ? (
              <div className="flex items-center justify-center py-8 text-sm text-muted-foreground">
                <span className="animate-pulse">加载中...</span>
              </div>
            ) : cookieInfo ? (
              <div className="space-y-3">
                <div className="flex items-center justify-between rounded-md bg-muted/50 px-3 py-2">
                  <span className="text-sm text-muted-foreground">
                    cookie 条数
                  </span>
                  <span className="text-sm font-medium">
                    {cookieInfo.count < 0
                      ? "解析失败"
                      : cookieInfo.count === 0
                        ? "无"
                        : `${cookieInfo.count} 条`}
                  </span>
                </div>
                <div className="flex items-center justify-between rounded-md bg-muted/50 px-3 py-2">
                  <span className="text-sm text-muted-foreground">
                    数据大小
                  </span>
                  <span className="text-sm font-medium">
                    {cookieInfo.size < 1024
                      ? `${cookieInfo.size} B`
                      : `${(cookieInfo.size / 1024).toFixed(1)} KB`}
                  </span>
                </div>
                <div className="flex items-center justify-between rounded-md bg-muted/50 px-3 py-2">
                  <span className="text-sm text-muted-foreground">
                    查询时间
                  </span>
                  <span className="text-sm font-medium">
                    {cookieInfo.updatedAt}
                  </span>
                </div>
                {cookieInfo.count === 0 && (
                  <p className="text-xs text-muted-foreground pt-2">
                    云端暂无保存的 cookie。打开爆文库后会自动同步。
                  </p>
                )}
              </div>
            ) : null}
          </div>
        </DialogContent>
      </Dialog>

      {/* VPS 代理设置对话框 */}
      <Dialog open={vpsProxyDialogOpen} onOpenChange={setVpsProxyDialogOpen}>
        <DialogContent className="w-[600px] max-w-[90vw] p-0 overflow-hidden">
          <div className="flex items-center justify-between border-b px-4 py-3">
            <div className="flex items-center gap-2">
              <LuNetwork className="h-4 w-4 text-primary" />
              <span className="text-sm font-semibold">VPS 登录代理设置</span>
            </div>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 w-7 p-0"
              onClick={() => setVpsProxyDialogOpen(false)}
            >
              <LuX className="h-4 w-4" />
            </Button>
          </div>

          <div className="flex items-center gap-2 border-b px-4 py-2">
            <div className="relative flex-1">
              <LuSearch className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={vpsProxySearch}
                onChange={(e) => setVpsProxySearch(e.target.value)}
                placeholder="搜索代理名称、IP、类型..."
                className="h-8 pl-7 text-xs"
              />
            </div>
            <span className="shrink-0 text-xs text-muted-foreground">
              共 {filteredVpsProxies.length} 个
            </span>
          </div>

          <div className="max-h-[360px] overflow-y-auto">
            {/* 无代理选项 */}
            <button
              type="button"
              className={`flex w-full items-center gap-3 border-b px-4 py-2 text-left transition-colors hover:bg-accent/50 ${
                vpsSelectedProxyId === null ? "bg-primary/10" : ""
              }`}
              onClick={() => setVpsSelectedProxyId(null)}
            >
              <div className="w-4">
                {vpsSelectedProxyId === null && (
                  <div className="size-3 rounded-full bg-primary" />
                )}
              </div>
              <div className="flex-1 min-w-0">
                <div className="text-xs font-medium text-foreground">
                  不使用代理（直连）
                </div>
                <div className="text-xs text-muted-foreground">
                  使用本机网络直接访问
                </div>
              </div>
            </button>

            {filteredVpsProxies.length === 0 ? (
              <div className="p-8 text-center text-sm text-muted-foreground">
                暂无可用代理，请先在代理中心添加
              </div>
            ) : (
              filteredVpsProxies.map((p: StoredProxy) => (
                <button
                  key={p.id}
                  type="button"
                  className={`flex w-full items-center gap-3 border-b px-4 py-2 text-left transition-colors hover:bg-accent/50 ${
                    vpsSelectedProxyId === p.id ? "bg-primary/10" : ""
                  }`}
                  onClick={() => setVpsSelectedProxyId(p.id)}
                >
                  <div className="w-4">
                    {vpsSelectedProxyId === p.id && (
                      <div className="size-3 rounded-full bg-primary" />
                    )}
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="truncate text-xs font-medium text-foreground">
                      {p.name}
                    </div>
                    <div className="truncate text-xs text-muted-foreground">
                      {p.proxy_settings.host}:{p.proxy_settings.port}
                    </div>
                  </div>
                  <div className="shrink-0 text-[10px] uppercase text-muted-foreground">
                    {p.proxy_settings.proxy_type}
                  </div>
                </button>
              ))
            )}
          </div>

          <div className="flex items-center justify-end gap-2 border-t px-4 py-3">
            <Button
              variant="outline"
              size="sm"
              className="h-8 text-xs"
              onClick={() => setVpsProxyDialogOpen(false)}
            >
              取消
            </Button>
            <Button
              size="sm"
              className="h-8 text-xs"
              onClick={() => void handleSaveVpsProxy()}
              disabled={vpsSavingProxy}
            >
              {vpsSavingProxy ? "保存中..." : "保存"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
