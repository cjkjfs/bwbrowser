"use client";

import { useCallback, useMemo, useState } from "react";
import {
  LuLoaderCircle,
  LuPencil,
  LuRefreshCw,
  LuShield,
  LuToggleLeft,
  LuToggleRight,
  LuTrash2,
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
import {
  type ManagementUser,
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

// ==================== 主组件 ====================

export function UserManagementPage() {
  const {
    users,
    roles,
    permissionLabels,
    isSuperAdmin,
    currentUserId,
    isLoading,
    loadUsers,
    addUser,
    updateUser,
    togglePermission,
    toggleStatus,
    deleteUser,
  } = useBwbrowserUserManagement();

  const [addDialogOpen, setAddDialogOpen] = useState(false);
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [permDialogOpen, setPermDialogOpen] = useState(false);
  const [selectedUserId, setSelectedUserId] = useState<number | null>(null);

  // 从 users 里实时取最新数据，避免对话框用旧快照
  const selectedUser = useMemo(
    () => users.find((u) => u.id === selectedUserId) ?? null,
    [users, selectedUserId],
  );

  const handleEdit = (user: ManagementUser) => {
    setSelectedUserId(user.id);
    setEditDialogOpen(true);
  };

  const handlePermissions = (user: ManagementUser) => {
    setSelectedUserId(user.id);
    setPermDialogOpen(true);
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
          <Button
            variant="ghost"
            size="sm"
            onClick={loadUsers}
            disabled={isLoading}
          >
            <LuRefreshCw
              className={cn("w-4 h-4 mr-2", isLoading && "animate-spin")}
            />
            刷新
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
              <TableHead>状态</TableHead>
              <TableHead>最后登录</TableHead>
              <TableHead className="text-right">操作</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading && users.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={7}
                  className="text-center py-12 text-muted-foreground"
                >
                  <LuLoaderCircle className="w-6 h-6 animate-spin mx-auto mb-2" />
                  加载中...
                </TableCell>
              </TableRow>
            ) : users.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={7}
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
    </div>
  );
}
