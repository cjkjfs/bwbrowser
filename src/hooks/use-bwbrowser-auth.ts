import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { CloudAuthState, CloudUser } from "@/types";

interface UseBwbrowserAuthReturn {
  user: CloudUser | null;
  loggedInAt: string | null;
  isLoggedIn: boolean;
  isLoading: boolean;
  login: (username: string, password: string) => Promise<CloudAuthState>;
  logout: () => Promise<void>;
  refreshProfile: () => Promise<CloudUser>;
}

export function useBwbrowserAuth(): UseBwbrowserAuthReturn {
  const [authState, setAuthState] = useState<CloudAuthState | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  const loadUser = useCallback(async () => {
    try {
      const state = await invoke<CloudAuthState | null>("bwbrowser_get_user");
      setAuthState(state);
    } catch (error) {
      console.error("Failed to load bwbrowser auth state:", error);
      setAuthState(null);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadUser();

    const unlistenChanged = listen("cloud-auth-changed", () => {
      void loadUser();
    });

    return () => {
      void unlistenChanged.then((unlisten) => {
        unlisten();
      });
    };
  }, [loadUser]);

  const login = useCallback(
    async (username: string, password: string): Promise<CloudAuthState> => {
      const state = await invoke<CloudAuthState>("bwbrowser_login", {
        username,
        password,
      });
      setAuthState(state);
      return state;
    },
    [],
  );

  const logout = useCallback(async () => {
    await invoke("bwbrowser_logout");
    setAuthState(null);
  }, []);

  const refreshProfile = useCallback(async (): Promise<CloudUser> => {
    const user = await invoke<CloudUser>("bwbrowser_refresh_profile");
    setAuthState((prev) =>
      prev
        ? { ...prev, user }
        : { user, logged_in_at: new Date().toISOString() },
    );
    return user;
  }, []);

  return {
    user: authState?.user ?? null,
    loggedInAt: authState?.logged_in_at ?? null,
    isLoggedIn: authState !== null,
    isLoading,
    login,
    logout,
    refreshProfile,
  };
}
