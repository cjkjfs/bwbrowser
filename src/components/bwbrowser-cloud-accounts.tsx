"use client";

import {
  type ColumnDef,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type SortingState,
  useReactTable,
  type VisibilityState,
} from "@tanstack/react-table";
import { invoke } from "@tauri-apps/api/core";
import { confirm as dialogConfirm } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { FiCheck, FiX } from "react-icons/fi";
import {
  LuArrowDown,
  LuArrowUp,
  LuCheck,
  LuChevronDown,
  LuChevronLeft,
  LuChevronRight,
  LuChevronsUpDown,
  LuClock,
  LuCopy,
  LuEye,
  LuEyeOff,
  LuGlobe,
  LuHeartPulse,
  LuLoaderCircle,
  LuMonitor,
  LuNetwork,
  LuPencil,
  LuPlay,
  LuPlus,
  LuRefreshCw,
  LuSearch,
  LuSettings2,
  LuStickyNote,
  LuTrash2,
  LuUnplug,
  LuUserCheck,
  LuUserPlus,
  LuUsers,
  LuWrench,
  LuX,
} from "react-icons/lu";
import { EnvFormDialog } from "@/components/bwbrowser-env-management";
import { ProxyFormDialog } from "@/components/proxy-form-dialog";
import { AnimatedSwitch } from "@/components/ui/animated-switch";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import {
  type BwbrowserAccount,
  type CloudUser,
  useBwbrowserAccounts,
} from "@/hooks/use-bwbrowser-accounts";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { useBwbrowserCompany } from "@/hooks/use-bwbrowser-company";
import type { BwbrowserEnvironment } from "@/hooks/use-bwbrowser-environments";
import { useBwbrowserPermissions } from "@/hooks/use-bwbrowser-permissions";
import { useProxyEvents } from "@/hooks/use-proxy-events";
import { translateBackendError } from "@/lib/backend-errors";
import { CLOUD_ROOT } from "@/lib/cloud-config";
import {
  dismissToast,
  showErrorToast,
  showLaunchProgressToast,
  showLaunchSuccessToast,
  showSuccessToast,
  updateLaunchProgressToast,
} from "@/lib/toast-utils";
import { extractSecretFromUrl, generateTOTP } from "@/lib/totp";
import { cn } from "@/lib/utils";
import type { ProxyCheckResult, StoredProxy } from "@/types";

// ==================== 类型定义 ====================

interface BwbrowserUpdateProxyResult {
  success?: boolean;
  geo_pending?: boolean;
  timezone?: string;
  language?: string;
  latitude?: number | null;
  longitude?: number | null;
}

interface BwbrowserCloudAccountsDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** 嵌入式模式：不显示 Dialog 外壳，直接渲染内容（用于嵌入主页面） */
  embedded?: boolean;
  /** 点击环境列"已绑定"时跳转到环境管理页面编辑该环境 */
  onNavigateToEnvManagement?: (envUuid?: string) => void;
}

// ==================== 常量 ====================

// 云端 Avatar 字段可能是完整 URL、data: 或相对路径，统一转为可直接渲染的地址
function resolveAvatarUrl(raw?: string): string | undefined {
  if (!raw) return undefined;
  const v = raw.trim();
  if (!v) return undefined;
  if (/^(https?:|data:|blob:)/i.test(v)) return v;
  if (v.startsWith("/")) return `${CLOUD_ROOT}${v}`;
  return v;
}

const PLATFORM_LABELS: Record<string, string> = {
  douyin: "抖音",
  tiktok: "TikTok",
  youtube: "YouTube",
  kuaishou: "快手",
  xiaohongshu: "小红书",
  bilibili: "B站",
  weibo: "微博",
  wechat_video: "视频号",
  xhs: "小红书",
  dy: "抖音",
  wb: "微博",
  netease: "网易",
  outlook: "Outlook",
  toutiao: "头条",
  baijiahao: "百度号",
  wechat_mp: "微信公众号",
  iqiyi: "爱奇艺",
  pdd: "拼多多",
  sohu_video: "搜狐视频",
  xigua: "西瓜视频",
  dian: "大众点评",
  jingdong: "京东",
  taobao: "淘宝",
  tengxun_video: "腾讯视频",
  facebook: "Facebook",
  instagram: "Instagram",
  twitter: "Twitter",
  douyin_hao: "抖音号",
  baidu: "百度",
  zhihu: "知乎",
  mail_163: "网易邮箱",
  qq: "QQ",
  dingtalk: "钉钉",
  csdn: "CSDN",
  juejin: "掘金",
  youku: "优酷",
  mango_tv: "芒果TV",
  meituan: "美团",
  eleme: "饿了么",
};

const PLATFORM_COLORS: Record<string, string> = {
  douyin: "bg-rose-500/10 text-rose-600 dark:text-rose-400 border-rose-500/20",
  tiktok:
    "bg-zinc-900/10 text-zinc-900 dark:bg-zinc-500/10 dark:text-zinc-200 border-zinc-500/20",
  youtube: "bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20",
  kuaishou:
    "bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20",
  xiaohongshu:
    "bg-pink-500/10 text-pink-600 dark:text-pink-400 border-pink-500/20",
  bilibili: "bg-sky-500/10 text-sky-600 dark:text-sky-400 border-sky-500/20",
  weibo:
    "bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/20",
  wechat_video:
    "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/20",
};

const _PAGE_SIZE = 20;

// 云端代理列表项（对应 Rust 端 BwbrowserProxyItem）
interface CloudProxyItem {
  id: string; // "bwbrowser_{proxy_id}"
  name: string;
  proxy_type: string;
  host: string;
  port: number;
  username: string | null;
  password: string | null;
  vless_uri: string | null;
  is_cloud_managed: boolean;
  country?: string | null;
  city?: string | null;
}

// 节点使用明细（对应 Rust 端 BwbrowserProxyUsageItem；每账号一条，客户端按平台聚合计数）
interface CloudProxyUsageItem {
  proxy_node: string;
  platform: string;
  account_name: string;
  phone_id: string;
  owner_name: string;
  owner_real_name: string;
}

// 平台显示名：直接用库里完整平台名，空值兜底
function platformShort(p: string): string {
  return p || "其他";
}

// 账号表 proxy_node 有新旧两种格式（type:host:port[:user:pass] 与旧 host:port），
// 统一提取 host:port 作为匹配 key；vless/trojan/ss 按 URI 全串匹配
function proxyNodeKey(pn: string): string {
  const s = (pn || "").trim();
  if (!s) return "";
  if (/^(vless|trojan|ss):\/\//i.test(s)) return s;
  const parts = s.split(":");
  const hostIdx = ["http", "socks5"].includes(parts[0].toLowerCase()) ? 1 : 0;
  return `${parts[hostIdx] ?? ""}:${parts[hostIdx + 1] ?? ""}`;
}

// 服务端 platform_config 拉取的全量 code->中文名（如 hongguo -> 红果短剧）。
// 拉取成功前 fallback 到本地硬编码表，因此拼音 code 只会在服务端数据到达前短暂出现。
let remotePlatformLabels: Record<string, string> = {};

// ==================== 设备 IP 映射表（SRC-IP-CIDR 提示） ====================
// 用户粘贴的 Clash 风格分流清单（"SRC-IP-CIDR,192.168.168.5/32,us031-70.39.192.115"），
// 仅存 localStorage 供设置代理弹窗展示提示，不进后端、不写任何账号数据。
const SRC_IP_RULES_KEY = "bwbrowser_src_ip_rules";

function parseSrcIpRules(text: string): Map<number, string> {
  const map = new Map<number, string>();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim().replace(/^[-*\s]+/, "");
    if (!line || line.startsWith("#")) continue;
    if (!line.startsWith("SRC-IP-CIDR,")) continue;
    const parts = line.split(",");
    const cidr = (parts[1] ?? "").trim();
    const node = (parts[2] ?? "").trim();
    if (!cidr || !node) continue;
    // key 取 IP 末段数字（设备编号对应末段，任意网段均可），value 保持 "IP/掩码,节点" 原格式用于展示
    const slash = cidr.indexOf("/");
    const ip = slash >= 0 ? cidr.slice(0, slash) : cidr;
    const segs = ip.split(".");
    const lastSeg = parseInt(segs[segs.length - 1] ?? "", 10);
    if (!Number.isFinite(lastSeg) || lastSeg <= 0 || lastSeg > 255) continue;
    if (!map.has(lastSeg)) map.set(lastSeg, `${cidr},${node}`);
  }
  return map;
}

function loadSrcIpRulesText(): string {
  try {
    return localStorage.getItem(SRC_IP_RULES_KEY) ?? "";
  } catch {
    return "";
  }
}

// 设备编号 = device_id 的数字部分（"005" -> 5），对应清单里 IP 末段为 n 的一行（任意网段）
function computeSrcIpRuleHint(
  account: { device_id?: string } | null,
): string | null {
  if (!account) return null;
  const n = parseInt((account.device_id ?? "").trim(), 10);
  if (!Number.isFinite(n) || n <= 0 || n > 255) return null;
  return parseSrcIpRules(loadSrcIpRulesText()).get(n) ?? null;
}

// ==================== 工具函数 ====================
function getPlatformLabel(platform: string): string {
  const remote = remotePlatformLabels[platform];
  if (remote !== undefined) return remote;
  return PLATFORM_LABELS[platform] || platform;
}

// 平台按显示名的拼音排序（Intl 中文排序即拼音序，WebView 原生支持）
const zhCollator = new Intl.Collator("zh-Hans-CN");

function _formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function formatDateShort(dateStr: string): string {
  if (!dateStr) return "-";
  // 尝试解析 ISO 格式或其他常见格式
  const d = new Date(dateStr);
  if (isNaN(d.getTime())) return dateStr.slice(0, 10);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${y}-${m}-${day} ${hh}:${mm}`;
}

/** 不带年份的短格式（表格列内省宽度），完整年份用 title 悬停展示 */
function formatDateNoYear(dateStr: string): string {
  if (!dateStr) return "-";
  const d = new Date(dateStr);
  if (isNaN(d.getTime())) return dateStr.slice(0, 10);
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${m}-${day} ${hh}:${mm}`;
}

function getPlatformColor(platform: string): string {
  return (
    PLATFORM_COLORS[platform] || "bg-muted text-muted-foreground border-border"
  );
}

function _showDevToast() {
  showErrorToast("功能开发中", {
    description: "该功能即将上线，敬请期待",
  });
}

// ==================== 平台徽章 ====================

function PlatformBadge({ platform }: { platform: string }) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded border px-1.5 py-0.5 text-[11px] font-medium",
        getPlatformColor(platform),
      )}
    >
      {getPlatformLabel(platform)}
    </span>
  );
}

// ==================== 状态徽章 ====================

function _StatusBadge({ status }: { status: number | undefined }) {
  if (status === 1) {
    return (
      <span className="inline-flex items-center gap-1 rounded-full bg-emerald-500/10 px-2 py-0.5 text-[11px] font-medium text-emerald-500">
        <span className="h-1.5 w-1.5 rounded-full bg-emerald-500 animate-pulse" />
        正常
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[11px] font-medium text-muted-foreground">
      <span className="h-1.5 w-1.5 rounded-full bg-muted-foreground" />
      停用
    </span>
  );
}

// ==================== 分页组件 ====================

function _PaginationBar({
  currentPage,
  totalPages,
  onPageChange,
}: {
  currentPage: number;
  totalPages: number;
  onPageChange: (page: number) => void;
}) {
  const getPageNumbers = () => {
    const pages: (number | "ellipsis")[] = [];
    const maxVisible = 7;

    if (totalPages <= maxVisible) {
      for (let i = 1; i <= totalPages; i++) {
        pages.push(i);
      }
    } else if (currentPage <= 3) {
      for (let i = 1; i <= 4; i++) pages.push(i);
      pages.push("ellipsis");
      pages.push(totalPages);
    } else if (currentPage >= totalPages - 2) {
      pages.push(1);
      pages.push("ellipsis");
      for (let i = totalPages - 3; i <= totalPages; i++) pages.push(i);
    } else {
      pages.push(1);
      pages.push("ellipsis");
      for (let i = currentPage - 1; i <= currentPage + 1; i++) pages.push(i);
      pages.push("ellipsis");
      pages.push(totalPages);
    }

    return pages;
  };

  return (
    <div className="flex items-center justify-between border-t border-border bg-background/10 px-4 py-2 backdrop-blur-2xl">
      <div className="whitespace-nowrap text-xs text-muted-foreground">
        第 {currentPage} / {totalPages} 页
      </div>
      {totalPages > 1 && (
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => currentPage > 1 && onPageChange(currentPage - 1)}
            disabled={currentPage === 1}
            className={cn(
              "flex h-7 w-7 items-center justify-center rounded-md border border-border text-muted-foreground transition-colors",
              currentPage === 1
                ? "cursor-not-allowed opacity-50"
                : "hover:bg-accent hover:text-foreground",
            )}
            aria-label="上一页"
          >
            <LuChevronLeft className="h-4 w-4" />
          </button>
          {getPageNumbers().map((page, index) =>
            page === "ellipsis" ? (
              <span
                key={`ellipsis-${index}`}
                className="flex h-7 w-7 items-center justify-center text-xs text-muted-foreground"
              >
                ...
              </span>
            ) : (
              <button
                type="button"
                key={page}
                onClick={() => onPageChange(page)}
                className={cn(
                  "flex h-7 min-w-7 items-center justify-center rounded-md border text-xs font-medium transition-colors",
                  page === currentPage
                    ? "border-primary bg-primary text-primary-foreground"
                    : "border-border text-muted-foreground hover:bg-accent hover:text-foreground",
                )}
              >
                {page}
              </button>
            ),
          )}
          <button
            type="button"
            onClick={() =>
              currentPage < totalPages && onPageChange(currentPage + 1)
            }
            disabled={currentPage === totalPages}
            className={cn(
              "flex h-7 w-7 items-center justify-center rounded-md border border-border text-muted-foreground transition-colors",
              currentPage === totalPages
                ? "cursor-not-allowed opacity-50"
                : "hover:bg-accent hover:text-foreground",
            )}
            aria-label="下一页"
          >
            <LuChevronRight className="h-4 w-4" />
          </button>
        </div>
      )}
    </div>
  );
}

// ==================== 主组件 ====================

