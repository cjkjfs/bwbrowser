"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useBwbrowserAuth } from "./use-bwbrowser-auth";

// ==================== 类型定义（与 Simprint API 对齐）====================

export interface BwbrowserAccount {
  id: number;
  account_name: string;
  login_account?: string;
  login_password?: string;
  device_id?: string;
  safe_link?: string;
  bind_phone?: string;
  sms_url?: string;
  pure_username?: string;
  nickname?: string;
  platform?: string;
  tags?: string;
  tags_list?: string[];
  category?: string;
  remark?: string;
  owner_id?: number;
  owner_name?: string;
  status?: number;
  followers?: number;
  likes?: number;
  video_count?: number;
  total_views?: number;
  visibility?: string;
  avatar_url?: string;
  verified?: boolean;
  cookie_updated_at?: string | null;
  last_logged_in_at?: string | null;
  proxy_node?: string;
  proxy_country?: string;
  proxy_city?: string;
  timezone?: string;
  proxy_timezone?: string;
  proxy_id?: number | null;
  fingerprint_updated_at?: string | null;
  bind_person_id?: number;
  account_type?: string;
  created_at?: string;
  updated_at?: string | null;
  env_uuid?: string;
  phone_id?: string;
  account_nickname?: string;

  backup_email?: string;
}

export interface PlatformStat {
  platform: string;
  count: number;
  followers?: number;
}

export interface OwnerStat {
  owner_id: number;
  owner_name: string;
  count: number;
}

export interface CloudUser {
  id: number;
  username?: string;
  real_name?: string;
  company_name?: string;
  sector?: string; // 视频解说赛道，逗号分隔多值
  leave_status?: string; // normal / on_leave / pending / missed / rest / unknown
}

export interface CloudUserListResult {
  success: boolean;
  users?: CloudUser[];
  message?: string;
}

export interface AccountSummary {
  success: boolean;
  total?: number;
  platforms?: PlatformStat[];
  owners?: OwnerStat[];
  company_name?: string;
  message?: string;
}

export interface AccountListResult {
  success: boolean;
  accounts: BwbrowserAccount[];
  total: number;
  can_view_password?: boolean;
  can_view_2fa?: boolean;
  can_view_sms?: boolean;
  is_manager?: boolean;
  is_super_admin?: boolean;
  message?: string;
}

interface UseBwbrowserAccountsReturn {
  accounts: BwbrowserAccount[];
  total: number;
  totalPages: number;
  isLoading: boolean;
  error: string | null;
  canViewPassword: boolean;
  canView2FA: boolean;
  canViewSMS: boolean;
  isManager: boolean;
  isSuperAdmin: boolean;
  // 汇总数据
  summary: AccountSummary | null;
  summaryLoading: boolean;
  // 用户列表（含考勤）
  users: CloudUser[];
  usersLoading: boolean;
  // 过滤
  platformFilter: string;
  ownerFilter: number | null; // null = 全部
  keyword: string;
  currentPage: number;
  pageSize: number;
  // 操作
  refresh: () => Promise<void>;
  refreshSummary: () => Promise<void>;
  fetchPage: (page: number, pageSize?: number) => Promise<void>;
  setPlatformFilter: (platform: string) => void;
  setOwnerFilter: (ownerId: number | null) => void;
  setKeyword: (keyword: string) => void;
}

/**
 * Bwbrowser 云端账号 Hook（对接 Simprint 账号 API）
 */
