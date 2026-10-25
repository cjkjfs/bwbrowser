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
  // 权限是否已解析完成：避免挂载瞬间 isSuperAdmin=false 的中间态误伤依赖方
  const [permissionsReady, setPermissionsReady] = useState(false);
  const [currentManagementUserId, setCurrentManagementUserId] = useState<
    number | null
  >(null);

  useEffect(() => {
    if (!isLoggedIn) {
      setPermissions({});
      setIsSuperAdmin(false);
      setPermissionsReady(false);
      setCurrentManagementUserId(null);
      return;
    }
    setPermissionsReady(false);
    let cancelled = false;
    invoke<CurrentManagementUser>("bwbrowser_get_current_management_user")
      .then((result) => {
        if (cancelled) return;
        setPermissions(result.permissions || {});
        setIsSuperAdmin(result.is_super_admin ?? false);
        setPermissionsReady(true);
        setCurrentManagementUserId(result.id);
      })
      .catch((e) => {
        console.error("[Permissions] 获取当前用户权限失败:", e);
        if (!cancelled) setPermissionsReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, [isLoggedIn]);

  const canManageUsers =
    isSuperAdmin || (permissions.allow_user_management ?? false);
  // 以下五个功能只按后台开关判断，超管也一样受控（超管给自己开权限即可）
  const canDownloadVideos = permissions.allow_video_download ?? false;
  // AI 洗稿：独立权限（users.php allow_ai_rewrite），决定左侧「AI 洗稿」菜单是否出现。
  const canAiRewrite = permissions.allow_ai_rewrite ?? false;
  // 远程管理（控制端）：决定左侧「远程管理」菜单是否出现。
  const canManageRemote =
    isSuperAdmin || (permissions.allow_remote_management ?? false);
  // 视频工具（视频匹配/快速合并/快速拆分）：users.php allow_video_tools
  const canVideoTools =
    isSuperAdmin || (permissions.allow_video_tools ?? false);
  // 音频处理（气口清除器）：users.php allow_audio_processor
  const canAudioProcessor =
    isSuperAdmin || (permissions.allow_audio_processor ?? false);
  // 扩展管理：users.php allow_extension_management
  const canExtensions =
    isSuperAdmin || (permissions.allow_extension_management ?? false);
  // 环境管理 / 代理中心 / 账号中心：只按后台开关判断
  const canProfileManagement = permissions.allow_profile_management ?? false;
  const canProxyCenter = permissions.allow_proxy_center ?? false;
  const canCloudAccount = permissions.allow_cloud_account ?? false;
  // 云端账号（左侧菜单「云端账号」页面）：users.php allow_cloud_accounts
  const canCloudAccounts = permissions.allow_cloud_accounts ?? false;

  return {
    permissions,
    isSuperAdmin,
    permissionsReady,
    canManageUsers,
    canDownloadVideos,
    canAiRewrite,
    canManageRemote,
    canVideoTools,
    canAudioProcessor,
    canExtensions,
    canProfileManagement,
    canProxyCenter,
    canCloudAccount,
    canCloudAccounts,
    currentManagementUserId,
  };
}
