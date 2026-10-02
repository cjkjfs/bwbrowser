"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  LuLayers,
  LuLoaderCircle,
  LuPencil,
  LuRefreshCw,
  LuShield,
  LuToggleLeft,
  LuToggleRight,
  LuTrash2,
  LuUserCheck,
  LuUserPlus,
  LuUsers,
} from "react-icons/lu";
import { toast } from "sonner";
import { AnimatedSwitch } from "@/components/ui/animated-switch";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { useBwbrowserCompany } from "@/hooks/use-bwbrowser-company";
import {
  type ManagementUser,
  type PermissionProfile,
  useBwbrowserUserManagement,
} from "@/hooks/use-bwbrowser-user-management";
import { cn } from "@/lib/utils";

// ==================== 用户编辑对话框 ====================

interface UserEditDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  user: ManagementUser | null;
  roles: { value: string; label: string }[];
  onSave: (data: {
    user_id: number;
    real_name: string;
    role: string;
    status: string;
    password?: string;
  }) => Promise<void>;
}

function UserEditDialog({
  open,
  onOpenChange,
  user,
  roles,
  onSave,
}: UserEditDialogProps) {
  const [realName, setRealName] = useState("");
  const [role, setRole] = useState("member");
  const [status, setStatus] = useState("active");
  const [password, setPassword] = useState("");
  const [saving, setSaving] = useState(false);

  // 重置表单
  const resetForm = useCallback(() => {
    if (user) {
      setRealName(user.real_name);
      setRole(user.role);
      setStatus(user.status);
    } else {
      setRealName("");
      setRole("member");
      setStatus("active");
    }
    setPassword("");
  }, [user]);

  // 打开时重置
  useMemo(() => {
    if (open) {
      resetForm();
    }
  }, [open, resetForm]);

  const handleSave = async () => {
    if (!user) return;
    if (!realName.trim()) {
      toast.error("姓名不能为空");
      return;
    }
    setSaving(true);
    try {
      await onSave({
        user_id: user.id,
        real_name: realName.trim(),
        role,
        status,
        password: password || undefined,
      });
      onOpenChange(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>编辑用户</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <Label>用户名</Label>
            <Input value={user?.username || ""} disabled />
          </div>
          <div className="space-y-2">
            <Label>姓名 *</Label>
            <Input
              value={realName}
              onChange={(e) => setRealName(e.target.value)}
              placeholder="请输入姓名"
            />
          </div>
          <div className="space-y-2">
            <Label>角色</Label>
            <Select value={role} onValueChange={setRole}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {roles.map((r) => (
                  <SelectItem key={r.value} value={r.value}>
                    {r.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2">
            <Label>状态</Label>
            <Select value={status} onValueChange={setStatus}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="active">启用</SelectItem>
                <SelectItem value="inactive">禁用</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2">
            <Label>新密码（留空则不修改）</Label>
            <Input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="请输入新密码"
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleSave} disabled={saving}>
            {saving && <LuLoaderCircle className="w-4 h-4 mr-2 animate-spin" />}
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== 添加用户对话框 ====================

interface AddUserDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  roles: { value: string; label: string }[];
  onAdd: (data: {
    username: string;
    password: string;
    real_name: string;
    role: string;
  }) => Promise<number>;
}

function AddUserDialog({
  open,
  onOpenChange,
  roles,
  onAdd,
}: AddUserDialogProps) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [realName, setRealName] = useState("");
  const [role, setRole] = useState("member");
  const [saving, setSaving] = useState(false);

  const resetForm = useCallback(() => {
    setUsername("");
    setPassword("");
    setRealName("");
    setRole("member");
  }, []);

  useMemo(() => {
    if (open) resetForm();
  }, [open, resetForm]);

  const handleAdd = async () => {
    if (!username.trim() || !password || !realName.trim()) {
      toast.error("用户名、密码、姓名不能为空");
      return;
    }
    setSaving(true);
    try {
      await onAdd({
        username: username.trim(),
        password,
        real_name: realName.trim(),
        role,
      });
      onOpenChange(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>添加用户</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <Label>用户名 *</Label>
            <Input
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="请输入用户名"
            />
          </div>
          <div className="space-y-2">
            <Label>密码 *</Label>
            <Input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="请输入密码"
            />
          </div>
          <div className="space-y-2">
            <Label>姓名 *</Label>
            <Input
              value={realName}
              onChange={(e) => setRealName(e.target.value)}
              placeholder="请输入姓名"
            />
          </div>
          <div className="space-y-2">
            <Label>角色</Label>
            <Select value={role} onValueChange={setRole}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {roles.map((r) => (
                  <SelectItem key={r.value} value={r.value}>
                    {r.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleAdd} disabled={saving}>
            {saving && <LuLoaderCircle className="w-4 h-4 mr-2 animate-spin" />}
            添加
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== 权限编辑对话框 ====================

interface PermissionsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  user: ManagementUser | null;
  permissionLabels: Record<string, string>;
  onToggle: (
    user_id: number,
    permission: string,
    value: boolean,
  ) => Promise<void>;
}

function PermissionsDialog({
  open,
  onOpenChange,
  user,
  permissionLabels,
  onToggle,
}: PermissionsDialogProps) {
  const [toggling, setToggling] = useState<string | null>(null);

  const handleToggle = async (permission: string, value: boolean) => {
    if (!user) return;
    setToggling(permission);
    try {
      await onToggle(user.id, permission, value);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setToggling(null);
    }
  };

  const entries = Object.entries(permissionLabels);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>权限设置 - {user?.real_name}</DialogTitle>
        </DialogHeader>
        <div className="py-2 max-h-[60vh] overflow-y-auto">
          {entries.length === 0 ? (
            <div className="text-muted-foreground text-sm py-8 text-center">
              暂无权限项
            </div>
          ) : (
            <div className="space-y-1">
              {entries.map(([key, label]) => {
                const enabled = user?.permissions?.[key] ?? false;
                return (
                  <div
                    key={key}
                    className="flex items-center justify-between py-2 px-3 rounded-md hover:bg-muted/50"
                  >
                    <div className="flex items-center gap-3">
                      <LuShield className="w-4 h-4 text-muted-foreground" />
                      <span className="text-sm">{label}</span>
                    </div>
                    <AnimatedSwitch
                      checked={enabled}
                      onCheckedChange={(v: boolean) => handleToggle(key, v)}
                      disabled={toggling === key}
                    />
                  </div>
                );
              })}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>关闭</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== 权限组编辑对话框 ====================

interface PermissionProfileDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  profile: PermissionProfile | null;
  fields: Record<string, string>;
  onSave: (data: {
    id: number | null;
    name: string;
    description: string;
    permissions: Record<string, boolean>;
  }) => Promise<void>;
}

function PermissionProfileDialog({
  open,
  onOpenChange,
  profile,
  fields,
  onSave,
}: PermissionProfileDialogProps) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [permissions, setPermissions] = useState<Record<string, boolean>>({});
  const [saving, setSaving] = useState(false);

  const resetForm = useCallback(() => {
    if (profile) {
      setName(profile.name);
      setDescription(profile.description || "");
      setPermissions({ ...profile.permissions });
    } else {
      setName("");
      setDescription("");
      setPermissions({});
    }
  }, [profile]);

  useMemo(() => {
    if (open) resetForm();
  }, [open, resetForm]);

  const handleToggle = (key: string, value: boolean) => {
    setPermissions((prev) => ({ ...prev, [key]: value }));
  };

  const handleSave = async () => {
    if (!name.trim()) {
      toast.error("权限组名称不能为空");
      return;
    }
    setSaving(true);
    try {
      await onSave({
        id: profile?.id ?? null,
        name: name.trim(),
        description: description.trim(),
        permissions,
      });
      onOpenChange(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const entries = Object.entries(fields);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{profile ? "编辑权限组" : "新建权限组"}</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <Label>权限组名称 *</Label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="请输入权限组名称"
            />
          </div>
          <div className="space-y-2">
            <Label>描述</Label>
            <Input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="请输入描述（可选）"
            />
          </div>
          <div className="space-y-2 max-h-[45vh] overflow-y-auto rounded-md border border-border p-3">
            <Label>权限选择</Label>
            {entries.length === 0 ? (
              <div className="text-muted-foreground text-sm py-4 text-center">
                暂无权限项
              </div>
            ) : (
              <div className="space-y-1">
                {entries.map(([key, label]) => (
                  <div
                    key={key}
                    className="flex items-center justify-between py-2 px-2 rounded-md hover:bg-muted/50"
                  >
                    <span className="text-sm">{label}</span>
                    <AnimatedSwitch
                      checked={permissions[key] ?? false}
                      onCheckedChange={(v: boolean) => handleToggle(key, v)}
                    />
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleSave} disabled={saving}>
            {saving && <LuLoaderCircle className="w-4 h-4 mr-2 animate-spin" />}
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== 套用权限组对话框 ====================

interface ApplyProfileDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  user: ManagementUser | null;
  profiles: PermissionProfile[];
  onApply: (user_id: number, profile_id: number) => Promise<void>;
}

function ApplyProfileDialog({
  open,
  onOpenChange,
  user,
  profiles,
  onApply,
}: ApplyProfileDialogProps) {
  const [profileId, setProfileId] = useState<string>("");
  const [saving, setSaving] = useState(false);

  useMemo(() => {
    if (open && user) {
      setProfileId(
        user.permission_profile_id ? String(user.permission_profile_id) : "",
      );
    }
  }, [open, user]);

  const handleApply = async () => {
    if (!user) return;
    setSaving(true);
    try {
      await onApply(user.id, profileId ? Number(profileId) : 0);
      onOpenChange(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>套用权限组 - {user?.real_name}</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-4">
          <p className="text-sm text-muted-foreground">
            选择权限组并套用到该用户，该用户的权限将被权限组覆盖。
          </p>
          <div className="space-y-2">
            <Label>权限组</Label>
            <Select value={profileId} onValueChange={setProfileId}>
              <SelectTrigger>
                <SelectValue placeholder="选择权限组" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="0">无（解除套用）</SelectItem>
                {profiles.map((p) => (
                  <SelectItem key={p.id} value={String(p.id)}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {user?.permission_profile_id && (
            <p className="text-xs text-warning">
              该用户当前已套用权限组，套用新权限组将覆盖原权限。
            </p>
          )}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleApply} disabled={saving}>
            {saving && <LuLoaderCircle className="w-4 h-4 mr-2 animate-spin" />}
            套用
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== 权限组管理对话框（列表） ====================

interface PermissionProfileManagerDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  profiles: PermissionProfile[];
  fields: Record<string, string>;
  onAdd: () => void;
  onEdit: (profile: PermissionProfile) => void;
  onDelete: (profile: PermissionProfile) => void;
}

function PermissionProfileManagerDialog({
  open,
  onOpenChange,
  profiles,
  fields,
  onAdd,
  onEdit,
  onDelete,
}: PermissionProfileManagerDialogProps) {
  const enabledCount = (permissions: Record<string, boolean>) =>
    Object.values(permissions).filter(Boolean).length;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>权限组管理</DialogTitle>
        </DialogHeader>
        <div className="py-2">
          <div className="flex items-center justify-between mb-3">
            <span className="text-sm text-muted-foreground">
              共 {profiles.length} 个权限组
            </span>
            <Button size="sm" onClick={onAdd}>
              <LuUserPlus className="w-4 h-4 mr-2" />
              新建权限组
            </Button>
          </div>
          {profiles.length === 0 ? (
            <div className="text-muted-foreground text-sm py-8 text-center">
              暂无权限组
            </div>
          ) : (
            <div className="max-h-[55vh] overflow-y-auto space-y-2">
              {profiles.map((p) => (
                <div
                  key={p.id}
                  className="flex items-center justify-between rounded-md border border-border p-3"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <LuLayers className="w-4 h-4 text-muted-foreground shrink-0" />
                      <span className="font-medium text-sm truncate">
                        {p.name}
                      </span>
                    </div>
                    <div className="text-xs text-muted-foreground mt-1">
                      {p.description || "无描述"} ·{" "}
                      {enabledCount(p.permissions)}/{Object.keys(fields).length}{" "}
                      项权限
                    </div>
                  </div>
                  <div className="flex items-center gap-1 shrink-0">
                    <Button variant="ghost" size="sm" onClick={() => onEdit(p)}>
                      <LuPencil className="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => onDelete(p)}
                      className="text-destructive hover:text-destructive"
                    >
                      <LuTrash2 className="w-4 h-4" />
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>关闭</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== 主组件 ====================

export function UserManagementPage() {
  const {
    users,
    roles,
    permissionLabels,
    isSuperAdmin,
    currentUserId,
    isLoading,
    permissionProfiles,
    permissionProfileFields,
    loadUsers,
    loadPermissionProfiles,
    addUser,
    updateUser,
    togglePermission,
    toggleStatus,
    deleteUser,
    savePermissionProfile,
    deletePermissionProfile,
    applyPermissionProfile,
  } = useBwbrowserUserManagement();

  const { isLoggedIn } = useBwbrowserAuth();
  const { companies, selectedCompanyId, setSelectedCompanyId } =
    useBwbrowserCompany(isSuperAdmin, isLoggedIn);

  // 跟随选中的公司刷新用户与权限组（超管切公司即重新拉取）
  useEffect(() => {
    loadUsers(selectedCompanyId);
    loadPermissionProfiles(selectedCompanyId);
  }, [selectedCompanyId, loadUsers, loadPermissionProfiles]);

  const [addDialogOpen, setAddDialogOpen] = useState(false);
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [permDialogOpen, setPermDialogOpen] = useState(false);
  const [selectedUserId, setSelectedUserId] = useState<number | null>(null);
  const [profileDialogOpen, setProfileDialogOpen] = useState(false);
  const [profileDialogEdit, setProfileDialogEdit] =
    useState<PermissionProfile | null>(null);
  const [profileManagerOpen, setProfileManagerOpen] = useState(false);
  const [applyDialogOpen, setApplyDialogOpen] = useState(false);

  // 从 users 里实时取最新数据，避免对话框用旧快照
  const selectedUser = useMemo(
    () => users.find((u) => u.id === selectedUserId) ?? null,
    [users, selectedUserId],
  );

  // 权限组 id → 权限组 映射
  const permissionProfileById = useMemo(() => {
    const m = new Map<number, PermissionProfile>();
    for (const p of permissionProfiles) m.set(p.id, p);
    return m;
  }, [permissionProfiles]);

  // 解析用户当前权限组：优先取真实关联的权限组；未关联时按权限值回退匹配最接近的权限组（与网页后台一致）
  const resolveProfileForUser = useCallback(
    (user: ManagementUser): PermissionProfile | null => {
      if (user.permission_profile_id) {
        const direct = permissionProfileById.get(user.permission_profile_id);
        if (direct) return direct;
      }
      const perms = user.permissions;
      if (!perms) return null;
      return (
        permissionProfiles.find((p) =>
          Object.keys(p.permissions).every(
            (k) => (perms[k] ?? false) === p.permissions[k],
          ),
        ) ?? null
      );
    },
    [permissionProfileById, permissionProfiles],
  );

  const handleEdit = (user: ManagementUser) => {
    setSelectedUserId(user.id);
    setEditDialogOpen(true);
  };

  const handlePermissions = (user: ManagementUser) => {
    setSelectedUserId(user.id);
    setPermDialogOpen(true);
  };

  const handleApplyProfile = (user: ManagementUser) => {
    setSelectedUserId(user.id);
    setApplyDialogOpen(true);
  };

  const handleOpenProfileManager = () => {
    setProfileManagerOpen(true);
  };

  const handleAddProfile = () => {
    setProfileDialogEdit(null);
    setProfileManagerOpen(false);
    setProfileDialogOpen(true);
  };

  const handleEditProfile = (profile: PermissionProfile) => {
    setProfileDialogEdit(profile);
    setProfileManagerOpen(false);
    setProfileDialogOpen(true);
  };

  const handleDelete = async (user: ManagementUser) => {
    if (!confirm(`确定要删除用户「${user.real_name}」吗？`)) return;
    try {
      await deleteUser(user.id, user.real_name);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  const handleToggleStatus = async (user: ManagementUser) => {
    const newStatus = user.status === "active" ? "inactive" : "active";
    try {
      await toggleStatus(user.id, newStatus);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  const handleDeleteProfile = async (profile: PermissionProfile) => {
    if (!confirm(`确定要删除权限组「${profile.name}」吗？`)) return;
    try {
      await deletePermissionProfile(
        profile.id,
        profile.name,
        selectedCompanyId,
      );
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="flex flex-col h-full">
      {/* 头部 */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-border">
        <div className="flex items-center gap-3">
          <LuUsers className="w-5 h-5" />
          <h2 className="text-lg font-semibold">用户管理</h2>
          <Badge variant="secondary" className="ml-2">
            {users.length} 人
          </Badge>
        </div>
        <div className="flex items-center gap-2">
          {isSuperAdmin && companies.length > 0 && (
            <Select
              value={selectedCompanyId ? String(selectedCompanyId) : ""}
              onValueChange={(v) => setSelectedCompanyId(v ? Number(v) : null)}
            >
              <SelectTrigger className="w-[160px] h-8 text-xs">
                <SelectValue placeholder="选择公司" />
              </SelectTrigger>
              <SelectContent>
                {companies.map((c) => (
                  <SelectItem key={c.id} value={String(c.id)}>
                    {c.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => loadUsers(selectedCompanyId)}
            disabled={isLoading}
          >
            <LuRefreshCw
              className={cn("w-4 h-4 mr-2", isLoading && "animate-spin")}
            />
            刷新
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleOpenProfileManager}
          >
            <LuLayers className="w-4 h-4 mr-2" />
            权限组管理
          </Button>
          <Button size="sm" onClick={() => setAddDialogOpen(true)}>
            <LuUserPlus className="w-4 h-4 mr-2" />
            添加用户
          </Button>
        </div>
      </div>

      {/* 表格 */}
      <div className="flex-1 overflow-auto">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-12">#</TableHead>
              <TableHead>用户名</TableHead>
              <TableHead>姓名</TableHead>
              <TableHead>角色</TableHead>
              <TableHead>权限组</TableHead>
              <TableHead>状态</TableHead>
              <TableHead>最后登录</TableHead>
              <TableHead className="text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading && users.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={8}
                  className="text-center py-12 text-muted-foreground"
                >
                  <LuLoaderCircle className="w-6 h-6 animate-spin mx-auto mb-2" />
                  加载中...
                </TableCell>
              </TableRow>
            ) : users.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={8}
                  className="text-center py-12 text-muted-foreground"
                >
                  暂无用户
                </TableCell>
              </TableRow>
            ) : (
              users.map((user, idx) => (
                <TableRow key={user.id}>
                  <TableCell className="text-muted-foreground text-sm">
                    {idx + 1}
                  </TableCell>
                  <TableCell className="font-medium">
                    {user.username}
                    {user.is_super_admin && (
                      <Badge
                        variant="destructive"
                        className="ml-2 text-[10px] py-0 h-4"
                      >
                        超管
                      </Badge>
                    )}
                  </TableCell>
                  <TableCell>{user.real_name}</TableCell>
                  <TableCell>
                    <Badge variant="outline">
                      {user.role_label || user.role}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    {(() => {
                      const profile = resolveProfileForUser(user);
                      return profile ? (
                        <Badge variant="secondary">
                          <LuLayers className="w-3 h-3 mr-1" />
                          {profile.name}
                        </Badge>
                      ) : (
                        <span className="text-muted-foreground text-sm">-</span>
                      );
                    })()}
                  </TableCell>
                  <TableCell>
                    <span
                      className={cn(
                        "inline-flex items-center gap-1.5 text-sm",
                        user.status === "active"
                          ? "text-success"
                          : "text-muted-foreground",
                      )}
                    >
                      <span
                        className={cn(
                          "w-2 h-2 rounded-full",
                          user.status === "active"
                            ? "bg-success"
                            : "bg-muted-foreground",
                        )}
                      />
                      {user.status === "active" ? "启用" : "禁用"}
                    </span>
                  </TableCell>
                  <TableCell className="text-sm text-muted-foreground">
                    {user.last_login_at || "-"}
                  </TableCell>
                  <TableCell className="text-right">
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="sm">
                          操作
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem onClick={() => handleEdit(user)}>
                          <LuPencil className="w-4 h-4 mr-2" />
                          编辑
                        </DropdownMenuItem>
                        {user.id !== currentUserId && (
                          <DropdownMenuItem
                            onClick={() => handlePermissions(user)}
                          >
                            <LuShield className="w-4 h-4 mr-2" />
                            权限设置
                          </DropdownMenuItem>
                        )}
                        {user.id !== currentUserId && (
                          <DropdownMenuItem
                            onClick={() => handleApplyProfile(user)}
                          >
                            <LuUserCheck className="w-4 h-4 mr-2" />
                            套用权限组
                          </DropdownMenuItem>
                        )}
                        {user.id !== currentUserId && (
                          <DropdownMenuItem
                            onClick={() => handleToggleStatus(user)}
                          >
                            {user.status === "active" ? (
                              <>
                                <LuToggleLeft className="w-4 h-4 mr-2" />
                                禁用账号
                              </>
                            ) : (
                              <>
                                <LuToggleRight className="w-4 h-4 mr-2" />
                                启用账号
                              </>
                            )}
                          </DropdownMenuItem>
                        )}
                        {isSuperAdmin &&
                          !user.is_super_admin &&
                          user.id !== currentUserId && (
                            <DropdownMenuItem
                              onClick={() => handleDelete(user)}
                              className="text-destructive focus:text-destructive"
                            >
                              <LuTrash2 className="w-4 h-4 mr-2" />
                              删除
                            </DropdownMenuItem>
                          )}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      {/* 对话框 */}
      <AddUserDialog
        open={addDialogOpen}
        onOpenChange={setAddDialogOpen}
        roles={roles}
        onAdd={addUser}
      />
      <UserEditDialog
        open={editDialogOpen}
        onOpenChange={setEditDialogOpen}
        user={selectedUser}
        roles={roles}
        onSave={updateUser}
      />
      <PermissionsDialog
        open={permDialogOpen}
        onOpenChange={setPermDialogOpen}
        user={selectedUser}
        permissionLabels={permissionLabels}
        onToggle={togglePermission}
      />
      <PermissionProfileManagerDialog
        open={profileManagerOpen}
        onOpenChange={setProfileManagerOpen}
        profiles={permissionProfiles}
        fields={permissionProfileFields}
        onAdd={handleAddProfile}
        onEdit={handleEditProfile}
        onDelete={handleDeleteProfile}
      />
      <PermissionProfileDialog
        open={profileDialogOpen}
        onOpenChange={setProfileDialogOpen}
        profile={profileDialogEdit}
        fields={permissionProfileFields}
        onSave={(data) =>
          savePermissionProfile({ ...data, companyId: selectedCompanyId })
        }
      />
      <ApplyProfileDialog
        open={applyDialogOpen}
        onOpenChange={setApplyDialogOpen}
        user={selectedUser}
        profiles={permissionProfiles}
        onApply={(userId, profileId) =>
          applyPermissionProfile(userId, profileId, selectedCompanyId)
        }
      />
    </div>
  );
}
