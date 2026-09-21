"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useBwbrowserAuth } from "./use-bwbrowser-auth";

interface CurrentManagementUser {
  success: boolean;
  id: number;
  permissions?: Record<string, boolean>;
  is_super_admin?: boolean;
}

export function useBwbrowserPermissions() {
  const { isLoggedIn } = useBwbrowserAuth();
  const [permissions, setPermissions] = useState<Record<string, boolean>>({});
  const [isSuperAdmin, setIsSuperAdmin] = useState(false);
  const [currentManagementUserId, setCurrentManagementUserId] = useState<
    number | null
  >(null);

  useEffect(() => {
    if (!isLoggedIn) {
      setPermissions({});
      setIsSuperAdmin(false);
      setCurrentManagementUserId(null);
      return;
    }
    let cancelled = false;
    invoke<CurrentManagementUser>("bwbrowser_get_current_management_user")
      .then((result) => {
        if (cancelled) return;
        setPermissions(result.permissions || {});
        setIsSuperAdmin(result.is_super_admin ?? false);
        setCurrentManagementUserId(result.id);
      })
      .catch((e) => {
        console.error("[Permissions] 获取当前用户权限失败:", e);
      });
    return () => {
      cancelled = true;
    };
  }, [isLoggedIn]);

  const canManageUsers =
    isSuperAdmin || (permissions.allow_user_management ?? false);
  const canDownloadVideos =
    isSuperAdmin || (permissions.allow_video_download ?? false);

  return {
    permissions,
    isSuperAdmin,
    canManageUsers,
    canDownloadVideos,
    currentManagementUserId,
  };
}
