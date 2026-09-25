"use client";

import {
  ColumnDef,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  SortingState,
  useReactTable,
} from "@tanstack/react-table";
import { invoke } from "@tauri-apps/api/core";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuActivity,
  LuCheck,
  LuChevronDown,
  LuChevronLeft,
  LuChevronRight,
  LuChevronsUpDown,
  LuDownload,
  LuLoaderCircle,
  LuMonitor,
  LuNetwork,
  LuPencil,
  LuPlay,
  LuRocket,
  LuSearch,
  LuTrash2,
  LuUpload,
  LuX,
  LuZap,
} from "react-icons/lu";
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
import { useBwbrowserPermissions } from "@/hooks/use-bwbrowser-permissions";
import {
  type BwbrowserProxy,
  extractBwbrowserProxyId,
  useBwbrowserProxies,
} from "@/hooks/use-bwbrowser-proxies";
import { useProxyEvents } from "@/hooks/use-proxy-events";
import { translateBackendError } from "@/lib/backend-errors";
import { runProxyCheck, useProxyCheck } from "@/lib/proxy-check-store";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";

// ==================== 类型定义 ====================

interface BwbrowserProxyManagementDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** 嵌入式模式：不显示 Dialog 外壳，直接渲染内容 */
  embedded?: boolean;
}

type ProxyStatus = "healthy" | "unreachable" | "unknown";

// ==================== 常量 ====================

const PROTOCOL_LABELS: Record<string, string> = {
  http: "HTTP",
  https: "HTTPS",
  socks5: "SOCKS5",
  ss: "SS",
  vless: "VLESS",
  trojan: "Trojan",
  ssh: "SSH",
};

const PROTOCOL_COLORS: Record<string, string> = {
  http: "bg-blue-500/10 text-blue-500",
  https: "bg-green-500/10 text-green-500",
  socks5: "bg-purple-500/10 text-purple-500",
  vless: "bg-cyan-500/10 text-cyan-500",
  trojan: "bg-pink-500/10 text-pink-500",
  ss: "bg-amber-500/10 text-amber-500",
  ssh: "bg-orange-500/10 text-orange-500",
};

const PAGE_SIZE = 22;

// ==================== 工具函数 ====================

function getProtocolLabel(type: string): string {
  return PROTOCOL_LABELS[type.toLowerCase()] || type.toUpperCase();
}

function getProtocolColor(type: string): string {
  return (
    PROTOCOL_COLORS[type.toLowerCase()] || "bg-muted text-muted-foreground"
  );
}

/** 将 BwbrowserProxy 转换为 StoredProxy 格式以供 ProxyCheck 使用 */
function toStoredProxyFormat(p: BwbrowserProxy) {
  return {
    id: p.id,
    name: p.name,
    proxy_settings: {
      proxy_type: p.proxy_type,
      host: p.host,
      port: p.port,
      username: p.username,
      password: p.password,
      vless_uri: p.vless_uri,
    },
    is_cloud_managed: true,
    geo_country: p.country,
    geo_city: p.city,
  };
}

// ==================== 状态徽章 ====================

function StatusBadge({ status }: { status: ProxyStatus }) {
  switch (status) {
    case "healthy":
      return (
        <span className="inline-flex items-center gap-1 rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium text-emerald-500">
          <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
          健康
        </span>
      );
    case "unreachable":
      return (
        <span className="inline-flex items-center gap-1 rounded-full bg-destructive/10 px-2 py-0.5 text-[10px] font-medium text-destructive">
          <span className="h-1.5 w-1.5 rounded-full bg-destructive" />
          不可达
        </span>
      );
    default:
      return (
        <span className="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[10px] font-medium text-muted-foreground">
          <span className="h-1.5 w-1.5 rounded-full bg-muted-foreground" />
          未知
        </span>
      );
  }
}

// ==================== 类型徽章 ====================

function TypeBadge({ type }: { type: string }) {
  return (
    <span
      className={cn(
        "inline-flex rounded px-2 py-0.5 text-[10px] font-medium uppercase",
        getProtocolColor(type),
      )}
    >
      {getProtocolLabel(type)}
    </span>
  );
}

// ==================== 延迟显示 ====================

function LatencyCell({ proxy }: { proxy: BwbrowserProxy }) {
  const stored = toStoredProxyFormat(proxy);
  const { result, checking } = useProxyCheck(stored);

  if (checking) {
    return (
      <div className="flex items-center gap-1 text-xs text-muted-foreground">
        <LuLoaderCircle className="h-3 w-3 animate-spin" />
        <span className="font-mono">检测中</span>
      </div>
    );
  }

  if (result?.is_valid && typeof result.latency_ms === "number") {
    return (
      <div className="flex items-center gap-1 text-xs">
        <LuZap className="h-3 w-3 text-emerald-500" />
        <span className="font-mono">{result.latency_ms} ms</span>
      </div>
    );
  }

  return <span className="text-xs text-muted-foreground">-</span>;
}

// ==================== 状态单元格 ====================

function StatusCell({ proxy }: { proxy: BwbrowserProxy }) {
  const stored = toStoredProxyFormat(proxy);
  const { result, checking } = useProxyCheck(stored);

  if (checking) {
    return (
      <span className="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[10px] font-medium text-muted-foreground">
        <LuLoaderCircle className="h-3 w-3 animate-spin" />
        检测中
      </span>
    );
  }

  if (result) {
    return <StatusBadge status={result.is_valid ? "healthy" : "unreachable"} />;
  }

  return <StatusBadge status="unknown" />;
}

// ==================== 测试按钮 ====================

