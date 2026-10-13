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
  // AI 洗稿：独立权限（users.php allow_ai_rewrite），决定左侧「AI 洗稿」菜单是否出现。
  const canAiRewrite = isSuperAdmin || (permissions.allow_ai_rewrite ?? false);
  // 远程管理（控制端）：决定左侧「远程管理」菜单是否出现。
  const canManageRemote =
    isSuperAdmin || (permissions.allow_remote_management ?? false);

  return {
    permissions,
    isSuperAdmin,
    canManageUsers,
    canDownloadVideos,
    canAiRewrite,
    canManageRemote,
    currentManagementUserId,
  };
}