export function BwbrowserCloudAccountsDialog({
  isOpen,
  onClose,
  embedded = false,
  onNavigateToEnvManagement,
}: BwbrowserCloudAccountsDialogProps) {
  const { t } = useTranslation();
  const [sorting, setSorting] = useState<SortingState>([]);
  // 列显隐：平时少看的列（节点IP/环境/时区/Cookie更新）默认折叠，表头「列设置」可勾选
  // 显示；用户的选择存 localStorage，下次打开保持。
  const [columnVisibility, setColumnVisibility] = useState<VisibilityState>(
    (() => {
      const defaults: VisibilityState = {
        proxy_node: false,
        env_uuid: false,
        timezone: false,
        cookie_updated_at: false,
      };
      try {
        const raw = localStorage.getItem("bwbrowser_cloud_columns");
        if (!raw) return defaults;
        const saved = JSON.parse(raw) as VisibilityState;
        return saved && typeof saved === "object"
          ? { ...defaults, ...saved }
          : defaults;
      } catch {
        return defaults;
      }
    })(),
  );
  const handleColumnVisibilityChange = useCallback(
    (
      updater: VisibilityState | ((old: VisibilityState) => VisibilityState),
    ) => {
      setColumnVisibility((prev) => {
        const next = typeof updater === "function" ? updater(prev) : updater;
        try {
          localStorage.setItem("bwbrowser_cloud_columns", JSON.stringify(next));
        } catch {
          // 存储不可用时仅本次会话生效
        }
        return next;
      });
    },
    [],
  );
  // 文字大小三档（小/中/大）：作用域 class 挂在弹窗根节点上，由 globals.css
  // 按档位覆盖表格里所有字号工具类；选择存 localStorage，下次打开保持。
  const [cloudFontSize, setCloudFontSize] = useState<
    "small" | "medium" | "large"
  >(
    (() => {
      try {
        const raw = localStorage.getItem("bwbrowser_cloud_font_size");
        return raw === "small" || raw === "large" ? raw : "medium";
      } catch {
        return "medium";
      }
    })(),
  );
  const handleFontSizeChange = useCallback(
    (size: "small" | "medium" | "large") => {
      setCloudFontSize(size);
      try {
        localStorage.setItem("bwbrowser_cloud_font_size", size);
      } catch {
        // 存储不可用时仅本次会话生效
      }
    },
    [],
  );
  // tanstack rowSelection 是 { rowId: true } 对象，不是 Set —— 用对象才能
  // 与 onRowSelectionChange 正确互搏（之前 Set 强转导致勾选永远无效）。
  const [selectedIds, setSelectedIds] = useState<Record<string, boolean>>({});
  const [activeTab, setActiveTab] = useState("accounts");

  // Cookie 健康检测：按账号 id 存最近一次判定结果
  const [healthStates, setHealthStates] = useState<
    Record<number, { status: string; message: string; checked_at: number }>
  >({});
  const [healthLoadingIds, setHealthLoadingIds] = useState<Set<number>>(
    new Set(),
  );
  const [sectorFilter, setSectorFilter] = useState<string | null>(null);
  const [autofillEnabled, setAutofillEnabled] = useState(true);

  useEffect(() => {
    void invoke<{ autofill_enabled: boolean }>("get_app_settings")
      .then((s) => setAutofillEnabled(s.autofill_enabled))
      .catch(() => {});
  }, []);

  // Persist the 自动输入 switch; revert the UI if the backend refuses.
  const handleAutofillToggle = useCallback(
    async (checked: boolean) => {
      setAutofillEnabled(checked);
      try {
        await invoke<boolean>("set_login_autofill_enabled", {
          enabled: checked,
        });
      } catch (err) {
        setAutofillEnabled((prev) => !prev);
        showErrorToast(`设置保存失败: ${translateBackendError(t, err)}`);
      }
    },
    [t],
  );

  const { isLoggedIn: isBwbrowserLoggedIn } = useBwbrowserAuth();
  const { isSuperAdmin: isSuperAdminPerm, permissionsReady } =
    useBwbrowserPermissions();
  const { companies, selectedCompanyId, setSelectedCompanyId } =
    useBwbrowserCompany(
      isSuperAdminPerm,
      isBwbrowserLoggedIn,
      permissionsReady,
    );

  const {
    accounts,
    total,
    totalPages,
    isLoading,
    error,
    summary,
    users,
    usersLoading,
    refresh,
    refreshSummary,
    fetchPage,
    setPlatformFilter,
    setOwnerFilter,
    setKeyword,
    currentPage,
    pageSize,
    setPageSize,
    platformFilter,
    ownerFilter,
    keyword,
    serverSort,
    setServerSort,
    isManager,
    isSuperAdmin,
    canViewPassword,
    canView2FA,
    canViewSMS,
  } = useBwbrowserAccounts(selectedCompanyId);

  // 从服务端 platform_config 拉全量平台中文名，用来替代散落的拼音 code 显示。
  const [, setPlatformNamesVersion] = useState(0);
  useEffect(() => {
    void invoke<Record<string, string>>("bwbrowser_platform_names")
      .then((names) => {
        remotePlatformLabels = names;
        setPlatformNamesVersion((v) => v + 1);
      })
      .catch(() => {});
  }, []);

  // 权限判断：根据 users.php 的 allow_2fa 和 allow_sms_management 字段
  const canView2FACode = canView2FA || isManager || isSuperAdmin;
  const canViewSMSCode = canViewSMS || isManager || isSuperAdmin;
  console.log("[CloudAccounts] 权限:", {
    canView2FA,
    canViewSMS,
    isManager,
    isSuperAdmin,
    canView2FACode,
    canViewSMSCode,
    canViewPassword,
  });

  // 按账号加载并展示 Cookie 健康状态（绿=通过，黄=未通过，灰=未检测）
  const loadHealthStates = useCallback(async () => {
    try {
      const states = await invoke<
        Array<{
          account_id: number;
          status: string;
          message: string;
          checked_at: number;
        }>
      >("bwbrowser_get_account_health_states");
      const next: Record<
        number,
        { status: string; message: string; checked_at: number }
      > = {};
      for (const s of states) {
        next[s.account_id] = {
          status: s.status,
          message: s.message,
          checked_at: s.checked_at,
        };
      }
      setHealthStates(next);
    } catch (err) {
      const msg = translateBackendError(t, err);
      if (msg && !msg.includes("PERMISSION")) {
        showErrorToast(`获取健康状态失败: ${msg}`);
      }
    }
  }, [t]);

  useEffect(() => {
    if (isBwbrowserLoggedIn) void loadHealthStates();
  }, [isBwbrowserLoggedIn, loadHealthStates]);

  const handleCheckHealth = useCallback(
    async (account: BwbrowserAccount) => {
      setHealthLoadingIds((prev) => {
        const next = new Set(prev);
        next.add(account.id);
        return next;
      });
      try {
        const result = await invoke<{
          account_id: number;
          status: string;
          message: string;
          checked_at: number;
        }>("bwbrowser_check_account_cookie_health", {
          accountId: account.id,
          accountName: account.account_name,
        });
        setHealthStates((prev) => ({
          ...prev,
          [result.account_id]: {
            status: result.status,
            message: result.message,
            checked_at: result.checked_at,
          },
        }));
        if (result.status === "valid") {
          showSuccessToast(
            result.message || `账号 ${account.account_name} Cookie 健康`,
          );
        } else {
          showErrorToast(
            result.message || `账号 ${account.account_name} Cookie 需重新登录`,
          );
        }
      } catch (err) {
        showErrorToast(`健康检测失败: ${translateBackendError(t, err)}`);
      } finally {
        setHealthLoadingIds((prev) => {
          const next = new Set(prev);
          next.delete(account.id);
          return next;
        });
      }
    },
    [t],
  );

  const [isRefreshing, setIsRefreshing] = useState(false);
  // 添加账号弹窗内的可搜索下拉开关
  const [addPlatformOpen, setAddPlatformOpen] = useState(false);
  const [addOwnerOpen, setAddOwnerOpen] = useState(false);
  const [showPasswordIds, setShowPasswordIds] = useState<Set<number>>(
    new Set(),
  );
  const [showEditPassword, setShowEditPassword] = useState(false);
  const [copiedPasswordId, setCopiedPasswordId] = useState<number | null>(null);
  const [proxyDialogOpen, setProxyDialogOpen] = useState(false);
  const [proxyDialogLoading, setProxyDialogLoading] = useState(false);
  // 代理弹窗的目标账号：单个（行内「设置代理」）或多选（批量操作栏）
  const [proxyDialogTargets, setProxyDialogTargets] = useState<
    BwbrowserAccount[]
  >([]);
  // 批量设置归属人弹窗
  const [ownerDialogOpen, setOwnerDialogOpen] = useState(false);
  const [ownerDialogTargets, setOwnerDialogTargets] = useState<
    BwbrowserAccount[]
  >([]);
  const [isSettingOwner, setIsSettingOwner] = useState(false);
  // 批量添加备注弹窗
  const [remarkDialogOpen, setRemarkDialogOpen] = useState(false);
  const [remarkDialogTargets, setRemarkDialogTargets] = useState<
    BwbrowserAccount[]
  >([]);
  const [remarkText, setRemarkText] = useState("");
  const [remarkAppend, setRemarkAppend] = useState(true);
  const [isSavingRemark, setIsSavingRemark] = useState(false);
  // 设备 IP 映射提示：单账号设置代理时显示 device_id 对应的 SRC-IP-CIDR 行
  const [srcIpRuleHint, setSrcIpRuleHint] = useState<string | null>(null);
  const [srcIpRulesOpen, setSrcIpRulesOpen] = useState(false);
  const [srcIpRulesText, setSrcIpRulesText] = useState("");
  const [selectedProxyId, setSelectedProxyId] = useState<string | null>(null);
  const [isSavingProxy, setIsSavingProxy] = useState(false);
  const [proxySearchQuery, setProxySearchQuery] = useState("");
  const [proxyPage, setProxyPage] = useState(0);
  const [cloudProxyOptions, setCloudProxyOptions] = useState<
    (StoredProxy & { country?: string | null; city?: string | null })[]
  >([]);
  // 节点使用明细：key = proxyNodeKey(账号 proxy_node)，value = 该节点下每账号一条明细
  const [proxyUsage, setProxyUsage] = useState<
    Map<string, CloudProxyUsageItem[]>
  >(new Map());
  // 代理健康测试结果缓存
  const [proxyCheckResults, setProxyCheckResults] = useState<
    Map<
      string,
      {
        checking: boolean;
        result: ProxyCheckResult | null;
        error: string | null;
      }
    >
  >(new Map());
  // 批量测试并发控制
  const BATCH_TEST_CONCURRENCY = 5;
  // 创建代理对话框
  const [createProxyDialogOpen, setCreateProxyDialogOpen] = useState(false);
  const PROXY_PAGE_SIZE = 10;

  // VPS 登录代理设置
  const [vpsProxyDialogOpen, setVpsProxyDialogOpen] = useState(false);
  const [vpsContextMenuOpen, setVpsContextMenuOpen] = useState(false);
  const [vpsContextMenuPos, setVpsContextMenuPos] = useState({ x: 0, y: 0 });
  const [vpsProxySearch, setVpsProxySearch] = useState("");
  const [vpsSelectedProxyId, setVpsSelectedProxyId] = useState<string | null>(
    null,
  );
  const [vpsSavingProxy, setVpsSavingProxy] = useState(false);

  // 环境编辑弹窗
  const [envDialogOpen, setEnvDialogOpen] = useState(false);
  const [envDialogAccount, setEnvDialogAccount] =
    useState<BwbrowserAccount | null>(null);
  const [cloudEnvs, setCloudEnvs] = useState<
    { env_uuid: string; name: string; browser_type: string }[]
  >([]);
  const [selectedEnvUuid, setSelectedEnvUuid] = useState<string | null>(null);
  const [envLoading, setEnvLoading] = useState(false);

  // 环境列下拉：直接编辑/删除该账号已绑定的环境
  const [boundEnvFormOpen, setBoundEnvFormOpen] = useState(false);
  const [boundEnvTarget, setBoundEnvTarget] =
    useState<BwbrowserEnvironment | null>(null);

  // 2FA/短信编辑弹窗
  const [codeEditOpen, setCodeEditOpen] = useState(false);
  const [codeEditAccount, setCodeEditAccount] =
    useState<BwbrowserAccount | null>(null);
  const [editSafeLink, setEditSafeLink] = useState("");
  const [editBindPhone, setEditBindPhone] = useState("");
  const [codeSaving, setCodeSaving] = useState(false);

  // 编辑账号弹窗
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [editAccount, setEditAccount] = useState<BwbrowserAccount | null>(null);
  const [editDetail, setEditDetail] = useState<BwbrowserAccount | null>(null);
  const [editLoading, setEditLoading] = useState(false);
  const [editSaving, setEditSaving] = useState(false);
  const [editForm, setEditForm] = useState({
    account_name: "",
    login_account: "",
    login_password: "",
    platform: "",
    remark: "",
    tags: [] as string[],
    tagInput: "",
    category: "",
    pure_username: "",
    device_id: "",
    bind_phone: "",
    safe_link: "",
    backup_email: "",
    channel_id: "",
    owner_id: 0 as number,
  });
  const [editPerms, setEditPerms] = useState<{
    canViewPassword: boolean;
    canView2FA: boolean;
    canViewSMS: boolean;
    canEditAll: boolean;
  }>({
    canViewPassword: false,
    canView2FA: false,
    canViewSMS: false,
    canEditAll: false,
  });

  // Cookie 管理弹窗
  const [cookieViewOpen, setCookieViewOpen] = useState(false);
  const [cookieViewAccount, setCookieViewAccount] =
    useState<BwbrowserAccount | null>(null);
  const [cookieViewLoading, setCookieViewLoading] = useState(false);
  const [cookieViewData, setCookieViewData] = useState<any>(null);

  // 2FA / 短信验证码状态
  const [twoFACodes, setTwoFACodes] = useState<
    Record<number, { code: string; loading: boolean; time: number }>
  >({});
  const [smsCodes, setSmsCodes] = useState<
    Record<number, { code: string; loading: boolean; time: number }>
  >({});

  const { storedProxies } = useProxyEvents();

  const handleRefresh = useCallback(async () => {
    setIsRefreshing(true);
    try {
      await Promise.all([refresh(), refreshSummary()]);
    } catch (e) {
      console.error(e);
    } finally {
      setIsRefreshing(false);
    }
  }, [refresh, refreshSummary]);

  const [addDialogOpen, setAddDialogOpen] = useState(false);
  const [addSaving, setAddSaving] = useState(false);
  const [addForm, setAddForm] = useState({
    phone_id: "",
    account_name: "",
    platform: "youtube",
    login_account: "",
    login_password: "",
    bind_phone: "",
    safe_link: "",
    backup_email: "",
    owner_id: 0,
    remark: "",
    tags: [] as string[],
    tagInput: "",
  });

  const handleAddAccount = useCallback(() => {
    setAddForm({
      phone_id: "",
      account_name: "",
      platform: "youtube",
      login_account: "",
      login_password: "",
      bind_phone: "",
      safe_link: "",
      backup_email: "",
      owner_id: 0,
      remark: "",
      tags: [],
      tagInput: "",
    });
    setAddDialogOpen(true);
  }, []);

  const handleSaveNewAccount = useCallback(async () => {
    if (!addForm.account_name.trim()) {
      showErrorToast("账号名称不能为空");
      return;
    }
    setAddSaving(true);
    try {
      await invoke("bwbrowser_create_account", {
        phoneId: addForm.phone_id.trim() || null,
        accountName: addForm.account_name.trim(),
        platform: addForm.platform || null,
        loginAccount: addForm.login_account.trim() || null,
        loginPassword: addForm.login_password || null,
        bindPhone: addForm.bind_phone.trim() || null,
        safeLink: addForm.safe_link.trim() || null,
        backupEmail: addForm.backup_email.trim() || null,
        ownerId: addForm.owner_id || null,
        remark: addForm.remark || null,
        tags: addForm.tags.join(",") || null,
      });
      showSuccessToast("账号创建成功");
      setAddDialogOpen(false);
      void refresh();
    } catch (e) {
      showErrorToast(`创建失败: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setAddSaving(false);
    }
  }, [addForm, refresh]);

  const handlePageChange = useCallback(
    (page: number) => {
      void fetchPage(page, pageSize);
    },
    [fetchPage, pageSize],
  );

  const handleSearchChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      setKeyword(e.target.value);
    },
    [setKeyword],
  );

  const handlePlatformChange = useCallback(
    (platform: string) => {
      setPlatformFilter(platform === platformFilter ? "all" : platform);
    },
    [setPlatformFilter, platformFilter],
  );

  const handleOwnerChange = useCallback(
    (ownerId: number | null) => {
      setOwnerFilter(ownerFilter === ownerId ? null : ownerId);
    },
    [setOwnerFilter, ownerFilter],
  );

  // 刷新 2FA 验证码
  const handleRefresh2FA = useCallback(
    async (accountId: number) => {
      const account = accounts.find((a) => a.id === accountId);
      if (!account?.safe_link) {
        showErrorToast("该账号未配置 2FA 链接");
        return;
      }
      setTwoFACodes((prev) => ({
        ...prev,
        [accountId]: { code: "", loading: true, time: Date.now() },
      }));
      try {
        // 优先检查 URL 是否包含 secret 参数（本地 TOTP 生成）
        const secret = extractSecretFromUrl(account.safe_link);
        if (secret) {
          const code = await generateTOTP(secret);
          setTwoFACodes((prev) => ({
            ...prev,
            [accountId]: { code, loading: false, time: Date.now() },
          }));
          return;
        }
        // 回退到远程 fetch
        const res = await fetch(account.safe_link, {
          method: "GET",
          headers: { "Content-Type": "application/json" },
        });
        const text = await res.text();
        let code = "";
        try {
          const json = JSON.parse(text);
          code =
            json.code ||
            json.data?.code ||
            json.data ||
            json.totp ||
            json.otp ||
            "";
        } catch {
          const match = text.match(/\b\d{6}\b/);
          if (match) code = match[0];
        }
        if (code) {
          setTwoFACodes((prev) => ({
            ...prev,
            [accountId]: { code, loading: false, time: Date.now() },
          }));
        } else {
          setTwoFACodes((prev) => ({
            ...prev,
            [accountId]: { code: "", loading: false, time: Date.now() },
          }));
          showErrorToast("未获取到 2FA 验证码");
        }
      } catch (e) {
        setTwoFACodes((prev) => ({
          ...prev,
          [accountId]: { code: "", loading: false, time: Date.now() },
        }));
        showErrorToast(
          `获取 2FA 失败: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
    },
    [accounts],
  );

  // 刷新短信验证码
  const handleRefreshSms = useCallback(
    async (accountId: number) => {
      const account = accounts.find((a) => a.id === accountId);
      if (!account?.bind_phone) {
        showErrorToast("该账号未配置短信验证链接");
        return;
      }
      setSmsCodes((prev) => ({
        ...prev,
        [accountId]: { code: "", loading: true, time: Date.now() },
      }));
      try {
        const res = await fetch(account.bind_phone, {
          method: "GET",
          headers: { "Content-Type": "application/json" },
        });
        const text = await res.text();
        let code = "";
        try {
          const json = JSON.parse(text);
          code =
            json.code ||
            json.data?.code ||
            json.data ||
            json.sms_code ||
            json.msg_code ||
            "";
        } catch {
          const match = text.match(/\b\d{4,6}\b/);
          if (match) code = match[0];
        }
        if (code) {
          setSmsCodes((prev) => ({
            ...prev,
            [accountId]: { code, loading: false, time: Date.now() },
          }));
        } else {
          setSmsCodes((prev) => ({
            ...prev,
            [accountId]: { code: "", loading: false, time: Date.now() },
          }));
          showErrorToast("未获取到短信验证码");
        }
      } catch (e) {
        setSmsCodes((prev) => ({
          ...prev,
          [accountId]: { code: "", loading: false, time: Date.now() },
        }));
        showErrorToast(
          `获取短信验证码失败: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
    },
    [accounts],
  );

  // 打开环境编辑弹窗
  const handleOpenEnvDialog = useCallback(
    async (account: BwbrowserAccount) => {
      setEnvDialogAccount(account);
      setSelectedEnvUuid(account.env_uuid ?? null);
      setEnvDialogOpen(true);
      setEnvLoading(true);
      try {
        const envs = await invoke<
          { env_uuid: string; name: string; browser_type: string }[]
        >("bwbrowser_list_envs", {
          // 环境按账号自己的公司查，页面公司筛选为空（超管默认查自己公司）时不至于查错
          companyId: account.company_id ?? selectedCompanyId ?? null,
        });
        setCloudEnvs(envs || []);
      } catch (e) {
        showErrorToast(
          `加载环境列表失败: ${e instanceof Error ? e.message : String(e)}`,
        );
        setCloudEnvs([]);
      } finally {
        setEnvLoading(false);
      }
    },
    [selectedCompanyId],
  );

  // 保存环境绑定
  const handleSaveEnvBinding = useCallback(async () => {
    if (!envDialogAccount) return;
    setCodeSaving(true);
    try {
      await invoke("bwbrowser_update_account_env", {
        accountId: envDialogAccount.id,
        envUuid: selectedEnvUuid,
      });
      showSuccessToast("环境绑定已保存");
      setEnvDialogOpen(false);
      void refresh();
    } catch (e) {
      showErrorToast(`保存失败: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setCodeSaving(false);
    }
  }, [envDialogAccount, selectedEnvUuid, refresh]);

  // 环境列下拉：编辑该账号已绑定的环境
  const handleEditBoundEnv = useCallback(async (account: BwbrowserAccount) => {
    const envUuid = account.env_uuid;
    if (!envUuid) return;
    try {
      // 按 uuid 直查环境（服务端不按公司过滤）。拉列表遍历会因公司过滤偏差
      // 误报"未找到该环境"——绑定关系本身就是 uuid，不存在列表匹配问题。
      const env = await invoke<
        Omit<BwbrowserEnvironment, "id" | "is_cloud_managed">
      >("bwbrowser_get_env", { envUuid });
      const target: BwbrowserEnvironment = {
        ...env,
        id: `bwbrowser_env_${envUuid}`,
        is_cloud_managed: true,
      };
      setBoundEnvTarget(target);
      setBoundEnvFormOpen(true);
    } catch (e) {
      showErrorToast(
        `加载环境失败: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }, []);

  // 环境列下拉：删除该账号已绑定的环境，并解绑该账号
  const handleDeleteBoundEnv = useCallback(
    async (account: BwbrowserAccount) => {
      const envUuid = account.env_uuid;
      if (!envUuid) return;
      if (
        !confirm(`确定删除环境「${envUuid}」吗？绑定该环境的账号会被解绑。`)
      ) {
        return;
      }
      try {
        await invoke("bwbrowser_delete_env", { envUuid });
      } catch (e) {
        showErrorToast(
          `删除环境失败: ${e instanceof Error ? e.message : String(e)}`,
        );
        return;
      }
      try {
        await invoke("bwbrowser_update_account_env", {
          accountId: account.id,
          envUuid: null,
        });
        showSuccessToast("环境已删除并解绑");
      } catch (e) {
        showErrorToast(
          `环境已删除，但解绑失败: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
      void refresh();
    },
    [refresh],
  );

  // 打开 2FA/短信编辑弹窗
  const _handleOpenCodeEdit = useCallback((account: BwbrowserAccount) => {
    setCodeEditAccount(account);
    setEditSafeLink(account.safe_link ?? "");
    setEditBindPhone(account.bind_phone ?? "");
    setCodeEditOpen(true);
  }, []);

  // 打开编辑账号对话框
  const handleOpenEdit = useCallback(
    async (account: BwbrowserAccount) => {
      setEditAccount(account);
      setEditDetail(null);
      setEditLoading(true);
      setEditDialogOpen(true);
      setShowEditPassword(false);
      setEditForm({
        account_name: account.account_name ?? "",
        login_account: account.login_account ?? "",
        login_password: "",
        platform: account.platform ?? "",
        remark: account.remark ?? "",
        tags:
          account.tags_list ??
          (account.tags
            ? account.tags
                .split(",")
                .map((t) => t.trim())
                .filter(Boolean)
            : []),
        tagInput: "",
        category: account.category ?? "",
        pure_username: account.pure_username ?? "",
        device_id: account.device_id ?? account.phone_id ?? "",
        bind_phone: account.bind_phone ?? "",
        safe_link: account.safe_link ?? "",
        backup_email: account.backup_email ?? "",
        channel_id:
          (account as BwbrowserAccount & { channel_id?: string }).channel_id ??
          "",
        owner_id: account.owner_id ?? 0,
      });

      // 获取权限
      try {
        const perms = await invoke<{
          allow_view_password: boolean;
          allow_2fa: boolean;
          allow_sms_management: boolean;
          is_manager: boolean;
          is_super_admin: boolean;
        }>("bwbrowser_get_permissions");
        console.log("[EditAccount] get_permissions 返回:", perms);
        const canEdit = perms.is_manager || perms.is_super_admin;
        setEditPerms({
          canViewPassword: perms.allow_view_password,
          canView2FA: perms.allow_2fa,
          canViewSMS: perms.allow_sms_management,
          canEditAll: canEdit,
        });
      } catch (e) {
        console.error("获取权限失败:", e);
        // 降级：使用 list 返回的权限
        setEditPerms({
          canViewPassword: canViewPassword,
          canView2FA: isManager || isSuperAdmin,
          canViewSMS: isManager || isSuperAdmin,
          canEditAll: isManager || isSuperAdmin,
        });
      }

      // 加载账号详情
      try {
        const detail = await invoke<BwbrowserAccount>(
          "bwbrowser_get_account_detail",
          {
            accountId: account.id,
          },
        );
        console.log("[EditAccount] get_account_detail 返回:", {
          id: detail.id,
          phone_id: detail.phone_id,
          owner_id: detail.owner_id,
          safe_link: detail.safe_link,
          bind_phone: detail.bind_phone,
          account_name: detail.account_name,
        });
        setEditDetail(detail);
        const tags =
          detail.tags_list ??
          (detail.tags
            ? detail.tags
                .split(",")
                .map((t) => t.trim())
                .filter(Boolean)
            : []);
        setEditForm((prev) => ({
          ...prev,
          account_name: detail.account_name ?? prev.account_name,
          login_account: detail.login_account ?? prev.login_account,
          login_password: detail.login_password ?? "",
          remark: detail.remark ?? prev.remark,
          tags,
          category: detail.category ?? prev.category,
          pure_username: detail.pure_username ?? prev.pure_username,
          device_id: detail.device_id ?? detail.phone_id ?? prev.device_id,
          bind_phone: detail.bind_phone ?? prev.bind_phone,
          safe_link: detail.safe_link ?? prev.safe_link,
          backup_email: detail.backup_email ?? prev.backup_email,
          channel_id:
            (detail as BwbrowserAccount & { channel_id?: string }).channel_id ??
            prev.channel_id,
          owner_id: detail.owner_id ?? prev.owner_id,
        }));
      } catch (e) {
        console.error("获取账号详情失败:", e);
      } finally {
        setEditLoading(false);
      }
    },
    [isManager, isSuperAdmin, canViewPassword],
  );

  // 保存 2FA/短信链接
  const handleSaveCodes = useCallback(async () => {
    if (!codeEditAccount) return;
    setCodeSaving(true);
    try {
      await invoke("bwbrowser_update_account_codes", {
        accountId: codeEditAccount.id,
        safeLink: editSafeLink.trim() || null,
        bindPhone: editBindPhone.trim() || null,
      });
      showSuccessToast("验证码链接已保存");
      setCodeEditOpen(false);
      void refresh();
    } catch (e) {
      showErrorToast(`保存失败: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setCodeSaving(false);
    }
  }, [codeEditAccount, editSafeLink, editBindPhone, refresh]);

  // 打开代理设置弹窗（单账号或批量多账号）
  const handleOpenProxyDialog = useCallback(
    async (targets: BwbrowserAccount[]) => {
      if (targets.length === 0) return;
      setProxyDialogTargets(targets);
      setProxySearchQuery("");
      setProxyPage(0);
      // 单账号时按 device_id 查映射表，弹窗里提示对应的 SRC-IP-CIDR 行
      setSrcIpRuleHint(
        targets.length === 1 ? computeSrcIpRuleHint(targets[0]) : null,
      );
      setSrcIpRulesOpen(false);
      // 先打开弹窗，再直接拉云端代理列表，避免弹窗被远程请求卡住
      setProxyDialogOpen(true);
      setProxyDialogLoading(true);
      try {
        // 直接拉云端代理列表（数据以云端为准，不读本地缓存、不同步到本地）
        // 超级管理员切换公司时，使用当前选中的公司 ID 拉取对应公司的代理
        let proxies: StoredProxy[] = [];
        try {
          const cloud = await invoke<CloudProxyItem[]>(
            "bwbrowser_list_proxies",
            { companyId: selectedCompanyId ? selectedCompanyId : null },
          );
          proxies = (cloud ?? []).map((item) => ({
            id: item.id,
            name: item.name,
            proxy_settings: {
              proxy_type: item.proxy_type,
              host: item.host,
              port: item.port,
              username: item.username ?? undefined,
              password: item.password ?? undefined,
              vless_uri: item.vless_uri ?? undefined,
            },
            is_cloud_managed: item.is_cloud_managed,
            country: item.country ?? undefined,
            city: item.city ?? undefined,
          }));
          setCloudProxyOptions(proxies);
        } catch (e) {
          console.warn("拉取云端代理列表失败:", e);
          setCloudProxyOptions([]);
        }

        // 并行拉节点使用统计（失败不影响弹窗，只是「使用」列显示为空）
        try {
          const usage = await invoke<CloudProxyUsageItem[]>(
            "bwbrowser_list_proxy_usage",
            { companyId: selectedCompanyId ? selectedCompanyId : null },
          );
          const map = new Map<string, CloudProxyUsageItem[]>();
          for (const u of usage ?? []) {
            const key = proxyNodeKey(u.proxy_node);
            if (!key) continue;
            const list = map.get(key);
            if (list) {
              list.push(u);
            } else {
              map.set(key, [u]);
            }
          }
          setProxyUsage(map);
        } catch (e) {
          console.warn("拉取节点使用统计失败:", e);
          setProxyUsage(new Map());
        }

        // 如果账号已有代理，自动选中（基于云端代理列表）；批量多账号时
        // 各自代理可能不同，不预选，由用户明确指定
        if (targets.length === 1 && targets[0].proxy_node) {
          const pn = targets[0].proxy_node;
          let existing: StoredProxy | undefined;
          // VLESS/Trojan: 按 URI 匹配
          if (pn.startsWith("vless://") || pn.startsWith("trojan://")) {
            existing = proxies.find((p) => p.proxy_settings.vless_uri === pn);
          } else {
            // HTTP/SOCKS5: 按 host:port 匹配（支持 type:host:port 和旧格式 host:port）
            const parts = pn.split(":");
            const hostIdx = ["http", "socks5"].includes(parts[0].toLowerCase())
              ? 1
              : 0;
            const host = parts[hostIdx];
            const port =
              parts.length >= hostIdx + 2
                ? parseInt(parts[hostIdx + 1], 10)
                : 0;
            existing = proxies.find(
              (p) =>
                p.proxy_settings.host === host &&
                p.proxy_settings.port === port,
            );
          }
          setSelectedProxyId(existing ? existing.id : null);
        } else {
          // 没有设置代理时，默认选中直连
          setSelectedProxyId("__direct__");
        }
      } finally {
        setProxyDialogLoading(false);
      }
    },
    [selectedCompanyId],
  );

  // 刷新云端代理列表（不重新打开弹窗）
  const refreshCloudProxyList = useCallback(async () => {
    setProxyDialogLoading(true);
    try {
      const cloud = await invoke<CloudProxyItem[]>("bwbrowser_list_proxies", {
        companyId: selectedCompanyId ? selectedCompanyId : null,
      });
      const proxies = (cloud ?? []).map((item) => ({
        id: item.id,
        name: item.name,
        proxy_settings: {
          proxy_type: item.proxy_type,
          host: item.host,
          port: item.port,
          username: item.username ?? undefined,
          password: item.password ?? undefined,
          vless_uri: item.vless_uri ?? undefined,
        },
        is_cloud_managed: item.is_cloud_managed,
        country: item.country ?? undefined,
        city: item.city ?? undefined,
      }));
      setCloudProxyOptions(proxies);
    } catch (e) {
      console.warn("刷新云端代理列表失败:", e);
    } finally {
      setProxyDialogLoading(false);
    }
  }, [selectedCompanyId]);

  // 测试单个代理健康状态
  const testSingleProxy = useCallback(async (proxy: StoredProxy) => {
    setProxyCheckResults((prev) => {
      const next = new Map(prev);
      next.set(proxy.id, { checking: true, result: null, error: null });
      return next;
    });
    try {
      const result = await invoke<ProxyCheckResult>("check_proxy_validity", {
        proxyId: proxy.id,
        proxySettings: proxy.proxy_settings,
      });
      setProxyCheckResults((prev) => {
        const next = new Map(prev);
        next.set(proxy.id, { checking: false, result, error: null });
        return next;
      });
      return result;
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setProxyCheckResults((prev) => {
        const next = new Map(prev);
        next.set(proxy.id, {
          checking: false,
          result: {
            ip: "",
            timestamp: Math.floor(Date.now() / 1000),
            is_valid: false,
          },
          error: msg,
        });
        return next;
      });
      throw e;
    }
  }, []);

  // 批量测试代理（并发控制）
  const batchTestProxies = useCallback(async (proxies: StoredProxy[]) => {
    if (proxies.length === 0) return;

    // 先全部标记为测试中
    setProxyCheckResults((prev) => {
      const next = new Map(prev);
      for (const p of proxies) {
        next.set(p.id, { checking: true, result: null, error: null });
      }
      return next;
    });

    // 并发控制：最多同时跑 BATCH_TEST_CONCURRENCY 个
    let index = 0;
    const results: Map<string, ProxyCheckResult | null> = new Map();
    const errors: Map<string, string> = new Map();

    async function worker() {
      while (index < proxies.length) {
        const currentIndex = index++;
        const proxy = proxies[currentIndex];
        try {
          const result = await invoke<ProxyCheckResult>(
            "check_proxy_validity",
            {
              proxyId: proxy.id,
              proxySettings: proxy.proxy_settings,
            },
          );
          results.set(proxy.id, result);
          // 逐个更新 UI，显示进度
          setProxyCheckResults((prev) => {
            const next = new Map(prev);
            next.set(proxy.id, { checking: false, result, error: null });
            return next;
          });
        } catch (e) {
          const msg = e instanceof Error ? e.message : String(e);
          errors.set(proxy.id, msg);
          setProxyCheckResults((prev) => {
            const next = new Map(prev);
            next.set(proxy.id, {
              checking: false,
              result: {
                ip: "",
                timestamp: Math.floor(Date.now() / 1000),
                is_valid: false,
              },
              error: msg,
            });
            return next;
          });
        }
      }
    }

    const workers = Array.from(
      { length: Math.min(BATCH_TEST_CONCURRENCY, proxies.length) },
      () => worker(),
    );
    await Promise.all(workers);
  }, []);

  // 创建新代理（云端）
  const handleCreateCloudProxy = useCallback(
    async (payload: {
      name: string;
      proxySettings: {
        proxy_type: string;
        host: string;
        port: number;
        username?: string;
        password?: string;
        vless_uri?: string;
      };
    }) => {
      const isUriType =
        payload.proxySettings.proxy_type === "vless" ||
        payload.proxySettings.proxy_type === "trojan";
      await invoke("bwbrowser_sync_proxy", {
        proxyId: null,
        proxyName: payload.name,
        proxyType: payload.proxySettings.proxy_type,
        host: payload.proxySettings.host,
        port: payload.proxySettings.port,
        username: payload.proxySettings.username ?? null,
        password: payload.proxySettings.password ?? null,
        country: null,
        city: null,
        timezone: null,
        protocolConfig: isUriType
          ? (payload.proxySettings.vless_uri ?? null)
          : null,
      });
      showSuccessToast("代理创建成功");
      // 创建成功后立即刷新列表
      await refreshCloudProxyList();
    },
    [refreshCloudProxyList],
  );

  // 保存账号代理
  const handleSaveAccountProxy = useCallback(async () => {
    const targets = proxyDialogTargets;
    if (targets.length === 0) return;

    // 批量下发：逐个账号调用，统计成功/失败后汇总提示（服务端按账号解析
    // 时区/语言，无法合并为一次请求）
    let applyProxyNodeStr = "";
    const applyProxyNode = async (proxyNode: string) => {
      applyProxyNodeStr = proxyNode;
      setIsSavingProxy(true);
      let ok = 0;
      let geoPending = 0;
      let lastTimezone: string | undefined;
      let lastLanguage: string | undefined;
      const failed: string[] = [];
      try {
        for (const target of targets) {
          try {
            const result = (await invoke("bwbrowser_update_account_proxy", {
              accountId: target.id,
              accountName: target.account_name,
              proxyNode,
            })) as BwbrowserUpdateProxyResult | null;
            ok += 1;
            if (result?.geo_pending) {
              geoPending += 1;
              setGeoPendingIds((prev) => new Set(prev).add(target.id));
            }
            lastTimezone = result?.timezone;
            lastLanguage = result?.language;
          } catch (e) {
            failed.push(
              `${target.account_name || target.id}: ${
                e instanceof Error ? e.message : String(e)
              }`,
            );
          }
        }

        if (targets.length === 1) {
          // 单账号沿用原有的明细提示
          if (failed.length > 0) {
            showErrorToast(
              proxyNode
                ? `设置代理失败: ${failed[0]}`
                : `清除代理失败: ${failed[0]}`,
            );
          } else if (geoPending > 0) {
            showSuccessToast("代理设置成功（时区后台解析中，稍后自动更新）");
          } else if (lastTimezone) {
            showSuccessToast(
              `代理设置成功 | 时区: ${lastTimezone}${
                lastLanguage ? ` | 语言: ${lastLanguage}` : ""
              }`,
            );
          } else {
            showSuccessToast(
              proxyNode
                ? "代理设置成功（未获取到时区）"
                : "已清除代理，使用直连模式",
            );
          }
        } else {
          const action = proxyNode ? "设置代理" : "清除代理";
          if (failed.length === 0) {
            showSuccessToast(
              geoPending > 0
                ? `${action}完成：${ok} 个账号（${geoPending} 个时区后台解析中）`
                : `${action}完成：${ok} 个账号`,
            );
          } else {
            showErrorToast(
              `${action}：成功 ${ok} 个，失败 ${failed.length} 个 —— ${failed
                .slice(0, 3)
                .join("；")}${failed.length > 3 ? "…" : ""}`,
            );
          }
        }
        setProxyDialogOpen(false);
        void refresh();
        // 编辑账号弹窗开着时，同步其代理节点显示（单账号场景：直连=空串，其余=新节点串）
        if (proxyDialogTargets.length === 1) {
          const target = proxyDialogTargets[0];
          setEditDetail((prev) =>
            prev && prev.id === target.id
              ? {
                  ...prev,
                  proxy_node:
                    selectedProxyId === "__direct__" ? "" : applyProxyNodeStr,
                }
              : prev,
          );
        }
      } finally {
        setIsSavingProxy(false);
      }
    };

    // 直连模式：传空字符串清除代理
    if (selectedProxyId === "__direct__") {
      await applyProxyNode("");
      return;
    }

    if (!selectedProxyId) return;

    const proxy = cloudProxyOptions.find((p) => p.id === selectedProxyId);
    if (!proxy) return;

    const ps = proxy.proxy_settings;
    const proxyType = ps.proxy_type || "http";
    let proxyNode: string;
    if (proxyType === "vless" || proxyType === "trojan") {
      proxyNode = ps.vless_uri || "";
      if (!proxyNode) {
        showErrorToast("该代理缺少协议 URI，请在代理管理中重新编辑");
        return;
      }
    } else if (ps.username && ps.password) {
      proxyNode = `${proxyType}:${ps.host}:${ps.port}:${ps.username}:${ps.password}`;
    } else {
      proxyNode = `${proxyType}:${ps.host}:${ps.port}`;
    }

    await applyProxyNode(proxyNode);
  }, [proxyDialogTargets, selectedProxyId, cloudProxyOptions, refresh]);

  // 批量清除代理：不开弹窗，直接逐个账号置为直连
  const handleBatchClearProxy = useCallback(
    async (targets: BwbrowserAccount[]) => {
      if (targets.length === 0) return;
      setIsSavingProxy(true);
      let ok = 0;
      const failed: string[] = [];
      try {
        for (const target of targets) {
          try {
            await invoke("bwbrowser_update_account_proxy", {
              accountId: target.id,
              accountName: target.account_name,
              proxyNode: "",
            });
            ok += 1;
          } catch (e) {
            failed.push(
              `${target.account_name || target.id}: ${
                e instanceof Error ? e.message : String(e)
              }`,
            );
          }
        }
        if (targets.length === 1) {
          if (failed.length > 0) {
            showErrorToast(`清除代理失败: ${failed[0]}`);
          } else {
            showSuccessToast("已清除代理，使用直连模式");
          }
        } else if (failed.length === 0) {
          showSuccessToast(`清除代理完成：${ok} 个账号`);
        } else {
          showErrorToast(
            `清除代理：成功 ${ok} 个，失败 ${failed.length} 个 —— ${failed
              .slice(0, 3)
              .join("；")}${failed.length > 3 ? "…" : ""}`,
          );
        }
        void refresh();
      } finally {
        setIsSavingProxy(false);
      }
    },
    [refresh],
  );

  // 批量设置归属人：逐个账号调 update_account_info（只传 owner_id）
  const handleBatchSetOwner = useCallback(
    async (targets: BwbrowserAccount[], owner: CloudUser) => {
      if (targets.length === 0) return;
      setIsSettingOwner(true);
      let ok = 0;
      const failed: string[] = [];
      try {
        for (const target of targets) {
          try {
            await invoke("bwbrowser_update_account_info", {
              accountId: target.id,
              accountName: null,
              loginAccount: null,
              loginPassword: null,
              remark: null,
              tags: null,
              category: null,
              pureUsername: null,
              ownerId: owner.id,
              status: null,
              phoneId: null,
              bindPhone: null,
              safeLink: null,
              backupEmail: null,
            });
            ok += 1;
          } catch (e) {
            failed.push(
              `${target.account_name || target.id}: ${
                e instanceof Error ? e.message : String(e)
              }`,
            );
          }
        }
        const label = owner.real_name || owner.username || `成员#${owner.id}`;
        if (targets.length === 1) {
          if (failed.length > 0) {
            showErrorToast(`设置归属人失败: ${failed[0]}`);
          } else {
            showSuccessToast(`归属人已设置为 ${label}`);
          }
        } else if (failed.length === 0) {
          showSuccessToast(`归属人已设置为 ${label}（${ok} 个账号）`);
        } else {
          showErrorToast(
            `设置归属人：成功 ${ok} 个，失败 ${failed.length} 个 —— ${failed
              .slice(0, 3)
              .join("；")}${failed.length > 3 ? "…" : ""}`,
          );
        }
        setOwnerDialogOpen(false);
        void refresh();
      } finally {
        setIsSettingOwner(false);
      }
    },
    [refresh],
  );

  // 批量添加备注：逐个账号调 update_account_info（只传 remark）。
  // 追加模式 = 原备注 + 换行 + 新内容（前端拼好整体下发，避免服务端拼接逻辑）。
  const handleBatchSetRemark = useCallback(async () => {
    const targets = remarkDialogTargets;
    const text = remarkText.trim();
    if (targets.length === 0) return;
    if (!text) return;
    setIsSavingRemark(true);
    let ok = 0;
    const failed: string[] = [];
    try {
      for (const target of targets) {
        const base = (target.remark ?? "").trim();
        const finalRemark = remarkAppend
          ? base
            ? `${base}\n${text}`
            : text
          : text;
        try {
          await invoke("bwbrowser_update_account_info", {
            accountId: target.id,
            accountName: null,
            loginAccount: null,
            loginPassword: null,
            remark: finalRemark,
            tags: null,
            category: null,
            pureUsername: null,
            ownerId: null,
            status: null,
            phoneId: null,
            bindPhone: null,
            safeLink: null,
            backupEmail: null,
          });
          ok += 1;
        } catch (e) {
          failed.push(
            `${target.account_name || target.id}: ${
              e instanceof Error ? e.message : String(e)
            }`,
          );
        }
      }
      const action = text ? "添加备注" : "清空备注";
      if (targets.length === 1) {
        if (failed.length > 0) {
          showErrorToast(`${action}失败: ${failed[0]}`);
        } else {
          showSuccessToast("备注已保存");
        }
      } else if (failed.length === 0) {
        showSuccessToast(`${action}完成：${ok} 个账号`);
      } else {
        showErrorToast(
          `${action}：成功 ${ok} 个，失败 ${failed.length} 个 —— ${failed
            .slice(0, 3)
            .join("；")}${failed.length > 3 ? "…" : ""}`,
        );
      }
      setRemarkDialogOpen(false);
      setRemarkText("");
      void refresh();
    } finally {
      setIsSavingRemark(false);
    }
  }, [remarkDialogTargets, remarkText, remarkAppend, refresh]);

  // 后台代理时区解析完成后刷新数据（不阻塞 UI）
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/event").then(({ listen }) => {
      if (disposed) return;
      void listen("proxy-geo-updated", (event) => {
        const payload = event.payload as { account_id?: number } | undefined;
        if (payload && typeof payload.account_id === "number") {
          setGeoPendingIds((prev) => {
            const next = new Set(prev);
            next.delete(payload.account_id as number);
            return next;
          });
        }
        void refresh();
      }).then((fn) => {
        unlisten = fn;
      });
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);

  // 代理弹窗：搜索过滤 + 分页
  const filteredProxies = useMemo(() => {
    if (!proxySearchQuery.trim()) return cloudProxyOptions;
    const q = proxySearchQuery.toLowerCase();
    return cloudProxyOptions.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.proxy_settings.host.includes(q) ||
        p.proxy_settings.proxy_type.toLowerCase().includes(q),
    );
  }, [cloudProxyOptions, proxySearchQuery]);

  // VPS 代理：搜索过滤
  const vpsFilteredProxies = useMemo(() => {
    const q = vpsProxySearch.trim().toLowerCase();
    if (!q) return storedProxies;
    return storedProxies.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.proxy_settings.host.includes(q) ||
        p.proxy_settings.proxy_type.toLowerCase().includes(q),
    );
  }, [storedProxies, vpsProxySearch]);

  const handleOpenVpsProxyDialog = useCallback(async () => {
    try {
      const info = await invoke<{ proxy_id: string | null } | null>(
        "bwbrowser_get_vps_profile_info",
      );
      setVpsSelectedProxyId(info?.proxy_id ?? null);
    } catch {
      setVpsSelectedProxyId(null);
    }
    setVpsProxySearch("");
    setVpsProxyDialogOpen(true);
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

  const handleVpsLogoContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setVpsContextMenuPos({ x: e.clientX, y: e.clientY });
    setVpsContextMenuOpen(true);
  }, []);

  // 点击外部关闭 VPS 右键菜单
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

  const proxyTotalPages = Math.max(
    1,
    Math.ceil(filteredProxies.length / PROXY_PAGE_SIZE),
  );
  const proxyCurrentPage = Math.min(proxyPage, proxyTotalPages - 1);
  const proxyPageData = filteredProxies.slice(
    proxyCurrentPage * PROXY_PAGE_SIZE,
    (proxyCurrentPage + 1) * PROXY_PAGE_SIZE,
  );
  const selectedProxy = selectedProxyId
    ? cloudProxyOptions.find((p) => p.id === selectedProxyId)
    : null;

  // 启动浏览器：有环境用环境，没环境则创建 profile + 设置代理后启动
  const [launchingIds, setLaunchingIds] = useState<Set<number>>(new Set());
  const [launchProgress, setLaunchProgress] = useState<
    Record<number, { pct: number; label: string }>
  >({});

  // 正在进行后台时区解析的账号 id 集合（不阻塞 UI）
  const [geoPendingIds, setGeoPendingIds] = useState<Set<number>>(new Set());

  // 浏览器关闭后清除「已启动」状态（昵称进度行 + 操作列按钮回原样）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<{ account_id: number }>("account-browser-closed", (event) => {
          const id = event.payload.account_id;
          setLaunchProgress((prev) => {
            if (!(id in prev)) return prev;
            const next = { ...prev };
            delete next[id];
            return next;
          });
        }),
      )
      .then((fn) => {
        unlisten = fn;
      })
      .catch((e) => console.warn("failed to listen account-browser-closed", e));
    return () => unlisten?.();
  }, []);

  const handleLaunchAccount = useCallback(
    async (account: BwbrowserAccount) => {
      // 套餐启动配额：超出配额的账号不允许启动浏览器
      if (account.launch_allowed === false) {
        showErrorToast("该账号超出当前套餐的启动配额，无法启动浏览器");
        return;
      }
      const accountId = account.id;
      setLaunchingIds((prev) => new Set(prev).add(accountId));
      setLaunchProgress((prev) => ({
        ...prev,
        [accountId]: { pct: 0, label: "正在启动..." },
      }));
      let unlistenProgress: (() => void) | undefined;

      // 监听后端真实启动进度，在按钮位置原位显示
      import("@tauri-apps/api/event")
        .then(({ listen }) =>
          listen<{ account_id: number; pct: number; label: string }>(
            "account-launch-progress",
            (event) => {
              if (event.payload.account_id !== accountId) return;
              const pct = Math.min(100, Math.max(0, event.payload.pct));
              setLaunchProgress((prev) => ({
                ...prev,
                [accountId]: {
                  pct,
                  label: event.payload.label || "正在启动...",
                },
              }));
            },
          ),
        )
        .then((fn) => {
          unlistenProgress = fn;
        })
        .catch((e) => {
          console.warn("failed to listen launch progress", e);
        });

      try {
        await invoke("bwbrowser_launch_account", {
          accountId: account.id,
          accountName: account.account_name || String(account.id),
          envUuid: account.env_uuid ?? null,
          proxyNode: account.proxy_node ?? null,
          platform: account.platform ?? null,
        });
        setLaunchProgress((prev) => ({
          ...prev,
          [accountId]: { pct: 100, label: "已启动" },
        }));
      } catch (err) {
        setLaunchProgress((prev) => {
          const next = { ...prev };
          delete next[accountId];
          return next;
        });
        const msg = err instanceof Error ? err.message : String(err);
        const translated = translateBackendError(t, msg);
        showErrorToast(translated);
      } finally {
        unlistenProgress?.();
        setLaunchingIds((prev) => {
          const next = new Set(prev);
          next.delete(accountId);
          return next;
        });
      }
    },
    [t],
  );

  // 平台级批量打开：本地启动浏览器打开当前筛选平台下的所有账号（复用账号级启动逻辑）
  const [batchLaunching, setBatchLaunching] = useState(false);
  const openAllForPlatform = () => {
    if (batchLaunching) return;
    const targets = accounts.filter(
      (a) => platformFilter === "all" || (a.platform ?? "") === platformFilter,
    );
    if (targets.length === 0) return;
    // 超出套餐启动配额的账号不能启动，直接跳过并提示
    const launchable = targets.filter((a) => a.launch_allowed !== false);
    const blocked = targets.length - launchable.length;
    if (launchable.length === 0) {
      showErrorToast("当前套餐启动配额已用完，无法批量打开");
      return;
    }
    setBatchLaunching(true);
    const run = async () => {
      try {
        if (blocked > 0) {
          showErrorToast(`已跳过 ${blocked} 个超出套餐配额的账号`);
        }
        for (const acc of launchable) {
          await handleLaunchAccount(acc);
          await new Promise((resolve) => setTimeout(resolve, 250));
        }
      } finally {
        setBatchLaunching(false);
      }
    };
    void run();
  };

  // 平台级批量健康检测：当前筛选平台下的全部账号，5 路并发
  const [healthBatchRunning, setHealthBatchRunning] = useState(false);
  const checkAllHealthForPlatform = () => {
    if (healthBatchRunning) return;
    const targets = accounts.filter(
      (a) => platformFilter === "all" || (a.platform ?? "") === platformFilter,
    );
    if (targets.length === 0) return;
    setHealthBatchRunning(true);
    const run = async () => {
      let valid = 0;
      let invalid = 0;
      let failed = 0;
      let idx = 0;
      const worker = async () => {
        while (idx < targets.length) {
          const acc = targets[idx++];
          setHealthLoadingIds((prev) => new Set(prev).add(acc.id));
          try {
            const result = await invoke<{
              account_id: number;
              status: string;
              message: string;
              checked_at: number;
            }>("bwbrowser_check_account_cookie_health", {
              accountId: acc.id,
              accountName: acc.account_name,
            });
            setHealthStates((prev) => ({
              ...prev,
              [result.account_id]: {
                status: result.status,
                message: result.message,
                checked_at: result.checked_at,
              },
            }));
            if (result.status === "valid") {
              valid++;
            } else {
              invalid++;
            }
          } catch {
            failed++;
          } finally {
            setHealthLoadingIds((prev) => {
              const next = new Set(prev);
              next.delete(acc.id);
              return next;
            });
          }
        }
      };
      await Promise.all(
        Array.from({ length: Math.min(5, targets.length) }, () => worker()),
      );
      setHealthBatchRunning(false);
      showSuccessToast(
        `健康检测完成：${valid} 正常 / ${invalid} 需重新登录${
          failed ? ` / ${failed} 失败` : ""
        }`,
      );
    };
    void run();
  };

  // 平台统计：始终用 summary 数据（来自 API 的全量统计，不依赖当前页）；
  // 按显示名拼音排序，「全部」固定在最前
  const platformStats = useMemo(() => {
    const list = (summary?.platforms ?? []).map((p) => ({
      platform: p.platform,
      count: p.count,
    }));
    list.sort((a, b) =>
      zhCollator.compare(
        getPlatformLabel(a.platform),
        getPlatformLabel(b.platform),
      ),
    );
    return [{ platform: "all", count: summary?.total ?? total }, ...list];
  }, [total, summary]);

  // 人员统计：以 users 列表为准（显示所有人），count 从 summary.owners 匹配
  const ownerStats = useMemo(() => {
    // 考勤状态颜色映射
    const statusMap: Record<
      string,
      { color: string; bg: string; title: string; border: string }
    > = {
      normal: {
        color: "text-green-700 dark:text-green-300",
        bg: "bg-green-100 dark:bg-green-900/30",
        title: "已打卡",
        border: "border-green-300 dark:border-green-700",
      },
      on_leave: {
        color: "text-red-700 dark:text-red-300",
        bg: "bg-red-100 dark:bg-red-900/30",
        title: "请假中",
        border: "border-red-300 dark:border-red-700",
      },
      pending: {
        color: "text-blue-700 dark:text-blue-300",
        bg: "bg-blue-100 dark:bg-blue-900/30",
        title: "审批中",
        border: "border-blue-300 dark:border-blue-700",
      },
      missed: {
        color: "text-yellow-700 dark:text-yellow-300",
        bg: "bg-yellow-100 dark:bg-yellow-900/30",
        title: "应打卡未打",
        border: "border-yellow-300 dark:border-yellow-700",
      },
      rest: {
        color: "text-muted-foreground",
        bg: "bg-background",
        title: "今日休息",
        border: "border-border",
      },
      unknown: {
        color: "text-muted-foreground",
        bg: "bg-background",
        title: "无数据",
        border: "border-border",
      },
    };

    // 如果还没有 users 数据，先用 summary.owners 兜底
    if (users.length === 0 && summary?.owners && summary.owners.length > 0) {
      return summary.owners
        .map((o) => {
          const s = statusMap.unknown;
          return {
            owner_id: o.owner_id,
            owner_name: o.owner_name,
            count: o.count,
            leave_status: "unknown",
            statusStyle: s,
            sectors: [],
          };
        })
        .sort((a, b) => zhCollator.compare(a.owner_name, b.owner_name));
    }

    // 构建 summary owner 的 count 映射
    const ownerCountMap = new Map<number, number>();
    if (summary?.owners) {
      for (const o of summary.owners) {
        ownerCountMap.set(o.owner_id, o.count);
      }
    }

    // 以 users 列表为基准，所有人都显示
    const result = users.map((u) => {
      const status = u.leave_status || "unknown";
      const s = statusMap[status] || statusMap.unknown;
      const displayName = u.company_name
        ? `${u.company_name} - ${u.real_name || u.username}`
        : u.real_name || u.username || String(u.id);
      const count = ownerCountMap.get(u.id) ?? 0;
      return {
        owner_id: u.id,
        owner_name: displayName,
        count,
        leave_status: status,
        statusStyle: s,
        sectors: (u.sector ?? "")
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean),
      };
    });

    // 按显示名拼音排序
    return result.sort((a, b) =>
      zhCollator.compare(a.owner_name, b.owner_name),
    );
  }, [summary, users]);

  // 所有赛道（去重，用于赛道筛选栏），拼音排序
  const allSectors = useMemo(() => {
    const set = new Set<string>();
    for (const o of ownerStats) {
      for (const s of o.sectors) set.add(s);
    }
    return Array.from(set).sort((a, b) => zhCollator.compare(a, b));
  }, [ownerStats]);

  // 当前赛道筛选下可见的人员（含其账号数）
  const visibleOwners = useMemo(() => {
    if (!sectorFilter) return ownerStats;
    return ownerStats.filter((o) => o.sectors.includes(sectorFilter));
  }, [ownerStats, sectorFilter]);

  // 一键修复：删除本地数据 + 服务器 Cookie（无法登录时使用）
  const handleRepairAccount = useCallback(async (account: BwbrowserAccount) => {
    // WebView2 会吞掉原生 confirm()，必须用 plugin-dialog 的原生对话框
    const ok = await dialogConfirm(
      `确定一键修复账号「${account.account_name || account.id}」吗？\n\n` +
        `将执行以下操作：\n` +
        `1. 删除本机的浏览数据（Cookie、缓存、登录状态）\n` +
        `2. 删除服务器上保存的该账号 Cookie\n\n` +
        `适用于账号无法登录的情况。修复后请重新点击「启动浏览器」，会打开全新的登录页面，需要重新登录一次账号。`,
      {
        kind: "warning",
        title: "一键修复",
        okLabel: "确定修复",
        cancelLabel: "取消",
      },
    ).catch(() => false);
    if (!ok) return;
    try {
      await invoke("bwbrowser_delete_local_cookies", {
        accountName: account.account_name || String(account.id),
        accountId: account.id,
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      showErrorToast(`删除本地数据失败: ${msg}`);
      return;
    }
    try {
      await invoke("bwbrowser_delete_cloud_cookies", { accountId: account.id });
      showSuccessToast("修复完成：本地数据与服务器 Cookie 已清除");
      setIsRefreshing(true);
      setTimeout(() => setIsRefreshing(false), 500);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      showErrorToast(`本地数据已清除，但删除服务器 Cookie 失败: ${msg}`);
    }
  }, []);

  // 表格列定义
  const columns = useMemo<ColumnDef<BwbrowserAccount>[]>(() => {
    const isAdmin = isManager || isSuperAdmin;
    const cols: ColumnDef<BwbrowserAccount>[] = [
      {
        id: "select",
        header: ({ table }) => (
          <Checkbox
            checked={
              table.getIsAllPageRowsSelected() ||
              (table.getIsSomePageRowsSelected() && "indeterminate")
            }
            onCheckedChange={(value) =>
              table.toggleAllPageRowsSelected(!!value)
            }
            aria-label="全选"
          />
        ),
        cell: ({ row }) => (
          <Checkbox
            checked={row.getIsSelected()}
            onCheckedChange={(value) => row.toggleSelected(!!value)}
            aria-label="选择行"
          />
        ),
        size: 40,
        enableSorting: false,
      },
      {
        accessorKey: "device_id",
        header: ({ column }) => (
          <button
            type="button"
            className="flex items-center gap-1 text-xs font-medium"
            onClick={() => column.toggleSorting(column.getIsSorted() === "asc")}
          >
            手机编号
            <LuChevronsUpDown className="h-3 w-3 opacity-50" />
          </button>
        ),
        cell: ({ row }) => (
          <div className="font-mono text-xs text-muted-foreground">
            {row.getValue("device_id") || "-"}
          </div>
        ),
        size: 70,
      },
      {
        accessorFn: (row) => row.followers ?? 0,
        id: "followers",
        enableSorting: false,
        header: () => {
          const active = serverSort?.by === "followers" ? serverSort : null;
          return (
            <button
              type="button"
              className="flex items-center gap-1 text-xs font-medium"
              title="全量数据排序（服务端）"
              onClick={() => {
                const next =
                  active && active.order === "desc"
                    ? { by: "followers", order: "asc" as const }
                    : { by: "followers", order: "desc" as const };
                setServerSort(next);
              }}
            >
              粉丝量
              {active ? (
                active.order === "desc" ? (
                  <LuArrowDown className="h-3 w-3" />
                ) : (
                  <LuArrowUp className="h-3 w-3" />
                )
              ) : (
                <LuChevronsUpDown className="h-3 w-3 opacity-50" />
              )}
            </button>
          );
        },
        cell: ({ row }) => {
          const followers = row.original.followers ?? 0;
          return (
            <div className="flex items-center gap-1 text-xs tabular-nums text-muted-foreground">
              <LuUsers className="h-3 w-3 shrink-0" />
              {followers.toLocaleString()}
            </div>
          );
        },
        size: 80,
      },
      {
        accessorKey: "account_name",
        header: ({ column }) => (
          <button
            type="button"
            className="flex items-center gap-1 text-xs font-medium"
            onClick={() => column.toggleSorting(column.getIsSorted() === "asc")}
          >
            账号名称
            <LuChevronsUpDown className="h-3 w-3 opacity-50" />
          </button>
        ),
        cell: ({ row }) => {
          const account = row.original;
          const avatarUrl = resolveAvatarUrl(account.avatar_url);
          const displayName = account.account_name;
          // 健康检测：图标颜色即结果（绿=健康，黄=需重新登录，灰=未检测）
          const h = healthStates[account.id];
          const hStatus = h?.status ?? "unknown";
          const hLoading = healthLoadingIds.has(account.id);
          // 点击昵称启动浏览器的进度（与操作列按钮同源）
          const lp = launchProgress[account.id];
          const launching = launchingIds.has(account.id);
          return (
            <div className="flex max-w-[200px] flex-col gap-1">
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => void handleCheckHealth(account)}
                  disabled={hLoading}
                  className={cn(
                    "shrink-0 rounded p-1 transition-colors hover:bg-muted disabled:opacity-50",
                    hStatus === "valid"
                      ? "text-success"
                      : hStatus === "unknown"
                        ? "text-muted-foreground/60"
                        : "text-warning",
                  )}
                  title={`Cookie健康${
                    hStatus === "valid"
                      ? "：正常"
                      : hStatus === "unknown"
                        ? "：未检测，点击检测"
                        : "：需重新登录，点击重新检测"
                  }${
                    h?.message ? `（${h.message}` : ""
                  }${h?.checked_at ? ` · ${formatDateShort(new Date(h.checked_at * 1000).toISOString())}）` : h?.message ? "）" : ""}`}
                >
                  {hLoading ? (
                    <LuLoaderCircle className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <LuHeartPulse className="h-3.5 w-3.5" />
                  )}
                </button>
                {avatarUrl ? (
                  <span
                    role="img"
                    aria-label={displayName}
                    className="size-6 shrink-0 rounded-full bg-cover bg-center border border-border"
                    style={{ backgroundImage: `url(${avatarUrl})` }}
                  />
                ) : (
                  <span className="grid size-6 shrink-0 place-items-center rounded-full bg-primary/10 text-[10px] font-bold text-primary">
                    {(displayName || "?").charAt(0).toUpperCase()}
                  </span>
                )}
                <button
                  type="button"
                  onClick={() => void handleLaunchAccount(account)}
                  disabled={account.launch_allowed === false}
                  title={
                    account.launch_allowed === false
                      ? "超出当前套餐的启动配额，无法启动"
                      : "点击启动浏览器"
                  }
                  className={cn(
                    "truncate text-left text-xs font-medium transition-colors",
                    account.launch_allowed === false
                      ? "cursor-not-allowed text-muted-foreground/40"
                      : "hover:text-primary hover:underline",
                  )}
                >
                  {displayName || "-"}
                </button>
              </div>
              {lp && (launching || lp.pct >= 100) && (
                <div className="flex items-center gap-1 text-[10px] leading-none text-emerald-600">
                  <LuLoaderCircle
                    className={cn(
                      "h-3 w-3 shrink-0",
                      lp.pct < 100 && "animate-spin",
                    )}
                  />
                  <span className="truncate tabular-nums">
                    {lp.pct >= 100
                      ? "已启动"
                      : `${lp.label} ${Math.floor(lp.pct)}%`}
                  </span>
                </div>
              )}
            </div>
          );
        },
        size: 160,
      },
      {
        accessorKey: "login_account",
        header: "登录账号",
        cell: ({ row }) => {
          const val = row.getValue("login_account") as string | undefined;
          if (!val)
            return <span className="text-xs text-muted-foreground">-</span>;
          return (
            <button
              type="button"
              className="font-mono text-xs text-muted-foreground truncate max-w-[160px] cursor-pointer hover:text-foreground transition-colors"
              title={`点击复制: ${val}`}
              onClick={() => {
                navigator.clipboard.writeText(val);
                showSuccessToast("登录账号已复制");
              }}
            >
              {val}
            </button>
          );
        },
        size: 160,
      },
      {
        accessorKey: "login_password",
        header: "登录密码",
        cell: ({ row }) => {
          const pwd = row.getValue("login_password") as string | undefined;
          if (!pwd)
            return <span className="text-xs text-muted-foreground">-</span>;
          if (!canViewPassword) {
            return (
              <span className="font-mono text-xs text-muted-foreground">
                {"•".repeat(Math.min(pwd.length, 8))}
              </span>
            );
          }
          const account = row.original as BwbrowserAccount;
          const isShown = showPasswordIds.has(account.id);
          const isCopied = copiedPasswordId === account.id;
          return (
            <div className="inline-flex items-center gap-1">
              <span className="font-mono text-xs text-foreground/80">
                {isShown ? pwd : "•".repeat(Math.min(pwd.length, 8))}
              </span>
              <button
                type="button"
                onClick={() => {
                  setShowPasswordIds((prev) => {
                    const next = new Set(prev);
                    if (next.has(account.id)) {
                      next.delete(account.id);
                    } else {
                      next.add(account.id);
                    }
                    return next;
                  });
                }}
                className="rounded p-0.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                title={isShown ? "隐藏密码" : "查看密码"}
              >
                {isShown ? (
                  <LuEyeOff className="h-3 w-3" />
                ) : (
                  <LuEye className="h-3 w-3" />
                )}
              </button>
              <button
                type="button"
                onClick={() => {
                  navigator.clipboard.writeText(pwd);
                  setCopiedPasswordId(account.id);
                  showSuccessToast("密码已复制");
                  setTimeout(() => setCopiedPasswordId(null), 2000);
                }}
                className="rounded p-0.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                title="复制密码"
              >
                {isCopied ? (
                  <LuCheck className="h-3 w-3 text-success" />
                ) : (
                  <LuCopy className="h-3 w-3" />
                )}
              </button>
            </div>
          );
        },
        size: 80,
      },
      ...(canView2FACode
        ? [
            {
              accessorKey: "safe_link",
              header: "2FA",
              cell: ({ row }: { row: { original: BwbrowserAccount } }) => {
                const account = row.original;
                const safeLink = account.safe_link;
                const codeState = twoFACodes[account.id];
                return (
                  <div className="inline-flex items-center gap-1">
                    {safeLink ? (
                      codeState?.code ? (
                        <button
                          type="button"
                          className="inline-flex items-center rounded bg-emerald-500/10 px-1.5 py-0.5 font-mono text-xs font-bold text-emerald-600 cursor-pointer hover:bg-emerald-500/20 dark:text-emerald-400"
                          onClick={() => {
                            navigator.clipboard.writeText(codeState.code);
                            showSuccessToast("2FA 验证码已复制");
                          }}
                          title={`点击复制 · ${new Date(codeState.time).toLocaleTimeString("zh-CN")}`}
                        >
                          {codeState.code}
                        </button>
                      ) : (
                        <span className="inline-flex items-center rounded bg-emerald-500/10 px-1.5 py-0.5 text-[11px] font-medium text-emerald-600">
                          已绑定
                        </span>
                      )
                    ) : (
                      <span className="text-[11px] text-muted-foreground">
                        未设置
                      </span>
                    )}
                    {safeLink && (
                      <button
                        type="button"
                        onClick={() => void handleRefresh2FA(account.id)}
                        className={cn(
                          "rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-primary",
                          codeState?.loading && "animate-spin",
                        )}
                        title="获取 2FA 验证码"
                        disabled={codeState?.loading}
                      >
                        <LuRefreshCw className="h-3 w-3" />
                      </button>
                    )}
                  </div>
                );
              },
              size: 110,
            },
          ]
        : []),
      ...(canViewSMSCode
        ? [
            {
              accessorKey: "bind_phone",
              header: "短信验证",
              cell: ({ row }: { row: { original: BwbrowserAccount } }) => {
                const account = row.original;
                const phone = account.bind_phone;
                return (
                  <div className="inline-flex items-center gap-1">
                    {phone ? (
                      phone.startsWith("http") ? (
                        <span className="inline-flex items-center rounded bg-blue-500/10 px-1.5 py-0.5 text-[11px] font-medium text-blue-600">
                          已绑定
                        </span>
                      ) : (
                        <button
                          type="button"
                          className="inline-flex items-center rounded bg-blue-500/10 px-1.5 py-0.5 text-[11px] font-medium text-blue-600 cursor-pointer hover:bg-blue-500/20"
                          onClick={() => {
                            navigator.clipboard.writeText(phone);
                            showSuccessToast("手机号已复制");
                          }}
                          title={phone}
                        >
                          {phone}
                        </button>
                      )
                    ) : (
                      <span className="text-[11px] text-muted-foreground">
                        未设置
                      </span>
                    )}
                    {phone?.startsWith("http") && (
                      <button
                        type="button"
                        onClick={() => void handleRefreshSms(account.id)}
                        className={cn(
                          "rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-primary",
                          smsCodes[account.id]?.loading && "animate-spin",
                        )}
                        title="获取短信验证码"
                        disabled={smsCodes[account.id]?.loading}
                      >
                        <LuRefreshCw className="h-3 w-3" />
                      </button>
                    )}
                  </div>
                );
              },
              size: 120,
            },
          ]
        : []),
      {
        accessorKey: "platform",
        header: "平台",
        cell: ({ row }) => (
          <PlatformBadge platform={row.getValue("platform") || "unknown"} />
        ),
        size: 80,
      },
      // 归属 / 环境列仅管理员可见
      ...(isAdmin
        ? [
            {
              accessorKey: "owner_name",
              header: "归属",
              cell: ({ row }: { row: { original: BwbrowserAccount } }) => (
                <span className="text-xs text-muted-foreground">
                  {row.original.owner_name || "-"}
                </span>
              ),
              size: 70,
            },
            {
              accessorKey: "env_uuid",
              header: "环境",
              cell: ({ row }: { row: { original: BwbrowserAccount } }) => {
                const account = row.original;
                const envUuid = account.env_uuid;
                return (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <button
                        type="button"
                        className={cn(
                          "inline-flex items-center gap-0.5 rounded-full px-1.5 py-0.5 text-[11px] font-medium transition-colors",
                          envUuid
                            ? "bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20 dark:text-emerald-400"
                            : "bg-muted text-muted-foreground hover:bg-muted/80",
                        )}
                        title={envUuid ? `已绑定: ${envUuid}` : "点击绑定环境"}
                      >
                        {envUuid ? "已绑定" : "设置"}
                        <LuChevronDown className="h-3 w-3 opacity-60" />
                      </button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="start" className="w-40">
                      {envUuid ? (
                        <>
                          <DropdownMenuItem
                            onClick={() => void handleEditBoundEnv(account)}
                          >
                            <LuPencil className="h-4 w-4 mr-2" />
                            编辑本环境
                          </DropdownMenuItem>
                          {onNavigateToEnvManagement ? (
                            <DropdownMenuItem
                              onClick={() => onNavigateToEnvManagement(envUuid)}
                            >
                              <LuMonitor className="h-4 w-4 mr-2" />
                              在环境中心打开
                            </DropdownMenuItem>
                          ) : null}
                          <DropdownMenuItem
                            onClick={() => void handleOpenEnvDialog(account)}
                          >
                            <LuMonitor className="h-4 w-4 mr-2" />
                            重新绑定
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            className="text-destructive focus:text-destructive"
                            onClick={() => void handleDeleteBoundEnv(account)}
                          >
                            <LuTrash2 className="h-4 w-4 mr-2" />
                            删除环境
                          </DropdownMenuItem>
                        </>
                      ) : (
                        <DropdownMenuItem
                          onClick={() => void handleOpenEnvDialog(account)}
                        >
                          <LuMonitor className="h-4 w-4 mr-2" />
                          绑定环境
                        </DropdownMenuItem>
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                );
              },
              size: 60,
            },
          ]
        : []),
      {
        accessorKey: "proxy_node",
        header: "节点IP",
        cell: ({ row }) => {
          const account = row.original;
          const proxy = account.proxy_node;
          const display = proxy
            ? (() => {
                // VLESS/Trojan: 显示类型+host
                if (
                  proxy.startsWith("vless://") ||
                  proxy.startsWith("trojan://")
                ) {
                  const scheme = proxy.split("://")[0];
                  try {
                    const u = new URL(proxy);
                    return `${scheme}:${u.hostname}:${u.port}`;
                  } catch {
                    return proxy.slice(0, 20);
                  }
                }
                // HTTP/SOCKS5: type:host:port 或旧格式 host:port
                const parts = proxy.split(":");
                const hostIdx = ["http", "socks5"].includes(
                  parts[0].toLowerCase(),
                )
                  ? 1
                  : 0;
                return parts.length >= hostIdx + 2
                  ? `${parts[hostIdx]}:${parts[hostIdx + 1]}`
                  : proxy;
              })()
            : null;
          return (
            <button
              type="button"
              onClick={() => void handleOpenProxyDialog([account])}
              className="flex min-w-0 items-center gap-1 text-left"
              title={proxy ? `${proxy}（点击更换）` : "点击设置代理"}
            >
              {account.proxy_country && (
                <span className="shrink-0 rounded bg-muted px-1 py-px text-[10px] font-medium text-muted-foreground truncate max-w-[70px]">
                  {account.proxy_country}
                </span>
              )}
              {display ? (
                <span className="font-mono text-xs text-blue-600 dark:text-blue-400 hover:underline truncate max-w-[220px] block">
                  {display}
                </span>
              ) : (
                <span className="text-xs text-muted-foreground hover:text-foreground">
                  未设置
                </span>
              )}
            </button>
          );
        },
        size: 260,
      },
      {
        accessorKey: "timezone",
        header: "时区",
        cell: ({ row }) => {
          const account = row.original;
          const tz = account.timezone ?? account.proxy_timezone ?? "";
          return (
            <span
              className="text-xs text-muted-foreground truncate"
              title={tz || undefined}
            >
              {tz || "-"}
            </span>
          );
        },
        size: 140,
      },
      {
        accessorKey: "cookie_updated_at",
        header: ({ column }) => (
          <button
            type="button"
            className="flex items-center gap-1 text-xs font-medium"
            onClick={() => column.toggleSorting(column.getIsSorted() === "asc")}
          >
            Cookie更新
            <LuChevronsUpDown className="h-3 w-3 opacity-50" />
          </button>
        ),
        cell: ({ row }) => {
          const account = row.original;
          const val = row.getValue("cookie_updated_at") as string | undefined;
          return (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
                  title={val ? `更新于 ${formatDateShort(val)}` : undefined}
                >
                  <LuClock className="h-3 w-3" />
                  {val ? formatDateNoYear(val) : "-"}
                  <LuChevronDown className="h-3 w-3 opacity-50" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-44">
                <DropdownMenuItem
                  onClick={async () => {
                    setCookieViewAccount(account);
                    setCookieViewLoading(true);
                    setCookieViewData(null);
                    setCookieViewOpen(true);
                    try {
                      const data = await invoke(
                        "bwbrowser_get_account_cookies",
                        {
                          accountId: account.id,
                        },
                      );
                      setCookieViewData(data);
                    } catch (err) {
                      const msg =
                        err instanceof Error ? err.message : String(err);
                      showErrorToast(`获取服务器 Cookie 失败: ${msg}`);
                    } finally {
                      setCookieViewLoading(false);
                    }
                  }}
                >
                  <LuClock className="h-4 w-4 mr-2" />
                  查看服务器Cookie
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={async () => {
                    if (
                      !confirm(
                        `确定删除账号「${account.account_name || account.id}」的本地 Cookie 和浏览数据？`,
                      )
                    )
                      return;
                    try {
                      await invoke("bwbrowser_delete_local_cookies", {
                        accountName: account.account_name || String(account.id),
                        accountId: account.id,
                      });
                      showSuccessToast("本地 Cookie 已清除");
                    } catch (err) {
                      const msg =
                        err instanceof Error ? err.message : String(err);
                      showErrorToast(`删除失败: ${msg}`);
                    }
                  }}
                  className="text-amber-600 dark:text-amber-400"
                >
                  <LuTrash2 className="h-4 w-4 mr-2" />
                  删除本地Cookie和数据
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={async () => {
                    if (
                      !confirm(
                        `确定删除账号「${account.account_name || account.id}」的服务器 Cookie？`,
                      )
                    )
                      return;
                    try {
                      await invoke("bwbrowser_delete_cloud_cookies", {
                        accountId: account.id,
                      });
                      showSuccessToast("服务器 Cookie 已删除");
                      // 刷新列表
                      setIsRefreshing(true);
                      setTimeout(() => setIsRefreshing(false), 500);
                    } catch (err) {
                      const msg =
                        err instanceof Error ? err.message : String(err);
                      showErrorToast(`删除失败: ${msg}`);
                    }
                  }}
                  className="text-destructive focus:text-destructive"
                >
                  <LuTrash2 className="h-4 w-4 mr-2" />
                  删除服务器Cookie
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          );
        },
        size: 130,
      },
      {
        id: "remark_tags",
        header: "备注/标签/编辑",
        cell: ({ row }) => {
          const account = row.original;
          const tagsArr = Array.isArray(account.tags)
            ? account.tags
            : account.tags
              ? account.tags.split(",").map((x: string) => x.trim())
              : [];
          const remark = account.remark ?? "";
          return (
            <div className="flex items-start gap-1.5">
              <div className="flex min-w-0 max-w-[140px] flex-col gap-0.5">
                {tagsArr.length > 0 &&
                  tagsArr.map((tag) => (
                    <span
                      key={tag}
                      className="truncate text-[11px] text-purple-600 dark:text-purple-400 leading-tight"
                      title={tag}
                    >
                      {tag}
                    </span>
                  ))}
                {remark && (
                  <span
                    className="truncate text-[11px] text-amber-600 dark:text-amber-500 leading-tight"
                    title={remark}
                  >
                    {remark}
                  </span>
                )}
              </div>
              <button
                type="button"
                onClick={() => handleOpenEdit(row.original)}
                className="text-muted-foreground hover:text-foreground transition-colors"
                title="编辑"
              >
                <LuPencil className="h-3 w-3" />
              </button>
            </div>
          );
        },
        size: 140,
        enableSorting: false,
      },
      {
        id: "actions",
        header: "操作",
        cell: ({ row }) => {
          const account = row.original;
          const isLaunching = launchingIds.has(account.id);
          const geoPending = geoPendingIds.has(account.id);
          const progress = launchProgress[account.id];
          const pct = progress ? Math.floor(progress.pct) : 0;
          return (
            <div className="flex items-center gap-1">
              <Button
                variant="ghost"
                size="sm"
                className={cn(
                  "h-6 max-w-[180px] px-2",
                  account.launch_allowed === false &&
                    "opacity-40 cursor-not-allowed",
                )}
                onClick={() => void handleLaunchAccount(account)}
                disabled={
                  isLaunching || geoPending || account.launch_allowed === false
                }
                title={
                  account.launch_allowed === false
                    ? "超出当前套餐的启动配额，无法启动"
                    : geoPending
                      ? "正在获取时区信息"
                      : "启动浏览器"
                }
              >
                {geoPending ? (
                  <>
                    <LuLoaderCircle className="h-3.5 w-3.5 shrink-0 animate-spin text-amber-500" />
                    <span className="ml-1 truncate text-xs text-amber-600">
                      正在获取时区信息
                    </span>
                  </>
                ) : isLaunching ? (
                  progress && progress.pct >= 100 ? (
                    <LuCheck className="h-3.5 w-3.5 shrink-0 text-emerald-500" />
                  ) : (
                    <LuLoaderCircle className="h-3.5 w-3.5 shrink-0 animate-spin text-emerald-500" />
                  )
                ) : (
                  <LuPlay className="h-3.5 w-3.5 shrink-0 text-emerald-500" />
                )}
                {!geoPending &&
                  (isLaunching && progress && progress.pct < 100 ? (
                    <span className="ml-1 truncate text-xs tabular-nums text-emerald-600">
                      {progress.label} {pct}%
                    </span>
                  ) : (
                    <span className="ml-1 text-xs">
                      {isLaunching
                        ? progress
                          ? "已启动"
                          : "启动中"
                        : "启动浏览器"}
                    </span>
                  ))}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="h-6 shrink-0 px-2"
                onClick={() => void handleRepairAccount(account)}
                title="删除本地数据和服务器 Cookie（无法登录时使用）"
              >
                <LuWrench className="h-3.5 w-3.5 shrink-0" />
                <span className="ml-1 text-xs">一键修复</span>
              </Button>
            </div>
          );
        },
        size: 260,
        enableSorting: false,
      },
    ];
    return cols;
  }, [
    handleLaunchAccount,
    launchingIds,
    launchProgress,
    geoPendingIds,
    handleOpenProxyDialog,
    smsCodes,
    handleOpenEnvDialog,
    twoFACodes,
    handleRefreshSms,
    handleRefresh2FA,
    canView2FACode,
    canViewSMSCode,
    canViewPassword,
    showPasswordIds,
    copiedPasswordId,
    onNavigateToEnvManagement,
    handleOpenEdit,
    handleEditBoundEnv,
    handleDeleteBoundEnv,
    healthStates,
    healthLoadingIds,
    isManager,
    isSuperAdmin,
    handleCheckHealth,
    handleRepairAccount,
    serverSort,
    setServerSort,
  ]);

  const table = useReactTable({
    data: accounts,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    onSortingChange: setSorting,
    onColumnVisibilityChange: handleColumnVisibilityChange,
    onRowSelectionChange: setSelectedIds,
    state: {
      sorting,
      rowSelection: selectedIds,
      columnVisibility,
    },
    getRowId: (row) => String(row.id),
    manualPagination: true,
    pageCount: totalPages,
  });

  // 当前勾选的账号（勾选状态跨页保留，但批量操作只作用于当前列表里存在的行）
  const selectedAccounts = useMemo(
    () => accounts.filter((a) => selectedIds[String(a.id)]),
    [accounts, selectedIds],
  );
  const selectedCount = selectedAccounts.length;

  const content = (
    <>
      {/* ========== Header 顶部栏 ========== */}
      <header className="flex items-center justify-between border-b border-border bg-background/10 px-4 py-2.5 backdrop-blur-2xl">
        <div className="flex items-center gap-3">
          <button
            type="button"
            className="flex items-center gap-2 rounded-md px-2 py-1 transition-colors hover:bg-accent/50"
            onClick={() => {
              const toastId = "vps-launch-progress-cloud";
              let unlistenToast: (() => void) | undefined;

              showLaunchProgressToast(toastId, "正在启动爆文库...", 0, "0%");

              // 监听后端真实启动进度，替代模拟动画
              import("@tauri-apps/api/event")
                .then(({ listen }) =>
                  listen<{ pct: number; label: string }>(
                    "vps-launch-progress",
                    (event) => {
                      const pct = Math.min(100, Math.max(0, event.payload.pct));
                      updateLaunchProgressToast(
                        toastId,
                        event.payload.label || "正在启动爆文库...",
                        pct,
                        `${Math.floor(pct)}%`,
                      );
                    },
                  ),
                )
                .then((fn) => {
                  unlistenToast = fn;
                })
                .catch((e) => {
                  console.warn("failed to listen vps launch progress", e);
                });

              invoke("bwbrowser_open_vps_login")
                .then(() => {
                  updateLaunchProgressToast(
                    toastId,
                    "正在启动爆文库...",
                    100,
                    "100%",
                  );
                  setTimeout(() => {
                    showLaunchSuccessToast(toastId, "爆文库已启动");
                  }, 300);
                })
                .catch((err: unknown) => {
                  dismissToast(toastId);
                  const msg = err instanceof Error ? err.message : String(err);
                  showErrorToast(`启动失败: ${msg}`);
                })
                .finally(() => {
                  unlistenToast?.();
                });
            }}
            onContextMenu={handleVpsLogoContextMenu}
            title="左键打开爆文库，右键设置代理"
          >
            <span className="flex h-7 w-7 items-center justify-center rounded-md bg-primary/10 text-primary">
              <LuUsers className="h-4 w-4" />
            </span>
            <span className="text-sm font-semibold">云端账号管理</span>
          </button>
          <span className="text-xs text-muted-foreground">
            · 共 {total} 个账号
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => void handleRefresh()}
            className="h-8 gap-1.5 px-3 text-xs"
            disabled={isRefreshing || isLoading}
          >
            {isRefreshing ? (
              <LuLoaderCircle className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <LuRefreshCw className="h-3.5 w-3.5" />
            )}
            刷新
          </Button>
          <Button
            variant="default"
            size="sm"
            onClick={handleAddAccount}
            className="h-8 gap-1.5 px-3 text-xs"
          >
            <LuUserPlus className="h-3.5 w-3.5" />
            添加账号
          </Button>
          <div
            className="flex h-8 cursor-pointer select-none items-center gap-1.5 rounded-md border border-border bg-card px-2 text-xs text-muted-foreground"
            title="开启后自动填入登录账号和密码，关闭后不再自动输入"
          >
            自动输入
            <AnimatedSwitch
              data-slot="autofill-toggle"
              checked={autofillEnabled}
              onCheckedChange={(v) => void handleAutofillToggle(v)}
            />
          </div>
        </div>
      </header>

      {/* ========== Tab 导航 ========== */}
      <div className="flex shrink-0 items-center gap-1 border-b border-border bg-background/50 px-4 py-1.5">
        <button
          type="button"
          onClick={() => setActiveTab("accounts")}
          className={cn(
            "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
            activeTab === "accounts"
              ? "bg-primary text-primary-foreground"
              : "text-muted-foreground hover:bg-muted hover:text-foreground",
          )}
        >
          <LuUsers className="h-3.5 w-3.5" />
          全部账号
        </button>
      </div>

      {/* ========== 公司切换栏（仅超级管理员可见） ========== */}
      {isSuperAdmin && companies.length > 0 && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-4 py-1.5">
          <span className="shrink-0 text-xs font-medium text-muted-foreground">
            切换公司
          </span>
          <Select
            value={
              selectedCompanyId !== null ? String(selectedCompanyId) : "my"
            }
            onValueChange={(val) =>
              setSelectedCompanyId(val === "my" ? null : Number(val))
            }
          >
            <SelectTrigger className="h-7 w-auto min-w-[140px] max-w-[260px] text-xs">
              <SelectValue placeholder="选择公司" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="my">我的公司</SelectItem>
              {companies.map((company) => (
                <SelectItem key={company.id} value={String(company.id)}>
                  {company.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      {/* ========== 赛道筛选栏（仅管理及以上角色可见） ========== */}
      {(isManager || isSuperAdmin) && (
        <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border bg-background/50 px-4 py-1.5">
          <button
            type="button"
            onClick={() => setSectorFilter(null)}
            className={cn(
              "shrink-0 rounded-full px-2.5 py-0.5 text-xs font-medium transition-colors border",
              sectorFilter === null
                ? "bg-primary text-primary-foreground border-primary"
                : "bg-background text-muted-foreground hover:bg-muted border-border",
            )}
          >
            全部赛道
          </button>
          {allSectors.map((s) => (
            <button
              type="button"
              key={s}
              onClick={() => setSectorFilter(sectorFilter === s ? null : s)}
              className={cn(
                "shrink-0 rounded-full px-2.5 py-0.5 text-xs font-medium transition-colors border",
                sectorFilter === s
                  ? "bg-indigo-600 text-white border-indigo-600"
                  : "bg-background text-muted-foreground hover:bg-muted border-border",
              )}
            >
              {s}
            </button>
          ))}
          {allSectors.length === 0 && (
            <span className="shrink-0 text-xs text-muted-foreground/60">
              暂无赛道
            </span>
          )}
        </div>
      )}

      {/* ========== 人员过滤栏（仅管理及以上角色可见） ========== */}
      {(isManager || isSuperAdmin) && (
        <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border bg-background/50 px-4 py-1.5">
          <button
            type="button"
            onClick={() => handleOwnerChange(null)}
            className={cn(
              "shrink-0 rounded-full px-2.5 py-0.5 text-xs font-medium transition-colors border",
              ownerFilter === null
                ? "bg-primary text-primary-foreground border-primary"
                : "bg-background text-muted-foreground hover:bg-muted border-border",
            )}
          >
            全部人员
          </button>
          {visibleOwners.map((owner) => (
            <button
              type="button"
              key={owner.owner_id}
              title={owner.statusStyle.title}
              onClick={() => handleOwnerChange(owner.owner_id)}
              className={cn(
                "shrink-0 rounded-full px-2.5 py-0.5 text-xs font-medium transition-colors border",
                ownerFilter === owner.owner_id
                  ? "bg-primary text-primary-foreground border-primary"
                  : `${owner.statusStyle.bg} ${owner.statusStyle.color} ${owner.statusStyle.border} hover:opacity-80`,
              )}
            >
              {owner.owner_name}
            </button>
          ))}
          {usersLoading && visibleOwners.length === 0 && (
            <span className="shrink-0 text-xs text-muted-foreground/60">
              加载中...
            </span>
          )}
        </div>
      )}

      {/* ========== 平台过滤栏 ========== */}
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-b border-border bg-muted/30 px-4 py-1.5">
        {platformStats.map((stat) => (
          <button
            type="button"
            key={stat.platform}
            onClick={() => handlePlatformChange(stat.platform)}
            className={cn(
              "shrink-0 flex items-center gap-1 rounded-full px-2.5 py-0.5 text-xs transition-colors",
              platformFilter === stat.platform
                ? "bg-blue-500 text-white font-medium"
                : "bg-blue-500/10 text-blue-600 hover:bg-blue-500/20 dark:text-blue-400",
            )}
          >
            {stat.platform === "all" ? (
              <LuGlobe className="h-3 w-3" />
            ) : (
              <span className="w-1.5 h-1.5 rounded-full bg-current opacity-70" />
            )}
            <span>{getPlatformLabel(stat.platform)}</span>
            <span
              className={cn(
                "font-medium",
                platformFilter === stat.platform
                  ? "text-white/90"
                  : "opacity-70",
              )}
            >
              {stat.count}
            </span>
          </button>
        ))}
      </div>

      {/* ========== 搜索栏 ========== */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-background/30 px-4 py-2">
        <div className="relative flex-1 max-w-md">
          <LuSearch className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            placeholder="搜索账号名称、登录账号、归属人、备注..."
            value={keyword}
            onChange={handleSearchChange}
            className="h-8 pl-8 text-xs"
          />
        </div>
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <span>共 {total} 条</span>
        </div>
        <div className="flex items-center gap-1">
          <Button
            size="sm"
            variant="outline"
            className="h-7 gap-1 text-xs"
            onClick={() => openAllForPlatform()}
            disabled={batchLaunching}
            title={
              "本地启动浏览器打开当前平台（" +
              getPlatformLabel(platformFilter) +
              "）的全部账号"
            }
          >
            {batchLaunching ? (
              <LuLoaderCircle className="h-3.5 w-3.5 animate-spin" />
            ) : null}
            {platformFilter === "all"
              ? "打开全部账号"
              : `打开全部 ${getPlatformLabel(platformFilter)}`}
          </Button>
          <Button
            size="sm"
            variant="outline"
            className="h-7 gap-1 text-xs"
            onClick={() => checkAllHealthForPlatform()}
            disabled={healthBatchRunning}
            title="检测当前平台全部账号的 Cookie 健康，结果由各账号行的心跳图标颜色显示"
          >
            {healthBatchRunning ? (
              <LuLoaderCircle className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <LuHeartPulse className="h-3.5 w-3.5" />
            )}
            一键检测健康
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                size="sm"
                variant="outline"
                className="h-7 gap-1 text-xs"
                title="显示/隐藏表格列"
              >
                <LuSettings2 className="h-3.5 w-3.5" />
                列设置
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-40">
              {[
                ["proxy_node", "节点IP"],
                ["env_uuid", "环境"],
                ["timezone", "时区"],
                ["cookie_updated_at", "Cookie更新"],
              ].map(([key, label]) => (
                <DropdownMenuItem
                  key={key}
                  className="gap-2"
                  onSelect={(e) => e.preventDefault()}
                  onClick={() =>
                    handleColumnVisibilityChange((v) => ({
                      ...v,
                      [key]: !v[key],
                    }))
                  }
                >
                  <Checkbox
                    checked={!!columnVisibility[key]}
                    className="h-3.5 w-3.5"
                  />
                  {label}
                </DropdownMenuItem>
              ))}
              <DropdownMenuSeparator />
              <div className="px-2 py-1.5 text-xs font-medium text-muted-foreground">
                文字大小
              </div>
              <div className="flex gap-1 px-2 pb-2">
                {(
                  [
                    ["small", "小"],
                    ["medium", "中"],
                    ["large", "大"],
                  ] as const
                ).map(([size, label]) => (
                  <button
                    key={size}
                    type="button"
                    className={cn(
                      "flex-1 rounded-md border px-2 py-1 text-xs transition-colors",
                      cloudFontSize === size
                        ? "border-primary bg-primary text-primary-foreground"
                        : "border-border hover:bg-accent",
                    )}
                    onClick={() => handleFontSizeChange(size)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      {/* ========== 表格 ========== */}
      <div className="min-h-0 flex-1 overflow-auto">
        {error && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <div className="text-sm text-red-500 mb-2">加载失败</div>
            <div className="text-xs text-muted-foreground mb-4">{error}</div>
            <Button
              size="sm"
              variant="outline"
              onClick={() => void handleRefresh()}
            >
              重新加载
            </Button>
          </div>
        )}
        {isLoading && accounts.length === 0 && !error && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <LuLoaderCircle className="h-8 w-8 animate-spin text-muted-foreground mb-2" />
            <div className="text-sm text-muted-foreground">加载中...</div>
          </div>
        )}
        {!isLoading && !error && accounts.length === 0 && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <LuUsers className="h-12 w-12 text-muted-foreground/30 mb-2" />
            <div className="text-sm text-muted-foreground">暂无账号</div>
            <div className="text-xs text-muted-foreground/70 mt-1">
              点击右上角「添加账号」创建第一个账号
            </div>
          </div>
        )}
        {accounts.length > 0 && (
          <Table>
            <TableHeader className="sticky top-0 bg-background/80 backdrop-blur">
              {table.getHeaderGroups().map((headerGroup) => (
                <TableRow key={headerGroup.id}>
                  {headerGroup.headers.map((header) => (
                    <TableHead
                      key={header.id}
                      style={{ width: header.getSize() }}
                      className="h-8 text-xs font-medium"
                    >
                      {header.isPlaceholder
                        ? null
                        : flexRender(
                            header.column.columnDef.header,
                            header.getContext(),
                          )}
                    </TableHead>
                  ))}
                </TableRow>
              ))}
            </TableHeader>
            <TableBody>
              {table.getRowModel().rows.map((row) => (
                <TableRow
                  key={row.id}
                  data-state={row.getIsSelected() && "selected"}
                  className={cn(
                    row.original.status === 1 &&
                      "text-emerald-600 dark:text-emerald-400",
                  )}
                >
                  {row.getVisibleCells().map((cell) => (
                    <TableCell
                      key={cell.id}
                      style={{ width: cell.column.getSize() }}
                      className="py-1.5"
                    >
                      {flexRender(
                        cell.column.columnDef.cell,
                        cell.getContext(),
                      )}
                    </TableCell>
                  ))}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>

      {/* ========== 底部分页栏 ========== */}
      <div className="flex shrink-0 items-center justify-between border-t border-border bg-background/50 px-4 py-2">
        <div className="text-xs text-muted-foreground">
          已选择 {selectedCount} 项
        </div>
        {selectedCount > 0 && (
          <div className="flex items-center gap-1.5">
            <Button
              variant="default"
              size="sm"
              className="h-7 gap-1 px-3 text-xs"
              disabled={isSavingProxy}
              onClick={() => void handleOpenProxyDialog(selectedAccounts)}
            >
              <LuNetwork className="h-3.5 w-3.5" />
              设置代理
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 px-3 text-xs"
              disabled={isSavingProxy}
              onClick={() => void handleBatchClearProxy(selectedAccounts)}
            >
              <LuX className="h-3.5 w-3.5" />
              清除代理
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 px-3 text-xs"
              disabled={isSettingOwner}
              onClick={() => {
                setOwnerDialogTargets(selectedAccounts);
                setOwnerDialogOpen(true);
              }}
            >
              <LuUserCheck className="h-3.5 w-3.5" />
              设置归属人
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 px-3 text-xs"
              disabled={isSavingRemark}
              onClick={() => {
                setRemarkDialogTargets(selectedAccounts);
                setRemarkText("");
                setRemarkDialogOpen(true);
              }}
            >
              <LuStickyNote className="h-3.5 w-3.5" />
              添加备注
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs"
              onClick={() => setSelectedIds({})}
            >
              取消选择
            </Button>
          </div>
        )}
        <div className="flex items-center gap-1">
          <Select
            value={String(pageSize)}
            onValueChange={(v) => setPageSize(Number(v))}
          >
            <SelectTrigger className="h-7 w-[86px] text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {[20, 50, 100, 200, 500, 1000].map((n) => (
                <SelectItem key={n} value={String(n)}>
                  {n} 条/页
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            variant="outline"
            size="sm"
            className="h-7 w-7 p-0"
            disabled={currentPage <= 1 || isLoading}
            onClick={() => handlePageChange(currentPage - 1)}
          >
            <LuChevronLeft className="h-3.5 w-3.5" />
          </Button>
          <span className="px-2 text-xs text-muted-foreground">
            {currentPage} / {totalPages || 1}
          </span>
          <Button
            variant="outline"
            size="sm"
            className="h-7 w-7 p-0"
            disabled={currentPage >= totalPages || isLoading}
            onClick={() => handlePageChange(currentPage + 1)}
          >
            <LuChevronRight className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

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
                handleOpenVpsProxyDialog();
              }}
            >
              <LuNetwork className="h-3.5 w-3.5 text-muted-foreground" />
              设置代理
            </button>
            <div className="my-1 h-px bg-border" />
            <button
              type="button"
              className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-xs text-destructive transition-colors hover:bg-accent focus:bg-accent focus:outline-none"
              onClick={() => {
                setVpsContextMenuOpen(false);
                handleDeleteVpsData();
              }}
            >
              <LuTrash2 className="h-3.5 w-3.5" />
              删除本地数据
            </button>
          </div>,
          document.body,
        )}
    </>
  );

  const ownerDialog = (
    <Dialog
      open={ownerDialogOpen}
      onOpenChange={(open) => !open && setOwnerDialogOpen(false)}
    >
      <DialogContent className="flex max-h-[70vh] w-[420px] max-w-[95vw] flex-col overflow-hidden p-0">
        <div className="flex shrink-0 items-center justify-between border-b px-4 py-3">
          <div className="flex items-center gap-2">
            <LuUserCheck className="h-4 w-4 text-primary" />
            <span className="text-sm font-semibold">
              设置归属人 -
              {ownerDialogTargets.length === 1
                ? ` ${ownerDialogTargets[0]?.account_name}`
                : ` ${ownerDialogTargets.length} 个账号`}
            </span>
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={() => setOwnerDialogOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {users.length === 0 ? (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              {usersLoading ? "成员列表加载中..." : "当前公司暂无成员列表"}
            </div>
          ) : (
            users.map((u) => (
              <button
                type="button"
                key={u.id}
                disabled={isSettingOwner}
                className="flex w-full items-center justify-between rounded-md px-3 py-2 text-left text-xs hover:bg-muted disabled:opacity-50"
                onClick={() => void handleBatchSetOwner(ownerDialogTargets, u)}
              >
                <span className="font-medium">
                  {u.real_name || u.username || `成员#${u.id}`}
                </span>
                {u.username && u.real_name && (
                  <span className="text-muted-foreground">{u.username}</span>
                )}
              </button>
            ))
          )}
        </div>
      </DialogContent>
    </Dialog>
  );

  const remarkDialog = (
    <Dialog
      open={remarkDialogOpen}
      onOpenChange={(open) => !open && setRemarkDialogOpen(false)}
    >
      <DialogContent className="flex w-[420px] max-w-[95vw] flex-col overflow-hidden p-0">
        <div className="flex shrink-0 items-center justify-between border-b px-4 py-3">
          <div className="flex items-center gap-2">
            <LuStickyNote className="h-4 w-4 text-primary" />
            <span className="text-sm font-semibold">
              添加备注 -
              {remarkDialogTargets.length === 1
                ? ` ${remarkDialogTargets[0]?.account_name}`
                : ` ${remarkDialogTargets.length} 个账号`}
            </span>
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={() => setRemarkDialogOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </Button>
        </div>
        <div className="flex flex-col gap-3 p-4">
          <Textarea
            value={remarkText}
            onChange={(e) => setRemarkText(e.target.value)}
            placeholder="输入备注内容"
            rows={4}
            disabled={isSavingRemark}
          />
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Checkbox
              checked={remarkAppend}
              onCheckedChange={(checked) => setRemarkAppend(checked === true)}
              disabled={isSavingRemark}
            />
            <span>追加到原备注（不勾选则覆盖）</span>
          </div>
          <Button
            size="sm"
            disabled={isSavingRemark || !remarkText.trim()}
            onClick={() => void handleBatchSetRemark()}
          >
            {isSavingRemark ? (
              <LuLoaderCircle className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <LuCheck className="h-3.5 w-3.5" />
            )}
            保存备注
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );

  const proxyDialog = (
    <Dialog
      open={proxyDialogOpen}
      onOpenChange={(open) => !open && setProxyDialogOpen(false)}
    >
      <DialogContent className="flex max-h-[92vh] w-[880px] max-w-[95vw] flex-col overflow-hidden p-0">
        {/* 标题栏 */}
        <div className="flex shrink-0 items-center justify-between border-b px-4 py-3">
          <div className="flex items-center gap-2">
            <LuNetwork className="h-4 w-4 text-primary" />
            <span className="text-sm font-semibold">
              设置代理 -
              {proxyDialogTargets.length === 1
                ? ` ${proxyDialogTargets[0]?.account_name}` +
                  (proxyDialogTargets[0]?.device_id?.trim()
                    ? `（设备编号 ${proxyDialogTargets[0]?.device_id?.trim()}）`
                    : "")
                : ` ${proxyDialogTargets.length} 个账号`}
            </span>
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={() => setProxyDialogOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </Button>
        </div>

        {/* 同步云端代理提示 */}
        {proxyDialogLoading && (
          <div className="flex items-center justify-center gap-2 border-b px-4 py-2 text-xs text-muted-foreground">
            <span className="inline-block h-3 w-3 animate-spin rounded-full border-2 border-muted-foreground/30 border-t-muted-foreground" />
            正在同步云端代理列表...
          </div>
        )}

        {/* 设备 IP 映射提示（来自本地 SRC-IP-CIDR 清单，仅展示，不写入账号数据） */}
        {proxyDialogTargets.length === 1 && (
          <div className="flex shrink-0 items-center gap-2 border-b bg-accent/40 px-4 py-2 text-xs">
            <span className="text-muted-foreground">设备IP映射:</span>
            {srcIpRuleHint ? (
              <button
                type="button"
                title="点击复制"
                className="flex items-center gap-1 font-mono text-foreground hover:text-primary"
                onClick={() => {
                  navigator.clipboard.writeText(srcIpRuleHint);
                  showSuccessToast("已复制");
                }}
              >
                {srcIpRuleHint}
                <LuCopy className="h-3 w-3" />
              </button>
            ) : (
              <span className="text-muted-foreground">
                未匹配 (设备编号:{" "}
                {proxyDialogTargets[0]?.device_id?.trim() || "-"})
              </span>
            )}
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto h-6 px-2 text-xs text-muted-foreground"
              onClick={async () => {
                if (!srcIpRulesOpen) {
                  let text = loadSrcIpRulesText();
                  if (!text.trim()) {
                    // 本机没有 → 尝试从云端拉取（换电脑/清缓存场景）
                    try {
                      const remote = await invoke<string | null>(
                        "bwbrowser_get_user_setting",
                        { settingKey: "src_ip_rules" },
                      );
                      if (remote?.trim()) {
                        text = remote;
                        localStorage.setItem(SRC_IP_RULES_KEY, remote);
                      }
                    } catch {
                      // 未登录或网络失败：保持空，让用户手动粘贴
                    }
                  }
                  setSrcIpRulesText(text);
                }
                setSrcIpRulesOpen(!srcIpRulesOpen);
              }}
            >
              <LuWrench className="mr-1 h-3 w-3" />
              映射表
            </Button>
          </div>
        )}
        {srcIpRulesOpen && (
          <div className="shrink-0 border-b px-4 py-2">
            <textarea
              value={srcIpRulesText}
              onChange={(e) => setSrcIpRulesText(e.target.value)}
              placeholder="粘贴 SRC-IP-CIDR 清单，每行: SRC-IP-CIDR,192.168.168.5/32,us031-70.39.192.115"
              className="h-40 w-full resize-y rounded border bg-background p-2 font-mono text-xs"
            />
            <div className="mt-2 flex items-center gap-2">
              <Button
                size="sm"
                className="h-7 px-3 text-xs"
                onClick={async () => {
                  const text = srcIpRulesText;
                  localStorage.setItem(SRC_IP_RULES_KEY, text);
                  setSrcIpRuleHint(computeSrcIpRuleHint(proxyDialogTargets[0]));
                  setSrcIpRulesOpen(false);
                  try {
                    await invoke("bwbrowser_save_user_setting", {
                      settingKey: "src_ip_rules",
                      data: text,
                    });
                    showSuccessToast("映射表已保存并同步云端");
                  } catch {
                    showSuccessToast(
                      "映射表已保存（云端同步失败，仅本机生效）",
                    );
                  }
                }}
              >
                保存
              </Button>
              <Button
                variant="ghost"
                size="sm"
                className="h-7 px-3 text-xs"
                onClick={() => setSrcIpRulesOpen(false)}
              >
                取消
              </Button>
              <span className="text-xs text-muted-foreground">
                保存后同步到云端账号，换电脑登录可用
              </span>
            </div>
          </div>
        )}

        {/* 搜索栏 */}
        <div className="flex shrink-0 items-center gap-2 border-b px-4 py-2">
          <div className="relative flex-1">
            <LuSearch className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={proxySearchQuery}
              onChange={(e) => {
                setProxySearchQuery(e.target.value);
                setProxyPage(0);
              }}
              placeholder="搜索代理名称、IP、类型..."
              className="h-8 pl-7 text-xs"
            />
          </div>
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1 text-xs"
            onClick={() => void batchTestProxies(filteredProxies)}
            disabled={filteredProxies.length === 0}
          >
            <LuPlay className="h-3 w-3" />
            批量测试
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1 text-xs"
            onClick={() => void refreshCloudProxyList()}
          >
            <LuRefreshCw className="h-3 w-3" />
            刷新
          </Button>
          <Button
            size="sm"
            className="h-8 gap-1 text-xs"
            onClick={() => setCreateProxyDialogOpen(true)}
          >
            <LuPlus className="h-3 w-3" />
            创建节点
          </Button>
          <span className="shrink-0 text-xs text-muted-foreground">
            共 {filteredProxies.length} 个
          </span>
        </div>

        {/* 代理列表表格 */}
        <div className="min-h-[460px] flex-1 overflow-y-auto">
          {filteredProxies.length === 0 ? (
            <div className="p-8 text-center text-sm text-muted-foreground">
              暂无可用代理，请先在代理中心添加
            </div>
          ) : (
            <table className="w-full table-fixed">
              <thead className="sticky top-0 bg-muted/50 backdrop-blur-sm">
                <tr className="border-b">
                  <th className="w-8 px-3 py-2 text-left">
                    <Checkbox
                      checked={
                        proxyPageData.length > 0 &&
                        proxyPageData.every((p) => p.id === selectedProxyId)
                      }
                      onCheckedChange={() => {}}
                      className="pointer-events-none opacity-0"
                    />
                  </th>
                  <th className="w-[30%] px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    代理信息
                  </th>
                  <th className="w-[9%] px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    类型
                  </th>
                  <th className="w-[8%] px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    来源
                  </th>
                  <th className="w-[26%] px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    使用
                  </th>
                  <th className="w-[10%] px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    状态
                  </th>
                  <th className="w-[9%] px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    操作
                  </th>
                </tr>
              </thead>
              <tbody>
                {/* 不使用代理（直连）选项 */}
                <tr
                  onClick={() => setSelectedProxyId("__direct__")}
                  className={cn(
                    "cursor-pointer border-b transition-colors",
                    selectedProxyId === "__direct__"
                      ? "bg-primary/5"
                      : "hover:bg-muted/30",
                  )}
                >
                  <td className="px-3 py-2">
                    <Checkbox
                      checked={selectedProxyId === "__direct__"}
                      onCheckedChange={() => setSelectedProxyId("__direct__")}
                    />
                  </td>
                  <td className="px-2 py-2">
                    <div className="flex items-center gap-2">
                      <LuUnplug className="h-3.5 w-3.5 text-muted-foreground" />
                      <span className="text-xs font-medium">
                        不使用代理（直连）
                      </span>
                    </div>
                  </td>
                  <td className="px-2 py-2">
                    <span className="inline-flex rounded bg-muted px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground">
                      直连
                    </span>
                  </td>
                  <td className="px-2 py-2">
                    <span className="inline-flex rounded bg-muted px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground">
                      系统
                    </span>
                  </td>
                  <td className="px-2 py-2">
                    {selectedProxyId === "__direct__" && (
                      <LuCheck className="h-3.5 w-3.5 text-primary" />
                    )}
                  </td>
                </tr>
                {proxyPageData.map((proxy) => {
                  const isSelected = selectedProxyId === proxy.id;
                  return (
                    <tr
                      key={proxy.id}
                      onClick={() => setSelectedProxyId(proxy.id)}
                      className={cn(
                        "cursor-pointer border-b transition-colors",
                        isSelected ? "bg-primary/5" : "hover:bg-muted/30",
                      )}
                    >
                      <td className="px-3 py-2">
                        <Checkbox
                          checked={isSelected}
                          onCheckedChange={() => setSelectedProxyId(proxy.id)}
                        />
                      </td>
                      <td className="px-2 py-2">
                        <div className="flex flex-col">
                          <div className="flex min-w-0 items-center gap-1">
                            {proxy.country && (
                              <span className="inline-flex shrink-0 rounded bg-muted px-1 py-px text-[10px] font-medium text-muted-foreground">
                                {proxy.country}
                              </span>
                            )}
                            <span
                              className="truncate text-xs font-medium"
                              title={proxy.name}
                            >
                              {proxy.name}
                            </span>
                          </div>
                          <span
                            className="truncate font-mono text-[11px] text-muted-foreground"
                            title={`${proxy.proxy_settings.host}:${proxy.proxy_settings.port}`}
                          >
                            {proxy.proxy_settings.host}:
                            {proxy.proxy_settings.port}
                          </span>
                        </div>
                      </td>
                      <td className="px-2 py-2">
                        <span className="inline-flex rounded bg-muted px-1.5 py-0.5 text-[11px] font-medium">
                          {proxy.proxy_settings.proxy_type}
                        </span>
                      </td>
                      <td className="px-2 py-2">
                        {proxy.is_cloud_managed ? (
                          <span className="inline-flex rounded bg-green-500/10 px-1.5 py-0.5 text-[11px] font-medium text-green-600 dark:text-green-400">
                            云端
                          </span>
                        ) : (
                          <span className="inline-flex rounded bg-muted px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground">
                            本地
                          </span>
                        )}
                      </td>
                      <td className="px-2 py-2">
                        {(() => {
                          const t = proxy.proxy_settings.proxy_type || "http";
                          const key = /^(vless|trojan|ss)$/i.test(t)
                            ? proxy.proxy_settings.vless_uri || ""
                            : `${proxy.proxy_settings.host}:${proxy.proxy_settings.port}`;
                          const usageList = key
                            ? proxyUsage.get(key)
                            : undefined;
                          if (!usageList || usageList.length === 0) {
                            return (
                              <span className="text-[11px] text-muted-foreground/60">
                                未使用
                              </span>
                            );
                          }
                          // 列内只显示平台+数量；悬停 title 显示明细：手机编号 归属人 账号名 平台
                          const counts = new Map<string, number>();
                          const lines: string[] = [];
                          for (const u of usageList) {
                            const ps = platformShort(u.platform);
                            counts.set(ps, (counts.get(ps) ?? 0) + 1);
                            const owner =
                              u.owner_real_name || u.owner_name || "-";
                            const ownerFull =
                              u.owner_real_name && u.owner_name
                                ? `${owner}(${u.owner_name})`
                                : owner;
                            lines.push(
                              `${u.phone_id || "-"}  ${ownerFull}  ${u.account_name || "-"}  ${ps}`,
                            );
                          }
                          return (
                            <span
                              className="inline-block max-w-full cursor-help border-b border-dashed border-muted-foreground/40 text-[11px] leading-5"
                              title={lines.join("\n")}
                            >
                              {[...counts.entries()].map(([ps, n]) => (
                                <span
                                  key={ps}
                                  className="mr-2 whitespace-nowrap"
                                >
                                  <span className="font-medium">{ps}</span> {n}{" "}
                                  个
                                </span>
                              ))}
                            </span>
                          );
                        })()}
                      </td>
                      <td className="px-2 py-2">
                        {(() => {
                          const check = proxyCheckResults.get(proxy.id);
                          const checking = check?.checking ?? false;
                          const result = check?.result ?? null;
                          const error = check?.error ?? null;
                          if (checking) {
                            return (
                              <div className="flex items-center gap-1.5">
                                <span className="inline-block h-3 w-3 animate-spin rounded-full border border-muted-foreground/40 border-t-muted-foreground" />
                                <span className="text-[11px] text-muted-foreground">
                                  测试中
                                </span>
                              </div>
                            );
                          }
                          if (result?.is_valid) {
                            return (
                              <div className="flex flex-col gap-0.5">
                                <div className="flex items-center gap-1">
                                  <FiCheck className="h-3 w-3 text-success" />
                                  <span className="text-[11px] font-medium text-success">
                                    可达
                                  </span>
                                  {typeof result.latency_ms === "number" && (
                                    <span className="text-[10px] text-muted-foreground">
                                      {result.latency_ms}ms
                                    </span>
                                  )}
                                </div>
                                {result.ip && (
                                  <span className="font-mono text-[10px] text-muted-foreground">
                                    {result.ip}
                                  </span>
                                )}
                              </div>
                            );
                          }
                          if (result && !result.is_valid) {
                            return (
                              <button
                                type="button"
                                onClick={(e) => {
                                  e.stopPropagation();
                                  void testSingleProxy(proxy);
                                }}
                                className="flex items-start gap-1 text-left hover:opacity-80"
                                title={error ?? "不可达，点击重试"}
                              >
                                <FiX className="mt-0.5 h-3 w-3 text-destructive-text" />
                                <div className="flex flex-col">
                                  <span className="text-[11px] font-medium text-destructive-text">
                                    不可达
                                  </span>
                                  <span className="text-[10px] text-muted-foreground">
                                    点击重试
                                  </span>
                                </div>
                              </button>
                            );
                          }
                          return (
                            <button
                              type="button"
                              onClick={(e) => {
                                e.stopPropagation();
                                void testSingleProxy(proxy);
                              }}
                              className="inline-flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
                            >
                              <LuPlay className="h-3 w-3" />
                              测试
                            </button>
                          );
                        })()}
                      </td>
                      <td className="px-2 py-2">
                        {isSelected && (
                          <LuCheck className="h-3.5 w-3.5 text-primary" />
                        )}
                        {proxy.is_cloud_managed && (
                          <button
                            type="button"
                            onClick={async (e) => {
                              e.stopPropagation();
                              const numericId = Number(
                                proxy.id.replace(/^bwbrowser_/, ""),
                              );
                              if (!numericId) return;
                              const ok = await dialogConfirm(
                                `确定删除节点「${proxy.name}」吗？\n删除后使用该节点的账号将失去代理。`,
                                { title: "删除节点", kind: "warning" },
                              ).catch(() => false);
                              if (!ok) return;
                              try {
                                await invoke("bwbrowser_delete_proxy", {
                                  proxyId: numericId,
                                });
                                if (selectedProxyId === proxy.id) {
                                  setSelectedProxyId(null);
                                }
                                showSuccessToast("节点已删除");
                                void refreshCloudProxyList();
                              } catch (err) {
                                showErrorToast(
                                  `删除失败: ${translateBackendError(t, err)}`,
                                );
                              }
                            }}
                            title="删除该节点"
                            className="ml-1 inline-flex items-center text-[11px] text-muted-foreground hover:text-destructive"
                          >
                            <LuTrash2 className="h-3 w-3" />
                          </button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>

        {/* 底部分页 + 操作 */}
        <div className="flex shrink-0 items-center justify-between border-t px-4 py-2">
          <div className="flex items-center gap-3">
            <span className="text-xs text-muted-foreground">
              每页 {PROXY_PAGE_SIZE} 条
            </span>
            <div className="flex items-center gap-1">
              <Button
                variant="outline"
                size="sm"
                className="h-6 w-6 p-0"
                disabled={proxyCurrentPage === 0}
                onClick={() => setProxyPage(proxyCurrentPage - 1)}
              >
                <LuChevronLeft className="h-3 w-3" />
              </Button>
              <span className="px-1 text-xs text-muted-foreground">
                {proxyCurrentPage + 1} / {proxyTotalPages}
              </span>
              <Button
                variant="outline"
                size="sm"
                className="h-6 w-6 p-0"
                disabled={proxyCurrentPage >= proxyTotalPages - 1}
                onClick={() => setProxyPage(proxyCurrentPage + 1)}
              >
                <LuChevronRight className="h-3 w-3" />
              </Button>
            </div>
          </div>
          <div className="flex items-center gap-3">
            {selectedProxyId === "__direct__" && (
              <span className="text-xs text-muted-foreground">
                已选择：不使用代理（直连）
              </span>
            )}
            {selectedProxy && selectedProxyId !== "__direct__" && (
              <span className="text-xs text-muted-foreground">
                已选择：{selectedProxy.name}~{selectedProxy.proxy_settings.host}
              </span>
            )}
            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                className="h-7"
                onClick={() => setProxyDialogOpen(false)}
                disabled={isSavingProxy}
              >
                取消
              </Button>
              <Button
                size="sm"
                className="h-7"
                onClick={handleSaveAccountProxy}
                disabled={!selectedProxyId || isSavingProxy}
              >
                {isSavingProxy ? "保存中..." : "设置"}
              </Button>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );

  const createProxyDialog = (
    <ProxyFormDialog
      isOpen={createProxyDialogOpen}
      onClose={() => setCreateProxyDialogOpen(false)}
      onSave={handleCreateCloudProxy}
      saveLabel="创建到云端"
    />
  );

  const vpsProxyDialog = (
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
            共 {vpsFilteredProxies.length} 个
          </span>
        </div>

        <div className="max-h-[360px] overflow-y-auto">
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

          {vpsFilteredProxies.length === 0 ? (
            <div className="p-8 text-center text-sm text-muted-foreground">
              暂无可用代理，请先在代理中心添加
            </div>
          ) : (
            vpsFilteredProxies.map((p: StoredProxy) => (
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
  );

  const envDialog = (
    <Dialog
      open={envDialogOpen}
      onOpenChange={(open) => !open && setEnvDialogOpen(false)}
    >
      <DialogContent className="w-[600px] max-w-[90vw] p-0 overflow-hidden">
        <div className="flex items-center justify-between border-b px-4 py-3">
          <div className="flex items-center gap-2">
            <LuMonitor className="h-4 w-4 text-primary" />
            <span className="text-sm font-semibold">
              环境绑定 - {envDialogAccount?.account_name}
            </span>
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={() => setEnvDialogOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </Button>
        </div>
        <div className="max-h-[400px] overflow-y-auto p-3">
          {envLoading ? (
            <div className="flex items-center justify-center py-8">
              <LuLoaderCircle className="h-5 w-5 animate-spin text-muted-foreground" />
            </div>
          ) : cloudEnvs.length === 0 ? (
            <div className="p-6 text-center text-sm text-muted-foreground">
              暂无可用环境，请在环境中心创建
            </div>
          ) : (
            <table className="w-full">
              <thead className="sticky top-0 bg-muted/50">
                <tr className="border-b">
                  <th className="w-8 px-2 py-2"></th>
                  <th className="px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    环境名称
                  </th>
                  <th className="px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    浏览器
                  </th>
                  <th className="px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    UUID
                  </th>
                </tr>
              </thead>
              <tbody>
                {cloudEnvs.map((env) => (
                  <tr
                    key={env.env_uuid}
                    onClick={() => setSelectedEnvUuid(env.env_uuid)}
                    className={cn(
                      "cursor-pointer border-b transition-colors",
                      selectedEnvUuid === env.env_uuid
                        ? "bg-primary/5"
                        : "hover:bg-muted/30",
                    )}
                  >
                    <td className="px-2 py-2">
                      <Checkbox
                        checked={selectedEnvUuid === env.env_uuid}
                        onCheckedChange={() => setSelectedEnvUuid(env.env_uuid)}
                      />
                    </td>
                    <td className="px-2 py-2 text-xs font-medium">
                      {env.name}
                    </td>
                    <td className="px-2 py-2 text-[11px] text-muted-foreground">
                      {env.browser_type}
                    </td>
                    <td className="px-2 py-2 font-mono text-[11px] text-muted-foreground">
                      {env.env_uuid.slice(0, 8)}...
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
        <div className="flex items-center justify-between border-t px-4 py-2">
          <Button
            variant="ghost"
            size="sm"
            className="h-7"
            onClick={() => setSelectedEnvUuid(null)}
            disabled={!selectedEnvUuid || codeSaving}
          >
            清除绑定
          </Button>
          <div className="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-7"
              onClick={() => setEnvDialogOpen(false)}
              disabled={codeSaving}
            >
              取消
            </Button>
            <Button
              size="sm"
              className="h-7"
              onClick={() => void handleSaveEnvBinding()}
              disabled={codeSaving}
            >
              {codeSaving ? "保存中..." : "保存"}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );

  // 环境列下拉：编辑该账号已绑定环境的表单弹窗
  const boundEnvDialog = (
    <EnvFormDialog
      isOpen={boundEnvFormOpen}
      onClose={() => setBoundEnvFormOpen(false)}
      editingEnv={boundEnvTarget}
      onSaved={() => {
        setBoundEnvFormOpen(false);
        void refresh();
      }}
    />
  );

  // 编辑账号对话框
  const editAccountDialog = (
    <Dialog
      open={editDialogOpen}
      onOpenChange={(open) => !open && setEditDialogOpen(false)}
    >
      <DialogContent className="w-[700px] max-w-[90vw] p-0 overflow-hidden max-h-[85vh] flex flex-col">
        <div className="flex items-center justify-between border-b px-4 py-3 flex-shrink-0">
          <span className="text-sm font-semibold">
            编辑账号 - {editAccount?.account_name}
          </span>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={() => setEditDialogOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </Button>
        </div>
        <div className="space-y-4 p-4 overflow-y-auto">
          {editLoading ? (
            <div className="flex items-center justify-center py-12">
              <div className="h-6 w-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
            </div>
          ) : (
            <>
              {/* 基本信息 */}
              <div className="space-y-2">
                <div className="text-xs font-semibold text-muted-foreground">
                  基本信息
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <div className="space-y-1">
                    <div className="text-xs font-medium">手机编号</div>
                    <Input
                      value={editForm.device_id}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          device_id: e.target.value,
                        }))
                      }
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
                    />
                  </div>
                  <div className="space-y-1">
                    <div className="text-xs font-medium">账号名称</div>
                    <Input
                      value={editForm.account_name}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          account_name: e.target.value,
                        }))
                      }
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
                    />
                  </div>
                  {(editForm.platform === "youtube" ||
                    editForm.platform === "YouTube") && (
                    <div className="space-y-1">
                      <div className="text-xs font-medium">
                        ▶ YouTube Channel ID
                      </div>
                      <Input
                        value={editForm.channel_id}
                        onChange={(e) =>
                          setEditForm((p) => ({
                            ...p,
                            channel_id: e.target.value,
                          }))
                        }
                        placeholder="UC 开头的频道 ID"
                      />
                    </div>
                  )}
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <div className="space-y-1">
                    <div className="text-xs font-medium">平台</div>
                    <Input
                      value={editForm.platform}
                      disabled
                      className="bg-muted"
                    />
                  </div>
                  <div className="space-y-1">
                    <div className="text-xs font-medium">平台用户名</div>
                    <Input
                      value={editForm.pure_username}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          pure_username: e.target.value,
                        }))
                      }
                      placeholder="平台用户名"
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
                    />
                  </div>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <div className="space-y-1">
                    <div className="flex items-center justify-between">
                      <span className="text-xs font-medium">代理节点</span>
                      {(isManager || isSuperAdmin) && editAccount && (
                        <button
                          type="button"
                          onClick={() => handleOpenProxyDialog([editAccount])}
                          className="text-xs text-primary hover:underline"
                        >
                          设置代理
                        </button>
                      )}
                    </div>
                    <Input
                      value={editDetail?.proxy_node || "未设置"}
                      disabled
                      className="bg-muted"
                    />
                  </div>
                </div>
                <div className="space-y-1">
                  <div className="text-xs font-medium">登录账号</div>
                  <Input
                    value={editForm.login_account}
                    onChange={(e) =>
                      setEditForm((p) => ({
                        ...p,
                        login_account: e.target.value,
                      }))
                    }
                    disabled={!editPerms.canEditAll}
                    className={cn(
                      !editPerms.canEditAll && "bg-muted opacity-70",
                    )}
                  />
                </div>
                {editPerms.canViewPassword && (
                  <div className="space-y-1">
                    <div className="flex items-center justify-between">
                      <span className="text-xs font-medium">登录密码</span>
                      <div className="flex items-center gap-1">
                        {editForm.login_password && (
                          <button
                            type="button"
                            onClick={() => {
                              navigator.clipboard.writeText(
                                editForm.login_password,
                              );
                              showSuccessToast("密码已复制");
                            }}
                            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            title="复制密码"
                          >
                            <LuCopy className="h-3 w-3" />
                          </button>
                        )}
                        <button
                          type="button"
                          onClick={() => setShowEditPassword((v) => !v)}
                          className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                          title={showEditPassword ? "隐藏密码" : "查看密码"}
                        >
                          {showEditPassword ? (
                            <LuEyeOff className="h-3 w-3" />
                          ) : (
                            <LuEye className="h-3 w-3" />
                          )}
                        </button>
                      </div>
                    </div>
                    <Input
                      type={showEditPassword ? "text" : "password"}
                      value={editForm.login_password}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          login_password: e.target.value,
                        }))
                      }
                      placeholder="留空则不修改密码"
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
                    />
                  </div>
                )}
              </div>

              {/* 安全信息 */}
              <div className="space-y-2 pt-2 border-t">
                <div className="text-xs font-semibold text-muted-foreground">
                  安全信息
                </div>
                {editPerms.canEditAll && (
                  <div className="space-y-1">
                    <div className="text-xs font-medium">安全连接 (2FA)</div>
                    <Input
                      value={editForm.safe_link}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          safe_link: e.target.value,
                        }))
                      }
                      placeholder="2FA 验证链接"
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
                    />
                  </div>
                )}
                {editPerms.canEditAll && (
                  <div className="space-y-1">
                    <div className="text-xs font-medium">绑定手机号</div>
                    <Input
                      value={editForm.bind_phone}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          bind_phone: e.target.value,
                        }))
                      }
                      placeholder="短信验证手机号"
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
                    />
                  </div>
                )}
                {editPerms.canEditAll && (
                  <div className="space-y-1">
                    <div className="text-xs font-medium">备用邮箱</div>
                    <Input
                      type="email"
                      value={editForm.backup_email}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          backup_email: e.target.value,
                        }))
                      }
                      placeholder="备用邮箱地址"
                    />
                  </div>
                )}
                {editDetail && (
                  <div className="grid grid-cols-2 gap-3">
                    <div className="space-y-1">
                      <div className="text-xs font-medium">归属人</div>
                      {editPerms.canEditAll ? (
                        <select
                          value={editForm.owner_id || 0}
                          onChange={(e) =>
                            setEditForm((p) => ({
                              ...p,
                              owner_id: Number(e.target.value),
                            }))
                          }
                          className="w-full rounded-md border bg-transparent px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
                        >
                          <option value={0}>未指定</option>
                          {users.map((u) => (
                            <option key={u.id} value={u.id}>
                              {u.real_name || u.username || `用户${u.id}`}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <Input
                          value={editDetail.owner_name || "-"}
                          disabled
                          className="bg-muted"
                        />
                      )}
                    </div>
                  </div>
                )}
              </div>

              {/* 标签 */}
              <div className="space-y-2 pt-2 border-t">
                <div className="text-xs font-medium">标签</div>
                <div className="flex flex-wrap gap-1.5 min-h-[32px] p-2 rounded-md border bg-background">
                  {editForm.tags.map((tag, idx) => (
                    <span
                      key={idx}
                      className="inline-flex items-center gap-1 rounded-md bg-accent px-2 py-0.5 text-xs"
                    >
                      {tag}
                      <button
                        type="button"
                        onClick={() => {
                          setEditForm((p) => ({
                            ...p,
                            tags: p.tags.filter((_, i) => i !== idx),
                          }));
                        }}
                        className="text-muted-foreground hover:text-foreground"
                      >
                        <LuX className="h-3 w-3" />
                      </button>
                    </span>
                  ))}
                  <input
                    type="text"
                    value={editForm.tagInput}
                    onChange={(e) =>
                      setEditForm((p) => ({ ...p, tagInput: e.target.value }))
                    }
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === ",") {
                        e.preventDefault();
                        const val = editForm.tagInput.trim();
                        if (val && !editForm.tags.includes(val)) {
                          setEditForm((p) => ({
                            ...p,
                            tags: [...p.tags, val],
                            tagInput: "",
                          }));
                        } else {
                          setEditForm((p) => ({ ...p, tagInput: "" }));
                        }
                      }
                      if (
                        e.key === "Backspace" &&
                        !editForm.tagInput &&
                        editForm.tags.length > 0
                      ) {
                        setEditForm((p) => ({
                          ...p,
                          tags: p.tags.slice(0, -1),
                        }));
                      }
                    }}
                    placeholder="输入标签后回车添加"
                    className="flex-1 min-w-[100px] bg-transparent text-xs focus:outline-none"
                  />
                </div>
              </div>

              {/* 备注 */}
              <div className="space-y-2 pt-2 border-t">
                <div className="text-xs font-medium">备注</div>
                <textarea
                  value={editForm.remark}
                  onChange={(e) =>
                    setEditForm((p) => ({ ...p, remark: e.target.value }))
                  }
                  placeholder="备注信息"
                  rows={3}
                  className="w-full rounded-md border bg-transparent px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-ring resize-none"
                />
              </div>

              {/* 账号数据 */}
              {editDetail && (
                <div className="space-y-2 pt-2 border-t">
                  <div className="text-xs font-semibold text-muted-foreground">
                    账号数据
                  </div>
                  <div className="grid grid-cols-2 gap-2 text-xs">
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">粉丝数</span>
                      <span className="font-medium">
                        {editDetail.followers?.toLocaleString() ?? 0}
                      </span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">关注数</span>
                      <span className="font-medium">
                        {editDetail.likes?.toLocaleString() ?? 0}
                      </span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">视频数</span>
                      <span className="font-medium">
                        {editDetail.video_count?.toLocaleString() ?? 0}
                      </span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">总播放</span>
                      <span className="font-medium">
                        {editDetail.total_views?.toLocaleString() ?? 0}
                      </span>
                    </div>
                  </div>
                </div>
              )}

              {/* 更多信息 */}
              {editDetail && (
                <div className="space-y-2 pt-2 border-t">
                  <div className="text-xs font-semibold text-muted-foreground">
                    更多信息
                  </div>
                  <div className="grid grid-cols-2 gap-2 text-xs">
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">环境绑定</span>
                      <span
                        className={cn(
                          editDetail.env_uuid
                            ? "text-success"
                            : "text-muted-foreground",
                        )}
                      >
                        {editDetail.env_uuid ? "已绑定" : "未绑定"}
                      </span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">可见性</span>
                      <span>{editDetail.visibility || "-"}</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-muted-foreground">认证</span>
                      <span
                        className={cn(
                          editDetail.verified
                            ? "text-success"
                            : "text-muted-foreground",
                        )}
                      >
                        {editDetail.verified ? "已认证" : "未认证"}
                      </span>
                    </div>
                    <div className="col-span-2 flex justify-between">
                      <span className="text-muted-foreground">Cookie 更新</span>
                      <span>
                        {editDetail.cookie_updated_at
                          ? new Date(
                              editDetail.cookie_updated_at,
                            ).toLocaleString()
                          : "-"}
                      </span>
                    </div>
                    <div className="col-span-2 flex justify-between">
                      <span className="text-muted-foreground">指纹更新</span>
                      <span>
                        {editDetail.fingerprint_updated_at
                          ? new Date(
                              editDetail.fingerprint_updated_at,
                            ).toLocaleString()
                          : "-"}
                      </span>
                    </div>
                    <div className="col-span-2 flex justify-between">
                      <span className="text-muted-foreground">创建时间</span>
                      <span>
                        {editDetail.created_at
                          ? new Date(
                              Number(editDetail.created_at) * 1000,
                            ).toLocaleString()
                          : "-"}
                      </span>
                    </div>
                  </div>
                </div>
              )}
            </>
          )}
        </div>
        <div className="flex items-center justify-end gap-2 border-t px-4 py-3 flex-shrink-0">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setEditDialogOpen(false)}
          >
            取消
          </Button>
          <Button
            size="sm"
            disabled={editLoading || editSaving}
            onClick={async () => {
              if (!editAccount) return;
              setEditSaving(true);
              try {
                const tagsStr = editForm.tags.join(",");
                await invoke("bwbrowser_update_account_info", {
                  accountId: editAccount.id,
                  accountName: editForm.account_name.trim() || null,
                  loginAccount: editForm.login_account.trim() || null,
                  loginPassword: editForm.login_password.trim() || null,
                  remark: editForm.remark || null,
                  tags: tagsStr || null,
                  category: editForm.category.trim() || null,
                  pureUsername: editForm.pure_username.trim() || null,
                  phoneId: editPerms.canEditAll
                    ? editForm.device_id.trim() || null
                    : null,
                  bindPhone: editPerms.canEditAll
                    ? editForm.bind_phone.trim() || null
                    : null,
                  safeLink: editPerms.canEditAll
                    ? editForm.safe_link.trim() || null
                    : null,
                  backupEmail: editPerms.canEditAll
                    ? editForm.backup_email.trim() || null
                    : null,
                  ownerId: editPerms.canEditAll
                    ? editForm.owner_id || null
                    : null,
                  channelId: editForm.channel_id.trim() || null,
                });
                showSuccessToast("保存成功");
                setEditDialogOpen(false);
                void refresh();
              } catch (e) {
                showErrorToast(
                  `保存失败: ${e instanceof Error ? e.message : String(e)}`,
                );
              } finally {
                setEditSaving(false);
              }
            }}
          >
            {editSaving ? "保存中..." : "保存"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );

  // 添加账号对话框
  const addAccountDialog = (
    <Dialog
      open={addDialogOpen}
      onOpenChange={(open) => !open && setAddDialogOpen(false)}
    >
      <DialogContent className="w-[700px] max-w-[90vw] p-0 overflow-hidden max-h-[85vh] flex flex-col">
        <div className="flex items-center justify-between border-b px-4 py-3 flex-shrink-0">
          <span className="text-sm font-semibold">添加账号</span>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={() => setAddDialogOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </Button>
        </div>
        <div className="space-y-4 p-4 overflow-y-auto">
          {/* 基本信息 */}
          <div className="space-y-2">
            <div className="text-xs font-semibold text-muted-foreground">
              基本信息
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <div className="text-xs font-medium">手机编号</div>
                <Input
                  value={addForm.phone_id}
                  onChange={(e) =>
                    setAddForm((p) => ({ ...p, phone_id: e.target.value }))
                  }
                  placeholder="手机编号（选填）"
                  className="h-8 text-xs"
                />
              </div>
              <div className="space-y-1">
                <div className="text-xs font-medium">
                  账号名称 <span className="text-destructive">*</span>
                </div>
                <Input
                  value={addForm.account_name}
                  onChange={(e) =>
                    setAddForm((p) => ({
                      ...p,
                      account_name: e.target.value,
                    }))
                  }
                  placeholder="账号名称"
                  className="h-8 text-xs"
                />
              </div>
            </div>
            <div className="space-y-1">
              <div className="text-xs font-medium">选择平台</div>
              <Popover open={addPlatformOpen} onOpenChange={setAddPlatformOpen}>
                <PopoverTrigger asChild>
                  <Button
                    variant="outline"
                    role="combobox"
                    aria-expanded={addPlatformOpen}
                    className="h-8 w-full justify-between px-3 text-xs font-normal"
                  >
                    <span className="truncate">
                      {PLATFORM_LABELS[addForm.platform] ??
                        remotePlatformLabels[addForm.platform] ??
                        addForm.platform ??
                        "选择平台"}
                    </span>
                    <LuChevronsUpDown className="ml-2 h-3 w-3 shrink-0 opacity-50" />
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-[280px] p-0" align="start">
                  <Command>
                    <CommandInput
                      placeholder="搜索平台..."
                      className="h-8 text-xs"
                    />
                    <CommandList>
                      <CommandEmpty>未找到平台</CommandEmpty>
                      <CommandGroup>
                        {Object.entries({
                          ...PLATFORM_LABELS,
                          ...remotePlatformLabels,
                        })
                          .sort((a, b) =>
                            zhCollator.compare(String(a[1]), String(b[1])),
                          )
                          .map(([key, label]) => (
                            <CommandItem
                              key={key}
                              value={`${label} ${key}`}
                              onSelect={() => {
                                setAddForm((p) => ({ ...p, platform: key }));
                                setAddPlatformOpen(false);
                              }}
                              className="text-xs"
                            >
                              <LuCheck
                                className={cn(
                                  "mr-2 h-3 w-3",
                                  addForm.platform === key
                                    ? "opacity-100"
                                    : "opacity-0",
                                )}
                              />
                              {label}
                            </CommandItem>
                          ))}
                      </CommandGroup>
                    </CommandList>
                  </Command>
                </PopoverContent>
              </Popover>
            </div>
          </div>

          {/* 登录信息 */}
          <div className="space-y-2 pt-2 border-t">
            <div className="text-xs font-semibold text-muted-foreground">
              登录信息
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <div className="text-xs font-medium">登录账号</div>
                <Input
                  value={addForm.login_account}
                  onChange={(e) =>
                    setAddForm((p) => ({
                      ...p,
                      login_account: e.target.value,
                    }))
                  }
                  placeholder="登录账号"
                  className="h-8 text-xs"
                />
              </div>
              <div className="space-y-1">
                <div className="text-xs font-medium">登录密码</div>
                <Input
                  type="password"
                  value={addForm.login_password}
                  onChange={(e) =>
                    setAddForm((p) => ({
                      ...p,
                      login_password: e.target.value,
                    }))
                  }
                  placeholder="登录密码"
                  className="h-8 text-xs"
                />
              </div>
            </div>
          </div>

          {/* 安全信息 */}
          <div className="space-y-2 pt-2 border-t">
            <div className="text-xs font-semibold text-muted-foreground">
              安全信息
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <div className="text-xs font-medium">绑定手机号</div>
                <Input
                  value={addForm.bind_phone}
                  onChange={(e) =>
                    setAddForm((p) => ({ ...p, bind_phone: e.target.value }))
                  }
                  placeholder="短信验证手机号"
                  className="h-8 text-xs"
                />
              </div>
              <div className="space-y-1">
                <div className="text-xs font-medium">安全连接 (2FA)</div>
                <Input
                  value={addForm.safe_link}
                  onChange={(e) =>
                    setAddForm((p) => ({ ...p, safe_link: e.target.value }))
                  }
                  placeholder="2FA 验证链接"
                  className="h-8 text-xs"
                />
              </div>
            </div>
            <div className="space-y-1">
              <div className="text-xs font-medium">备用邮箱</div>
              <Input
                type="email"
                value={addForm.backup_email}
                onChange={(e) =>
                  setAddForm((p) => ({
                    ...p,
                    backup_email: e.target.value,
                  }))
                }
                placeholder="备用邮箱地址"
                className="h-8 text-xs"
              />
            </div>
            <div className="space-y-1">
              <div className="text-xs font-medium">归属人</div>
              <Popover open={addOwnerOpen} onOpenChange={setAddOwnerOpen}>
                <PopoverTrigger asChild>
                  <Button
                    variant="outline"
                    role="combobox"
                    aria-expanded={addOwnerOpen}
                    className="h-8 w-full justify-between px-3 text-xs font-normal"
                  >
                    <span className="truncate">
                      {(() => {
                        const u = users.find((x) => x.id === addForm.owner_id);
                        return u
                          ? u.real_name || u.username || `用户${u.id}`
                          : "未指定";
                      })()}
                    </span>
                    <LuChevronsUpDown className="ml-2 h-3 w-3 shrink-0 opacity-50" />
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-[280px] p-0" align="start">
                  <Command>
                    <CommandInput
                      placeholder="搜索归属人..."
                      className="h-8 text-xs"
                    />
                    <CommandList>
                      <CommandEmpty>未找到用户</CommandEmpty>
                      <CommandGroup>
                        <CommandItem
                          value="__none__ 未指定"
                          onSelect={() => {
                            setAddForm((p) => ({ ...p, owner_id: 0 }));
                            setAddOwnerOpen(false);
                          }}
                          className="text-xs"
                        >
                          <LuCheck
                            className={cn(
                              "mr-2 h-3 w-3",
                              addForm.owner_id === 0
                                ? "opacity-100"
                                : "opacity-0",
                            )}
                          />
                          未指定
                        </CommandItem>
                        {users.map((u) => {
                          const name =
                            u.real_name || u.username || `用户${u.id}`;
                          return (
                            <CommandItem
                              key={u.id}
                              value={`${name} ${u.username ?? ""}`}
                              onSelect={() => {
                                setAddForm((p) => ({ ...p, owner_id: u.id }));
                                setAddOwnerOpen(false);
                              }}
                              className="text-xs"
                            >
                              <LuCheck
                                className={cn(
                                  "mr-2 h-3 w-3",
                                  addForm.owner_id === u.id
                                    ? "opacity-100"
                                    : "opacity-0",
                                )}
                              />
                              {name}
                            </CommandItem>
                          );
                        })}
                      </CommandGroup>
                    </CommandList>
                  </Command>
                </PopoverContent>
              </Popover>
            </div>
          </div>

          {/* 备注 */}
          <div className="space-y-2 pt-2 border-t">
            <div className="text-xs font-medium">备注</div>
            <textarea
              value={addForm.remark}
              onChange={(e) =>
                setAddForm((p) => ({ ...p, remark: e.target.value }))
              }
              placeholder="备注信息"
              rows={3}
              className="w-full rounded-md border bg-transparent px-3 py-2 text-sm focus:outline-none focus:ring-1 focus:ring-ring resize-none"
            />
          </div>

          {/* 标签 */}
          <div className="space-y-2 pt-2 border-t">
            <div className="text-xs font-medium">标签</div>
            <div className="flex flex-wrap gap-1.5 min-h-[32px] p-2 rounded-md border bg-background">
              {addForm.tags.map((tag, idx) => (
                <span
                  key={idx}
                  className="inline-flex items-center gap-1 rounded-md bg-accent px-2 py-0.5 text-xs"
                >
                  {tag}
                  <button
                    type="button"
                    onClick={() => {
                      setAddForm((p) => ({
                        ...p,
                        tags: p.tags.filter((_, i) => i !== idx),
                      }));
                    }}
                    className="text-muted-foreground hover:text-foreground"
                  >
                    <LuX className="h-3 w-3" />
                  </button>
                </span>
              ))}
              <input
                type="text"
                value={addForm.tagInput}
                onChange={(e) =>
                  setAddForm((p) => ({ ...p, tagInput: e.target.value }))
                }
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === ",") {
                    e.preventDefault();
                    const val = addForm.tagInput.trim();
                    if (val && !addForm.tags.includes(val)) {
                      setAddForm((p) => ({
                        ...p,
                        tags: [...p.tags, val],
                        tagInput: "",
                      }));
                    }
                  }
                }}
                placeholder="输入标签后回车"
                className="flex-1 min-w-[80px] bg-transparent text-xs outline-none"
              />
            </div>
          </div>
        </div>
        <div className="flex items-center justify-end gap-2 border-t px-4 py-3 flex-shrink-0">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setAddDialogOpen(false)}
            disabled={addSaving}
          >
            取消
          </Button>
          <Button
            size="sm"
            onClick={() => void handleSaveNewAccount()}
            disabled={addSaving}
          >
            {addSaving ? "创建中..." : "创建"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );

  // 查看服务器 Cookie 对话框
  const cookieViewDialog = (
    <Dialog
      open={cookieViewOpen}
      onOpenChange={(o) => !o && setCookieViewOpen(false)}
    >
      <DialogContent className="w-[600px] max-w-[90vw] max-h-[80vh] p-0 overflow-hidden flex flex-col">
        <div className="px-4 py-3 border-b flex items-center justify-between flex-shrink-0">
          <div className="font-medium text-sm">
            服务器 Cookie -{" "}
            {cookieViewAccount?.account_name || cookieViewAccount?.id}
          </div>
          <button
            type="button"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setCookieViewOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </button>
        </div>
        <div className="flex-1 overflow-auto p-4">
          {cookieViewLoading ? (
            <div className="flex items-center justify-center py-12">
              <LuLoaderCircle className="h-6 w-6 animate-spin text-muted-foreground" />
              <span className="ml-2 text-sm text-muted-foreground">
                加载中...
              </span>
            </div>
          ) : cookieViewData?.cookie ? (
            <div className="space-y-3">
              <div className="flex items-center justify-between text-xs text-muted-foreground">
                <span>平台: {cookieViewData.platform || "-"}</span>
                <span>
                  更新时间:{" "}
                  {cookieViewData.cookie_updated_at
                    ? new Date(
                        cookieViewData.cookie_updated_at,
                      ).toLocaleString()
                    : "-"}
                </span>
              </div>
              <div className="border rounded-md p-3 bg-muted/30 font-mono text-xs max-h-[400px] overflow-auto whitespace-pre-wrap break-all">
                {typeof cookieViewData.cookie === "string"
                  ? cookieViewData.cookie
                  : JSON.stringify(cookieViewData.cookie, null, 2)}
              </div>
              <div className="flex justify-end gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    const text =
                      typeof cookieViewData.cookie === "string"
                        ? cookieViewData.cookie
                        : JSON.stringify(cookieViewData.cookie, null, 2);
                    navigator.clipboard.writeText(text);
                    showSuccessToast("已复制到剪贴板");
                  }}
                >
                  复制
                </Button>
              </div>
            </div>
          ) : (
            <div className="text-center py-12 text-sm text-muted-foreground">
              服务器暂无 Cookie 数据
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );

  const codeEditDialog = (
    <Dialog
      open={codeEditOpen}
      onOpenChange={(open) => !open && setCodeEditOpen(false)}
    >
      <DialogContent className="w-[500px] max-w-[90vw] p-0 overflow-hidden">
        <div className="flex items-center justify-between border-b px-4 py-3">
          <span className="text-sm font-semibold">
            编辑验证码链接 - {codeEditAccount?.account_name}
          </span>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0"
            onClick={() => setCodeEditOpen(false)}
          >
            <LuX className="h-4 w-4" />
          </Button>
        </div>
        <div className="space-y-3 p-4">
          <div className="space-y-1">
            <div className="text-xs font-medium">2FA 链接</div>
            <Input
              value={editSafeLink}
              onChange={(e) => setEditSafeLink(e.target.value)}
              placeholder={`${CLOUD_ROOT}/tk/2fa.php?secret=...`}
              className="h-8 text-xs"
            />
            <p className="text-[11px] text-muted-foreground">
              支持 ?secret=XXX 格式的 URL（本地 TOTP 生成）或返回验证码的 API
            </p>
          </div>
          <div className="space-y-1">
            <div className="text-xs font-medium">短信验证链接</div>
            <Input
              value={editBindPhone}
              onChange={(e) => setEditBindPhone(e.target.value)}
              placeholder="https://third-api.tkib.org/...?secret=..."
              className="h-8 text-xs"
            />
            <p className="text-[11px] text-muted-foreground">
              返回验证码的 API URL 或手机号
            </p>
          </div>
        </div>
        <div className="flex justify-end gap-2 border-t px-4 py-2">
          <Button
            variant="outline"
            size="sm"
            className="h-7"
            onClick={() => setCodeEditOpen(false)}
            disabled={codeSaving}
          >
            取消
          </Button>
          <Button
            size="sm"
            className="h-7"
            onClick={() => void handleSaveCodes()}
            disabled={codeSaving}
          >
            {codeSaving ? "保存中..." : "保存"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );

  if (embedded) {
    return (
      <div
        className={`bwbrowser-cloud-page cloud-font-${cloudFontSize} flex h-full min-h-0 w-full flex-col gap-0 overflow-hidden`}
      >
        {content}
        {proxyDialog}
        {ownerDialog}
        {remarkDialog}
        {createProxyDialog}
        {vpsProxyDialog}
        {envDialog}
        {boundEnvDialog}
        {codeEditDialog}
        {editAccountDialog}
        {addAccountDialog}
        {cookieViewDialog}
      </div>
    );
  }

  return (
    <>
      <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
        <DialogContent
          className={`bwbrowser-cloud-page cloud-font-${cloudFontSize} flex h-[92vh] w-[96vw] max-w-[96vw] flex-col gap-0 overflow-hidden p-0`}
        >
          {content}
        </DialogContent>
      </Dialog>
      {proxyDialog}
      {ownerDialog}
      {remarkDialog}
      {createProxyDialog}
      {vpsProxyDialog}
      {envDialog}
      {boundEnvDialog}
      {codeEditDialog}
      {editAccountDialog}
      {addAccountDialog}
      {cookieViewDialog}
    </>
  );
}
