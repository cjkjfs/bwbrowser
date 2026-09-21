"use client";

import {
  ColumnDef,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  SortingState,
  useReactTable,
} from "@tanstack/react-table";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  LuChevronDown,
  LuChevronLeft,
  LuChevronRight,
  LuChevronsUpDown,
  LuCookie,
  LuDownload,
  LuEraser,
  LuFolderTree,
  LuGlobe,
  LuList,
  LuMonitor,
  LuMousePointer2,
  LuNetwork,
  LuPencil,
  LuPlay,
  LuPlus,
  LuRotateCcw,
  LuSearch,
  LuSquare,
  LuStickyNote,
  LuTag,
  LuTrash2,
  LuUser,
  LuUserPlus,
  LuX,
} from "react-icons/lu";
import { toast } from "sonner";
import { LoadingButton } from "@/components/loading-button";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
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
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { useBwbrowserCompany } from "@/hooks/use-bwbrowser-company";
import {
  type BwbrowserEnvironment,
  extractBwbrowserEnvUuid,
  useBwbrowserEnvironments,
} from "@/hooks/use-bwbrowser-environments";
import { useBwbrowserPermissions } from "@/hooks/use-bwbrowser-permissions";
import { cn } from "@/lib/utils";

// ============================================================
// 常量定义
// ============================================================

interface BwbrowserEnvManagementDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** 嵌入式模式：不显示 Dialog 外壳，直接渲染内容 */
  embedded?: boolean;
}

type ViewType = "all" | "opened" | "trash";

const BROWSER_TYPES = [
  { value: "chromium", label: "Chromium" },
  { value: "chrome", label: "Chrome" },
  { value: "edge", label: "Edge" },
  { value: "firefox", label: "Firefox" },
];

const PAGE_SIZE = 10;

// ============================================================
// 工具函数
// ============================================================

function formatDateTime(dateStr?: string): string {
  if (!dateStr) return "—";
  try {
    const d = new Date(dateStr);
    return d.toLocaleString("zh-CN", { hour12: false });
  } catch {
    return dateStr;
  }
}

/**
 * 显示"功能开发中"提示
 */
function showWipToast(featureName: string) {
  toast.info(`${featureName}功能开发中，敬请期待`);
}

// ============================================================
// 环境表单对话框（创建/编辑）
// ============================================================

