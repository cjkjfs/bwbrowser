"use client";

import {
  type ColumnDef,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type SortingState,
  useReactTable,
} from "@tanstack/react-table";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import {
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
  LuLoaderCircle,
  LuMonitor,
  LuNetwork,
  LuPencil,
  LuPlay,
  LuRefreshCw,
  LuSearch,
  LuTag,
  LuTrash2,
  LuUnplug,
  LuUserPlus,
  LuUsers,
  LuX,
} from "react-icons/lu";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  type BwbrowserAccount,
  useBwbrowserAccounts,
} from "@/hooks/use-bwbrowser-accounts";
import { useProxyEvents } from "@/hooks/use-proxy-events";
import { translateBackendError } from "@/lib/backend-errors";
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
import type { StoredProxy } from "@/types";

// ==================== 类型定义 ====================

interface BwbrowserUpdateProxyResult {
  success?: boolean;
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

// ==================== 工具函数 ====================
function getPlatformLabel(platform: string): string {
  return PLATFORM_LABELS[platform] || platform;
}

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

// ==================== 批量操作栏 ====================

function _BatchActionBar({
  selectedCount,
  onBatchStart,
  onBatchTestNode,
  onClearSelection,
}: {
  selectedCount: number;
  onBatchStart: () => void;
  onBatchTestNode: () => void;
  onClearSelection: () => void;
}) {
  if (selectedCount === 0) return null;

  return (
    <div className="absolute bottom-0 left-0 right-0 z-20 flex items-center justify-between border-t border-border bg-background/95 px-4 py-2 backdrop-blur-2xl">
      <div className="text-xs text-muted-foreground">
        已选择{" "}
        <span className="font-medium text-foreground">{selectedCount}</span>{" "}
        个账号
      </div>
      <div className="flex items-center gap-2">
        <Button
          variant="default"
          size="sm"
          onClick={onBatchStart}
          className="h-7 gap-1 px-3 text-xs"
        >
          <LuPlay className="h-3.5 w-3.5" />
          批量启动
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={onBatchTestNode}
          className="h-7 gap-1 px-3 text-xs"
        >
          <LuGlobe className="h-3.5 w-3.5" />
          批量测试节点
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={onClearSelection}
          className="h-7 gap-1 px-2 text-xs"
        >
          <LuX className="h-3.5 w-3.5" />
          取消选择
        </Button>
      </div>
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
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [activeTab, setActiveTab] = useState("accounts");

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
    platformFilter,
    ownerFilter,
    keyword,
    isManager,
    isSuperAdmin,
    canViewPassword,
    canView2FA,
    canViewSMS,
  } = useBwbrowserAccounts();

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

  const [isRefreshing, setIsRefreshing] = useState(false);
  const [showPasswordIds, setShowPasswordIds] = useState<Set<number>>(
    new Set(),
  );
  const [showEditPassword, setShowEditPassword] = useState(false);
  const [copiedPasswordId, setCopiedPasswordId] = useState<number | null>(null);
  const [proxyDialogOpen, setProxyDialogOpen] = useState(false);
  const [proxyDialogAccount, setProxyDialogAccount] =
    useState<BwbrowserAccount | null>(null);
  const [selectedProxyId, setSelectedProxyId] = useState<string | null>(null);
  const [isSavingProxy, setIsSavingProxy] = useState(false);
  const [proxySearchQuery, setProxySearchQuery] = useState("");
  const [proxyPage, setProxyPage] = useState(0);
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
    nickname: "",
    phone_id: "",
    bind_phone: "",
    safe_link: "",
    backup_email: "",
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

  // 用户账号同步
  const [syncingUsers, setSyncingUsers] = useState(false);

  // 2FA / 短信验证码状态
  const [twoFACodes, setTwoFACodes] = useState<
    Record<number, { code: string; loading: boolean; time: number }>
  >({});
  const [smsCodes, setSmsCodes] = useState<
    Record<number, { code: string; loading: boolean; time: number }>
  >({});

  const { storedProxies, loadProxies: reloadStoredProxies } = useProxyEvents();

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

  const handleSyncUsersToAccounts = useCallback(async () => {
    setSyncingUsers(true);
    try {
      const result = await invoke<{
        total_users: number;
        created: number;
        already_existed: number;
        failed: number;
        message: string;
      }>("bwbrowser_sync_users_to_accounts");
      showSuccessToast(result.message);
      // 同步完成后刷新账号列表
      void handleRefresh();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      showErrorToast(`同步失败: ${msg}`);
    } finally {
      setSyncingUsers(false);
    }
  }, [handleRefresh]);

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
  const handleOpenEnvDialog = useCallback(async (account: BwbrowserAccount) => {
    setEnvDialogAccount(account);
    setSelectedEnvUuid(account.env_uuid ?? null);
    setEnvDialogOpen(true);
    setEnvLoading(true);
    try {
      const envs = await invoke<
        { env_uuid: string; name: string; browser_type: string }[]
      >("bwbrowser_list_envs");
      setCloudEnvs(envs || []);
    } catch (e) {
      showErrorToast(
        `加载环境列表失败: ${e instanceof Error ? e.message : String(e)}`,
      );
      setCloudEnvs([]);
    } finally {
      setEnvLoading(false);
    }
  }, []);

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
        nickname: account.nickname ?? "",
        phone_id: account.phone_id ?? "",
        bind_phone: account.bind_phone ?? "",
        safe_link: account.safe_link ?? "",
        backup_email: account.backup_email ?? "",
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
          nickname: detail.account_nickname ?? detail.nickname ?? prev.nickname,
          phone_id: detail.phone_id ?? prev.phone_id,
          bind_phone: detail.bind_phone ?? prev.bind_phone,
          safe_link: detail.safe_link ?? prev.safe_link,
          backup_email: detail.backup_email ?? prev.backup_email,
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

  // 打开代理设置弹窗
  const handleOpenProxyDialog = useCallback(
    async (account: BwbrowserAccount) => {
      setProxyDialogAccount(account);
      setProxySearchQuery("");
      setProxyPage(0);

      // 先同步云端代理到本地，确保本地有最新代理列表
      try {
        await invoke("bwbrowser_sync_proxies_to_local");
        await reloadStoredProxies();
      } catch (e) {
        console.warn("同步云端代理到本地失败:", e);
      }

      // 同步后重新获取最新的 storedProxies
      let proxies = storedProxies;
      try {
        const stored = await invoke<StoredProxy[]>("get_stored_proxies");
        proxies = stored;
      } catch {
        // 用已有列表
      }

      // 如果账号已有代理，自动选中
      if (account.proxy_node) {
        const pn = account.proxy_node;
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
            parts.length >= hostIdx + 2 ? parseInt(parts[hostIdx + 1], 10) : 0;
          existing = proxies.find(
            (p) =>
              p.proxy_settings.host === host && p.proxy_settings.port === port,
          );
        }
        setSelectedProxyId(existing ? existing.id : null);
      } else {
        // 没有设置代理时，默认选中直连
        setSelectedProxyId("__direct__");
      }
      setProxyDialogOpen(true);
    },
    [storedProxies, reloadStoredProxies],
  );

