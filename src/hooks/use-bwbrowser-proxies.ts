"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useBwbrowserAuth } from "./use-bwbrowser-auth";

export interface BwbrowserProxy {
  id: string; // "bwbrowser_{proxy_id}"
  name: string;
  proxy_type: string;
  host: string;
  port: number;
  username?: string;
  password?: string;
  vless_uri?: string;
  country?: string;
  city?: string;
  provider?: string;
  is_cloud_managed: boolean;
}

interface UseBwbrowserProxiesReturn {
  proxies: BwbrowserProxy[];
  isLoading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  createProxy: (data: CreateProxyData) => Promise<number>;
  updateProxy: (proxyId: number, data: CreateProxyData) => Promise<void>;
  deleteProxy: (proxyId: number) => Promise<void>;
}

export interface CreateProxyData {
  proxy_name: string;
  proxy_type: string;
  host: string;
  port: number;
  username?: string;
  password?: string;
  country?: string;
  city?: string;
  protocol_config?: string;
}

/**
 * Bwbrowser 云端代理 Hook
 * 云优先模式：所有代理数据来自云端，本地只做缓存
 */
export function useBwbrowserProxies(): UseBwbrowserProxiesReturn {
  const { isLoggedIn } = useBwbrowserAuth();
  const [proxies, setProxies] = useState<BwbrowserProxy[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadProxies = useCallback(async () => {
    if (!isLoggedIn) {
      setProxies([]);
      return;
    }
    setIsLoading(true);
    setError(null);
    try {
      // 先同步云端代理到本地缓存
      try {
        await invoke("bwbrowser_sync_proxies_to_local");
      } catch (e) {
        console.warn("同步云端代理到本地失败:", e);
      }
      const result = await invoke<BwbrowserProxy[]>("bwbrowser_list_proxies");
      setProxies(result || []);
    } catch (err) {
      console.error("Failed to load bwbrowser proxies:", err);
      setError(String(err));
    } finally {
      setIsLoading(false);
    }
  }, [isLoggedIn]);

  // 登录状态变化时自动加载
  useEffect(() => {
    void loadProxies();
  }, [loadProxies]);

  const createProxy = useCallback(
    async (data: CreateProxyData): Promise<number> => {
      const proxyId = await invoke<number>("bwbrowser_sync_proxy", {
        proxyId: null,
        proxyName: data.proxy_name,
        proxyType: data.proxy_type,
        host: data.host,
        port: data.port,
        username: data.username,
        password: data.password,
        country: data.country,
        city: data.city,
        protocolConfig: data.protocol_config,
      });
      // 显式同步到本地缓存（确保设置代理时能找到）
      try {
        await invoke("bwbrowser_sync_proxies_to_local");
      } catch (e) {
        console.error("同步云端代理到本地失败:", e);
      }
      await loadProxies();
      return proxyId;
    },
    [loadProxies],
  );

  const updateProxy = useCallback(
    async (proxyId: number, data: CreateProxyData): Promise<void> => {
      await invoke<number>("bwbrowser_sync_proxy", {
        proxyId,
        proxyName: data.proxy_name,
        proxyType: data.proxy_type,
        host: data.host,
        port: data.port,
        username: data.username,
        password: data.password,
        country: data.country,
        city: data.city,
        protocolConfig: data.protocol_config,
      });
      try {
        await invoke("bwbrowser_sync_proxies_to_local");
      } catch (e) {
        console.error("同步云端代理到本地失败:", e);
      }
      await loadProxies();
    },
    [loadProxies],
  );

  const deleteProxy = useCallback(
    async (proxyId: number): Promise<void> => {
      await invoke("bwbrowser_delete_proxy", { proxyId });
      try {
        await invoke("bwbrowser_sync_proxies_to_local");
      } catch (e) {
        console.error("同步删除到本地失败:", e);
      }
      await loadProxies();
    },
    [loadProxies],
  );

  return {
    proxies,
    isLoading,
    error,
    refresh: loadProxies,
    createProxy,
    updateProxy,
    deleteProxy,
  };
}

/**
 * 从 Bwbrowser proxy id 字符串中提取数字 ID
 * 例如 "bwbrowser_123" -> 123
 */
export function extractBwbrowserProxyId(id: string): number | null {
  const match = id.match(/^bwbrowser_(\d+)$/);
  if (match) {
    return parseInt(match[1], 10);
  }
  return null;
}