function EnvFormDialog({
  isOpen,
  onClose,
  editingEnv,
  onSaved,
}: {
  isOpen: boolean;
  onClose: () => void;
  editingEnv: BwbrowserEnvironment | null;
  onSaved: () => void;
}) {
  const { createEnv, updateEnv } = useBwbrowserEnvironments();
  const [isSaving, setIsSaving] = useState(false);
  const [name, setName] = useState("");
  const [browserType, setBrowserType] = useState("chromium");
  const [description, setDescription] = useState("");
  const [remark, setRemark] = useState("");
  const [status, setStatus] = useState("ready");

  useEffect(() => {
    if (!isOpen) return;
    if (editingEnv) {
      setName(editingEnv.name);
      setBrowserType(editingEnv.browser_type);
      setDescription(editingEnv.description || "");
      setRemark(editingEnv.remark || "");
      setStatus(editingEnv.status);
    } else {
      setName("");
      setBrowserType("chromium");
      setDescription("");
      setRemark("");
      setStatus("ready");
    }
  }, [editingEnv, isOpen]);

  const handleSave = async () => {
    if (!name.trim()) {
      toast.error("请输入环境名称");
      return;
    }
    setIsSaving(true);
    try {
      if (editingEnv) {
        const uuid = extractBwbrowserEnvUuid(editingEnv.id);
        if (!uuid) throw new Error("无效的环境 ID");
        await updateEnv(uuid, {
          name: name.trim(),
          description: description.trim() || undefined,
          browser_type: browserType,
          remark: remark.trim() || undefined,
          status,
        });
        toast.success("环境已更新");
      } else {
        await createEnv({
          name: name.trim(),
          description: description.trim() || undefined,
          browser_type: browserType,
          remark: remark.trim() || undefined,
          status,
        });
        toast.success("环境已创建");
      }
      onSaved();
      onClose();
    } catch (err) {
      console.error("Failed to save env:", err);
      toast.error(String(err));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[85vh] max-w-lg">
        <DialogHeader>
          <DialogTitle>{editingEnv ? "编辑环境" : "新建环境"}</DialogTitle>
        </DialogHeader>
        <ScrollArea className="max-h-[60vh] pr-1">
          <div className="grid gap-4 py-4">
            <div className="space-y-2">
              <Label htmlFor="env-name">环境名称</Label>
              <Input
                id="env-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="请输入环境名称"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="env-browser">浏览器类型</Label>
              <Select value={browserType} onValueChange={setBrowserType}>
                <SelectTrigger id="env-browser">
                  <SelectValue placeholder="选择浏览器类型" />
                </SelectTrigger>
                <SelectContent>
                  {BROWSER_TYPES.map((t) => (
                    <SelectItem key={t.value} value={t.value}>
                      {t.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label htmlFor="env-status">状态</Label>
              <Select value={status} onValueChange={setStatus}>
                <SelectTrigger id="env-status">
                  <SelectValue placeholder="选择状态" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="ready">就绪</SelectItem>
                  <SelectItem value="running">运行中</SelectItem>
                  <SelectItem value="stopped">已停止</SelectItem>
                  <SelectItem value="error">错误</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <Label htmlFor="env-desc">描述</Label>
              <Input
                id="env-desc"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="环境描述（可选）"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="env-remark">备注</Label>
              <Input
                id="env-remark"
                value={remark}
                onChange={(e) => setRemark(e.target.value)}
                placeholder="备注信息（可选）"
              />
            </div>
          </div>
        </ScrollArea>
        <DialogFooter className="pt-4">
          <Button variant="outline" onClick={onClose}>
            取消
          </Button>
          <LoadingButton onClick={() => void handleSave()} isLoading={isSaving}>
            {editingEnv ? "保存修改" : "创建环境"}
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ============================================================
// 编辑名称对话框
// ============================================================

function EditNameDialog({
  isOpen,
  onClose,
  environment,
  onSaved,
}: {
  isOpen: boolean;
  onClose: () => void;
  environment: BwbrowserEnvironment | null;
  onSaved: () => void;
}) {
  const { updateEnv } = useBwbrowserEnvironments();
  const [isSaving, setIsSaving] = useState(false);
  const [name, setName] = useState("");

  useEffect(() => {
    if (!isOpen || !environment) return;
    setName(environment.name);
  }, [environment, isOpen]);

  const handleSave = async () => {
    if (!name.trim() || !environment) {
      toast.error("请输入环境名称");
      return;
    }
    setIsSaving(true);
    try {
      const uuid = extractBwbrowserEnvUuid(environment.id);
      if (!uuid) throw new Error("无效的环境 ID");
      await updateEnv(uuid, {
        name: name.trim(),
        browser_type: environment.browser_type,
      });
      toast.success("名称已更新");
      onSaved();
      onClose();
    } catch (err) {
      console.error("Failed to update name:", err);
      toast.error(String(err));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>编辑名称</DialogTitle>
        </DialogHeader>
        <div className="py-4">
          <div className="space-y-2">
            <Label htmlFor="edit-name-input">环境名称</Label>
            <Input
              id="edit-name-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="请输入环境名称"
              autoFocus
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            取消
          </Button>
          <LoadingButton onClick={() => void handleSave()} isLoading={isSaving}>
            保存
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ============================================================
// 编辑备注对话框
// ============================================================

function EditNoteDialog({
  isOpen,
  onClose,
  environment,
  onSaved,
}: {
  isOpen: boolean;
  onClose: () => void;
  environment: BwbrowserEnvironment | null;
  onSaved: () => void;
}) {
  const { updateEnv } = useBwbrowserEnvironments();
  const [isSaving, setIsSaving] = useState(false);
  const [remark, setRemark] = useState("");

  useEffect(() => {
    if (!isOpen || !environment) return;
    setRemark(environment.remark || "");
  }, [environment, isOpen]);

  const handleSave = async () => {
    if (!environment) return;
    setIsSaving(true);
    try {
      const uuid = extractBwbrowserEnvUuid(environment.id);
      if (!uuid) throw new Error("无效的环境 ID");
      await updateEnv(uuid, {
        name: environment.name,
        remark: remark.trim() || undefined,
        browser_type: environment.browser_type,
      });
      toast.success("备注已更新");
      onSaved();
      onClose();
    } catch (err) {
      console.error("Failed to update note:", err);
      toast.error(String(err));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>编辑备注</DialogTitle>
        </DialogHeader>
        <div className="py-4">
          <div className="space-y-2">
            <Label htmlFor="edit-note-input">备注内容</Label>
            <Input
              id="edit-note-input"
              value={remark}
              onChange={(e) => setRemark(e.target.value)}
              placeholder="请输入备注信息"
              autoFocus
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            取消
          </Button>
          <LoadingButton onClick={() => void handleSave()} isLoading={isSaving}>
            保存
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ============================================================
// 设置启动 URL 对话框
// ============================================================

function EditUrlDialog({
  isOpen,
  onClose,
  environment,
  onSaved,
}: {
  isOpen: boolean;
  onClose: () => void;
  environment: BwbrowserEnvironment | null;
  onSaved: () => void;
}) {
  const { updateEnv } = useBwbrowserEnvironments();
  const [isSaving, setIsSaving] = useState(false);
  const [url, setUrl] = useState("");

  useEffect(() => {
    if (!isOpen || !environment) return;
    setUrl(environment.start_urls?.[0] || "");
  }, [environment, isOpen]);

  const handleSave = async () => {
    if (!environment) return;
    setIsSaving(true);
    try {
      const uuid = extractBwbrowserEnvUuid(environment.id);
      if (!uuid) throw new Error("无效的环境 ID");
      const startUrls = url.trim() ? JSON.stringify([url.trim()]) : undefined;
      await updateEnv(uuid, {
        name: environment.name,
        start_urls: startUrls,
        browser_type: environment.browser_type,
      });
      toast.success("启动 URL 已更新");
      onSaved();
      onClose();
    } catch (err) {
      console.error("Failed to update URL:", err);
      toast.error(String(err));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>设置启动 URL</DialogTitle>
        </DialogHeader>
        <div className="py-4">
          <div className="space-y-2">
            <Label htmlFor="edit-url-input">启动 URL</Label>
            <Input
              id="edit-url-input"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://example.com"
              autoFocus
            />
            <p className="text-xs text-muted-foreground">
              环境启动时自动打开的网址（暂支持单个 URL）
            </p>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            取消
          </Button>
          <LoadingButton onClick={() => void handleSave()} isLoading={isSaving}>
            保存
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ============================================================
// Header 顶部栏组件
// ============================================================

function EnvironmentHeader({
  viewType,
  onViewTypeChange,
  onManageTags,
  onExport,
  onNewEnv,
  onSearchClick,
}: {
  viewType: ViewType;
  onViewTypeChange: (v: ViewType) => void;
  onManageTags: () => void;
  onExport: () => void;
  onNewEnv: () => void;
  onSearchClick: () => void;
}) {
  const viewTypes: { value: ViewType; label: string }[] = [
    { value: "all", label: "全部" },
    { value: "opened", label: "已打开" },
    { value: "trash", label: "回收站" },
  ];

  return (
    <header className="flex items-center justify-between border-b border-border px-4 py-2">
      <div className="flex items-center gap-4">
        {/* 视图类型分段按钮 */}
        <div className="flex items-center gap-1 rounded-xl bg-secondary p-1">
          {viewTypes.map((view) => (
            <button
              type="button"
              key={view.value}
              onClick={() => onViewTypeChange(view.value)}
              className={cn(
                "rounded-xl px-3 py-1.5 text-xs transition-all duration-200",
                viewType === view.value
                  ? "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:bg-muted hover:text-foreground",
              )}
            >
              {view.label}
            </button>
          ))}
        </div>
      </div>

      <div className="flex items-center gap-2">
        {/* 搜索按钮 */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              onClick={onSearchClick}
              className="h-8 gap-2"
            >
              <LuSearch className="size-4" />
              <div className="h-3 w-px bg-border" />
              <span className="text-[10px] font-medium opacity-60">Ctrl+K</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            <p>搜索环境</p>
          </TooltipContent>
        </Tooltip>

        {/* 同步器按钮 */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              onClick={() => showWipToast("同步器")}
              className="h-8 gap-2 text-xs font-medium"
            >
              <LuMousePointer2 className="size-4" />
              <span>同步器</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            <p>同步器</p>
          </TooltipContent>
        </Tooltip>

        {/* 标签管理按钮 */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              onClick={onManageTags}
              className="h-8 gap-2 text-xs font-medium"
            >
              <LuTag className="size-4" />
              <span>标签管理</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            <p>标签管理</p>
          </TooltipContent>
        </Tooltip>

        {/* 导出按钮 */}
        <Button
          variant="outline"
          size="sm"
          onClick={onExport}
          className="h-8 gap-2 text-xs font-medium"
        >
          <LuDownload className="size-4" />
          <span>导出</span>
        </Button>

        {/* 新建环境按钮 */}
        <Button
          size="sm"
          onClick={onNewEnv}
          className="h-8 gap-2 text-xs font-medium"
        >
          <LuPlus className="size-4" />
          <span>新建环境</span>
        </Button>
      </div>
    </header>
  );
}

// ============================================================
// Stats 统计栏组件
// ============================================================

function EnvironmentStats({
  total,
  running,
  proxyErrors,
  searchQuery,
  onSearchChange,
  onManageTags,
}: {
  total: number;
  running: number;
  proxyErrors: number;
  searchQuery: string;
  onSearchChange: (v: string) => void;
  onManageTags: () => void;
}) {
  const [searchExpanded, setSearchExpanded] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);

  const handleSearchToggle = () => {
    setSearchExpanded(true);
  };

  const handleSearchClose = useCallback(() => {
    if (!searchQuery.trim()) {
      setSearchExpanded(false);
    }
  }, [searchQuery]);

  useEffect(() => {
    if (searchExpanded && searchInputRef.current) {
      searchInputRef.current.focus();
    }
  }, [searchExpanded]);

  return (
    <section className="flex h-10 items-center gap-6 border-b border-border px-6">
      <div className="flex items-center gap-2 text-[11px] font-semibold">
        <span className="uppercase text-muted-foreground">总数:</span>
        <span className="font-mono text-foreground">{total}</span>
      </div>
      <div className="flex items-center gap-2 text-[11px] font-semibold">
        <span className="uppercase text-muted-foreground">运行中:</span>
        <span className="font-mono font-bold text-green-600">{running}</span>
      </div>
      <div className="flex items-center gap-2 text-[11px] font-semibold">
        <span className="uppercase text-muted-foreground">代理错误:</span>
        <span className="font-mono font-bold text-destructive">
          {proxyErrors}
        </span>
      </div>

      <div className="ml-auto flex items-center gap-2">
        {/* 搜索框 */}
        {searchExpanded ? (
          <div role="group" onMouseLeave={handleSearchClose}>
            <Input
              ref={searchInputRef}
              type="text"
              placeholder="搜索环境..."
              value={searchQuery}
              onChange={(e) => onSearchChange(e.target.value)}
              className="h-7 w-72 text-xs"
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  onSearchChange("");
                  setSearchExpanded(false);
                }
              }}
              onBlur={() => {
                setTimeout(() => {
                  if (!searchQuery.trim()) {
                    setSearchExpanded(false);
                  }
                }, 150);
              }}
            />
          </div>
        ) : (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="sm"
                onClick={handleSearchToggle}
                className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
              >
                <LuSearch className="size-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              <p>搜索</p>
            </TooltipContent>
          </Tooltip>
        )}

        {/* 分割线 */}
        <div className="h-5 w-px bg-border" />

        {/* 同步器按钮 */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => showWipToast("同步器")}
              className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
            >
              <LuMousePointer2 className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            <p>同步器</p>
          </TooltipContent>
        </Tooltip>

        {/* 分割线 */}
        <div className="h-5 w-px bg-border" />

        {/* 标签管理按钮 */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              onClick={onManageTags}
              className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
            >
              <LuTag className="size-4" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            <p>标签管理</p>
          </TooltipContent>
        </Tooltip>
      </div>
    </section>
  );
}

// ============================================================
// 批量操作浮动栏
// ============================================================

function BatchActionsBar({
  selectedCount,
  onBatchStart,
  onBatchStop,
  onMoveToGroup,
  onAssignTag,
  onDelete,
  onClearSelection,
  isTrashMode,
  onBatchRestore,
  onBatchPermanentDelete,
}: {
  selectedCount: number;
  onBatchStart: () => void;
  onBatchStop: () => void;
  onMoveToGroup: () => void;
  onAssignTag: () => void;
  onDelete: () => void;
  onClearSelection: () => void;
  isTrashMode: boolean;
  onBatchRestore: () => void;
  onBatchPermanentDelete: () => void;
}) {
  if (selectedCount === 0) return null;

  return (
    <div className="absolute bottom-6 left-1/2 z-50 flex -translate-x-1/2 items-center gap-1.5 rounded-lg border border-border/50 bg-background/95 px-3 py-2 shadow-lg backdrop-blur-xl">
      {isTrashMode ? (
        <>
          <Button
            variant="ghost"
            size="sm"
            onClick={onBatchRestore}
            className="gap-2 text-xs font-semibold"
          >
            <LuRotateCcw className="size-3.5" />
            批量恢复
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={onBatchPermanentDelete}
            className="gap-2 text-xs font-semibold text-destructive hover:text-destructive"
          >
            <LuTrash2 className="size-3.5" />
            批量永久删除
          </Button>
        </>
      ) : (
        <>
          <Button
            variant="ghost"
            size="sm"
            onClick={onBatchStart}
            className="gap-2 text-xs font-semibold"
          >
            <LuPlay className="size-3.5" />
            批量启动
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={onBatchStop}
            className="gap-2 text-xs font-semibold"
          >
            <LuSquare className="size-3.5" />
            批量停止
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={onMoveToGroup}
            className="gap-2 text-xs font-semibold"
          >
            <LuFolderTree className="size-3.5" />
            移动到分组
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={onAssignTag}
            className="gap-2 text-xs font-semibold"
          >
            <LuTag className="size-3.5" />
            分配标签
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={onDelete}
            className="gap-2 text-xs font-semibold text-destructive hover:text-destructive"
          >
            <LuTrash2 className="size-3.5" />
            删除
          </Button>
        </>
      )}
      <div className="mx-1 h-5 w-px bg-border/50" />
      <Button
        variant="ghost"
        size="sm"
        onClick={onClearSelection}
        className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
      >
        <LuX className="size-4" />
      </Button>
    </div>
  );
}

// ============================================================
// 底部分页栏
// ============================================================

function PaginationBar({
  currentPage,
  totalPages,
  totalItems,
  pageSize,
  onPageChange,
  onPageSizeChange,
}: {
  currentPage: number;
  totalPages: number;
  totalItems: number;
  pageSize: number;
  onPageChange: (page: number) => void;
  onPageSizeChange: (size: number) => void;
}) {
  const getPageNumbers = () => {
    const pages: (number | "ellipsis")[] = [];
    const maxVisible = 7;

    if (totalPages <= maxVisible) {
      for (let i = 1; i <= totalPages; i++) {
        pages.push(i);
      }
    } else {
      if (currentPage <= 3) {
        for (let i = 1; i <= 4; i++) pages.push(i);
        pages.push("ellipsis");
        pages.push(totalPages);
      } else if (currentPage >= totalPages - 2) {
        pages.push(1);
        pages.push("ellipsis");
        for (let i = totalPages - 3; i <= totalPages; i++) pages.push(i);
      } else {
        pages.push(1);
        pages.push("ellipsis");
        for (let i = currentPage - 1; i <= currentPage + 1; i++) pages.push(i);
        pages.push("ellipsis");
        pages.push(totalPages);
      }
    }

    return pages;
  };

  return (
    <div className="flex items-center justify-between border-t border-border px-6 py-2">
      <div className="flex items-center gap-4">
        <div className="whitespace-nowrap text-xs text-muted-foreground">
          第 {currentPage} / {totalPages} 页
        </div>
        <div className="flex items-center gap-2">
          <span className="whitespace-nowrap text-xs text-muted-foreground">
            每页
          </span>
          <Select
            value={String(pageSize)}
            onValueChange={(value) => onPageSizeChange(Number(value))}
          >
            <SelectTrigger className="h-7 w-[70px] text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {[10, 15, 20, 50].map((size) => (
                <SelectItem key={size} value={String(size)} className="text-xs">
                  {size}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className="whitespace-nowrap text-xs text-muted-foreground">
            共 {totalItems} 条
          </span>
        </div>
      </div>

      <div className="flex items-center gap-1">
        <Button
          variant="outline"
          size="sm"
          onClick={() => onPageChange(currentPage - 1)}
          disabled={currentPage === 1}
          className="h-8 w-8 p-0"
        >
          <LuChevronLeft className="size-4" />
        </Button>

        {getPageNumbers().map((page, index) =>
          page === "ellipsis" ? (
            <span key={index} className="px-2 text-xs text-muted-foreground">
              ...
            </span>
          ) : (
            <Button
              key={index}
              variant={page === currentPage ? "default" : "outline"}
              size="sm"
              onClick={() => onPageChange(page)}
              className="h-8 w-8 p-0 text-xs"
            >
              {page}
            </Button>
          ),
        )}

        <Button
          variant="outline"
          size="sm"
          onClick={() => onPageChange(currentPage + 1)}
          disabled={currentPage === totalPages}
          className="h-8 w-8 p-0"
        >
          <LuChevronRight className="size-4" />
        </Button>
      </div>
    </div>
  );
}

// ============================================================
// 操作下拉菜单
// ============================================================

function ActionsDropdown({
  environment: _environment,
  isRunning,
  isTrashMode,
  onEdit,
  onToggle,
  onExport,
  onAddProxy,
  onSelectProxy,
  onOpenAccountDialog,
  onEditName,
  onEditNote,
  onEditUrl,
  onEditTags,
  onEditCookies,
  onClearCache,
  onDelete,
  onRestore,
  onPermanentDelete,
}: {
  environment: BwbrowserEnvironment;
  isRunning: boolean;
  isTrashMode: boolean;
  onEdit: () => void;
  onToggle: () => void;
  onExport: () => void;
  onAddProxy: () => void;
  onSelectProxy: () => void;
  onOpenAccountDialog: () => void;
  onEditName: () => void;
  onEditNote: () => void;
  onEditUrl: () => void;
  onEditTags: () => void;
  onEditCookies: () => void;
  onClearCache: () => void;
  onDelete: () => void;
  onRestore: () => void;
  onPermanentDelete: () => void;
}) {
  if (isTrashMode) {
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
          >
            <LuChevronsUpDown className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-44">
          <DropdownMenuItem onClick={onRestore} className="cursor-pointer">
            <LuRotateCcw className="mr-2 size-4" />
            <span>恢复</span>
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={onPermanentDelete}
            className="cursor-pointer text-destructive focus:text-destructive"
          >
            <LuTrash2 className="mr-2 size-4" />
            <span>永久删除</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    );
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-7 w-7 p-0 text-muted-foreground hover:text-foreground"
        >
          <LuChevronsUpDown className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-44">
        {/* 编辑窗口 */}
        <DropdownMenuItem onClick={onEdit} className="cursor-pointer">
          <LuPencil className="mr-2 size-4" />
          <span>编辑窗口</span>
        </DropdownMenuItem>

        {/* 打开/关闭窗口 */}
        <DropdownMenuItem onClick={onToggle} className="cursor-pointer">
          {isRunning ? (
            <>
              <LuSquare className="mr-2 size-4" />
              <span>关闭窗口</span>
            </>
          ) : (
            <>
              <LuPlay className="mr-2 size-4" />
              <span>打开窗口</span>
            </>
          )}
        </DropdownMenuItem>

        {/* 导出窗口 */}
        <DropdownMenuItem onClick={onExport} className="cursor-pointer">
          <LuDownload className="mr-2 size-4" />
          <span>导出窗口</span>
        </DropdownMenuItem>

        <DropdownMenuSeparator />

        {/* 代理操作 - 二级菜单 */}
        <DropdownMenuSub>
          <DropdownMenuSubTrigger className="cursor-pointer">
            <LuNetwork className="mr-2 size-4" />
            <span>代理操作</span>
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-40">
            <DropdownMenuItem onClick={onAddProxy} className="cursor-pointer">
              <LuPlus className="mr-2 size-4" />
              <span>添加代理</span>
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={onSelectProxy}
              className="cursor-pointer"
            >
              <LuList className="mr-2 size-4" />
              <span>选择代理</span>
            </DropdownMenuItem>
          </DropdownMenuSubContent>
        </DropdownMenuSub>

        {/* 平台账号 */}
        <DropdownMenuItem
          onClick={onOpenAccountDialog}
          className="cursor-pointer"
        >
          <LuUser className="mr-2 size-4" />
          <span>平台账号</span>
        </DropdownMenuItem>

        <DropdownMenuSeparator />

        {/* 编辑名称 */}
        <DropdownMenuItem onClick={onEditName} className="cursor-pointer">
          <LuPencil className="mr-2 size-4" />
          <span>编辑名称</span>
        </DropdownMenuItem>

        {/* 编辑备注 */}
        <DropdownMenuItem onClick={onEditNote} className="cursor-pointer">
          <LuStickyNote className="mr-2 size-4" />
          <span>编辑备注</span>
        </DropdownMenuItem>

        {/* 设置启动 URL */}
        <DropdownMenuItem onClick={onEditUrl} className="cursor-pointer">
          <LuGlobe className="mr-2 size-4" />
          <span>设置 URL</span>
        </DropdownMenuItem>

        {/* 编辑标签 */}
        <DropdownMenuItem onClick={onEditTags} className="cursor-pointer">
          <LuTag className="mr-2 size-4" />
          <span>编辑标签</span>
        </DropdownMenuItem>

        <DropdownMenuSeparator />

        {/* 编辑 Cookies */}
        <DropdownMenuItem onClick={onEditCookies} className="cursor-pointer">
          <LuCookie className="mr-2 size-4" />
          <span>编辑 Cookies</span>
        </DropdownMenuItem>

        {/* 清除缓存 */}
        <DropdownMenuItem onClick={onClearCache} className="cursor-pointer">
          <LuEraser className="mr-2 size-4" />
          <span>清除缓存</span>
        </DropdownMenuItem>

        <DropdownMenuSeparator />

        {/* 删除环境 */}
        <DropdownMenuItem
          onClick={onDelete}
          className="cursor-pointer text-destructive focus:text-destructive"
        >
          <LuTrash2 className="mr-2 size-4" />
          <span>删除环境</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ============================================================
// 空状态组件
// ============================================================

function EmptyState({
  viewType,
  searchQuery,
}: {
  viewType: ViewType;
  searchQuery: string;
}) {
  const getContent = () => {
    if (searchQuery) {
      return {
        title: "没有找到匹配的环境",
        description: "试试其他关键词",
      };
    }
    switch (viewType) {
      case "opened":
        return {
          title: "暂无已打开的环境",
          description: "启动环境后会显示在这里",
        };
      case "trash":
        return {
          title: "回收站为空",
          description: "删除的环境会暂时保存在这里",
        };
      default:
        return {
          title: "还没有环境",
          description: "点击右上角新建第一个环境",
        };
    }
  };

  const content = getContent();

  return (
    <div className="flex flex-col items-center justify-center py-10 px-4">
      <div className="mb-4 flex size-16 items-center justify-center rounded-full bg-gradient-to-br from-blue-500/20 to-indigo-500/20">
        <LuMonitor className="size-8 text-blue-500/60" />
      </div>
      <h4 className="mb-1 text-sm font-medium text-foreground">
        {content.title}
      </h4>
      <p className="max-w-[240px] text-center text-xs text-muted-foreground">
        {content.description}
      </p>
    </div>
  );
}

// ============================================================
// 主组件
// ============================================================

export function BwbrowserEnvManagementDialog({
  isOpen,
  onClose,
  embedded = false,
}: BwbrowserEnvManagementDialogProps) {
  const { isLoggedIn: isBwbrowserLoggedIn } = useBwbrowserAuth();
  const { isSuperAdmin: isSuperAdminPerm } = useBwbrowserPermissions();
  const { companies, selectedCompanyId, setSelectedCompanyId } =
    useBwbrowserCompany(isSuperAdminPerm, isBwbrowserLoggedIn);

  const { environments, isLoading, error, refresh, deleteEnv } =
    useBwbrowserEnvironments(selectedCompanyId);

  // 视图状态
  const [viewType, setViewType] = useState<ViewType>("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [sorting, setSorting] = useState<SortingState>([]);
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(PAGE_SIZE);

  // 选择状态
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  // 对话框状态
  const [formDialogOpen, setFormDialogOpen] = useState(false);
  const [editingEnv, setEditingEnv] = useState<BwbrowserEnvironment | null>(
    null,
  );
  const [deleteTarget, setDeleteTarget] = useState<BwbrowserEnvironment | null>(
    null,
  );
  const [isDeleting, setIsDeleting] = useState(false);

  // 编辑名称/备注/URL 对话框
  const [editNameDialogOpen, setEditNameDialogOpen] = useState(false);
  const [editNoteDialogOpen, setEditNoteDialogOpen] = useState(false);
  const [editUrlDialogOpen, setEditUrlDialogOpen] = useState(false);
  const [actionTargetEnv, setActionTargetEnv] =
    useState<BwbrowserEnvironment | null>(null);

  // 监听 Ctrl+K 快捷键
  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "k") {
        e.preventDefault();
        // 聚焦搜索（通过 Stats 组件中的状态来实现，这里简单 toast 提示）
        showWipToast("搜索对话框");
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen]);

  // 过滤后的环境列表
  const filteredEnvironments = useMemo(() => {
    let result = environments;

    // 视图过滤
    if (viewType === "opened") {
      result = result.filter((e) => e.status.toLowerCase() === "running");
    } else if (viewType === "trash") {
      // 回收站视图：UI 占位，暂时显示空列表
      result = [];
    }

    // 搜索过滤
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      result = result.filter(
        (e) =>
          e.name.toLowerCase().includes(q) ||
          e.browser_type.toLowerCase().includes(q) ||
          e.description?.toLowerCase().includes(q) ||
          e.remark?.toLowerCase().includes(q) ||
          e.owner_name?.toLowerCase().includes(q),
      );
    }

    return result;
  }, [environments, searchQuery, viewType]);

  // 统计数据
  const stats = useMemo(() => {
    const total = environments.length;
    const running = environments.filter(
      (e) => e.status.toLowerCase() === "running",
    ).length;
    const proxyErrors = 0; // 暂不支持代理错误统计
    return { total, running, proxyErrors };
  }, [environments]);

  // 分页计算
  const totalItems = filteredEnvironments.length;
  const totalPages = Math.max(1, Math.ceil(totalItems / pageSize));
  const paginatedEnvironments = useMemo(() => {
    const start = (currentPage - 1) * pageSize;
    return filteredEnvironments.slice(start, start + pageSize);
  }, [filteredEnvironments, currentPage, pageSize]);

  // 当过滤结果变化时，重置到第一页
  useEffect(() => {
    setCurrentPage(1);
    setSelectedIds(new Set());
  }, []);

  // 表格列定义
  const columns: ColumnDef<BwbrowserEnvironment>[] = useMemo(
    () => [
      {
        id: "select",
        header: ({ table }) => (
          <div className="flex items-center justify-center">
            <Checkbox
              checked={
                table.getIsAllPageRowsSelected() ||
                (table.getIsSomePageRowsSelected() && "indeterminate")
              }
              onCheckedChange={(value) =>
                table.toggleAllPageRowsSelected(!!value)
              }
              aria-label="全选"
            />
          </div>
        ),
        cell: ({ row }) => (
          <div className="flex items-center justify-center">
            <Checkbox
              checked={row.getIsSelected()}
              onCheckedChange={(value) => row.toggleSelected(!!value)}
              aria-label="选择行"
              onClick={(e) => e.stopPropagation()}
            />
          </div>
        ),
        size: 48,
        enableSorting: false,
      },
      {
        id: "index",
        header: "序号",
        cell: ({ row }) => {
          const startIndex = (currentPage - 1) * pageSize;
          return (
            <span className="font-mono text-xs text-muted-foreground">
              {startIndex + row.index + 1}
            </span>
          );
        },
        size: 64,
        enableSorting: false,
      },
      {
        id: "fingerprint",
        header: "",
        cell: ({ row }) => {
          const env = row.original;
          const browser = env.browser_type.toLowerCase();
          return (
            <div className="flex items-center">
              <div className="inline-flex items-center rounded-md border bg-background px-2 py-1">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="leading-none">
                      <LuGlobe className="size-4 text-blue-500" />
                    </span>
                  </TooltipTrigger>
                  <TooltipContent>
                    <p>
                      {BROWSER_TYPES.find((b) => b.value === browser)?.label ||
                        env.browser_type}
                    </p>
                  </TooltipContent>
                </Tooltip>
              </div>
            </div>
          );
        },
        size: 80,
        enableSorting: false,
      },
      {
        id: "name",
        header: "名称",
        accessorKey: "name",
        cell: ({ row }) => {
          const env = row.original;
          return (
            <div className="min-w-0">
              {env.description ? (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="cursor-help font-medium">{env.name}</span>
                  </TooltipTrigger>
                  <TooltipContent>
                    <p>{env.description}</p>
                  </TooltipContent>
                </Tooltip>
              ) : (
                <span className="font-medium">{env.name}</span>
              )}
              {env.remark && (
                <p className="truncate text-xs text-muted-foreground">
                  {env.remark}
                </p>
              )}
            </div>
          );
        },
        size: 200,
      },
      {
        id: "proxy",
        header: "代理",
        cell: () => (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => showWipToast("代理配置")}
                className="h-6 w-6 p-0 text-muted-foreground hover:text-foreground"
              >
                <LuNetwork className="size-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              <p>未设置代理</p>
            </TooltipContent>
          </Tooltip>
        ),
        size: 64,
        enableSorting: false,
      },
      {
        id: "account",
        header: "账号",
        cell: () => (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => showWipToast("平台账号")}
                className="h-6 w-6 p-0 text-muted-foreground hover:text-foreground"
              >
                <LuUserPlus className="size-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>
              <p>添加账号</p>
            </TooltipContent>
          </Tooltip>
        ),
        size: 80,
        enableSorting: false,
      },
      {
        id: "group",
        header: () => (
          <div className="flex items-center gap-1 whitespace-nowrap">
            <span className="shrink-0">分组</span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-5 w-5 p-0 text-muted-foreground hover:text-foreground"
                >
                  <LuChevronDown className="size-3" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="w-32">
                <DropdownMenuItem
                  onClick={() => showWipToast("分组筛选")}
                  className="cursor-pointer text-xs"
                >
                  全部分组
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => showWipToast("分组筛选")}
                  className="cursor-pointer text-xs"
                >
                  默认分组
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        ),
        cell: () => (
          <button
            type="button"
            onClick={() => showWipToast("分组管理")}
            className="flex items-center gap-1 whitespace-nowrap rounded border border-dashed border-purple-300 bg-purple-50 px-2 py-0.5 text-[10px] text-purple-600 transition-opacity hover:opacity-80"
          >
            <LuFolderTree className="size-3" />
            未设置
          </button>
        ),
        size: 100,
        enableSorting: false,
      },
      {
        id: "tag",
        header: () => (
          <div className="flex items-center gap-1 whitespace-nowrap">
            <span className="shrink-0">标签</span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-5 w-5 p-0 text-muted-foreground hover:text-foreground"
                >
                  <LuChevronDown className="size-3" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="w-32">
                <DropdownMenuItem
                  onClick={() => showWipToast("标签筛选")}
                  className="cursor-pointer text-xs"
                >
                  全部标签
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        ),
        cell: () => (
          <button
            type="button"
            onClick={() => showWipToast("标签管理")}
            className="flex items-center gap-1 whitespace-nowrap rounded border border-dashed border-blue-300 bg-blue-50 px-2 py-0.5 text-[10px] text-blue-600 transition-opacity hover:opacity-80"
          >
            <LuTag className="size-3" />
            未设置
          </button>
        ),
        size: 100,
        enableSorting: false,
      },
      {
        id: "lastAction",
        header: "最后操作",
        accessorKey: "updated_at",
        cell: ({ row }) => (
          <span className="font-mono text-xs text-muted-foreground">
            {formatDateTime(row.original.updated_at || row.original.created_at)}
          </span>
        ),
        size: 160,
      },
      {
        id: "actions",
        header: "操作",
        cell: ({ row }) => {
          const env = row.original;
          const isRunning = env.status.toLowerCase() === "running";
          const isTrashMode = viewType === "trash";

          const handleEdit = () => {
            setEditingEnv(env);
            setFormDialogOpen(true);
          };

          const handleToggle = () => {
            showWipToast(isRunning ? "停止环境" : "启动环境");
          };

          const handleDelete = () => {
            setDeleteTarget(env);
          };

          const handleEditName = () => {
            setActionTargetEnv(env);
            setEditNameDialogOpen(true);
          };

          const handleEditNote = () => {
            setActionTargetEnv(env);
            setEditNoteDialogOpen(true);
          };

          const handleEditUrl = () => {
            setActionTargetEnv(env);
            setEditUrlDialogOpen(true);
          };

          const handleRestore = () => {
            showWipToast("恢复环境");
          };

          const handlePermanentDelete = () => {
            setDeleteTarget(env);
          };

          return (
            <div className="flex items-center gap-1">
              {!isTrashMode && (
                <Button
                  variant={isRunning ? "destructive" : "default"}
                  size="sm"
                  onClick={handleToggle}
                  className="gap-1.5 px-3 py-1 text-[11px] font-bold"
                >
                  {isRunning ? (
                    <>
                      <LuSquare className="size-3" />
                      停止
                    </>
                  ) : (
                    <>
                      <LuPlay className="size-3" />
                      启动
                    </>
                  )}
                </Button>
              )}
              <ActionsDropdown
                environment={env}
                isRunning={isRunning}
                isTrashMode={isTrashMode}
                onEdit={handleEdit}
                onToggle={handleToggle}
                onExport={() => showWipToast("导出环境")}
                onAddProxy={() => showWipToast("添加代理")}
                onSelectProxy={() => showWipToast("选择代理")}
                onOpenAccountDialog={() => showWipToast("平台账号")}
                onEditName={handleEditName}
                onEditNote={handleEditNote}
                onEditUrl={handleEditUrl}
                onEditTags={() => showWipToast("编辑标签")}
                onEditCookies={() => showWipToast("编辑 Cookies")}
                onClearCache={() => showWipToast("清除缓存")}
                onDelete={handleDelete}
                onRestore={handleRestore}
                onPermanentDelete={handlePermanentDelete}
              />
            </div>
          );
        },
        size: 180,
        enableSorting: false,
      },
    ],
    [currentPage, pageSize, viewType],
  );

  // 当前页的 rowSelection 状态
  const rowSelection = useMemo(
    () =>
      Object.fromEntries(
        paginatedEnvironments
          .map((env, idx) => [String(idx), selectedIds.has(env.id)] as const)
          .filter(([, selected]) => selected),
      ),
    [paginatedEnvironments, selectedIds],
  );

  const handleRowSelectionChange = useCallback(
    (updater: any) => {
      // 计算新的 rowSelection
      const prevSelection = rowSelection;
      const newSelection =
        typeof updater === "function" ? updater(prevSelection) : updater;

      const newSelectedIds = new Set<string>();
      // 保留非当前页的选择
      selectedIds.forEach((id) => {
        const isCurrentPage = paginatedEnvironments.some((e) => e.id === id);
        if (!isCurrentPage) {
          newSelectedIds.add(id);
        }
      });
      // 添加当前页的选择
      Object.entries(newSelection).forEach(([key, selected]) => {
        const env = paginatedEnvironments[Number(key)];
        if (env && selected) {
          newSelectedIds.add(env.id);
        }
      });
      setSelectedIds(newSelectedIds);
    },
    [rowSelection, selectedIds, paginatedEnvironments],
  );

  const table = useReactTable({
    data: paginatedEnvironments,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    onSortingChange: setSorting,
    onRowSelectionChange: handleRowSelectionChange,
    state: {
      sorting,
      rowSelection,
    },
    manualPagination: true,
  });

  // 删除确认
  const handleConfirmDelete = async () => {
    if (!deleteTarget) return;
    const uuid = extractBwbrowserEnvUuid(deleteTarget.id);
    if (!uuid) return;
    setIsDeleting(true);
    try {
      await deleteEnv(uuid);
      toast.success("环境已删除");
      setDeleteTarget(null);
      setSelectedIds((prev) => {
        const next = new Set(prev);
        next.delete(deleteTarget.id);
        return next;
      });
    } catch (err) {
      console.error("Failed to delete env:", err);
      toast.error(String(err));
    } finally {
      setIsDeleting(false);
    }
  };

  const handleNewEnv = () => {
    setEditingEnv(null);
    setFormDialogOpen(true);
  };

  const handleRefresh = () => {
    void refresh();
  };

  const handleManageTags = () => {
    showWipToast("标签管理");
  };

  const handleExport = () => {
    showWipToast("导出环境");
  };

  const handleSearchClick = () => {
    showWipToast("搜索对话框");
  };

  // 批量操作
  const handleBatchStart = () => {
    showWipToast("批量启动");
  };

  const handleBatchStop = () => {
    showWipToast("批量停止");
  };

  const handleMoveToGroup = () => {
    showWipToast("移动到分组");
  };

  const handleAssignTag = () => {
    showWipToast("分配标签");
  };

  const handleBatchDelete = () => {
    if (selectedIds.size === 0) return;
    toast.info(`已选择 ${selectedIds.size} 个环境，批量删除功能开发中`);
  };

  const handleClearSelection = () => {
    setSelectedIds(new Set());
  };

  const handleBatchRestore = () => {
    showWipToast("批量恢复");
  };

  const handleBatchPermanentDelete = () => {
    showWipToast("批量永久删除");
  };

  const handlePageChange = (page: number) => {
    setCurrentPage(page);
  };

  const handlePageSizeChange = (size: number) => {
    setPageSize(size);
  };

  const isTrashMode = viewType === "trash";

  const mainContent = (
    <>
      {/* Header 顶部栏 */}
      <EnvironmentHeader
        viewType={viewType}
        onViewTypeChange={setViewType}
        onManageTags={handleManageTags}
        onExport={handleExport}
        onNewEnv={handleNewEnv}
        onSearchClick={handleSearchClick}
      />

      {/* 公司切换栏（仅超级管理员可见） */}
      {isSuperAdminPerm && companies.length > 0 && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-4 py-1.5">
          <span className="shrink-0 text-xs font-medium text-muted-foreground">
            切换公司
          </span>
          <Select
            value={selectedCompanyId !== null ? String(selectedCompanyId) : "my"}
            onValueChange={(val) =>
              setSelectedCompanyId(val === "my" ? null : Number(val))
            }
          >
            <SelectTrigger className="h-7 w-auto min-w-[140px] max-w-[260px] text-xs">
              <SelectValue placeholder="选择公司" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="my">我的公司</SelectItem>
              {companies.map((company) => (
                <SelectItem key={company.id} value={String(company.id)}>
                  {company.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      {/* Stats 统计栏 */}
      <EnvironmentStats
        total={stats.total}
        running={stats.running}
        proxyErrors={stats.proxyErrors}
        searchQuery={searchQuery}
        onSearchChange={setSearchQuery}
        onManageTags={handleManageTags}
      />

      {/* Table 表格区域 */}
      <div className="relative flex-1 overflow-hidden">
        {isLoading ? (
          <div className="flex h-64 items-center justify-center">
            <p className="text-sm text-muted-foreground">加载中...</p>
          </div>
        ) : error ? (
          <div className="flex h-64 flex-col items-center justify-center gap-2">
            <p className="text-sm text-destructive">加载失败：{error}</p>
            <Button variant="outline" size="sm" onClick={handleRefresh}>
              重试
            </Button>
          </div>
        ) : filteredEnvironments.length === 0 ? (
          <EmptyState viewType={viewType} searchQuery={searchQuery} />
        ) : (
          <ScrollArea className="h-full max-h-[calc(90vh-180px)]">
            <Table>
              <TableHeader>
                {table.getHeaderGroups().map((headerGroup) => (
                  <TableRow key={headerGroup.id}>
                    {headerGroup.headers.map((header) => (
                      <TableHead
                        key={header.id}
                        style={{ width: header.getSize() }}
                        className="h-9"
                      >
                        {flexRender(
                          header.column.columnDef.header,
                          header.getContext(),
                        )}
                      </TableHead>
                    ))}
                  </TableRow>
                ))}
              </TableHeader>
              <TableBody>
                {table.getRowModel().rows.map((row) => (
                  <TableRow
                    key={row.id}
                    data-state={row.getIsSelected() && "selected"}
                    className={cn(
                      row.original.status.toLowerCase() === "running" &&
                        "bg-green-50/50 hover:bg-green-50/70",
                    )}
                  >
                    {row.getVisibleCells().map((cell) => (
                      <TableCell
                        key={cell.id}
                        style={{ width: cell.column.getSize() }}
                      >
                        {flexRender(
                          cell.column.columnDef.cell,
                          cell.getContext(),
                        )}
                      </TableCell>
                    ))}
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </ScrollArea>
        )}

        {/* 批量操作浮动栏 */}
        <BatchActionsBar
          selectedCount={selectedIds.size}
          onBatchStart={handleBatchStart}
          onBatchStop={handleBatchStop}
          onMoveToGroup={handleMoveToGroup}
          onAssignTag={handleAssignTag}
          onDelete={handleBatchDelete}
          onClearSelection={handleClearSelection}
          isTrashMode={isTrashMode}
          onBatchRestore={handleBatchRestore}
          onBatchPermanentDelete={handleBatchPermanentDelete}
        />
      </div>

      {/* Pagination 底部分页栏 */}
      <PaginationBar
        currentPage={currentPage}
        totalPages={totalPages}
        totalItems={totalItems}
        pageSize={pageSize}
        onPageChange={handlePageChange}
        onPageSizeChange={handlePageSizeChange}
      />
    </>
  );

  return (
    <>
      {embedded ? (
        <div className="flex h-full min-h-0 w-full flex-col overflow-hidden p-0">
          {mainContent}
        </div>
      ) : (
        <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
          <DialogContent className="flex max-h-[90vh] max-w-6xl flex-col p-0">
            {mainContent}
          </DialogContent>
        </Dialog>
      )}

      {/* 新增/编辑环境对话框 */}
      <EnvFormDialog
        isOpen={formDialogOpen}
        onClose={() => setFormDialogOpen(false)}
        editingEnv={editingEnv}
        onSaved={handleRefresh}
      />

      {/* 编辑名称对话框 */}
      <EditNameDialog
        isOpen={editNameDialogOpen}
        onClose={() => setEditNameDialogOpen(false)}
        environment={actionTargetEnv}
        onSaved={handleRefresh}
      />

      {/* 编辑备注对话框 */}
      <EditNoteDialog
        isOpen={editNoteDialogOpen}
        onClose={() => setEditNoteDialogOpen(false)}
        environment={actionTargetEnv}
        onSaved={handleRefresh}
      />

      {/* 设置启动 URL 对话框 */}
      <EditUrlDialog
        isOpen={editUrlDialogOpen}
        onClose={() => setEditUrlDialogOpen(false)}
        environment={actionTargetEnv}
        onSaved={handleRefresh}
      />

      {/* 删除确认对话框 */}
      <Dialog
        open={deleteTarget !== null}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>
              {isTrashMode ? "确认永久删除" : "确认删除环境"}
            </DialogTitle>
            <DialogDescription>
              {isTrashMode
                ? `确定要永久删除环境「${deleteTarget?.name}」吗？此操作不可撤销。`
                : `确定要删除环境「${deleteTarget?.name}」吗？删除后将移至回收站。`}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDeleteTarget(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              onClick={() => void handleConfirmDelete()}
              disabled={isDeleting}
            >
              {isDeleting ? "删除中..." : isTrashMode ? "永久删除" : "删除"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
