"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { useBwbrowserAuth } from "./use-bwbrowser-auth";

// ==================== 类型定义 ====================

export interface ManagementUser {
  id: number;
  username: string;
  real_name: string;
  role: string;
  role_label: string;
  status: string;
  is_super_admin?: boolean;
  phone?: string | null;
  email?: string | null;
  created_at?: string | null;
  last_login_at?: string | null;
  permissions?: Record<string, boolean>;
}

export interface ManagementRoleOption {
  value: string;
  label: string;
}

export interface ManagementUsersListResponse {
  success: boolean;
  users?: ManagementUser[];
  is_super_admin?: boolean;
  permission_labels?: Record<string, string>;
}

export interface CurrentManagementUser {
  success: boolean;
  id: number;
  username: string;
  real_name: string;
  role: string;
  role_label: string;
  is_super_admin?: boolean;
  permissions?: Record<string, boolean>;
}

// ==================== Hook ====================

export function useBwbrowserUserManagement() {
  const { isLoggedIn } = useBwbrowserAuth();
  const [users, setUsers] = useState<ManagementUser[]>([]);
  const [roles, setRoles] = useState<ManagementRoleOption[]>([]);
  const [permissionLabels, setPermissionLabels] = useState<
    Record<string, string>
  >({});
  const [isSuperAdmin, setIsSuperAdmin] = useState(false);
  const [currentManagementUser, setCurrentManagementUser] =
    useState<CurrentManagementUser | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const currentUserId = currentManagementUser?.id ?? null;
  const canManageUsers =
    (currentManagementUser?.is_super_admin ?? false) ||
    (currentManagementUser?.permissions?.allow_user_management ?? false);

  // 加载当前用户信息
  const loadCurrentUser = useCallback(async () => {
    if (!isLoggedIn) return;
    try {
      const result = await invoke<CurrentManagementUser>(
        "bwbrowser_get_current_management_user",
      );
      setCurrentManagementUser(result);
    } catch (e) {
      console.warn("[UserManagement] 获取当前用户信息失败:", e);
    }
  }, [isLoggedIn]);

  // 加载用户列表
  const loadUsers = useCallback(async () => {
    if (!isLoggedIn) return;
    setIsLoading(true);
    setError(null);
    try {
      const result = await invoke<ManagementUsersListResponse>(
        "bwbrowser_list_management_users",
      );
      if (!result.success) {
        throw new Error("获取用户列表失败");
      }
      setUsers(result.users || []);
      setIsSuperAdmin(result.is_super_admin || false);
      setPermissionLabels(result.permission_labels || {});
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
      toast.error(msg);
    } finally {
      setIsLoading(false);
    }
  }, [isLoggedIn]);

  // 加载角色列表
  const loadRoles = useCallback(async () => {
    if (!isLoggedIn) return;
    try {
      const result = await invoke<ManagementRoleOption[]>(
        "bwbrowser_list_management_roles",
      );
      setRoles(result);
    } catch (e) {
      console.warn("[UserManagement] 加载角色列表失败:", e);
    }
  }, [isLoggedIn]);

  // 添加用户
  const addUser = useCallback(
    async (data: {
      username: string;
      password: string;
      real_name: string;
      role: string;
    }) => {
      const userId = await invoke<number>("bwbrowser_add_management_user", {
        username: data.username,
        password: data.password,
        realName: data.real_name,
        role: data.role,
      });
      toast.success(`用户 ${data.real_name} 添加成功`);
      await loadUsers();
      return userId;
    },
    [loadUsers],
  );

  // 更新用户
  const updateUser = useCallback(
    async (data: {
      user_id: number;
      real_name: string;
      role: string;
      status: string;
      password?: string;
    }) => {
      await invoke<void>("bwbrowser_update_management_user", {
        userId: data.user_id,
        realName: data.real_name,
        role: data.role,
        status: data.status,
        password: data.password || null,
      });
      toast.success("用户信息已更新");
      await loadUsers();
    },
    [loadUsers],
  );

  // 切换权限
  const togglePermission = useCallback(
    async (user_id: number, permission: string, value: boolean) => {
      await invoke<void>("bwbrowser_toggle_management_user_permission", {
        userId: user_id,
        permission,
        value,
      });
      setUsers((prev) =>
        prev.map((u) =>
          u.id === user_id
            ? {
                ...u,
                permissions: {
                  ...(u.permissions || {}),
                  [permission]: value,
                },
              }
            : u,
        ),
      );
    },
    [],
  );

  // 切换状态
  const toggleStatus = useCallback(
    async (user_id: number, status: string) => {
      await invoke<void>("bwbrowser_toggle_management_user_status", {
        userId: user_id,
        status,
      });
      toast.success("用户状态已更新");
      await loadUsers();
    },
    [loadUsers],
  );

  // 删除用户
  const deleteUser = useCallback(
    async (user_id: number, userName: string) => {
      await invoke<void>("bwbrowser_delete_management_user", {
        userId: user_id,
      });
      toast.success(`用户 ${userName} 已删除`);
      await loadUsers();
    },
    [loadUsers],
  );

  // 初始加载
  useEffect(() => {
    if (isLoggedIn) {
      loadCurrentUser();
      loadUsers();
      loadRoles();
    }
  }, [isLoggedIn, loadCurrentUser, loadUsers, loadRoles]);

  return {
    users,
    roles,
    permissionLabels,
    isSuperAdmin,
    currentUserId,
    canManageUsers,
    currentManagementUser,
    isLoading,
    error,
    loadUsers,
    addUser,
    updateUser,
    togglePermission,
    toggleStatus,
    deleteUser,
  };
}