function TestButton({
  proxy,
  onTested,
}: {
  proxy: BwbrowserProxy;
  onTested?: () => void;
}) {
  const { t } = useTranslation();
  const stored = toStoredProxyFormat(proxy);
  const { checking } = useProxyCheck(stored);

  const handleTest = useCallback(async () => {
    try {
      const result = await runProxyCheck(stored, (error) =>
        translateBackendError(t, error),
      );
      // 云端代理测试成功且探测到国家时，自动回传国家/城市/时区到云端，
      // 这样云端账号的国家就会自动显示（无需手动填写）
      if (result?.is_valid && result.country && stored.is_cloud_managed) {
        const numId = extractBwbrowserProxyId(stored.id);
        if (numId !== null) {
          // 等待回传完成后再刷新列表，确保该节点国家立即更新显示
          try {
            await invoke("bwbrowser_sync_proxy_geo", {
              proxyId: numId,
              country: result.country,
              city: result.city ?? undefined,
              timezone: result.timezone ?? undefined,
            });
            onTested?.();
          } catch (e) {
            console.error("回传国家到云端失败:", e);
          }
        }
      }
    } catch (error) {
      // runProxyCheck 内部已经处理了 toast
      console.error("Proxy check failed:", error);
    }
  }, [stored, t, onTested]);

  return (
    <button
      type="button"
      onClick={() => void handleTest()}
      disabled={checking}
      className="flex h-7 w-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
      title="测试代理"
    >
      {checking ? (
        <LuLoaderCircle className="h-4 w-4 animate-spin" />
      ) : (
        <LuPlay className="h-4 w-4" />
      )}
    </button>
  );
}

// ==================== 类型筛选表头 ====================

function TypeHeader({
  proxyTypeFilter,
  onProxyTypeFilterChange,
}: {
  proxyTypeFilter: string;
  onProxyTypeFilterChange: (value: string) => void;
}) {
  const getLabel = () => {
    switch (proxyTypeFilter) {
      case "http":
        return "HTTP";
      case "https":
        return "HTTPS";
      case "socks5":
        return "SOCKS5";
      case "vless":
        return "VLESS";
      case "trojan":
        return "Trojan";
      case "ss":
        return "SS";
      default:
        return "";
    }
  };

  return (
    <div className="flex items-center gap-1 whitespace-nowrap">
      <span className="shrink-0">类型</span>
      {proxyTypeFilter === "all" ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="flex h-5 w-5 shrink-0 items-center justify-center rounded transition-colors hover:bg-accent"
            >
              <LuChevronDown className="h-3 w-3" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="w-32">
            <DropdownMenuItem
              onClick={() => onProxyTypeFilterChange("all")}
              className="cursor-pointer text-xs"
            >
              全部
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => onProxyTypeFilterChange("http")}
              className="cursor-pointer text-xs"
            >
              HTTP
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => onProxyTypeFilterChange("https")}
              className="cursor-pointer text-xs"
            >
              HTTPS
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => onProxyTypeFilterChange("socks5")}
              className="cursor-pointer text-xs"
            >
              SOCKS5
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => onProxyTypeFilterChange("vless")}
              className="cursor-pointer text-xs"
            >
              VLESS
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => onProxyTypeFilterChange("trojan")}
              className="cursor-pointer text-xs"
            >
              Trojan
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => onProxyTypeFilterChange("ss")}
              className="cursor-pointer text-xs"
            >
              SS
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ) : (
        <>
          <span
            className="max-w-[80px] truncate rounded bg-primary/10 px-1 text-[9px] font-bold text-primary"
            title={getLabel()}
          >
            {getLabel()}
          </span>
          <button
            type="button"
            onClick={() => onProxyTypeFilterChange("all")}
            className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-destructive transition-colors hover:bg-destructive/20"
          >
            <LuX className="h-3 w-3" />
          </button>
        </>
      )}
    </div>
  );
}

// ==================== 分页组件 ====================

