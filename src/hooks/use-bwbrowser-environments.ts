"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useBwbrowserAuth } from "./use-bwbrowser-auth";

export interface BwbrowserEnvironment {
  id: string; // "bwbrowser_env_{env_uuid}"
  env_uuid: string;
  name: string;
  browser_type: string;
  status: string;
  description?: string;
  remark?: string;
  owner_name?: string;
  fingerprint_config?: Record<string, unknown>;
  start_urls?: string[];
  updated_at?: string;
  created_at?: string;
  is_cloud_managed: boolean;
}

interface UseBwbrowserEnvironmentsReturn {
  environments: BwbrowserEnvironment[];
  isLoading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  createEnv: (data: CreateEnvData) => Promise<string>;
  updateEnv: (envUuid: string, data: CreateEnvData) => Promise<void>;
  deleteEnv: (envUuid: string) => Promise<void>;
}

export interface CreateEnvData {
  name: string;
  description?: string;
  browser_type: string;
  fingerprint_config?: string; // JSON 字符串
  start_urls?: string; // JSON 数组字符串
  remark?: string;
  status?: string;
}

/**
 * Bwbrowser 云端环境 Hook
 * 云优先模式：所有环境数据来自云端
 */
export function useBwbrowserEnvironments(): UseBwbrowserEnvironmentsReturn {
  const { isLoggedIn } = useBwbrowserAuth();
  const [environments, setEnvironments] = useState<BwbrowserEnvironment[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadEnvironments = useCallback(async () => {
    if (!isLoggedIn) {
      setEnvironments([]);
      return;
    }
    setIsLoading(true);
    setError(null);
    try {
      const result = await invoke<BwbrowserEnvironment[]>(
        "bwbrowser_list_envs",
      );
      setEnvironments(result || []);
    } catch (err) {
      console.error("Failed to load bwbrowser environments:", err);
      setError(String(err));
    } finally {
      setIsLoading(false);
    }
  }, [isLoggedIn]);

  // 登录状态变化时自动加载
  useEffect(() => {
    void loadEnvironments();
  }, [loadEnvironments]);

  const createEnv = useCallback(
    async (data: CreateEnvData): Promise<string> => {
      // 生成一个 UUID 作为 env_uuid
      const envUuid = crypto.randomUUID();
      await invoke<string>("bwbrowser_sync_env", {
        envUuid,
        name: data.name,
        description: data.description,
        browserType: data.browser_type,
        fingerprintConfig: data.fingerprint_config,
        startUrls: data.start_urls,
        remark: data.remark,
        status: data.status || "ready",
      });
      // 刷新列表
      await loadEnvironments();
      return envUuid;
    },
    [loadEnvironments],
  );

  const updateEnv = useCallback(
    async (envUuid: string, data: CreateEnvData): Promise<void> => {
      await invoke<string>("bwbrowser_sync_env", {
        envUuid,
        name: data.name,
        description: data.description,
        browserType: data.browser_type,
        fingerprintConfig: data.fingerprint_config,
        startUrls: data.start_urls,
        remark: data.remark,
        status: data.status || "ready",
      });
      await loadEnvironments();
    },
    [loadEnvironments],
  );

  const deleteEnv = useCallback(
    async (envUuid: string): Promise<void> => {
      await invoke("bwbrowser_delete_env", { envUuid });
      await loadEnvironments();
    },
    [loadEnvironments],
  );

  return {
    environments,
    isLoading,
    error,
    refresh: loadEnvironments,
    createEnv,
    updateEnv,
    deleteEnv,
  };
}

/**
 * 从 Bwbrowser env id 字符串中提取 uuid
 * 例如 "bwbrowser_env_abc-123" -> "abc-123"
 */
export function extractBwbrowserEnvUuid(id: string): string | null {
  const match = id.match(/^bwbrowser_env_(.+)$/);
  if (match) {
    return match[1];
  }
  return null;
}
