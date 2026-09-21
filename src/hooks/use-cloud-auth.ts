import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { CloudAuthState, CloudUser } from "@/types";

interface UseCloudAuthReturn {
  user: CloudUser | null;
  /** When this desktop signed in, as the backend recorded it. */
  loggedInAt: string | null;
  isLoggedIn: boolean;
  isLoading: boolean;
  /** 是否通过 Bwbrowser 云端登录 */
  isBwbrowserLogin: boolean;
  exchangeDeviceCode: (code: string) => Promise<CloudAuthState>;
  logout: () => Promise<void>;
  refreshProfile: () => Promise<CloudUser>;
}

export function useCloudAuth(): UseCloudAuthReturn {
  const [authState, setAuthState] = useState<CloudAuthState | null>(null);
  const [bwbrowserAuthState, setBwbrowserAuthState] =
    useState<CloudAuthState | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  const loadUser = useCallback(async () => {
    try {
      // 优先检查 Bwbrowser 登录状态
      const bwbrowserState = await invoke<CloudAuthState | null>(
        "bwbrowser_get_user",
      );
      setBwbrowserAuthState(bwbrowserState);

      if (bwbrowserState) {
        setAuthState(bwbrowserState);
        setIsLoading(false);
        return;
      }

      // 如果 Bwbrowser 没登录，检查 Bwbrowser 原生云登录
      const state = await invoke<CloudAuthState | null>("cloud_get_user");
      setAuthState(state);
    } catch (error) {
      console.error("Failed to load cloud auth state:", error);
      setAuthState(null);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadUser();

    const unlistenExpired = listen("cloud-auth-expired", () => {
      setAuthState(null);
    });

    const unlistenChanged = listen("cloud-auth-changed", () => {
      void loadUser();
    });

    return () => {
      void unlistenExpired.then((unlisten) => {
        unlisten();
      });
      void unlistenChanged.then((unlisten) => {
        unlisten();
      });
    };
  }, [loadUser]);

  const exchangeDeviceCode = useCallback(
    async (code: string): Promise<CloudAuthState> => {
      const state = await invoke<CloudAuthState>("cloud_exchange_device_code", {
        code,
      });
      setAuthState(state);
      return state;
    },
    [],
  );

  const logout = useCallback(async () => {
    // 如果是 Bwbrowser 登录，调用 Bwbrowser 登出
    if (bwbrowserAuthState) {
      await invoke("bwbrowser_logout");
      setBwbrowserAuthState(null);
      setAuthState(null);
      return;
    }
    await invoke("cloud_logout");
    setAuthState(null);
  }, [bwbrowserAuthState]);

  const refreshProfile = useCallback(async (): Promise<CloudUser> => {
    // 如果是 Bwbrowser 登录，刷新 Bwbrowser 用户信息
    if (bwbrowserAuthState) {
      const user = await invoke<CloudUser>("bwbrowser_refresh_profile");
      setAuthState((prev) =>
        prev
          ? { ...prev, user }
          : { user, logged_in_at: new Date().toISOString() },
      );
      return user;
    }
    const user = await invoke<CloudUser>("cloud_refresh_profile");
    setAuthState((prev) =>
      prev
        ? { ...prev, user }
        : { user, logged_in_at: new Date().toISOString() },
    );
    return user;
  }, [bwbrowserAuthState]);

  return {
    user: authState?.user ?? null,
    loggedInAt: authState?.logged_in_at ?? null,
    isLoggedIn: authState !== null,
    isLoading,
    isBwbrowserLogin: bwbrowserAuthState !== null,
    exchangeDeviceCode,
    logout,
    refreshProfile,
  };
}