function PaginationBar({
  currentPage,
  totalPages,
  onPageChange,
}: {
  currentPage: number;
  totalPages: number;
  onPageChange: (page: number) => void;
}) {
  const getPageNumbers = () => {
    const pages: (number | "ellipsis")[] = [];
    const maxVisible = 7;

    if (totalPages <= maxVisible) {
      for (let i = 1; i <= totalPages; i++) {
        pages.push(i);
      }
    } else if (currentPage <= 3) {
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

    return pages;
  };

  if (totalPages <= 1) {
    return (
      <div className="flex items-center justify-between border-t border-border bg-background/10 px-6 py-2 backdrop-blur-2xl">
        <div className="whitespace-nowrap text-xs text-muted-foreground">
          第 {currentPage} / {totalPages} 页
        </div>
        <div className="w-32" />
      </div>
    );
  }

  return (
    <div className="flex items-center justify-between border-t border-border bg-background/10 px-6 py-2 backdrop-blur-2xl">
      <div className="whitespace-nowrap text-xs text-muted-foreground">
        第 {currentPage} / {totalPages} 页
      </div>
      <div className="flex items-center gap-1">
        <button
          type="button"
          onClick={() => currentPage > 1 && onPageChange(currentPage - 1)}
          disabled={currentPage === 1}
          className={cn(
            "flex h-7 w-7 items-center justify-center rounded-md border border-border text-muted-foreground transition-colors",
            currentPage === 1
              ? "cursor-not-allowed opacity-50"
              : "hover:bg-accent hover:text-foreground",
          )}
          aria-label="上一页"
        >
          <LuChevronLeft className="h-4 w-4" />
        </button>
        {getPageNumbers().map((page, index) =>
          page === "ellipsis" ? (
            <span
              key={`ellipsis-${index}`}
              className="flex h-7 w-7 items-center justify-center text-xs text-muted-foreground"
            >
              ...
            </span>
          ) : (
            <button
              type="button"
              key={page}
              onClick={() => onPageChange(page)}
              className={cn(
                "flex h-7 min-w-7 items-center justify-center rounded-md border text-xs font-medium transition-colors",
                page === currentPage
                  ? "border-primary bg-primary text-primary-foreground"
                  : "border-border text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
            >
              {page}
            </button>
          ),
        )}
        <button
          type="button"
          onClick={() =>
            currentPage < totalPages && onPageChange(currentPage + 1)
          }
          disabled={currentPage === totalPages}
          className={cn(
            "flex h-7 w-7 items-center justify-center rounded-md border border-border text-muted-foreground transition-colors",
            currentPage === totalPages
              ? "cursor-not-allowed opacity-50"
              : "hover:bg-accent hover:text-foreground",
          )}
          aria-label="下一页"
        >
          <LuChevronRight className="h-4 w-4" />
        </button>
      </div>
    </div>
  );
}

// ==================== 主组件 ====================

export function BwbrowserProxyManagementDialog({
  isOpen,
  onClose,
  embedded = false,
}: BwbrowserProxyManagementDialogProps) {
  const { t } = useTranslation();
  const { isLoggedIn: isBwbrowserLoggedIn } = useBwbrowserAuth();
  const { isSuperAdmin: isSuperAdminPerm } = useBwbrowserPermissions();
  const { companies, selectedCompanyId, setSelectedCompanyId } =
    useBwbrowserCompany(isSuperAdminPerm, isBwbrowserLoggedIn);

  const { proxies, isLoading, error, refresh, deleteProxy } =
    useBwbrowserProxies(selectedCompanyId);
  const { storedProxies } = useProxyEvents();

  const [sorting, setSorting] = useState<SortingState>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchDialogOpen, setSearchDialogOpen] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [proxyTypeFilter, setProxyTypeFilter] = useState("all");
  const [currentPage, setCurrentPage] = useState(1);

  const [showForm, setShowForm] = useState(false);
  const [editingProxy, setEditingProxy] = useState<BwbrowserProxy | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<BwbrowserProxy | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);

  // Ctrl+K 打开搜索
  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key === "k") {
        event.preventDefault();
        setSearchDialogOpen(true);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen]);

  // 过滤 + 搜索 + 类型筛选
  const filteredProxies = useMemo(() => {
    let result = proxies;

    // 类型筛选
    if (proxyTypeFilter !== "all") {
      result = result.filter(
        (p) => p.proxy_type.toLowerCase() === proxyTypeFilter.toLowerCase(),
      );
    }

    // 搜索
    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      result = result.filter(
        (p) =>
          p.name.toLowerCase().includes(q) ||
          p.host.toLowerCase().includes(q) ||
          p.proxy_type.toLowerCase().includes(q) ||
          p.country?.toLowerCase().includes(q) ||
          p.city?.toLowerCase().includes(q),
      );
    }

    return result;
  }, [proxies, searchQuery, proxyTypeFilter]);

  // 统计数据
  const stats = useMemo(() => {
    const healthy = 0;
    const unreachable = 0;
    // 注意：实际状态需要从 proxy-check-store 获取
    // 这里先基于是否有缓存结果来粗略估计，更精确的在每行中展示
    return {
      total: filteredProxies.length,
      healthy,
      unreachable,
    };
  }, [filteredProxies]);

  // 分页
  const totalPages = Math.max(1, Math.ceil(filteredProxies.length / PAGE_SIZE));
  const paginatedProxies = useMemo(() => {
    const start = (currentPage - 1) * PAGE_SIZE;
    return filteredProxies.slice(start, start + PAGE_SIZE);
  }, [filteredProxies, currentPage]);

  // 当过滤结果变化时重置到第一页
  useEffect(() => {
    setCurrentPage(1);
  }, []);

  const selectedCount = selectedIds.size;

  // === 操作处理 ===

  const handleCreate = () => {
    setEditingProxy(null);
    setShowForm(true);
  };

  const handleEdit = useCallback((proxy: BwbrowserProxy) => {
    setEditingProxy(proxy);
    setShowForm(true);
  }, []);

  const handleDeleteClick = useCallback((proxy: BwbrowserProxy) => {
    setDeleteTarget(proxy);
  }, []);

  // 导入到本地
  const handlePullToLocal = useCallback(async (proxy: BwbrowserProxy) => {
    const numId = extractBwbrowserProxyId(proxy.id);
    if (numId === null) return;
    try {
      await invoke("bwbrowser_pull_proxy_to_local", { proxyId: numId });
      showSuccessToast("已导入到本地代理");
    } catch (err) {
      showErrorToast(`导入失败: ${String(err)}`);
    }
  }, []);

  // 从本地上传（打开选择本地代理的对话框）
  const [pushDialogOpen, setPushDialogOpen] = useState(false);
  const [selectedLocalProxyId, setSelectedLocalProxyId] = useState<
    string | null
  >(null);
  const [isPushing, setIsPushing] = useState(false);

  const handlePushToCloud = useCallback(async () => {
    if (!selectedLocalProxyId) return;
    setIsPushing(true);
    try {
      await invoke("bwbrowser_push_local_proxy_to_cloud", {
        localProxyId: selectedLocalProxyId,
      });
      showSuccessToast("已上传到云端");
      setPushDialogOpen(false);
      setSelectedLocalProxyId(null);
      void refresh();
    } catch (err) {
      showErrorToast(`上传失败: ${String(err)}`);
    } finally {
      setIsPushing(false);
    }
  }, [selectedLocalProxyId, refresh]);

  const handleConfirmDelete = async () => {
    if (!deleteTarget) return;
    const numId = extractBwbrowserProxyId(deleteTarget.id);
    if (numId === null) return;

    setIsDeleting(true);
    try {
      await deleteProxy(numId);
      showSuccessToast("代理已删除");
      setDeleteTarget(null);
      setSelectedIds((prev) => {
        const next = new Set(prev);
        next.delete(deleteTarget.id);
        return next;
      });
    } catch (err) {
      showErrorToast(String(err));
    } finally {
      setIsDeleting(false);
    }
  };

  const handleImport = () => {
    showErrorToast("功能开发中", { description: "导入功能即将上线，敬请期待" });
  };

  const handleBatchImport = () => {
    showErrorToast("功能开发中", {
      description: "批量导入功能即将上线，敬请期待",
    });
  };

  const handleExport = () => {
    showErrorToast("功能开发中", { description: "导出功能即将上线，敬请期待" });
  };

  const handleTestSelected = async () => {
    if (selectedIds.size === 0) return;
    const targets = filteredProxies.filter((p) => selectedIds.has(p.id));
    if (targets.length === 0) {
      showErrorToast("没有可测试的代理", { description: "选中的代理已不存在" });
      return;
    }

    let pass = 0;
    let fail = 0;
    let synced = 0;
    let skipped = 0;

    // 并发测试：同时最多 CONCURRENCY 个代理并行探测，可用且探测到国家的自动回传到云端
    const CONCURRENCY = 5;
    const total = targets.length;
    let cursor = 0;

    async function testOne(proxy: (typeof targets)[number]) {
      const stored = toStoredProxyFormat(proxy);
      try {
        const result = await runProxyCheck(stored, (error) =>
          translateBackendError(t, error),
        );
        if (result?.is_valid) {
          pass += 1;
          if (result.country && stored.is_cloud_managed) {
            const numId = extractBwbrowserProxyId(stored.id);
            if (numId !== null) {
              try {
                await invoke("bwbrowser_sync_proxy_geo", {
                  proxyId: numId,
                  country: result.country,
                  city: result.city ?? undefined,
                  timezone: result.timezone ?? undefined,
                });
                synced += 1;
              } catch {
                // 回传失败不阻断整体汇总，仅计数
              }
            }
          }
        } else {
          fail += 1;
        }
      } catch {
        fail += 1;
      }
    }

    async function worker() {
      while (cursor < total) {
        const next = cursor++;
        if (next >= total) break;
        await testOne(targets[next]);
      }
    }

    // 启动 CONCURRENCY 个 worker 并行消费任务队列
    await Promise.allSettled(
      Array.from({ length: Math.min(CONCURRENCY, total) }, () => worker()),
    );
    skipped = total - pass - fail;

    // 批量检测完成，刷新列表以更新各国国家显示
    void refresh();

    if (pass > 0) {
      showSuccessToast(`批量测试完成：可用 ${pass} / ${targets.length}`, {
        description:
          synced > 0
            ? `已回传 ${synced} 个代理国家到云端；可用 ${pass} 个，失败 ${fail} 个${skipped ? `，跳过 ${skipped} 个` : ""}`
            : `可用 ${pass} 个，失败 ${fail} 个${skipped ? `，跳过 ${skipped} 个` : ""}`,
      });
    } else {
      showErrorToast(`批量测试完成：全部不可用`, {
        description: `失败 ${fail} 个${skipped ? `，跳过 ${skipped} 个` : ""}`,
      });
    }
  };

  // === 表格列定义 ===

  const columns = useMemo<ColumnDef<BwbrowserProxy>[]>(
    () => [
      {
        id: "select",
        header: ({ table }) => (
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
        ),
        cell: ({ row }) => (
          <Checkbox
            checked={row.getIsSelected()}
            onCheckedChange={(value) => row.toggleSelected(!!value)}
            aria-label="选择行"
          />
        ),
        size: 36,
        enableSorting: false,
      },
      {
        id: "name",
        accessorKey: "name",
        header: "名称",
        size: 200,
        cell: ({ row }) => {
          const p = row.original;
          return (
            <div className="flex items-center gap-2">
              <LuNetwork className="h-4 w-4 shrink-0 text-primary" />
              <div className="min-w-0">
                <span className="truncate text-xs font-medium text-foreground">
                  {p.name}
                </span>
                <div className="mt-0.5 font-mono text-[10px] text-muted-foreground">
                  ID: {p.id.replace("bwbrowser_", "").slice(0, 8)}
                </div>
              </div>
            </div>
          );
        },
      },
      {
        id: "address",
        header: "地址",
        size: 180,
        cell: ({ row }) => {
          const p = row.original;
          return (
            <div>
              <div className="font-mono text-xs text-foreground">
                {p.host}:{p.port}
              </div>
              {p.username && (
                <div className="mt-0.5 text-xs text-muted-foreground">
                  账号: {p.username}
                </div>
              )}
            </div>
          );
        },
      },
      {
        id: "type",
        header: () => (
          <TypeHeader
            proxyTypeFilter={proxyTypeFilter}
            onProxyTypeFilterChange={setProxyTypeFilter}
          />
        ),
        size: 96,
        cell: ({ row }) => <TypeBadge type={row.original.proxy_type} />,
        enableSorting: false,
      },
      {
        id: "country",
        accessorKey: "country",
        header: "国家",
        size: 96,
        cell: ({ row }) => {
          const p = row.original;
          const country = p.country || p.city || "-";
          return <span className="text-xs text-foreground">{country}</span>;
        },
      },
      {
        id: "latency",
        header: "延迟",
        size: 96,
        cell: ({ row }) => <LatencyCell proxy={row.original} />,
        enableSorting: false,
      },
      {
        id: "status",
        header: "状态",
        size: 96,
        cell: ({ row }) => <StatusCell proxy={row.original} />,
        enableSorting: false,
      },
      {
        id: "linkedEnvironments",
        header: "关联环境",
        size: 96,
        cell: () => (
          <div className="flex items-center gap-1.5">
            <LuMonitor className="h-3 w-3 text-muted-foreground" />
            <span className="font-mono text-xs text-muted-foreground">0</span>
          </div>
        ),
        enableSorting: false,
      },
      {
        id: "createdAt",
        header: "创建时间",
        size: 112,
        cell: () => <span className="text-xs text-muted-foreground">-</span>,
        enableSorting: false,
      },
      {
        id: "actions",
        header: "操作",
        size: 112,
        cell: ({ row }) => {
          const p = row.original;
          return (
            <div className="flex items-center gap-0.5">
              <TestButton proxy={p} onTested={() => void refresh()} />
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="flex h-7 w-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    title="更多操作"
                  >
                    <LuChevronsUpDown className="h-4 w-4" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-36">
                  <DropdownMenuItem
                    onClick={() => handleEdit(p)}
                    className="cursor-pointer"
                  >
                    <LuPencil className="mr-2 h-4 w-4" />
                    <span className="text-xs">编辑</span>
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onClick={() => handlePullToLocal(p)}
                    className="cursor-pointer"
                  >
                    <LuDownload className="mr-2 h-4 w-4" />
                    <span className="text-xs">导入到本地</span>
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onClick={() => handleDeleteClick(p)}
                    className="cursor-pointer text-destructive focus:text-destructive"
                  >
                    <LuTrash2 className="mr-2 h-4 w-4" />
                    <span className="text-xs">删除</span>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          );
        },
        enableSorting: false,
      },
    ],
    [
      proxyTypeFilter,
      handleEdit,
      handleDeleteClick,
      handlePullToLocal,
      refresh,
    ],
  );

  const table = useReactTable({
    data: paginatedProxies,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    onSortingChange: setSorting,
    state: {
      sorting,
      rowSelection: Object.fromEntries(
        Array.from(selectedIds).map((id) => [id, true]),
      ),
    },
    onRowSelectionChange: (updater) => {
      if (typeof updater === "function") {
        const newSelection = updater(
          Object.fromEntries(Array.from(selectedIds).map((id) => [id, true])),
        );
        setSelectedIds(new Set(Object.keys(newSelection)));
      }
    },
    getRowId: (row) => row.id,
  });

  const mainContent = (
    <>
      {/* ========== Header 顶部栏 ========== */}
      <header className="flex items-center justify-between border-b border-border bg-background/10 px-4 py-2 backdrop-blur-2xl">
        <div className="flex items-center gap-4">
          <span className="text-sm font-semibold">云端代理中心</span>
        </div>
        <div className="ml-auto flex items-center gap-2">
          {/* 搜索按钮 */}
          <button
            type="button"
            onClick={() => setSearchDialogOpen(true)}
            className="flex h-8 items-center justify-center gap-2 rounded-md border border-border bg-background px-3 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="搜索"
          >
            <LuSearch className="h-4 w-4" />
            <div className="h-3 w-px bg-border" />
            <span className="text-[10px] font-medium opacity-60">Ctrl+K</span>
          </button>
          {/* 导入 */}
          <button
            type="button"
            onClick={handleImport}
            className="flex h-8 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="导入"
          >
            <LuUpload className="h-3.5 w-3.5" />
            导入
          </button>
          {/* 批量导入 */}
          <button
            type="button"
            onClick={handleBatchImport}
            className="flex h-8 items-center gap-1.5 rounded-md border border-blue-500/30 bg-blue-500/5 px-3 text-xs font-medium text-blue-600 transition-colors hover:border-blue-500/50 hover:bg-blue-500/10 dark:text-blue-400"
            title="批量导入"
          >
            <LuRocket className="h-3.5 w-3.5" />
            批量导入
          </button>
          {/* 从本地上传 */}
          <button
            type="button"
            onClick={() => setPushDialogOpen(true)}
            className="flex h-8 items-center gap-1.5 rounded-md border border-emerald-500/30 bg-emerald-500/5 px-3 text-xs font-medium text-emerald-600 transition-colors hover:border-emerald-500/50 hover:bg-emerald-500/10 dark:text-emerald-400"
            title="从本地上传到云端"
          >
            <LuUpload className="h-3.5 w-3.5" />
            上传本地
          </button>
          {/* 导出 */}
          <button
            type="button"
            onClick={handleExport}
            className="flex h-8 items-center gap-1.5 rounded-md border border-border bg-background px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="导出"
          >
            <LuDownload className="h-3.5 w-3.5" />
            导出
          </button>
          {/* 创建 - 主按钮 */}
          <button
            type="button"
            onClick={handleCreate}
            className="h-8 rounded-md border border-primary bg-primary px-4 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
          >
            创建
          </button>
        </div>
      </header>

      {/* ========== Stats 统计栏 ========== */}
      <section className="flex h-10 items-center gap-6 border-b border-border bg-background px-6">
        <div className="flex items-center gap-2 text-[11px] font-semibold">
          <span className="uppercase text-muted-foreground">TOTAL:</span>
          <span className="font-mono text-foreground">{stats.total}</span>
        </div>
        <div className="flex items-center gap-2 text-[11px] font-semibold">
          <span className="uppercase text-muted-foreground">HEALTHY:</span>
          <span className="font-mono font-bold text-emerald-500">
            {stats.healthy}
          </span>
        </div>
        <div className="flex items-center gap-2 text-[11px] font-semibold">
          <span className="uppercase text-muted-foreground">UNREACHABLE:</span>
          <span className="font-mono font-bold text-destructive">
            {stats.unreachable}
          </span>
        </div>
        <div className="flex items-center gap-2 text-[11px] font-semibold">
          <span className="uppercase text-muted-foreground">LOCAL:</span>
          <span className="font-mono font-bold text-blue-500">
            {storedProxies.length}
          </span>
        </div>
        <div className="ml-auto">
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8"
            onClick={handleTestSelected}
            disabled={selectedCount === 0}
            title={
              selectedCount > 0
                ? `测试选中的 ${selectedCount} 个代理`
                : "请先选择要测试的代理"
            }
          >
            <LuActivity className="h-4 w-4" />
          </Button>
        </div>
      </section>

      {/* ========== 公司切换栏（仅超级管理员可见） ========== */}
      {isSuperAdminPerm && companies.length > 0 && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-4 py-1.5">
          <span className="shrink-0 text-xs font-medium text-muted-foreground">
            切换公司
          </span>
          <Select
            value={
              selectedCompanyId !== null ? String(selectedCompanyId) : "my"
            }
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

      {/* ========== Table 表格区域 ========== */}
      <div className="flex-1 overflow-hidden">
        {isLoading && proxies.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <div className="flex flex-col items-center gap-2 text-muted-foreground">
              <LuLoaderCircle className="h-6 w-6 animate-spin" />
              <span>正在从云端加载代理...</span>
            </div>
          </div>
        ) : error && proxies.length === 0 ? (
          <div className="flex h-full items-center justify-center">
            <div className="flex flex-col items-center gap-2 text-muted-foreground">
              <p className="text-destructive">{error}</p>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void refresh()}
              >
                重试
              </Button>
            </div>
          </div>
        ) : filteredProxies.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3">
            <div className="flex h-16 w-16 items-center justify-center rounded-full bg-gradient-to-br from-blue-500/20 to-indigo-500/20">
              <LuNetwork className="h-8 w-8 text-blue-500/60" />
            </div>
            <h4 className="text-sm font-medium text-foreground">
              {searchQuery || proxyTypeFilter !== "all"
                ? "没有找到匹配的代理"
                : "云端还没有代理"}
            </h4>
            <p className="max-w-[240px] text-center text-xs text-muted-foreground">
              {searchQuery || proxyTypeFilter !== "all"
                ? "试试其他关键词或筛选条件"
                : "点击右上角创建，或使用本地代理"}
            </p>
            {storedProxies.length > 0 && (
              <div className="mt-2 w-full max-w-2xl overflow-auto border border-border rounded-lg">
                <div className="border-b border-border bg-muted/50 px-3 py-1.5 text-xs font-semibold text-muted-foreground">
                  本地代理 ({storedProxies.length})
                </div>
                <div className="divide-y divide-border max-h-[200px] overflow-auto">
                  {storedProxies.map((proxy) => (
                    <div
                      key={proxy.id}
                      className="flex items-center justify-between px-3 py-2"
                    >
                      <div className="flex flex-col">
                        <span className="text-xs font-medium">
                          {proxy.name}
                        </span>
                        <span className="font-mono text-[10px] text-muted-foreground">
                          {proxy.proxy_settings.host}:
                          {proxy.proxy_settings.port}
                          {" · "}
                          {proxy.proxy_settings.proxy_type}
                        </span>
                      </div>
                      <span className="rounded bg-emerald-500/10 px-1.5 py-0.5 text-[9px] font-medium text-emerald-500">
                        本地
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        ) : (
          <div className="h-full overflow-auto">
            <Table>
              <TableHeader className="sticky top-0 z-10 bg-background">
                {table.getHeaderGroups().map((headerGroup) => (
                  <TableRow key={headerGroup.id}>
                    {headerGroup.headers.map((header) => (
                      <TableHead
                        key={header.id}
                        style={{ width: header.getSize() }}
                        className={cn(
                          "h-9",
                          header.column.getCanSort() &&
                            "cursor-pointer select-none",
                        )}
                        onClick={header.column.getToggleSortingHandler()}
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
          </div>
        )}
      </div>

      {/* ========== Pagination 底部分页栏 ========== */}
      <PaginationBar
        currentPage={currentPage}
        totalPages={totalPages}
        onPageChange={setCurrentPage}
      />
    </>
  );

  return (
    <>
      {embedded ? (
        <div className="flex h-full min-h-0 w-full flex-col gap-0 overflow-hidden">
          {mainContent}
        </div>
      ) : (
        <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
          <DialogContent className="flex max-h-[90vh] w-[1000px] max-w-[95vw] flex-col gap-0 overflow-hidden p-0">
            {mainContent}
          </DialogContent>
        </Dialog>
      )}

      {/* ========== 搜索对话框 ========== */}
      <SearchDialog
        open={searchDialogOpen}
        onOpenChange={setSearchDialogOpen}
        searchQuery={searchQuery}
        onSearchChange={setSearchQuery}
      />

      {/* ========== 代理表单对话框 ========== */}
      <ProxyFormDialog
        isOpen={showForm}
        onClose={() => setShowForm(false)}
        editingProxy={editingProxy}
        onSaved={() => {
          setShowForm(false);
        }}
      />

      {/* ========== 从本地上传对话框 ========== */}
      <Dialog
        open={pushDialogOpen}
        onOpenChange={(open) => !open && setPushDialogOpen(false)}
      >
        <DialogContent className="w-[480px] max-w-[90vw]">
          <DialogHeader>
            <DialogTitle>上传本地代理到云端</DialogTitle>
            <DialogDescription>
              选择一个本地代理，将其上传到云端代理中心
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-2">
            <Label>选择本地代理</Label>
            <div className="max-h-[260px] overflow-y-auto border rounded-md">
              <LocalProxyList
                selectedId={selectedLocalProxyId}
                onSelect={setSelectedLocalProxyId}
              />
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={() => setPushDialogOpen(false)}>
              取消
            </Button>
            <Button
              onClick={handlePushToCloud}
              disabled={!selectedLocalProxyId || isPushing}
            >
              {isPushing ? "上传中..." : "上传到云端"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* ========== 删除确认对话框 ========== */}
      <Dialog
        open={deleteTarget !== null}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>确认删除代理</DialogTitle>
            <DialogDescription>
              确定要删除代理「{deleteTarget?.name}」吗？此操作不可撤销。
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
              {isDeleting ? "删除中..." : "删除"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

// ==================== 本地代理选择列表 ====================

function LocalProxyList({
  selectedId,
  onSelect,
}: {
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const { storedProxies } = useProxyEvents();

  if (storedProxies.length === 0) {
    return (
      <div className="p-6 text-center text-sm text-muted-foreground">
        暂无本地代理
      </div>
    );
  }

  return (
    <div className="divide-y divide-border">
      {storedProxies.map((proxy) => (
        <button
          key={proxy.id}
          type="button"
          onClick={() => onSelect(proxy.id)}
          className={cn(
            "w-full flex items-center justify-between px-3 py-2 text-left transition-colors",
            selectedId === proxy.id ? "bg-primary/10" : "hover:bg-muted/50",
          )}
        >
          <div className="flex flex-col">
            <span className="text-xs font-medium">{proxy.name}</span>
            <span className="font-mono text-[10px] text-muted-foreground">
              {proxy.proxy_settings.host}:{proxy.proxy_settings.port}
              {" · "}
              {proxy.proxy_settings.proxy_type}
            </span>
          </div>
          {selectedId === proxy.id && (
            <LuCheck className="h-4 w-4 text-primary" />
          )}
        </button>
      ))}
    </div>
  );
}

// ==================== 代理URL解析 ====================

interface ParsedProxy {
  protocol: string;
  host: string;
  port: string;
  username: string;
  password: string;
  fullUri?: string;
  fragment?: string;
}

const ADVANCED_PROTOCOLS = ["vless", "trojan", "ss"];

/**
 * 解析代理URL字符串，支持多种格式：
 * - protocol://user:pass@host:port
 * - user:pass@host:port
 * - host:port:user:pass (冒号分隔)
 * - host:port
 * - vless://uuid@host:port?params#name (完整 VLESS URI)
 * - trojan://password@host:port?params#name (完整 Trojan URI)
 * - ss://method:password@host:port#name (完整 SS URI)
 */
function parseProxyUrl(raw: string): ParsedProxy | null {
  const text = raw.trim();
  if (!text) return null;

  // 格式1: vless://uuid@host:port?params#name (完整 VLESS URI)
  const vlessFullMatch = text.match(
    /^vless:\/\/([^@?#]+)@([^:?#]+):(\d+)(?:\?([^#]*))?(?:#(.*))?$/i,
  );
  if (vlessFullMatch) {
    return {
      protocol: "vless",
      host: vlessFullMatch[2],
      port: vlessFullMatch[3],
      username: "",
      password: "",
      fullUri: text,
      fragment: vlessFullMatch[5] || "",
    };
  }

  // 格式2: trojan://password@host:port?params#name (完整 Trojan URI)
  const trojanFullMatch = text.match(
    /^trojan:\/\/([^@?#]+)@([^:?#]+):(\d+)(?:\?([^#]*))?(?:#(.*))?$/i,
  );
  if (trojanFullMatch) {
    return {
      protocol: "trojan",
      host: trojanFullMatch[2],
      port: trojanFullMatch[3],
      username: "",
      password: "",
      fullUri: text,
      fragment: trojanFullMatch[5] || "",
    };
  }

  // 格式3: ss://base64@host:port#name 或 ss://method:password@host:port#name
  const ssFullMatch = text.match(
    /^ss:\/\/([^@?#]+)@([^:?#]+):(\d+)(?:\?([^#]*))?(?:#(.*))?$/i,
  );
  if (ssFullMatch) {
    return {
      protocol: "ss",
      host: ssFullMatch[2],
      port: ssFullMatch[3],
      username: "",
      password: "",
      fullUri: text,
      fragment: ssFullMatch[5] || "",
    };
  }

  // 格式4: protocol://user:pass@host:port
  const protoMatch = text.match(
    /^(https?|socks5?|ssh):\/\/(?:([^:@:]+)(?::([^@]+))?@)?([^:]+):(\d+)$/i,
  );
  if (protoMatch) {
    const protocol = protoMatch[1].toLowerCase();
    return {
      protocol:
        protocol === "sock4" || protocol === "socks4" ? "socks5" : protocol,
      host: protoMatch[4],
      port: protoMatch[5],
      username: protoMatch[2] ? decodeURIComponent(protoMatch[2]) : "",
      password: protoMatch[3] ? decodeURIComponent(protoMatch[3]) : "",
    };
  }

  // 格式5: protocol://host:port (无认证)
  const protoNoAuthMatch = text.match(
    /^(https?|socks5?|ssh):\/\/([^:]+):(\d+)$/i,
  );
  if (protoNoAuthMatch) {
    const protocol = protoNoAuthMatch[1].toLowerCase();
    return {
      protocol:
        protocol === "sock4" || protocol === "socks4" ? "socks5" : protocol,
      host: protoNoAuthMatch[2],
      port: protoNoAuthMatch[3],
      username: "",
      password: "",
    };
  }

  // 格式6: user:pass@host:port (无协议)
  const userAtHostMatch = text.match(/^(?:([^:@:]+):([^@]+)@)?([^:]+):(\d+)$/);
  if (userAtHostMatch) {
    return {
      protocol: "http",
      host: userAtHostMatch[3],
      port: userAtHostMatch[4],
      username: userAtHostMatch[1]
        ? decodeURIComponent(userAtHostMatch[1])
        : "",
      password: userAtHostMatch[2]
        ? decodeURIComponent(userAtHostMatch[2])
        : "",
    };
  }

  // 格式7: host:port:user:pass (冒号分隔，常见于代理列表)
  const colonParts = text.split(":");
  if (colonParts.length === 4) {
    const port = colonParts[1];
    if (/^\d+$/.test(port)) {
      return {
        protocol: "http",
        host: colonParts[0],
        port,
        username: colonParts[2],
        password: colonParts[3],
      };
    }
  }

  // 格式8: host:port
  if (colonParts.length === 2 && /^\d+$/.test(colonParts[1])) {
    return {
      protocol: "http",
      host: colonParts[0],
      port: colonParts[1],
      username: "",
      password: "",
    };
  }

  return null;
}

// ==================== 代理表单对话框 ====================

interface ProxyFormDialogProps {
  isOpen: boolean;
  onClose: () => void;
  editingProxy: BwbrowserProxy | null;
  onSaved: () => void;
}

function ProxyFormDialog({
  isOpen,
  onClose,
  editingProxy,
  onSaved,
}: ProxyFormDialogProps) {
  const { createProxy, updateProxy } = useBwbrowserProxies();
  const [isSaving, setIsSaving] = useState(false);

  const [name, setName] = useState("");
  const [proxyType, setProxyType] = useState("http");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [vlessUri, setVlessUri] = useState("");
  const [country, setCountry] = useState("");
  const [city, setCity] = useState("");

  const isAdvanced = ADVANCED_PROTOCOLS.includes(proxyType);

  // 编辑时填充数据
  useEffect(() => {
    if (!isOpen) return;
    if (editingProxy) {
      setName(editingProxy.name);
      setProxyType(editingProxy.proxy_type);
      setHost(editingProxy.host);
      setPort(String(editingProxy.port));
      setUsername(editingProxy.username || "");
      setPassword(editingProxy.password || "");
      setVlessUri(editingProxy.vless_uri || "");
      setCountry(editingProxy.country || "");
      setCity(editingProxy.city || "");
    } else {
      setName("");
      setProxyType("http");
      setHost("");
      setPort("");
      setUsername("");
      setPassword("");
      setVlessUri("");
      setCountry("");
      setCity("");
    }
  }, [editingProxy, isOpen]);

  const handlePasteParse = useCallback(async () => {
    let text = "";
    try {
      text = await readText();
    } catch {
      showErrorToast("无法读取剪贴板，请手动粘贴");
      return;
    }
    text = text.trim();
    if (!text) {
      showErrorToast("剪贴板为空");
      return;
    }

    const parsed = parseProxyUrl(text);
    if (!parsed) {
      showErrorToast("无法解析代理格式");
      return;
    }

    const validTypes = [
      "http",
      "https",
      "socks5",
      "ss",
      "vless",
      "trojan",
      "ssh",
    ];
    if (validTypes.includes(parsed.protocol)) {
      setProxyType(parsed.protocol);
    }
    setHost(parsed.host);
    setPort(parsed.port);

    if (parsed.fullUri) {
      setVlessUri(parsed.fullUri);
      setUsername("");
      setPassword("");
    } else {
      setVlessUri("");
      if (parsed.username) setUsername(parsed.username);
      if (parsed.password) setPassword(parsed.password);
    }

    if (parsed.fragment) {
      setName(parsed.fragment);
    } else if (!name.trim()) {
      setName(`${parsed.host}:${parsed.port}`);
    }

    showSuccessToast(
      `已解析: ${parsed.protocol.toUpperCase()} ${parsed.host}:${parsed.port}`,
    );
  }, [name]);

  const handleSave = async () => {
    // 高级协议（VLESS/Trojan/SS）：如果只填了 URI 没填 host/port，
    // 从 URI 中自动解析
    let finalHost = host;
    let finalPort = port;
    if (isAdvanced && vlessUri.trim()) {
      const parsed = parseProxyUrl(vlessUri.trim());
      if (parsed) {
        if (!finalHost.trim()) finalHost = parsed.host;
        if (!finalPort.trim()) finalPort = parsed.port;
      }
    }

    if (!name.trim() || !finalHost.trim() || !finalPort.trim()) {
      showErrorToast("请填写名称、主机和端口");
      return;
    }

    const portNum = parseInt(finalPort, 10);
    if (isNaN(portNum) || portNum <= 0 || portNum > 65535) {
      showErrorToast("端口号无效");
      return;
    }

    if (isAdvanced && !vlessUri.trim()) {
      showErrorToast("请粘贴完整协议链接");
      return;
    }

    setIsSaving(true);
    try {
      const data: {
        proxy_name: string;
        proxy_type: string;
        host: string;
        port: number;
        username?: string;
        password?: string;
        country?: string;
        city?: string;
        protocol_config?: string;
      } = {
        proxy_name: name.trim(),
        proxy_type: proxyType,
        host: finalHost.trim(),
        port: portNum,
        country: country.trim() || undefined,
        city: city.trim() || undefined,
      };

      if (isAdvanced) {
        data.protocol_config = vlessUri.trim();
      } else {
        data.username = username.trim() || undefined;
        data.password = password || undefined;
      }

      if (editingProxy) {
        const numId = extractBwbrowserProxyId(editingProxy.id);
        if (numId !== null) {
          await updateProxy(numId, data);
        }
      } else {
        await createProxy(data);
      }
      showSuccessToast(editingProxy ? "代理已更新" : "代理已创建");
      onSaved();
    } catch (err) {
      showErrorToast(String(err));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-h-[85vh] max-w-lg">
        <DialogHeader>
          <DialogTitle>{editingProxy ? "编辑代理" : "新建代理"}</DialogTitle>
        </DialogHeader>
        <div className="max-h-[60vh] overflow-y-auto pr-1">
          <div className="grid gap-4 py-4">
            {/* 一键粘贴自动分析 */}
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="default"
                onClick={() => void handlePasteParse()}
                className="w-full gap-2"
              >
                <LuZap className="h-4 w-4" />
                一键粘贴自动分析
              </Button>
            </div>

            <div className="space-y-2">
              <Label>代理名称</Label>
              <Input
                placeholder="例如：美国-洛杉矶-01"
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label>协议类型</Label>
                <Select value={proxyType} onValueChange={setProxyType}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="http">HTTP</SelectItem>
                    <SelectItem value="https">HTTPS</SelectItem>
                    <SelectItem value="socks5">SOCKS5</SelectItem>
                    <SelectItem value="ss">SS (Shadowsocks)</SelectItem>
                    <SelectItem value="vless">VLESS</SelectItem>
                    <SelectItem value="trojan">Trojan</SelectItem>
                    <SelectItem value="ssh">SSH</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-2">
                <Label>端口</Label>
                <Input
                  type="number"
                  placeholder="1080"
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                />
              </div>
            </div>

            <div className="space-y-2">
              <Label>主机地址</Label>
              <Input
                placeholder="proxy.example.com 或 1.2.3.4"
                value={host}
                onChange={(e) => setHost(e.target.value)}
              />
            </div>

            {isAdvanced ? (
              <div className="space-y-2">
                <Label>完整链接</Label>
                <textarea
                  className="flex min-h-[80px] w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-xs ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                  placeholder={
                    proxyType === "vless"
                      ? "vless://uuid@host:443?encryption=none&flow=xtls-rprx-vision&security=reality&sni=...&pbk=...&sid=...&type=tcp#name"
                      : proxyType === "trojan"
                        ? "trojan://password@host:443?security=tls&sni=...#name"
                        : "ss://method:password@host:port#name"
                  }
                  value={vlessUri}
                  onChange={(e) => setVlessUri(e.target.value)}
                />
                <p className="text-[10px] text-muted-foreground">
                  粘贴完整协议URI，系统会自动解析主机、端口和名称
                </p>
              </div>
            ) : (
              <div className="grid grid-cols-2 gap-4">
                <div className="space-y-2">
                  <Label>用户名（可选）</Label>
                  <Input
                    placeholder="账号"
                    value={username}
                    onChange={(e) => setUsername(e.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label>密码（可选）</Label>
                  <Input
                    type="password"
                    placeholder="密码"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                </div>
              </div>
            )}

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-2">
                <Label>国家（可选）</Label>
                <Input
                  placeholder="例如：US"
                  value={country}
                  onChange={(e) => setCountry(e.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label>城市（可选）</Label>
                <Input
                  placeholder="例如：Los Angeles"
                  value={city}
                  onChange={(e) => setCity(e.target.value)}
                />
              </div>
            </div>
          </div>
        </div>
        <DialogFooter className="pt-4">
          <Button variant="outline" onClick={onClose}>
            取消
          </Button>
          <LoadingButton onClick={() => void handleSave()} isLoading={isSaving}>
            {editingProxy ? "保存修改" : "创建代理"}
          </LoadingButton>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== 搜索对话框 ====================

interface SearchDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  searchQuery: string;
  onSearchChange: (value: string) => void;
}

function SearchDialog({
  open,
  onOpenChange,
  searchQuery,
  onSearchChange,
}: SearchDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        hideClose
        className="top-1/6 max-w-2xl gap-0 overflow-hidden border-0 bg-background/95 p-0 shadow-2xl backdrop-blur-2xl"
      >
        <div className="relative">
          <LuSearch className="pointer-events-none absolute left-4 top-1/2 h-5 w-5 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="text"
            placeholder="搜索代理名称、主机、地区..."
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" || event.key === "Enter") {
                onOpenChange(false);
              }
            }}
            autoFocus
            className="h-16 border-0 bg-transparent pl-12 pr-4 text-base focus-visible:ring-0 focus-visible:ring-offset-0"
          />
        </div>
      </DialogContent>
    </Dialog>
  );
}