  // 保存账号代理
  const handleSaveAccountProxy = useCallback(async () => {
    if (!proxyDialogAccount) return;

    // 直连模式：传空字符串清除代理
    if (selectedProxyId === "__direct__") {
      setIsSavingProxy(true);
      try {
        await invoke("bwbrowser_update_account_proxy", {
          accountId: proxyDialogAccount.id,
          accountName: proxyDialogAccount.account_name,
          proxyNode: "",
        });
        showSuccessToast("已清除代理，使用直连模式");
        setProxyDialogOpen(false);
        void refresh();
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        showErrorToast(`清除代理失败: ${msg}`);
      } finally {
        setIsSavingProxy(false);
      }
      return;
    }

    if (!selectedProxyId) return;

    const proxy = storedProxies.find((p) => p.id === selectedProxyId);
    if (!proxy) return;

    setIsSavingProxy(true);
    try {
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

      console.log("[bwbrowser_update_account_proxy] 调用参数:", {
        accountId: proxyDialogAccount.id,
        accountName: proxyDialogAccount.account_name,
        proxyNode,
        proxyType,
        host: ps.host,
        port: ps.port,
      });

      const result = (await invoke("bwbrowser_update_account_proxy", {
        accountId: proxyDialogAccount.id,
        accountName: proxyDialogAccount.account_name,
        proxyNode,
      })) as BwbrowserUpdateProxyResult | null;

      console.log("[bwbrowser_update_account_proxy] 返回结果:", result);

      const tz = result?.timezone;
      const lang = result?.language;
      if (tz) {
        showSuccessToast(
          `代理设置成功 | 时区: ${tz}${lang ? ` | 语言: ${lang}` : ""}`,
        );
      } else {
        showSuccessToast("代理设置成功（未获取到时区）");
      }
      setProxyDialogOpen(false);
      void refresh();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      showErrorToast(`设置代理失败: ${msg}`);
    } finally {
      setIsSavingProxy(false);
    }
  }, [proxyDialogAccount, selectedProxyId, storedProxies, refresh]);

