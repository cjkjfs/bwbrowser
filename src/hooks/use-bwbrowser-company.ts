"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";

export interface BwbrowserCompany {
  id: number;
  name: string;
  code?: string;
}

interface CompanyListResult {
  success: boolean;
  companies?: BwbrowserCompany[];
  message?: string;
}

interface UseBwbrowserCompanyReturn {
  companies: BwbrowserCompany[];
  selectedCompanyId: number | null;
  setSelectedCompanyId: (id: number | null) => void;
  isLoading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
}

const STORAGE_KEY = "bwbrowser_selected_company_id";

export function useBwbrowserCompany(
  isSuperAdmin: boolean,
  isLoggedIn: boolean,
  // 权限解析完成前不判超管：挂载瞬间 isSuperAdmin 还是 false，提前清空会把
  // 超管恢复的公司选择连同 localStorage 一起删掉，导致刷新后过滤状态丢失
  permissionsReady = true,
): UseBwbrowserCompanyReturn {
  const [companies, setCompanies] = useState<BwbrowserCompany[]>([]);
  const [selectedCompanyId, setSelectedCompanyIdState] = useState<
    number | null
  >(() => {
    if (typeof window !== "undefined") {
      const saved = localStorage.getItem(STORAGE_KEY);
      return saved ? Number(saved) : null;
    }
    return null;
  });
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchCompanies = useCallback(async () => {
    if (!isLoggedIn || !isSuperAdmin) {
      setCompanies([]);
      return;
    }
    setIsLoading(true);
    setError(null);
    try {
      const result = await invoke<CompanyListResult>(
        "bwbrowser_list_companies",
      );
      if (result.success && result.companies) {
        setCompanies(result.companies);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      console.error("[BwbrowserCompany] 获取公司列表失败:", e);
    } finally {
      setIsLoading(false);
    }
  }, [isLoggedIn, isSuperAdmin]);

  const setSelectedCompanyId = useCallback((id: number | null) => {
    setSelectedCompanyIdState(id);
    if (id === null) {
      localStorage.removeItem(STORAGE_KEY);
    } else {
      localStorage.setItem(STORAGE_KEY, String(id));
    }
  }, []);

  useEffect(() => {
    void fetchCompanies();
  }, [fetchCompanies]);

  // 非超管或未登录时清空选中公司（等权限解析完成后再判，防止中间态误清）
  useEffect(() => {
    if (!permissionsReady) return;
    if (!isSuperAdmin || !isLoggedIn) {
      setSelectedCompanyId(null);
    }
  }, [isSuperAdmin, isLoggedIn, permissionsReady, setSelectedCompanyId]);

  return {
    companies,
    selectedCompanyId,
    setSelectedCompanyId,
    isLoading,
    error,
    refresh: fetchCompanies,
  };
}
