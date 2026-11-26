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
  // 公司列表加载后 null 是否自动落到第一家公司。「我的公司」选项移除后，
  // 云端账号/环境/代理/用户管理页 null=旧「我的公司」语义，需自动填充；
  // remote-management 页 null=「全部公司」语义，必须保持 null
  autoPickFirstCompany = true,
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
        // 「我的公司」选项已移除：超管未选公司(null=旧「我的公司」语义)时自动
        // 落到公司列表第一家（服务端按 id ASC 返回，第一家=超管自己的公司），
        // 数据语义与原「我的公司」完全一致，仅 UI 上不再出现冗余选项。
        // autoPickFirstCompany=false 的页面（remote 全部公司语义）保持 null
        if (autoPickFirstCompany) {
          setSelectedCompanyIdState((cur) => {
            if (
              cur === null &&
              result.companies &&
              result.companies.length > 0
            ) {
              const first = result.companies[0].id;
              localStorage.setItem(STORAGE_KEY, String(first));
              return first;
            }
            return cur;
          });
        }
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      console.error("[BwbrowserCompany] 获取公司列表失败:", e);
    } finally {
      setIsLoading(false);
    }
  }, [isLoggedIn, isSuperAdmin, autoPickFirstCompany]);

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