  // 代理弹窗：搜索过滤 + 分页
  const filteredProxies = useMemo(() => {
    if (!proxySearchQuery.trim()) return storedProxies;
    const q = proxySearchQuery.toLowerCase();
    return storedProxies.filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.proxy_settings.host.includes(q) ||
        p.proxy_settings.proxy_type.toLowerCase().includes(q),
    );
  }, [storedProxies, proxySearchQuery]);

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
    ? storedProxies.find((p) => p.id === selectedProxyId)
    : null;

  // 启动浏览器：有环境用环境，没环境则创建 profile + 设置代理后启动
  const [launchingId, setLaunchingId] = useState<number | null>(null);

  const handleLaunchAccount = useCallback(
    async (account: BwbrowserAccount) => {
      setLaunchingId(account.id);
      const accountName = account.account_name || String(account.id);
      const toastId = `account-launch-${account.id}`;
      let pct = 0;
      let animationTimer: number | null = null;

      const animateProgress = () => {
        if (pct < 60) {
          pct += 4 + Math.random() * 3;
        } else if (pct < 85) {
          pct += 1.5 + Math.random() * 1.5;
        } else if (pct < 95) {
          pct += 0.3 + Math.random() * 0.4;
        }
        if (pct > 95) pct = 95;

        updateLaunchProgressToast(
          toastId,
          `正在启动 ${accountName}...`,
          pct,
          `${Math.floor(pct)}%`,
        );

        if (pct < 95) {
          animationTimer = window.setTimeout(animateProgress, 120);
        }
      };

      showLaunchProgressToast(toastId, `正在启动 ${accountName}...`, 0, "0%");
      animationTimer = window.setTimeout(animateProgress, 100);

      try {
        await invoke("bwbrowser_launch_account", {
          accountId: account.id,
          accountName: account.account_name || String(account.id),
          envUuid: account.env_uuid ?? null,
          proxyNode: account.proxy_node ?? null,
          platform: account.platform ?? null,
        });
        if (animationTimer) {
          clearTimeout(animationTimer);
          animationTimer = null;
        }
        updateLaunchProgressToast(
          toastId,
          `正在启动 ${accountName}...`,
          100,
          "100%",
        );
        setTimeout(() => {
          showLaunchSuccessToast(toastId, `${accountName} 已启动`);
        }, 300);
      } catch (err) {
        if (animationTimer) {
          clearTimeout(animationTimer);
          animationTimer = null;
        }
        dismissToast(toastId);
        const msg = err instanceof Error ? err.message : String(err);
        const translated = translateBackendError(t, msg);
        showErrorToast(translated);
      } finally {
        setLaunchingId(null);
      }
    },
    [t],
  );

  // 平台统计（优先用 summary 数据，兜底用当前页数据）
  // 当选了人员时，使用该人员的完整汇总统计数据
  const platformStats = useMemo(() => {
    // 有汇总数据时（包括选择了人员过滤的情况），直接用汇总数据
    if (summary?.platforms && summary.platforms.length > 0) {
      return [
        { platform: "all", count: summary.total ?? total },
        ...summary.platforms.map((p) => ({
          platform: p.platform,
          count: p.count,
        })),
      ];
    }
    // 兜底：用当前页数据统计
    const stats: Record<string, number> = {};
    const sourceAccounts =
      ownerFilter !== null
        ? accounts.filter((a) => a.owner_id === ownerFilter)
        : accounts;
    for (const acc of sourceAccounts) {
      const p = acc.platform || "unknown";
      stats[p] = (stats[p] || 0) + 1;
    }
    const result = Object.entries(stats)
      .sort((a, b) => b[1] - a[1])
      .map(([platform, count]) => ({ platform, count }));
    const allCount = ownerFilter !== null ? sourceAccounts.length : total;
    return [{ platform: "all", count: allCount }, ...result];
  }, [accounts, total, summary, ownerFilter]);

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
          };
        })
        .sort((a, b) => b.count - a.count);
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
      };
    });

    // 按账号数降序排列
    return result.sort((a, b) => b.count - a.count);
  }, [summary, users]);

  // 表格列定义
  const columns = useMemo<ColumnDef<BwbrowserAccount>[]>(() => {
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
        header: "手机编号",
        cell: ({ row }) => (
          <div className="font-mono text-xs text-muted-foreground">
            {row.getValue("device_id") || "-"}
          </div>
        ),
        size: 70,
      },
      {
        accessorKey: "account_name",
        header: ({ column }) => (
          <button
            type="button"
            className="flex items-center gap-1 text-xs font-medium"
            onClick={() => column.toggleSorting(column.getIsSorted() === "asc")}
          >
            账号昵称
            <LuChevronsUpDown className="h-3 w-3 opacity-50" />
          </button>
        ),
        cell: ({ row }) => {
          const account = row.original;
          return (
            <span className="text-xs font-medium">
              {account.account_name || account.nickname || "-"}
            </span>
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
      {
        accessorKey: "owner_name",
        header: "归属",
        cell: ({ row }) => (
          <span className="text-xs text-muted-foreground">
            {row.getValue("owner_name") || "-"}
          </span>
        ),
        size: 70,
      },
      {
        accessorKey: "env_uuid",
        header: "环境",
        cell: ({ row }) => {
          const account = row.original;
          const envUuid = account.env_uuid;
          return (
            <button
              type="button"
              onClick={() => {
                if (envUuid && onNavigateToEnvManagement) {
                  onNavigateToEnvManagement(envUuid);
                } else {
                  void handleOpenEnvDialog(account);
                }
              }}
              className={cn(
                "inline-flex items-center rounded-full px-1.5 py-0.5 text-[11px] font-medium transition-colors",
                envUuid
                  ? "bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20 dark:text-emerald-400"
                  : "bg-muted text-muted-foreground hover:bg-muted/80",
              )}
              title={envUuid ? `已绑定: ${envUuid} · 点击编辑` : "点击绑定环境"}
            >
              {envUuid ? "已绑定" : "设置"}
            </button>
          );
        },
        size: 60,
      },
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
              onClick={() => handleOpenProxyDialog(account)}
              className="text-left"
              title={proxy ? `${proxy}（点击更换）` : "点击设置代理"}
            >
              {display ? (
                <span className="font-mono text-xs text-blue-600 dark:text-blue-400 hover:underline truncate max-w-[110px] block">
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
        size: 110,
      },
      {
        accessorKey: "last_login_ip",
        header: "上次登录IP",
        cell: () => <span className="text-xs text-muted-foreground">-</span>,
        size: 100,
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
                >
                  <LuClock className="h-3 w-3" />
                  {val ? formatDateShort(val) : "-"}
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
          const tags = account.tags;
          const remark = account.remark;
          return (
            <div className="flex items-center gap-1.5">
              {tags && (
                <span
                  className="text-[11px] text-purple-600 dark:text-purple-400 truncate max-w-[60px]"
                  title={tags}
                >
                  {tags}
                </span>
              )}
              {remark && (
                <span
                  className="text-[11px] text-amber-600 dark:text-amber-500 truncate max-w-[60px]"
                  title={remark}
                >
                  {remark}
                </span>
              )}
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
          const isLaunching = launchingId === account.id;
          return (
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2"
              onClick={() => void handleLaunchAccount(account)}
              disabled={isLaunching}
              title="启动浏览器"
            >
              {isLaunching ? (
                <LuLoaderCircle className="h-3.5 w-3.5 animate-spin text-emerald-500" />
              ) : (
                <LuPlay className="h-3.5 w-3.5 text-emerald-500" />
              )}
              <span className="ml-1 text-xs">启动浏览器</span>
            </Button>
          );
        },
        size: 120,
        enableSorting: false,
      },
    ];
    return cols;
  }, [
    handleLaunchAccount,
    launchingId,
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
  ]);

  const table = useReactTable({
    data: accounts,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    onSortingChange: setSorting,
    onRowSelectionChange: setSelectedIds as any,
    state: {
      sorting,
      rowSelection: Object.fromEntries(
        Array.from(selectedIds).map((id) => [String(id), true]),
      ),
    },
    getRowId: (row) => String(row.id),
    manualPagination: true,
    pageCount: totalPages,
  });

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
              let pct = 0;
              let animationTimer: number | null = null;

              const animateProgress = () => {
                if (pct < 60) {
                  pct += 4 + Math.random() * 3;
                } else if (pct < 85) {
                  pct += 1.5 + Math.random() * 1.5;
                } else if (pct < 95) {
                  pct += 0.3 + Math.random() * 0.4;
                }
                if (pct > 95) pct = 95;

                updateLaunchProgressToast(
                  toastId,
                  "正在启动爆文库...",
                  pct,
                  `${Math.floor(pct)}%`,
                );

                if (pct < 95) {
                  animationTimer = window.setTimeout(animateProgress, 120);
                }
              };

              showLaunchProgressToast(toastId, "正在启动爆文库...", 0, "0%");
              animationTimer = window.setTimeout(animateProgress, 100);

              invoke("bwbrowser_open_vps_login")
                .then(() => {
                  if (animationTimer) {
                    clearTimeout(animationTimer);
                    animationTimer = null;
                  }
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
                  if (animationTimer) {
                    clearTimeout(animationTimer);
                    animationTimer = null;
                  }
                  dismissToast(toastId);
                  const msg = err instanceof Error ? err.message : String(err);
                  showErrorToast(`启动失败: ${msg}`);
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
            variant="outline"
            size="sm"
            onClick={() => void handleSyncUsersToAccounts()}
            className="h-8 gap-1.5 px-3 text-xs"
            disabled={syncingUsers || isLoading}
          >
            {syncingUsers ? (
              <LuRefreshCw className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <LuUsers className="h-3.5 w-3.5" />
            )}
            同步用户
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
        <button
          type="button"
          onClick={() => showErrorToast("分组功能开发中")}
          className={cn(
            "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
            activeTab === "groups"
              ? "bg-primary text-primary-foreground"
              : "text-muted-foreground hover:bg-muted hover:text-foreground",
          )}
        >
          <LuTag className="h-3.5 w-3.5" />
          账号分组
        </button>
      </div>

      {/* ========== 人员过滤栏（仅管理及以上角色可见） ========== */}
      {(isManager || isSuperAdmin) && (
        <div className="flex shrink-0 items-center gap-1.5 border-b border-border bg-background/50 px-4 py-1.5 overflow-x-auto">
          <span className="shrink-0 text-xs font-medium text-muted-foreground">
            账号管理
          </span>
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
            全部账号
          </button>
          {ownerStats.map((owner) => (
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
          {usersLoading && ownerStats.length === 0 && (
            <span className="shrink-0 text-xs text-muted-foreground/60">
              加载中...
            </span>
          )}
        </div>
      )}

      {/* ========== 平台过滤栏 ========== */}
      <div className="flex shrink-0 items-center gap-1 border-b border-border bg-muted/30 px-4 py-1.5 overflow-x-auto">
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
            placeholder="搜索账号昵称、登录账号、归属人..."
            value={keyword}
            onChange={handleSearchChange}
            className="h-8 pl-8 text-xs"
          />
        </div>
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <span>共 {total} 条</span>
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
          已选择 {selectedIds.size} 项
        </div>
        <div className="flex items-center gap-1">
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

  const proxyDialog = (
    <Dialog
      open={proxyDialogOpen}
      onOpenChange={(open) => !open && setProxyDialogOpen(false)}
    >
      <DialogContent className="w-[900px] max-w-[95vw] p-0 overflow-hidden">
        {/* 标题栏 */}
        <div className="flex items-center justify-between border-b px-4 py-3">
          <div className="flex items-center gap-2">
            <LuNetwork className="h-4 w-4 text-primary" />
            <span className="text-sm font-semibold">
              设置代理 - {proxyDialogAccount?.account_name}
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

        {/* 搜索栏 */}
        <div className="flex items-center gap-2 border-b px-4 py-2">
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
          <span className="shrink-0 text-xs text-muted-foreground">
            共 {filteredProxies.length} 个
          </span>
        </div>

        {/* 代理列表表格 */}
        <div className="max-h-[520px] overflow-y-auto">
          {filteredProxies.length === 0 ? (
            <div className="p-8 text-center text-sm text-muted-foreground">
              暂无可用代理，请先在代理中心添加
            </div>
          ) : (
            <table className="w-full">
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
                  <th className="px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    代理信息
                  </th>
                  <th className="w-16 px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    类型
                  </th>
                  <th className="w-20 px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    来源
                  </th>
                  <th className="w-12 px-2 py-2 text-left text-xs font-medium text-muted-foreground">
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
                          <span className="text-xs font-medium">
                            {proxy.name}
                          </span>
                          <span className="font-mono text-[11px] text-muted-foreground">
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
                        {isSelected && (
                          <LuCheck className="h-3.5 w-3.5 text-primary" />
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
        <div className="flex items-center justify-between border-t px-4 py-2">
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
                      value={editForm.phone_id}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          phone_id: e.target.value,
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
                    <div className="text-xs font-medium">账号昵称</div>
                    <Input
                      value={editForm.nickname}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          nickname: e.target.value,
                        }))
                      }
                      placeholder="账号昵称"
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
                    />
                  </div>
                </div>
                <div className="grid grid-cols-2 gap-3">
                  <div className="space-y-1">
                    <div className="text-xs font-medium">代理节点</div>
                    <Input
                      value={editDetail?.proxy_node || "未设置"}
                      disabled
                      className="bg-muted"
                    />
                  </div>
                  <div className="space-y-1">
                    <div className="text-xs font-medium">分类</div>
                    <Input
                      value={editForm.category}
                      onChange={(e) =>
                        setEditForm((p) => ({
                          ...p,
                          category: e.target.value,
                        }))
                      }
                      placeholder="账号分类"
                      disabled={!editPerms.canEditAll}
                      className={cn(
                        !editPerms.canEditAll && "bg-muted opacity-70",
                      )}
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
                {editPerms.canView2FA && (
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
                {editPerms.canViewSMS && (
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
                      <span className="text-muted-foreground">账号类型</span>
                      <span>{editDetail.account_type || "-"}</span>
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
                      <span className="text-muted-foreground">上次登录IP</span>
                      <span>{editDetail.last_login_ip || "-"}</span>
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
                  nickname: editForm.nickname.trim() || null,
                  phoneId: editPerms.canEditAll
                    ? editForm.phone_id.trim() || null
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
              <select
                value={addForm.platform}
                onChange={(e) =>
                  setAddForm((p) => ({ ...p, platform: e.target.value }))
                }
                className="w-full rounded-md border bg-transparent px-3 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
              >
                {Object.entries(PLATFORM_LABELS).map(([key, label]) => (
                  <option key={key} value={key}>
                    {label}
                  </option>
                ))}
              </select>
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
              <select
                value={addForm.owner_id}
                onChange={(e) =>
                  setAddForm((p) => ({
                    ...p,
                    owner_id: Number(e.target.value),
                  }))
                }
                className="w-full rounded-md border bg-transparent px-3 py-1.5 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
              >
                <option value={0}>未指定</option>
                {users.map((u) => (
                  <option key={u.id} value={u.id}>
                    {u.real_name || u.username || `用户${u.id}`}
                  </option>
                ))}
              </select>
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
              placeholder="https://yacm.xin/tk/2fa.php?secret=..."
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
      <div className="flex h-full min-h-0 w-full flex-col gap-0 overflow-hidden">
        {content}
        {proxyDialog}
        {vpsProxyDialog}
        {envDialog}
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
        <DialogContent className="flex max-h-[90vh] w-[1200px] max-w-[95vw] flex-col gap-0 overflow-hidden p-0">
          {content}
        </DialogContent>
      </Dialog>
      {proxyDialog}
      {vpsProxyDialog}
      {envDialog}
      {codeEditDialog}
      {editAccountDialog}
      {addAccountDialog}
      {cookieViewDialog}
    </>
  );
}