export function useBwbrowserAccounts(
  companyId?: number | null,
): UseBwbrowserAccountsReturn {
  const { isLoggedIn } = useBwbrowserAuth();
  const [accounts, setAccounts] = useState<BwbrowserAccount[]>([]);
  const [total, setTotal] = useState(0);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [canViewPassword, setCanViewPassword] = useState(false);
  const [canView2FA, setCanView2FA] = useState(false);
  const [canViewSMS, setCanViewSMS] = useState(false);
  const [isManager, setIsManager] = useState(false);
  const [isSuperAdmin, setIsSuperAdmin] = useState(false);
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  // 从 localStorage 读取持久化的筛选条件
  const [platformFilter, setPlatformFilterState] = useState(() => {
    if (typeof window !== "undefined") {
      return localStorage.getItem("bwbrowser_platform_filter") || "all";
    }
    return "all";
  });
  const [ownerFilter, setOwnerFilterState] = useState<number | null>(() => {
    if (typeof window !== "undefined") {
      const saved = localStorage.getItem("bwbrowser_owner_filter");
      return saved ? Number(saved) : null;
    }
    return null;
  });
  const [keyword, setKeywordState] = useState("");

  // 汇总数据
  const [summary, setSummary] = useState<AccountSummary | null>(null);
  const [summaryLoading, setSummaryLoading] = useState(false);

  // 用户列表（含考勤状态）
  const [users, setUsers] = useState<CloudUser[]>([]);
  const [usersLoading, setUsersLoading] = useState(false);

  const totalPages = Math.max(1, Math.ceil(total / pageSize));

  const fetchPage = useCallback(
    async (
      page: number,
      pSize = pageSize,
      platform = platformFilter,
      kw = keyword,
      ownerId = ownerFilter,
      cid = companyId ?? null,
    ) => {
      if (!isLoggedIn) return;

      setIsLoading(true);
      setError(null);

      try {
        const result = await invoke<AccountListResult>(
          "bwbrowser_list_accounts",
          {
            page,
            pageSize: pSize,
            platform: platform === "all" ? null : platform,
            keyword: kw || null,
            ownerId: ownerId ?? null,
            companyId: cid,
          },
        );

        if (!result.success) {
          throw new Error(result.message || "获取账号列表失败");
        }

        console.log("[BwbrowserAccounts] list_accounts 返回:", {
          can_view_password: result.can_view_password,
          can_view_2fa: result.can_view_2fa,
          can_view_sms: result.can_view_sms,
          is_manager: result.is_manager,
          is_super_admin: result.is_super_admin,
          accounts_count: result.accounts?.length ?? 0,
          first_account: result.accounts?.[0]
            ? {
                id: result.accounts[0].id,
                phone_id: result.accounts[0].phone_id,
                owner_id: result.accounts[0].owner_id,
                safe_link: result.accounts[0].safe_link,
                bind_phone: result.accounts[0].bind_phone,
              }
            : null,
        });

        setAccounts(result.accounts || []);
        setTotal(result.total || 0);
        setCurrentPage(page);
        setPageSize(pSize);
        if (typeof result.can_view_password === "boolean") {
          setCanViewPassword(result.can_view_password);
        }
        if (typeof result.can_view_2fa === "boolean") {
          setCanView2FA(result.can_view_2fa);
        }
        if (typeof result.can_view_sms === "boolean") {
          setCanViewSMS(result.can_view_sms);
        }
        if (typeof result.is_manager === "boolean") {
          setIsManager(result.is_manager);
        }
        if (typeof result.is_super_admin === "boolean") {
          setIsSuperAdmin(result.is_super_admin);
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        console.error("[BwbrowserAccounts] 获取账号列表失败:", e);
      } finally {
        setIsLoading(false);
      }
    },
    [isLoggedIn, pageSize, platformFilter, keyword, ownerFilter, companyId],
  );

  const fetchSummary = useCallback(
    async (ownerId: number | null = ownerFilter) => {
      if (!isLoggedIn) return;

      setSummaryLoading(true);
      try {
        const result = await invoke<AccountSummary>(
          "bwbrowser_get_account_summary",
          {
            ownerId: ownerId ?? null,
            companyId: companyId ?? null,
          },
        );
        if (result.success) {
          setSummary(result);
        }
      } catch (e) {
        console.error("[BwbrowserAccounts] 获取汇总失败:", e);
      } finally {
        setSummaryLoading(false);
      }
    },
    [isLoggedIn, ownerFilter, companyId],
  );

  const fetchUsers = useCallback(async () => {
    if (!isLoggedIn) return;

    setUsersLoading(true);
    try {
      const result = await invoke<CloudUserListResult>(
        "bwbrowser_list_cloud_users",
        {
          companyId: companyId ?? null,
        },
      );
      if (result.success && result.users) {
        setUsers(result.users);
      }
    } catch (e) {
      console.error("[BwbrowserAccounts] 获取用户列表失败:", e);
    } finally {
      setUsersLoading(false);
    }
  }, [isLoggedIn, companyId]);

  const refresh = useCallback(() => {
    return fetchPage(currentPage, pageSize);
  }, [fetchPage, currentPage, pageSize]);

  const refreshSummary = useCallback(() => {
    return fetchSummary(ownerFilter);
  }, [fetchSummary, ownerFilter]);

  const setPlatformFilter = useCallback((platform: string) => {
    setPlatformFilterState(platform);
    localStorage.setItem("bwbrowser_platform_filter", platform);
    setCurrentPage(1);
  }, []);

  const setOwnerFilter = useCallback((ownerId: number | null) => {
    setOwnerFilterState(ownerId);
    if (ownerId === null) {
      localStorage.removeItem("bwbrowser_owner_filter");
    } else {
      localStorage.setItem("bwbrowser_owner_filter", String(ownerId));
    }
    setCurrentPage(1);
  }, []);

  const setKeyword = useCallback((kw: string) => {
    setKeywordState(kw);
  }, []);

  // 登录状态变化时自动加载
  useEffect(() => {
    if (isLoggedIn) {
      void fetchPage(1, pageSize, platformFilter, keyword, ownerFilter);
      void fetchSummary(ownerFilter);
      void fetchUsers();
    } else {
      setAccounts([]);
      setTotal(0);
      setSummary(null);
      setUsers([]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    isLoggedIn,
    platformFilter,
    ownerFilter,
    fetchSummary,
    pageSize,
    keyword,
    fetchUsers,
    fetchPage,
  ]);

  // 过滤条件变化时自动刷新（带防抖）
  useEffect(() => {
    if (!isLoggedIn) return;

    const timer = setTimeout(() => {
      void fetchPage(1, pageSize, platformFilter, keyword, ownerFilter);
    }, 300);

    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [platformFilter, keyword, ownerFilter, pageSize, isLoggedIn, fetchPage]);

  return {
    accounts,
    total,
    totalPages,
    isLoading,
    error,
    canViewPassword,
    canView2FA,
    canViewSMS,
    isManager,
    isSuperAdmin,
    summary,
    summaryLoading,
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
  };
}
