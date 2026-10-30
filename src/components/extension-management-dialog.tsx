"use client";

import {
  type ColumnDef,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type RowSelectionState,
  type SortingState,
  useReactTable,
} from "@tanstack/react-table";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { FaChrome } from "react-icons/fa";
import {
  LuChevronDown,
  LuChevronUp,
  LuCopy,
  LuExternalLink,
  LuFolderOpen,
  LuGlobe,
  LuLink,
  LuPencil,
  LuPuzzle,
  LuSearch,
  LuTrash2,
  LuUpload,
  LuUsers,
} from "react-icons/lu";
import { AssignmentImpact } from "@/components/assignment-impact";
import {
  DataTableActionBar,
  DataTableActionBarAction,
  DataTableActionBarSelection,
} from "@/components/data-table-action-bar";
import { ProfileUsageButton } from "@/components/profile-usage-button";
import {
  AnimatedTabs,
  AnimatedTabsContent,
  AnimatedTabsList,
  AnimatedTabsTrigger,
} from "@/components/ui/animated-tabs";
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
import { FadingScrollArea } from "@/components/ui/fading-scroll-area";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ProBadge } from "@/components/ui/pro-badge";
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
import { useProfileReferences } from "@/hooks/use-profile-references";
import { parseBackendError, translateBackendError } from "@/lib/backend-errors";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";
import type {
  Extension,
  ExtensionGroup,
  FetchedExtension,
  ProfileGroup,
} from "@/types";
import { DeleteConfirmationDialog } from "./delete-confirmation-dialog";
import { RippleButton } from "./ui/ripple";

type SyncStatus = "disabled" | "syncing" | "synced" | "error" | "waiting";

/** A payload staged in the UI, before it is handed to the backend. */
type PendingSource =
  | { kind: "archive"; fileName: string; data: number[] }
  | { kind: "folder"; path: string };

const ARCHIVE_EXTENSIONS = [".crx", ".zip"];

function pathBaseName(path: string): string {
  const segments = path.split(/[/\\]/).filter(Boolean);
  return segments[segments.length - 1] ?? path;
}

function _getSyncStatusDot(
  item: { sync_enabled?: boolean; last_sync?: number },
  liveStatus: SyncStatus | undefined,
  t: (key: string, options?: Record<string, unknown>) => string,
): { color: string; tooltip: string; animate: boolean } {
  const status = liveStatus ?? (item.sync_enabled ? "synced" : "disabled");

  switch (status) {
    case "syncing":
      return {
        color: "bg-warning",
        tooltip: t("profileTable.syncTooltipSyncing"),
        animate: true,
      };
    case "synced":
      return {
        color: "bg-success",
        tooltip: item.last_sync
          ? t("profileTable.syncTooltipSyncedAt", {
              time: new Date(item.last_sync * 1000).toLocaleString(),
            })
          : t("profileTable.syncTooltipSynced"),
        animate: false,
      };
    case "waiting":
      return {
        color: "bg-warning",
        tooltip: t("profileTable.syncTooltipWaiting"),
        animate: false,
      };
    case "error":
      return {
        color: "bg-destructive",
        tooltip: t("profileTable.syncTooltipError"),
        animate: false,
      };
    default:
      return {
        color: "bg-muted-foreground",
        tooltip: t("profileTable.syncTooltipNotSynced"),
        animate: false,
      };
  }
}

interface ExtensionManagementDialogProps {
  isOpen: boolean;
  onClose: () => void;
  limitedMode: boolean;
  subPage?: boolean;
  /** Which tab is displayed when the dialog mounts; defaults to "extensions". */
  initialTab?: "extensions" | "groups";
}

export function ExtensionManagementDialog({
  isOpen,
  onClose,
  limitedMode,
  subPage,
  initialTab = "extensions",
}: ExtensionManagementDialogProps) {
  const { t } = useTranslation();
  const [extensions, setExtensions] = useState<Extension[]>([]);
  const [extensionGroups, setExtensionGroups] = useState<ExtensionGroup[]>([]);
  const [isLoading, setIsLoading] = useState(false);

  // Extension import state
  const [isUploading, setIsUploading] = useState(false);
  const [extensionName, setExtensionName] = useState("");
  const [pendingSource, setPendingSource] = useState<PendingSource | null>(
    null,
  );
  const [linkFolder, setLinkFolder] = useState(false);
  const [showUrlForm, setShowUrlForm] = useState(false);
  const [urlInput, setUrlInput] = useState("");
  const [isFetchingUrl, setIsFetchingUrl] = useState(false);
  /** Identity read out of a downloaded archive's own manifest, shown before
   * the extension is saved so the user sees what a link actually resolved to. */
  const [fetchedIdentity, setFetchedIdentity] = useState<{
    name: string;
    version: string | null;
    source: string;
  } | null>(null);

  // Group state
  const [_showCreateGroup, _setShowCreateGroup] = useState(false);
  const [_newGroupName, _setNewGroupName] = useState("");
  const [_editingGroup, _setEditingGroup] = useState<ExtensionGroup | null>(
    null,
  );
  const [_editGroupName, _setEditGroupName] = useState("");
  const [_editGroupExtensionIds, _setEditGroupExtensionIds] = useState<
    string[]
  >([]);

  // Delete state
  const [extensionToDelete, setExtensionToDelete] = useState<Extension | null>(
    null,
  );
  const [groupToDelete, setGroupToDelete] = useState<ExtensionGroup | null>(
    null,
  );
  const [isDeleting, setIsDeleting] = useState(false);

  // Bulk delete state
  const [bulkExtDeleteOpen, setBulkExtDeleteOpen] = useState(false);
  const [bulkGroupDeleteOpen, setBulkGroupDeleteOpen] = useState(false);

  // Assign-group-to-users state. Targets are `creator:<email>` / `group:<id>`
  // keys resolved into profile ids when the assignment runs. The dialog opens
  // either from an extension row (direct extension assignment; the backend
  // manages its shadow group) or from a group row (explicit group assignment).
  const [assignUsersGroup, setAssignUsersGroup] =
    useState<ExtensionGroup | null>(null);
  const [assignUsersExtension, setAssignUsersExtension] =
    useState<Extension | null>(null);
  const [assignUsersTargets, setAssignUsersTargets] = useState<string[]>([]);
  const [assignUsersProfileGroups, setAssignUsersProfileGroups] = useState<
    ProfileGroup[]
  >([]);
  const [assignUsersCloudUsers, setAssignUsersCloudUsers] = useState<
    { id: number; name: string }[]
  >([]);
  const [assignUsersOwnerProfiles, setAssignUsersOwnerProfiles] = useState<
    Record<number, string[]>
  >({});
  const [assignUsersAssignments, setAssignUsersAssignments] = useState<
    {
      extension_id: string;
      user_id: number;
      account_ids?: number[] | null;
      vps_login?: boolean | null;
    }[]
  >([]);
  // 每个云端用户的账号范围：accounts null = 全部账号（旧行为），数组 = 勾选的账号 id
  //（可为空数组 = 不装到任何账号）；vpsLogin = 同时作用于 VPS 登录（爆文库）
  const [assignUsersAccountScopes, setAssignUsersAccountScopes] = useState<
    Record<number, { accounts: number[] | null; vpsLogin: boolean }>
  >({});
  // 展开的账号选择面板 + 账号列表缓存（懒加载，含 VPS 未启动账号）
  const [assignAcctDialogUser, setAssignAcctDialogUser] = useState<{
    id: number;
    name: string;
  } | null>(null);
  const [assignAcctQuery, setAssignAcctQuery] = useState("");
  const [assignUsersAcctLists, setAssignUsersAcctLists] = useState<
    Record<number, "loading" | "error" | { id: number; label: string }[]>
  >({});
  const [isAssigningUsers, setIsAssigningUsers] = useState(false);
  // 复制分配：把源插件的用户/账号范围绑定整份复制到目标插件（覆盖目标现有绑定）
  const [copyAssignExtension, setCopyAssignExtension] =
    useState<Extension | null>(null);
  const [copySourceId, setCopySourceId] = useState<string>("");
  const [copySourceCounts, setCopySourceCounts] = useState<
    Record<string, number>
  >({});
  const [isCopyingAssign, setIsCopyingAssign] = useState(false);
  // 打开复制弹窗时拉一次全量绑定，统计每个插件可复制的分配数
  useEffect(() => {
    if (!copyAssignExtension) return;
    let active = true;
    invoke<{ extension_id: string; user_id: number }[]>(
      "list_extension_user_assignments",
    )
      .then((items) => {
        if (!active) return;
        const counts: Record<string, number> = {};
        for (const a of items ?? []) {
          counts[a.extension_id] = (counts[a.extension_id] ?? 0) + 1;
        }
        setCopySourceCounts(counts);
      })
      .catch(() => {
        if (active) setCopySourceCounts({});
      });
    return () => {
      active = false;
    };
  }, [copyAssignExtension]);
  const closeAssignUsers = () => {
    setAssignUsersGroup(null);
    setAssignUsersExtension(null);
    setAssignAcctDialogUser(null);
  };
  const assignUsersSourceName =
    assignUsersGroup?.name ?? assignUsersExtension?.name ?? "";

  // Table state
  const [extSorting, setExtSorting] = useState<SortingState>([]);
  const [extRowSelection, setExtRowSelection] = useState<RowSelectionState>({});
  const [_groupSorting, _setGroupSorting] = useState<SortingState>([]);
  const [groupRowSelection, setGroupRowSelection] = useState<RowSelectionState>(
    {},
  );

  // Edit extension state
  const { profiles: referencedProfiles, failed: referencesFailed } =
    useProfileReferences(isOpen);
  const [editingExtension, setEditingExtension] = useState<Extension | null>(
    null,
  );
  const [editExtensionName, setEditExtensionName] = useState("");
  const [pendingUpdateSource, setPendingUpdateSource] =
    useState<PendingSource | null>(null);
  const [editLinkFolder, setEditLinkFolder] = useState(false);

  // Extension icons
  const [extensionIcons, setExtensionIcons] = useState<Record<string, string>>(
    {},
  );

  // Sync state
  const [_extSyncStatus, setExtSyncStatus] = useState<
    Record<string, SyncStatus>
  >({});
  const [_isTogglingExtSync, _setIsTogglingExtSync] = useState<
    Record<string, boolean>
  >({});
  const [_isTogglingGroupSync, _setIsTogglingGroupSync] = useState<
    Record<string, boolean>
  >({});

  // Tab — keyed off `initialTab` so remounting the dialog with a new initial
  // tab (e.g. via the Mod+E shortcut toggle) jumps to that tab.
  const [activeTab, setActiveTab] = useState<"extensions" | "groups">(
    initialTab,
  );

  const loadData = useCallback(async () => {
    if (limitedMode) return;
    setIsLoading(true);
    try {
      const [exts, groups] = await Promise.all([
        invoke<Extension[]>("list_extensions"),
        invoke<ExtensionGroup[]>("list_extension_groups"),
      ]);
      setExtensions(exts);
      setExtensionGroups(groups);
    } catch {
      // User may not have pro subscription
      setExtensions([]);
      setExtensionGroups([]);
    } finally {
      setIsLoading(false);
    }
  }, [limitedMode]);

  const loadIcons = useCallback(async (exts: Extension[]) => {
    const icons: Record<string, string> = {};
    for (const ext of exts) {
      try {
        const icon = await invoke<string | null>("get_extension_icon", {
          extensionId: ext.id,
        });
        if (icon) {
          icons[ext.id] = icon;
        }
      } catch {
        // Icon not available
      }
    }
    setExtensionIcons(icons);
  }, []);

  useEffect(() => {
    if (isOpen) {
      void loadData();
    } else {
      // Drop selection when the dialog closes so the floating action
      // bars (portaled to body) don't linger on the page.
      setExtRowSelection({});
      setGroupRowSelection({});
    }
  }, [isOpen, loadData]);

  useEffect(() => {
    if (extensions.length > 0) {
      void loadIcons(extensions);
    }
  }, [extensions, loadIcons]);

  // Listen for extension sync status events
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    const setupListener = async () => {
      unlisten = await listen<{ id: string; status: string }>(
        "extension-sync-status",
        (event) => {
          const { id, status } = event.payload;
          setExtSyncStatus((prev) => ({
            ...prev,
            [id]: status as SyncStatus,
          }));
        },
      );
    };

    void setupListener();
    return () => {
      unlisten?.();
    };
  }, []);

  /** Structured backend codes win; anything else falls back to a local message
   * so the user never sees a raw Rust string. */
  const showActionError = useCallback(
    (err: unknown, fallback: string) => {
      showErrorToast(
        parseBackendError(err) ? translateBackendError(t, err) : fallback,
      );
    },
    [t],
  );

  // Distinct creator emails across all profiles — one entry per cloud user.
  const assignUsersCreatorOptions = useMemo(() => {
    const byEmail = new Map<string, number>();
    for (const profile of referencedProfiles ?? []) {
      const email = profile.created_by_email?.trim();
      if (!email) continue;
      byEmail.set(email, (byEmail.get(email) ?? 0) + 1);
    }
    return Array.from(byEmail.entries())
      .map(([email, count]) => ({ key: `creator:${email}`, email, count }))
      .sort((a, b) => a.email.localeCompare(b.email));
  }, [referencedProfiles]);

  const assignUsersGroupOptions = useMemo(
    () =>
      assignUsersProfileGroups.map((g) => ({
        key: `group:${g.id}`,
        name: g.name,
        count: (referencedProfiles ?? []).filter((p) => p.group_id === g.id)
          .length,
      })),
    [assignUsersProfileGroups, referencedProfiles],
  );

  const assignUsersCloudOptions = useMemo(() => {
    const options = assignUsersCloudUsers.map((u) => ({
      key: `cloud:${u.id}`,
      name: u.name,
      count: (assignUsersOwnerProfiles[u.id] ?? []).length,
    }));
    // 已绑定但不在云端用户列表里的账号也要出现，否则绑了却看不到、取消不了。
    if (assignUsersExtension) {
      const known = new Set(options.map((o) => o.key));
      for (const a of assignUsersAssignments) {
        if (a.extension_id !== assignUsersExtension.id) continue;
        const key = `cloud:${a.user_id}`;
        if (known.has(key)) continue;
        options.push({
          key,
          name: `#${a.user_id}`,
          count: (assignUsersOwnerProfiles[a.user_id] ?? []).length,
        });
      }
    }
    return options;
  }, [
    assignUsersCloudUsers,
    assignUsersOwnerProfiles,
    assignUsersAssignments,
    assignUsersExtension,
  ]);

  // 勾选态与已绑定态的差异：勾选新增的待绑定，取消勾选的待解绑。
  // 范围（account_ids / vps_login）也算差异：已绑定但改了范围要重新提交。
  // 提交按钮的可用性与文案都以它为准（全部取消勾选也要能提交解绑）。
  const assignUsersDiff = useMemo(() => {
    const ext = assignUsersExtension;
    if (!ext)
      return {
        toAssign: [] as {
          userId: number;
          accountIds: number[] | null;
          vpsLogin: boolean;
        }[],
        toUnassign: [] as number[],
      };
    const desired = new Map<
      number,
      { accounts: number[] | null; vps: boolean }
    >();
    for (const key of assignUsersTargets) {
      if (!key.startsWith("cloud:")) continue;
      const uid = Number(key.slice("cloud:".length));
      const s = assignUsersAccountScopes[uid];
      desired.set(uid, {
        accounts: s ? s.accounts : null,
        vps: s ? s.vpsLogin === true : false,
      });
    }
    const current = new Map<
      number,
      { accounts: number[] | null; vps: boolean }
    >();
    for (const a of assignUsersAssignments) {
      if (a.extension_id !== ext.id) continue;
      current.set(a.user_id, {
        accounts: Array.isArray(a.account_ids) ? a.account_ids : null,
        vps: a.vps_login === true,
      });
    }
    const toAssign: {
      userId: number;
      accountIds: number[] | null;
      vpsLogin: boolean;
    }[] = [];
    const toUnassign: number[] = [];
    for (const [uid, scope] of desired) {
      const cur = current.get(uid);
      if (cur === undefined || JSON.stringify(cur) !== JSON.stringify(scope)) {
        toAssign.push({
          userId: uid,
          accountIds: scope.accounts,
          vpsLogin: scope.vps,
        });
      }
    }
    for (const uid of current.keys()) {
      if (!desired.has(uid)) toUnassign.push(uid);
    }
    return { toAssign, toUnassign };
  }, [
    assignUsersAssignments,
    assignUsersExtension,
    assignUsersTargets,
    assignUsersAccountScopes,
  ]);

  // 拉取单个用户的云端账号列表（翻页拉全），写入缓存
  const fetchAssignUserAccounts = useCallback(async (userId: number) => {
    setAssignUsersAcctLists((p) => ({ ...p, [userId]: "loading" }));
    try {
      const all: { id: number; label: string }[] = [];
      for (let pg = 1; ; pg++) {
        const result = await invoke<{
          accounts: {
            id: number;
            account_name: string;
            login_account?: string | null;
            phone_id?: string | null;
          }[];
          total?: number;
        }>("bwbrowser_list_accounts", {
          page: pg,
          pageSize: 200,
          platform: null,
          keyword: null,
          ownerId: userId,
          companyId: null,
          sortBy: null,
          sortOrder: null,
        });
        for (const a of result.accounts ?? []) {
          const base = a.phone_id || a.account_name || `#${a.id}`;
          all.push({
            id: a.id,
            label: a.login_account ? `${base} (${a.login_account})` : base,
          });
        }
        if (
          (result.accounts?.length ?? 0) < 200 ||
          (result.total != null && all.length >= result.total)
        ) {
          break;
        }
      }
      setAssignUsersAcctLists((p) => ({ ...p, [userId]: all }));
    } catch {
      setAssignUsersAcctLists((p) => ({ ...p, [userId]: "error" }));
    }
  }, []);

  // 打开账号选择弹窗（懒拉云端账号列表，翻页拉全）。
  // 默认一个都不勾（accounts=[] 且不含爆文库），由使用者显式选择。
  const openAssignAcctDialog = useCallback(
    async (userId: number, userName: string) => {
      setAssignAcctDialogUser({ id: userId, name: userName });
      setAssignAcctQuery("");
      setAssignUsersAccountScopes((p) =>
        p[userId] ? p : { ...p, [userId]: { accounts: [], vpsLogin: false } },
      );
      if (!assignUsersAcctLists[userId]) {
        void fetchAssignUserAccounts(userId);
      }
    },
    [assignUsersAcctLists, fetchAssignUserAccounts],
  );

  const setAcctScope = (
    userId: number,
    scope: { accounts: number[] | null; vpsLogin: boolean },
  ) => {
    setAssignUsersAccountScopes((p) => ({ ...p, [userId]: scope }));
  };

  // ==================== 批量选账号（多个已勾选用户一次筛选） ====================
  const [batchAcctOpen, setBatchAcctOpen] = useState(false);
  const [batchQuery, setBatchQuery] = useState("");
  const batchUserIds = useMemo(
    () =>
      assignUsersTargets
        .filter((k) => k.startsWith("cloud:"))
        .map((k) => Number(k.slice("cloud:".length))),
    [assignUsersTargets],
  );

  const openBatchAcctDialog = useCallback(() => {
    setBatchAcctOpen(true);
    setBatchQuery("");
    for (const uid of batchUserIds) {
      if (!assignUsersAcctLists[uid]) void fetchAssignUserAccounts(uid);
    }
  }, [batchUserIds, assignUsersAcctLists, fetchAssignUserAccounts]);

  // 批量 VPS 登录：勾选 = 所有已选用户都启用爆文库注入，取消 = 全部关闭
  const batchVpsChecked =
    batchUserIds.length > 0 &&
    batchUserIds.every(
      (uid) => assignUsersAccountScopes[uid]?.vpsLogin === true,
    );
  const setBatchVps = (v: boolean) => {
    setAssignUsersAccountScopes((p) => {
      const next = { ...p };
      for (const uid of batchUserIds) {
        const cur = next[uid];
        next[uid] = {
          // 没开过账号选择弹窗的用户保持「全部账号」语义，勾 VPS 只加标记
          accounts: cur ? cur.accounts : null,
          vpsLogin: v,
        };
      }
      return next;
    });
  };

  // 全选筛选结果：把每个用户匹配到的账号并入其范围（不覆盖已勾的其他账号）
  const selectAllBatchMatches = () => {
    const q = batchQuery.trim().toLowerCase();
    setAssignUsersAccountScopes((p) => {
      const next = { ...p };
      for (const uid of batchUserIds) {
        const list = assignUsersAcctLists[uid];
        if (!list || list === "loading" || list === "error") continue;
        const matches = q
          ? list.filter((a) => a.label.toLowerCase().includes(q))
          : list;
        if (matches.length === 0) continue;
        const cur = next[uid] ?? { accounts: [], vpsLogin: false };
        const curList = Array.isArray(cur.accounts) ? cur.accounts : [];
        const merged = Array.from(
          new Set([...curList, ...matches.map((m) => m.id)]),
        );
        next[uid] = {
          accounts: merged.length === list.length ? null : merged,
          vpsLogin: cur.vpsLogin,
        };
      }
      return next;
    });
  };

  // 清空筛选结果：从每个用户的范围里移除匹配到的账号
  const clearBatchMatches = () => {
    const q = batchQuery.trim().toLowerCase();
    setAssignUsersAccountScopes((p) => {
      const next = { ...p };
      for (const uid of batchUserIds) {
        const list = assignUsersAcctLists[uid];
        if (!list || list === "loading" || list === "error") continue;
        const matches = q
          ? list.filter((a) => a.label.toLowerCase().includes(q))
          : list;
        if (matches.length === 0) continue;
        const cur = next[uid];
        const curList = Array.isArray(cur?.accounts) ? cur.accounts : [];
        next[uid] = {
          accounts: curList.filter((id) => !matches.some((m) => m.id === id)),
          vpsLogin: cur ? cur.vpsLogin : false,
        };
      }
      return next;
    });
  };

  // 复制分配提交：单次后端调用完成复制 + 本地重算 + 公司 KV 同步
  const handleCopyAssign = async () => {
    const target = copyAssignExtension;
    if (!target || !copySourceId) return;
    setIsCopyingAssign(true);
    try {
      const result = await invoke<{ copied: number; assigned: number }>(
        "copy_extension_assignments",
        { fromExtensionId: copySourceId, toExtensionId: target.id },
      );
      if (result.copied === 0) {
        showSuccessToast(t("extensions.copyAssign.noSource"));
      } else {
        showSuccessToast(
          t("extensions.copyAssign.success", { n: result.copied }),
        );
      }
      setCopyAssignExtension(null);
    } catch (err) {
      showActionError(err, t("extensions.assignUsers.failed"));
    } finally {
      setIsCopyingAssign(false);
    }
  };

  // 账号选择弹窗的搜索过滤（手机编号/账号名称/登录账号）
  const assignAcctMatches = useMemo(() => {
    const uid = assignAcctDialogUser?.id;
    const acct = uid === undefined ? [] : assignUsersAcctLists[uid];
    if (!acct || acct === "loading" || acct === "error") return [];
    const q = assignAcctQuery.trim().toLowerCase();
    if (!q) return acct;
    return acct.filter((a) => a.label.toLowerCase().includes(q));
  }, [assignAcctDialogUser, assignUsersAcctLists, assignAcctQuery]);

  useEffect(() => {
    if (!assignUsersGroup && !assignUsersExtension) return;
    // 分组模式是手动勾选，不复用上一次弹窗残留的勾选态。
    if (assignUsersGroup && !assignUsersExtension) setAssignUsersTargets([]);
    let active = true;
    invoke<ProfileGroup[]>("get_profile_groups")
      .then((groups) => {
        if (active) setAssignUsersProfileGroups(groups);
      })
      .catch(() => {
        if (active) setAssignUsersProfileGroups([]);
      });
    invoke<{
      success: boolean;
      users?: { id: number; username: string; real_name: string }[];
    }>("bwbrowser_list_cloud_users", { companyId: null })
      .then((resp) => {
        if (active)
          setAssignUsersCloudUsers(
            (resp.users ?? []).map((u) => ({
              id: u.id,
              name: u.real_name || u.username || `#${u.id}`,
            })),
          );
      })
      .catch(() => {
        if (active) setAssignUsersCloudUsers([]);
      });
    invoke<{
      owners?: { owner_id: number; profile_ids: string[] }[];
    }>("bwbrowser_get_owner_profile_map")
      .then((resp) => {
        if (!active) return;
        const byOwner: Record<number, string[]> = {};
        for (const owner of resp.owners ?? []) {
          byOwner[owner.owner_id] = owner.profile_ids ?? [];
        }
        setAssignUsersOwnerProfiles(byOwner);
      })
      .catch(() => {
        if (active) setAssignUsersOwnerProfiles({});
      });
    invoke<
      {
        extension_id: string;
        user_id: number;
        account_ids?: number[] | null;
        vps_login?: boolean | null;
      }[]
    >("list_extension_user_assignments")
      .then((items) => {
        if (!active) return;
        setAssignUsersAssignments(items ?? []);
        // 用户绑定模式：勾选态 = 当前绑定关系，取消勾选即解绑。
        const ext = assignUsersExtension;
        if (ext) {
          setAssignUsersTargets(
            (items ?? [])
              .filter((a) => a.extension_id === ext.id)
              .map((a) => `cloud:${a.user_id}`),
          );
        }
      })
      .catch(() => {
        if (active) setAssignUsersAssignments([]);
      });
    return () => {
      active = false;
    };
  }, [assignUsersGroup, assignUsersExtension]);

  const toggleAssignUsersTarget = (key: string, checked: boolean) => {
    setAssignUsersTargets((prev) =>
      checked ? [...prev, key] : prev.filter((k) => k !== key),
    );
  };

  const handleAssignGroupToUsers = async () => {
    const source = assignUsersGroup ?? assignUsersExtension;
    if (!source) return;
    if (assignUsersExtension) {
      // 用户绑定模式：勾选 = 绑定到该云端用户（长期生效，名下浏览器自动装，
      // 取消勾选 = 自动卸载）。浏览器解析在后端按云端归属完成。
      const ext = assignUsersExtension;
      if (
        assignUsersDiff.toAssign.length === 0 &&
        assignUsersDiff.toUnassign.length === 0
      ) {
        showSuccessToast(t("extensions.assignUsers.nothingAssigned"));
        return;
      }
      setIsAssigningUsers(true);
      try {
        // 单次批量调用：后端一次拉全云端账号、逐用户本地重算、只同步一次
        // 公司 KV，避免逐用户串行 invoke 的多次网络往返把界面卡死
        const result = await invoke<{
          assigned: number;
          skipped: number;
          removed: number;
        }>("assign_extension_to_users_bulk", {
          extensionId: ext.id,
          toAssign: assignUsersDiff.toAssign.map((t) => ({
            userId: t.userId,
            accountIds: t.accountIds,
            vpsLogin: t.vpsLogin,
          })),
          toUnassign: assignUsersDiff.toUnassign,
        });
        const { assigned, skipped, removed } = result;
        if (
          assignUsersDiff.toAssign.length > 0 &&
          assignUsersDiff.toUnassign.length > 0
        ) {
          showSuccessToast(t("extensions.assignUsers.savedChanges"));
        } else if (assigned > 0) {
          showSuccessToast(
            skipped > 0
              ? t("extensions.assignUsers.successWithSkipped", {
                  count: assigned,
                  skipped,
                })
              : t("extensions.assignUsers.success", { count: assigned }),
          );
        } else if (assignUsersDiff.toUnassign.length > 0) {
          // removed 是本机已清掉的浏览器数；用户还没在任何机器上启动浏览器时
          // 为 0，但绑定记录确实已取消，不能回落到「已绑定」的提示。
          showSuccessToast(
            removed > 0
              ? t("extensions.assignUsers.unassigned", { count: removed })
              : t("extensions.assignUsers.unassignedNoBrowsers"),
          );
        } else {
          showSuccessToast(t("extensions.assignUsers.boundAuto"));
        }
        closeAssignUsers();
      } catch (err) {
        showActionError(err, t("extensions.assignUsers.failed"));
      } finally {
        setIsAssigningUsers(false);
      }
      return;
    }
    const profileIds = new Set<string>();
    for (const key of assignUsersTargets) {
      // Cloud-user targets resolve through cloud account ownership
      // (owner_id → local profiles via the account profile name).
      if (key.startsWith("cloud:")) {
        const userId = Number(key.slice("cloud:".length));
        for (const profileId of assignUsersOwnerProfiles[userId] ?? []) {
          profileIds.add(profileId);
        }
        continue;
      }
      const [kind, ...rest] = key.split(":");
      const value = rest.join(":");
      for (const profile of referencedProfiles ?? []) {
        const matches =
          kind === "creator"
            ? profile.created_by_email?.trim() === value
            : kind === "group" && profile.group_id === value;
        if (matches) profileIds.add(profile.id);
      }
    }
    if (profileIds.size === 0) {
      showSuccessToast(t("extensions.assignUsers.nothingOwned"));
      return;
    }
    setIsAssigningUsers(true);
    try {
      const result = await invoke<{ assigned: number; skipped: number }>(
        "assign_extension_group_to_profiles_bulk",
        {
          groupId: assignUsersGroup?.id,
          profileIds: Array.from(profileIds),
        },
      );
      if (result.assigned > 0) {
        showSuccessToast(
          result.skipped > 0
            ? t("extensions.assignUsers.successWithSkipped", {
                count: result.assigned,
                skipped: result.skipped,
              })
            : t("extensions.assignUsers.success", { count: result.assigned }),
        );
      } else {
        showSuccessToast(t("extensions.assignUsers.nothingAssigned"));
      }
      closeAssignUsers();
    } catch (err) {
      showActionError(err, t("extensions.assignUsers.failed"));
    } finally {
      setIsAssigningUsers(false);
    }
  };

  const resetImportForm = useCallback(() => {
    setPendingSource(null);
    setExtensionName("");
    setLinkFolder(false);
    setFetchedIdentity(null);
  }, []);

  const closeEditExtension = useCallback(() => {
    setEditingExtension(null);
    setEditExtensionName("");
    setPendingUpdateSource(null);
    setEditLinkFolder(false);
  }, []);

  const readArchiveFile = useCallback(
    (file: File): Promise<PendingSource | null> => {
      const isValid = ARCHIVE_EXTENSIONS.some((ext) =>
        file.name.toLowerCase().endsWith(ext),
      );
      if (!isValid) {
        showErrorToast(t("extensions.invalidFileType"));
        return Promise.resolve(null);
      }

      return new Promise((resolve) => {
        const reader = new FileReader();
        reader.onload = (event) => {
          const arrayBuffer = event.target?.result as ArrayBuffer;
          resolve({
            kind: "archive",
            fileName: file.name,
            data: Array.from(new Uint8Array(arrayBuffer)),
          });
        };
        reader.onerror = () => {
          showErrorToast(t("extensions.readError"));
          resolve(null);
        };
        reader.readAsArrayBuffer(file);
      });
    },
    [t],
  );

  const handleEditFileSelect = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      e.target.value = "";
      if (!file) return;

      void readArchiveFile(file).then((source) => {
        if (!source) return;
        setPendingUpdateSource(source);
        setEditLinkFolder(false);
      });
    },
    [readArchiveFile],
  );

  const handleFileSelect = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      e.target.value = "";
      if (!file) return;

      void readArchiveFile(file).then((source) => {
        if (!source) return;
        setExtensionName(
          file.name.replace(/\.(crx|zip)$/i, "").replace(/[-_]/g, " "),
        );
        setLinkFolder(false);
        setFetchedIdentity(null);
        setPendingSource(source);
      });
    },
    [readArchiveFile],
  );

  /** Native directory picker, the "Load unpacked" entry point. */
  const pickExtensionFolder = useCallback(async (): Promise<string | null> => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("extensions.selectFolderTitle"),
      });
      return typeof selected === "string" ? selected : null;
    } catch (err) {
      console.error("Failed to open folder dialog:", err);
      showErrorToast(t("importProfile.folderDialogFailed"));
      return null;
    }
  }, [t]);

  const handleLoadUnpacked = useCallback(async () => {
    const folder = await pickExtensionFolder();
    if (!folder) return;
    setExtensionName(pathBaseName(folder).replace(/[-_]/g, " "));
    setLinkFolder(false);
    setFetchedIdentity(null);
    setPendingSource({ kind: "folder", path: folder });
  }, [pickExtensionFolder]);

  const handleEditFolderSelect = useCallback(async () => {
    const folder = await pickExtensionFolder();
    if (!folder) return;
    setPendingUpdateSource({ kind: "folder", path: folder });
  }, [pickExtensionFolder]);

  /** Stage a link exactly as a picked file is staged: the download is
   * validated and its manifest read in the backend, and the user still
   * confirms the name before anything is stored. */
  const handleFetchFromUrl = useCallback(async () => {
    const link = urlInput.trim();
    if (!link || isFetchingUrl) return;
    setIsFetchingUrl(true);
    try {
      const fetched = await invoke<FetchedExtension>(
        "fetch_extension_from_url",
        { url: link },
      );
      const identity = fetched.name?.trim() ?? "";
      setPendingSource({
        kind: "archive",
        fileName: fetched.file_name,
        data: fetched.file_data,
      });
      setExtensionName(identity || fetched.file_name.replace(/\.zip$/i, ""));
      setLinkFolder(false);
      setFetchedIdentity({
        name: identity || fetched.file_name,
        version: fetched.version?.trim() || null,
        source: fetched.source_url,
      });
      setShowUrlForm(false);
      setUrlInput("");
      showSuccessToast(t("extensions.fetchSuccess"));
    } catch (err) {
      showActionError(err, t("extensions.fetchFailed"));
    } finally {
      setIsFetchingUrl(false);
    }
  }, [urlInput, isFetchingUrl, showActionError, t]);

  const handleUpload = useCallback(async () => {
    if (!pendingSource || !extensionName.trim()) return;
    setIsUploading(true);
    try {
      if (pendingSource.kind === "folder") {
        await invoke("add_unpacked_extension", {
          name: extensionName.trim(),
          path: pendingSource.path,
          link: linkFolder,
        });
      } else {
        await invoke("add_extension", {
          name: extensionName.trim(),
          fileName: pendingSource.fileName,
          fileData: pendingSource.data,
        });
      }
      showSuccessToast(t("extensions.uploadSuccess"));
      resetImportForm();
      void loadData();
    } catch (err) {
      showActionError(err, t("extensions.uploadFailed"));
    } finally {
      setIsUploading(false);
    }
  }, [
    pendingSource,
    extensionName,
    linkFolder,
    resetImportForm,
    loadData,
    showActionError,
    t,
  ]);

  const handleDeleteExtension = useCallback(async () => {
    if (!extensionToDelete) return;
    setIsDeleting(true);
    try {
      await invoke("delete_extension", { extensionId: extensionToDelete.id });
      showSuccessToast(t("extensions.deleteSuccess"));
      setExtensionToDelete(null);
      void loadData();
    } catch (err) {
      showActionError(err, t("extensions.deleteFailed"));
    } finally {
      setIsDeleting(false);
    }
  }, [extensionToDelete, loadData, showActionError, t]);

  const handleDeleteGroup = useCallback(async () => {
    if (!groupToDelete) return;
    setIsDeleting(true);
    try {
      await invoke("delete_extension_group", { groupId: groupToDelete.id });
      showSuccessToast(t("extensions.groupDeleteSuccess"));
      setGroupToDelete(null);
      void loadData();
    } catch (err) {
      showActionError(err, t("extensions.groupDeleteFailed"));
    } finally {
      setIsDeleting(false);
    }
  }, [groupToDelete, loadData, showActionError, t]);

  const selectedExtensions = useMemo(
    () => extensions.filter((ext) => extRowSelection[ext.id]),
    [extensions, extRowSelection],
  );

  const selectedGroups = useMemo(
    () => extensionGroups.filter((group) => groupRowSelection[group.id]),
    [extensionGroups, groupRowSelection],
  );

  const handleBulkDeleteExtensions = useCallback(async () => {
    if (selectedExtensions.length === 0) return;
    setIsDeleting(true);
    try {
      await Promise.allSettled(
        selectedExtensions.map((ext) =>
          invoke("delete_extension", { extensionId: ext.id }),
        ),
      );
      showSuccessToast(t("extensions.deleteSuccess"));
      setBulkExtDeleteOpen(false);
      setExtRowSelection({});
      void loadData();
    } catch (err) {
      showActionError(err, t("extensions.deleteFailed"));
    } finally {
      setIsDeleting(false);
    }
  }, [selectedExtensions, loadData, showActionError, t]);

  const handleBulkDeleteGroups = useCallback(async () => {
    if (selectedGroups.length === 0) return;
    setIsDeleting(true);
    try {
      await Promise.allSettled(
        selectedGroups.map((group) =>
          invoke("delete_extension_group", { groupId: group.id }),
        ),
      );
      showSuccessToast(t("extensions.groupDeleteSuccess"));
      setBulkGroupDeleteOpen(false);
      setGroupRowSelection({});
      void loadData();
    } catch (err) {
      showActionError(err, t("extensions.groupDeleteFailed"));
    } finally {
      setIsDeleting(false);
    }
  }, [selectedGroups, loadData, showActionError, t]);

  const renderCompatIcons = useCallback(
    (compat: string[]) => {
      const hasChromium = compat.includes("chromium");
      if (!hasChromium) return null;
      return (
        <div className="flex shrink-0 items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="inline-flex">
                <FaChrome className="size-3.5 text-muted-foreground" />
              </span>
            </TooltipTrigger>
            <TooltipContent>
              {t("extensions.compatibility.chromium")}
            </TooltipContent>
          </Tooltip>
        </div>
      );
    },
    [t],
  );

  const renderExtensionIcon = useCallback(
    (ext: Extension, size: "sm" | "md" = "md") => {
      const sizeClass = size === "sm" ? "size-4" : "size-5";
      if (extensionIcons[ext.id]) {
        return (
          // biome-ignore lint/performance/noImgElement: base64 data URI icons cannot use next/image
          <img
            src={extensionIcons[ext.id]}
            alt=""
            className={`${sizeClass} shrink-0 rounded-sm`}
          />
        );
      }
      return (
        <LuPuzzle className={`${sizeClass} shrink-0 text-muted-foreground`} />
      );
    },
    [extensionIcons],
  );

  /** What the extension actually is: a stored archive, a folder packed into
   * the store, or a folder loaded in place from the user's disk. */
  const renderSource = useCallback(
    (ext: Extension) => {
      if (ext.linked_path) {
        return (
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="inline-flex min-w-0 items-center gap-1 text-xs text-muted-foreground">
                <LuLink className="size-3 shrink-0" />
                <span className="truncate">
                  {t("extensions.source.linked")}
                </span>
              </span>
            </TooltipTrigger>
            <TooltipContent>
              <p className="max-w-xs break-all">
                {t("extensions.source.linkedTooltip", {
                  path: ext.linked_path,
                })}
              </p>
            </TooltipContent>
          </Tooltip>
        );
      }
      return (
        <span className="block min-w-0 truncate text-xs text-muted-foreground">
          {ext.source_kind === "unpacked"
            ? t("extensions.source.unpacked")
            : t("extensions.source.archive")}
        </span>
      );
    },
    [t],
  );

  const _MAX_VISIBLE_ICONS = 3;

  const handleUpdateExtension = useCallback(async () => {
    if (!editingExtension || !editExtensionName.trim()) return;
    try {
      if (pendingUpdateSource?.kind === "folder") {
        await invoke("update_extension_from_path", {
          extensionId: editingExtension.id,
          name: editExtensionName.trim(),
          path: pendingUpdateSource.path,
          link: editLinkFolder,
        });
      } else {
        await invoke("update_extension", {
          extensionId: editingExtension.id,
          name: editExtensionName.trim(),
          fileName: pendingUpdateSource?.fileName ?? null,
          fileData: pendingUpdateSource?.data ?? null,
        });
      }
      showSuccessToast(t("extensions.updateSuccess"));
      closeEditExtension();
      void loadData();
    } catch (err) {
      showActionError(err, t("extensions.updateFailed"));
    }
  }, [
    editingExtension,
    editExtensionName,
    pendingUpdateSource,
    editLinkFolder,
    closeEditExtension,
    loadData,
    showActionError,
    t,
  ]);

  const extensionColumns = useMemo<ColumnDef<Extension>[]>(
    () => [
      {
        id: "select",
        size: 36,
        enableSorting: false,
        header: ({ table }) => (
          <Checkbox
            checked={
              table.getIsAllRowsSelected() ||
              (table.getIsSomeRowsSelected() && "indeterminate")
            }
            onCheckedChange={(value) => {
              table.toggleAllRowsSelected(!!value);
            }}
            aria-label={t("common.aria.selectAll")}
          />
        ),
        cell: ({ row }) => (
          <Checkbox
            checked={row.getIsSelected()}
            onCheckedChange={(value) => {
              row.toggleSelected(!!value);
            }}
            aria-label={t("common.aria.selectRow")}
          />
        ),
      },
      {
        id: "icon",
        size: 36,
        enableSorting: false,
        header: () => null,
        cell: ({ row }) => renderExtensionIcon(row.original, "sm"),
      },
      {
        accessorKey: "name",
        enableSorting: true,
        sortingFn: "alphanumeric",
        header: ({ column }) => (
          <Button
            variant="ghost"
            onClick={() => {
              column.toggleSorting(column.getIsSorted() === "asc");
            }}
            className="h-auto cursor-pointer justify-start p-0 text-left font-semibold"
          >
            {t("common.labels.name")}
            {column.getIsSorted() === "asc" ? (
              <LuChevronUp className="ml-2 size-4" />
            ) : column.getIsSorted() === "desc" ? (
              <LuChevronDown className="ml-2 size-4" />
            ) : null}
          </Button>
        ),
        cell: ({ row }) => (
          <button
            type="button"
            className="block min-w-0 max-w-full rounded-sm text-left text-sm font-medium hover:text-muted-foreground focus-visible:outline-2 focus-visible:outline-ring"
            onClick={() => {
              const ext = row.original;
              setEditingExtension(ext);
              setEditExtensionName(ext.name);
              setPendingUpdateSource(null);
              setEditLinkFolder(Boolean(ext.linked_path));
            }}
          >
            <span className="block truncate">{row.original.name}</span>
            <span className="block truncate text-xs font-normal text-muted-foreground">
              {row.original.manifest_name &&
              row.original.manifest_name !== row.original.name
                ? row.original.manifest_name
                : row.original.file_name}
            </span>
          </button>
        ),
      },
      {
        id: "compat",
        size: 100,
        enableSorting: false,
        header: () => t("extensions.compatibility.label"),
        cell: ({ row }) =>
          renderCompatIcons(row.original.browser_compatibility),
      },
      {
        id: "source",
        size: 128,
        enableSorting: false,
        header: () => t("appFeedback.source"),
        cell: ({ row }) => renderSource(row.original),
      },
      {
        id: "usage",
        size: 88,
        enableSorting: false,
        header: () => t("profiles.title"),
        cell: ({ row }) => (
          <ProfileUsageButton
            label={t("appFeedback.assignedProfiles", {
              name: row.original.name,
            })}
            failed={referencesFailed}
            profiles={
              referencedProfiles?.filter((profile) =>
                extensionGroups.some(
                  (group) =>
                    group.id === profile.extension_group_id &&
                    group.extension_ids.includes(row.original.id),
                ),
              ) ?? null
            }
          />
        ),
      },
      {
        id: "actions",
        size: 104,
        enableSorting: false,
        header: () => t("common.labels.actions"),
        cell: ({ row }) => {
          const ext = row.original;
          return (
            <div className="flex shrink-0 justify-end gap-0.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="size-7 p-0"
                    onClick={() => {
                      setAssignUsersExtension(ext);
                      setAssignUsersTargets([]);
                    }}
                  >
                    <LuUsers className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>
                  {t("extensions.assignUsers.title")}
                </TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="size-7 p-0"
                    onClick={() => {
                      setEditingExtension(ext);
                      setEditExtensionName(ext.name);
                      setPendingUpdateSource(null);
                      setEditLinkFolder(Boolean(ext.linked_path));
                    }}
                  >
                    <LuPencil className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t("extensions.editExtension")}</TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="size-7 p-0"
                    onClick={() => {
                      setExtensionToDelete(ext);
                    }}
                  >
                    <LuTrash2 className="size-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{t("extensions.delete")}</TooltipContent>
              </Tooltip>
            </div>
          );
        },
      },
    ],
    [
      t,
      referencedProfiles,
      referencesFailed,
      extensionGroups,
      renderExtensionIcon,
      renderCompatIcons,
      renderSource,
    ],
  );

  const extTable = useReactTable({
    data: extensions,
    columns: extensionColumns,
    state: { sorting: extSorting, rowSelection: extRowSelection },
    onSortingChange: setExtSorting,
    onRowSelectionChange: setExtRowSelection,
    enableRowSelection: () => !limitedMode,
    getSortedRowModel: getSortedRowModel(),
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => row.id,
  });

  return (
    <>
      <Dialog open={isOpen} onOpenChange={onClose} subPage={subPage}>
        <DialogContent className="flex max-h-[85vh] max-w-[min(80rem,calc(100%-4rem))] flex-col">
          {!subPage && (
            <DialogHeader>
              <DialogTitle className="flex items-center gap-2">
                <LuPuzzle className="size-5" />
                {t("extensions.title")}
                {limitedMode && <ProBadge />}
              </DialogTitle>
              <DialogDescription>
                {t("extensions.description")}
              </DialogDescription>
            </DialogHeader>
          )}

          <div className="@container relative flex min-h-0 w-full flex-1 flex-col">
            {limitedMode && (
              <>
                <div className="absolute inset-0 z-1 bg-background/30 backdrop-blur-[6px]" />
                <div className="absolute inset-y-0 left-0 z-2 w-6 bg-linear-to-r from-background to-transparent" />
                <div className="absolute inset-y-0 right-0 z-2 w-6 bg-linear-to-l from-background to-transparent" />
                <div className="absolute inset-x-0 top-0 z-2 h-6 bg-linear-to-b from-background to-transparent" />
                <div className="absolute inset-x-0 bottom-0 z-2 h-6 bg-linear-to-t from-background to-transparent" />
                <div className="absolute inset-0 z-3 flex items-center justify-center">
                  <div className="flex items-center gap-2 rounded-md bg-background/80 px-3 py-1.5">
                    <ProBadge />
                    <span className="text-sm font-medium text-muted-foreground">
                      {t("extensions.proRequired")}
                    </span>
                  </div>
                </div>
              </>
            )}

            <AnimatedTabs
              key={initialTab}
              value={activeTab}
              onValueChange={(v) => setActiveTab(v as "extensions" | "groups")}
              className="flex min-h-0 flex-1 flex-col"
            >
              <div className="flex shrink-0 flex-wrap items-center justify-between gap-2">
                <AnimatedTabsList>
                  <AnimatedTabsTrigger
                    value="extensions"
                    disabled={limitedMode}
                  >
                    <span>{t("extensions.extensionsTab")}</span>
                    <span className="text-xs tabular-nums">
                      {extensions.length}
                    </span>
                  </AnimatedTabsTrigger>
                </AnimatedTabsList>
                <div className="flex items-center gap-2">
                  {activeTab === "extensions" && (
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <RippleButton
                            size="sm"
                            variant="outline"
                            disabled={limitedMode}
                            onClick={() =>
                              document.getElementById("ext-file-input")?.click()
                            }
                            aria-label={t("extensions.upload")}
                          >
                            <LuUpload className="size-4" />
                            <span className="hidden @2xl:inline">
                              {t("extensions.upload")}
                            </span>
                          </RippleButton>
                        </TooltipTrigger>
                        <TooltipContent>
                          {t("extensions.upload")}
                        </TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <RippleButton
                            size="sm"
                            variant="outline"
                            disabled={limitedMode}
                            onClick={() => void handleLoadUnpacked()}
                            aria-label={t("extensions.loadUnpacked")}
                          >
                            <LuFolderOpen className="size-4" />
                            <span className="hidden @2xl:inline">
                              {t("extensions.loadUnpacked")}
                            </span>
                          </RippleButton>
                        </TooltipTrigger>
                        <TooltipContent>
                          {t("extensions.loadUnpackedTooltip")}
                        </TooltipContent>
                      </Tooltip>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <RippleButton
                            size="sm"
                            variant="outline"
                            disabled={limitedMode}
                            onClick={() => {
                              setShowUrlForm((open) => !open);
                            }}
                            aria-label={t("extensions.fromUrl")}
                          >
                            <LuGlobe className="size-4" />
                            <span className="hidden @2xl:inline">
                              {t("extensions.fromUrl")}
                            </span>
                          </RippleButton>
                        </TooltipTrigger>
                        <TooltipContent>
                          {t("extensions.fromUrlTooltip")}
                        </TooltipContent>
                      </Tooltip>
                    </>
                  )}
                </div>
              </div>

              {/* Notice */}
              <div className="mt-4 shrink-0 rounded-md bg-muted/50 p-3 text-sm text-muted-foreground">
                {t("extensions.managedNotice")}
              </div>

              <AnimatedTabsContent
                value="extensions"
                className="mt-4 min-h-0 flex-1 flex-col data-[state=active]:flex"
              >
                <div className="flex min-h-0 flex-1 flex-col gap-4">
                  <Input
                    id="ext-file-input"
                    type="file"
                    accept=".crx,.zip"
                    className="hidden"
                    onChange={handleFileSelect}
                    disabled={limitedMode}
                  />

                  {/* Import from a link */}
                  {showUrlForm && (
                    <div className="space-y-2 rounded-md border p-3">
                      <div className="flex gap-2">
                        <Input
                          id="ext-url-input"
                          value={urlInput}
                          onChange={(e) => {
                            setUrlInput(e.target.value);
                          }}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") void handleFetchFromUrl();
                          }}
                          placeholder={t("extensions.urlPlaceholder")}
                          className="flex-1"
                          disabled={isFetchingUrl}
                        />
                        <RippleButton
                          size="sm"
                          onClick={() => void handleFetchFromUrl()}
                          disabled={
                            isFetchingUrl || urlInput.trim().length === 0
                          }
                        >
                          {isFetchingUrl
                            ? t("extensions.fetching")
                            : t("extensions.fetchExtension")}
                        </RippleButton>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => {
                            setShowUrlForm(false);
                            setUrlInput("");
                          }}
                        >
                          {t("common.buttons.cancel")}
                        </Button>
                      </div>
                      <p className="text-xs text-muted-foreground">
                        {t("extensions.urlHint")}
                      </p>
                    </div>
                  )}

                  {/* Import form */}
                  {pendingSource && (
                    <div className="space-y-3 rounded-md border p-3">
                      <div className="text-sm text-muted-foreground">
                        {pendingSource.kind === "folder"
                          ? t("extensions.selectedFolder")
                          : t("extensions.selectedFile")}
                        :{" "}
                        <span className="font-medium break-all text-foreground">
                          {pendingSource.kind === "folder"
                            ? pendingSource.path
                            : pendingSource.fileName}
                        </span>
                      </div>
                      {fetchedIdentity && (
                        <div className="space-y-0.5">
                          <div
                            data-slot="extension-fetched-identity"
                            className="text-sm font-medium break-all text-foreground"
                          >
                            {fetchedIdentity.version
                              ? t("extensions.fetchedIdentity", {
                                  name: fetchedIdentity.name,
                                  version: fetchedIdentity.version,
                                })
                              : fetchedIdentity.name}
                          </div>
                          <p className="text-xs break-all text-muted-foreground">
                            {t("extensions.fetchedFrom", {
                              source: fetchedIdentity.source,
                            })}
                          </p>
                        </div>
                      )}
                      {pendingSource.kind === "folder" && (
                        <div className="flex items-start gap-2">
                          <Checkbox
                            id="ext-link-folder"
                            checked={linkFolder}
                            onCheckedChange={(value) => {
                              setLinkFolder(value === true);
                            }}
                            className="mt-0.5"
                          />
                          <div className="space-y-0.5">
                            <Label
                              htmlFor="ext-link-folder"
                              className="text-sm font-normal"
                            >
                              {t("extensions.linkFolder")}
                            </Label>
                            <p className="text-xs text-muted-foreground">
                              {linkFolder
                                ? t("extensions.linkFolderOn")
                                : t("extensions.linkFolderOff")}
                            </p>
                          </div>
                        </div>
                      )}
                      <div className="flex gap-2">
                        <Input
                          value={extensionName}
                          onChange={(e) => {
                            setExtensionName(e.target.value);
                          }}
                          placeholder={t("extensions.namePlaceholder")}
                          className="flex-1"
                        />
                        <RippleButton
                          size="sm"
                          onClick={() => void handleUpload()}
                          disabled={isUploading || !extensionName.trim()}
                        >
                          {isUploading
                            ? t("common.buttons.loading")
                            : t("common.buttons.add")}
                        </RippleButton>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={resetImportForm}
                        >
                          {t("common.buttons.cancel")}
                        </Button>
                      </div>
                    </div>
                  )}

                  {/* Extensions list */}
                  {isLoading ? (
                    <div className="text-sm text-muted-foreground">
                      {t("common.buttons.loading")}
                    </div>
                  ) : extensions.length === 0 ? (
                    <div className="text-sm text-muted-foreground">
                      {t("extensions.empty")}
                    </div>
                  ) : (
                    <FadingScrollArea
                      className={cn(
                        "min-h-0 flex-1",
                        selectedExtensions.length > 0 && "pb-16",
                      )}
                      style={
                        {
                          "--scroll-fade-top-offset": "32px",
                        } as React.CSSProperties
                      }
                    >
                      <Table
                        className="w-full table-fixed"
                        containerClassName="overflow-visible"
                      >
                        <TableHeader className="sticky top-0 z-10 bg-background [&_tr]:border-0">
                          {extTable.getHeaderGroups().map((headerGroup) => (
                            <TableRow
                              key={headerGroup.id}
                              className="border-0!"
                            >
                              {headerGroup.headers.map((header) => (
                                <TableHead
                                  key={header.id}
                                  style={{
                                    width:
                                      header.column.id === "name"
                                        ? undefined
                                        : `${header.column.getSize()}px`,
                                  }}
                                  className={cn(
                                    header.column.id === "name" && "max-w-0",
                                  )}
                                >
                                  {header.isPlaceholder
                                    ? null
                                    : flexRender(
                                        header.column.columnDef.header,
                                        header.getContext(),
                                      )}
                                </TableHead>
                              ))}
                            </TableRow>
                          ))}
                        </TableHeader>
                        <TableBody>
                          {extTable.getRowModel().rows.map((row) => (
                            <TableRow
                              key={row.id}
                              data-state={row.getIsSelected() && "selected"}
                              className="border-0! hover:bg-muted"
                            >
                              {row.getVisibleCells().map((cell) => (
                                <TableCell
                                  key={cell.id}
                                  style={{
                                    width:
                                      cell.column.id === "name"
                                        ? undefined
                                        : `${cell.column.getSize()}px`,
                                  }}
                                  className={cn(
                                    cell.column.id === "name" && "max-w-0",
                                  )}
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
                    </FadingScrollArea>
                  )}
                </div>
              </AnimatedTabsContent>
            </AnimatedTabs>
          </div>

          {!subPage && (
            <DialogFooter>
              <RippleButton variant="outline" onClick={onClose}>
                {t("common.buttons.close")}
              </RippleButton>
            </DialogFooter>
          )}
        </DialogContent>
      </Dialog>

      {/* Extension editing dialog */}
      <Dialog
        open={editingExtension !== null}
        onOpenChange={(open) => {
          if (!open) closeEditExtension();
        }}
      >
        <DialogContent className="flex max-h-[90vh] max-w-lg flex-col">
          <DialogHeader>
            <DialogTitle>{t("extensions.editExtension")}</DialogTitle>
            <DialogDescription>
              {t("extensions.editExtensionDescription")}
            </DialogDescription>
          </DialogHeader>

          <ScrollArea className="-mx-6 flex-1 overflow-y-auto px-6">
            {editingExtension && (
              <div className="space-y-4">
                <AssignmentImpact
                  sources={[editExtensionName || editingExtension.name]}
                  groups={extensionGroups.filter((group) =>
                    group.extension_ids.includes(editingExtension.id),
                  )}
                  profiles={referencedProfiles}
                  failed={referencesFailed}
                  pending={pendingUpdateSource !== null}
                />
                <div className="space-y-2">
                  <Label>{t("common.labels.name")}</Label>
                  <Input
                    value={editExtensionName}
                    onChange={(e) => {
                      setEditExtensionName(e.target.value);
                    }}
                    placeholder={t("extensions.namePlaceholder")}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void handleUpdateExtension();
                    }}
                  />
                </div>

                {editingExtension.manifest_name &&
                  editingExtension.manifest_name !== editExtensionName && (
                    <div className="flex flex-wrap items-center gap-2 text-xs">
                      <span className="break-words text-muted-foreground">
                        {editingExtension.manifest_name}
                      </span>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() =>
                          setEditExtensionName(
                            editingExtension.manifest_name ??
                              editingExtension.name,
                          )
                        }
                      >
                        {t("appFeedback.useManifestName")}
                      </Button>
                    </div>
                  )}
                {/* Metadata from manifest.json */}
                <div className="space-y-2 rounded-md border p-3">
                  <Label className="text-xs tracking-wide text-muted-foreground uppercase">
                    {t("extensions.metadata")}
                  </Label>
                  <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-sm">
                    {editingExtension.version && (
                      <>
                        <span className="text-muted-foreground">
                          {t("extensions.version")}
                        </span>
                        <span>{editingExtension.version}</span>
                      </>
                    )}
                    {editingExtension.author && (
                      <>
                        <span className="text-muted-foreground">
                          {t("extensions.author")}
                        </span>
                        <span>{editingExtension.author}</span>
                      </>
                    )}
                    {editingExtension.description && (
                      <>
                        <span className="text-muted-foreground">
                          {t("common.labels.description")}
                        </span>
                        <span className="line-clamp-3">
                          {editingExtension.description}
                        </span>
                      </>
                    )}
                    <span className="text-muted-foreground">
                      {t("extensions.compatibility.label")}
                    </span>
                    <div className="flex items-center gap-1">
                      {renderCompatIcons(
                        editingExtension.browser_compatibility,
                      )}
                    </div>
                    <span className="text-muted-foreground">
                      {t("extensions.source.label")}
                    </span>
                    <span>
                      {editingExtension.linked_path
                        ? t("extensions.source.linked")
                        : editingExtension.source_kind === "unpacked"
                          ? t("extensions.source.unpacked")
                          : t("extensions.source.archive")}
                    </span>
                    {editingExtension.linked_path ? (
                      <>
                        <span className="text-muted-foreground">
                          {t("extensions.source.folderLabel")}
                        </span>
                        <span className="break-all">
                          {editingExtension.linked_path}
                        </span>
                        <p className="col-span-2 text-xs text-muted-foreground">
                          {t("extensions.linkFolderOn")}
                        </p>
                      </>
                    ) : (
                      <>
                        <span className="text-muted-foreground">
                          {t("common.labels.type")}
                        </span>
                        <span>.{editingExtension.file_type}</span>
                      </>
                    )}
                    {editingExtension.homepage_url && (
                      <>
                        <span className="text-muted-foreground">
                          {t("extensions.homepage")}
                        </span>
                        <a
                          href={editingExtension.homepage_url}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="flex min-w-0 items-center gap-1 text-primary-text hover:underline"
                        >
                          <span className="truncate">
                            {editingExtension.homepage_url}
                          </span>
                          <LuExternalLink className="size-3 shrink-0" />
                        </a>
                      </>
                    )}
                    {!editingExtension.version &&
                      !editingExtension.author &&
                      !editingExtension.description &&
                      !editingExtension.homepage_url && (
                        <span className="col-span-2 text-xs text-muted-foreground">
                          {t("extensions.noMetadata")}
                        </span>
                      )}
                  </div>
                </div>

                {/* Replace the payload with another archive or folder */}
                <div className="space-y-2">
                  <Label>{t("extensions.replaceSource")}</Label>
                  <div className="flex flex-wrap items-center gap-2">
                    <RippleButton
                      size="sm"
                      variant="outline"
                      onClick={() =>
                        document.getElementById("ext-edit-file-input")?.click()
                      }
                    >
                      <LuUpload className="mr-1 size-3" />
                      {t("extensions.selectFile")}
                    </RippleButton>
                    <input
                      id="ext-edit-file-input"
                      type="file"
                      accept=".crx,.zip"
                      className="hidden"
                      onChange={handleEditFileSelect}
                    />
                    <RippleButton
                      size="sm"
                      variant="outline"
                      onClick={() => void handleEditFolderSelect()}
                    >
                      <LuFolderOpen className="mr-1 size-3" />
                      {t("extensions.selectFolder")}
                    </RippleButton>
                    {pendingUpdateSource && (
                      <span className="max-w-[200px] truncate text-xs text-muted-foreground">
                        {pendingUpdateSource.kind === "folder"
                          ? pendingUpdateSource.path
                          : pendingUpdateSource.fileName}
                      </span>
                    )}
                  </div>
                  {pendingUpdateSource?.kind === "folder" && (
                    <div className="flex items-start gap-2 pt-1">
                      <Checkbox
                        id="ext-edit-link-folder"
                        checked={editLinkFolder}
                        onCheckedChange={(value) => {
                          setEditLinkFolder(value === true);
                        }}
                        className="mt-0.5"
                      />
                      <div className="space-y-0.5">
                        <Label
                          htmlFor="ext-edit-link-folder"
                          className="text-sm font-normal"
                        >
                          {t("extensions.linkFolder")}
                        </Label>
                        <p className="text-xs text-muted-foreground">
                          {editLinkFolder
                            ? t("extensions.linkFolderOn")
                            : t("extensions.linkFolderOff")}
                        </p>
                      </div>
                    </div>
                  )}
                </div>
              </div>
            )}
          </ScrollArea>

          <DialogFooter>
            <Button variant="outline" onClick={closeEditExtension}>
              {t("common.buttons.cancel")}
            </Button>
            <RippleButton
              onClick={() => void handleUpdateExtension()}
              disabled={!editExtensionName.trim()}
            >
              {t("common.buttons.save")}
            </RippleButton>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Assign extension / group to users */}
      <Dialog
        open={assignUsersGroup !== null || assignUsersExtension !== null}
        onOpenChange={(open) => {
          if (!open) closeAssignUsers();
        }}
      >
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2 text-base">
              <LuUsers className="size-4" />
              {t("extensions.assignUsers.title")}
            </DialogTitle>
            <DialogDescription>
              {assignUsersExtension
                ? t("extensions.assignUsers.userDescription", {
                    name: assignUsersSourceName,
                  })
                : t("extensions.assignUsers.description", {
                    name: assignUsersSourceName,
                  })}
            </DialogDescription>
          </DialogHeader>
          {(
            assignUsersExtension
              ? assignUsersCloudUsers.length === 0
              : assignUsersCloudUsers.length === 0 &&
                assignUsersCreatorOptions.length === 0 &&
                assignUsersGroupOptions.length === 0
          ) ? (
            <p className="py-6 text-center text-sm text-muted-foreground">
              {t("extensions.assignUsers.empty")}
            </p>
          ) : (
            <div className="max-h-72 space-y-1 overflow-y-auto pr-1">
              {assignUsersCloudOptions.length > 0 && (
                <>
                  <div className="flex items-center justify-between px-2 pt-1">
                    <p className="text-xs font-medium text-muted-foreground">
                      {t("extensions.assignUsers.cloudSection")}
                    </p>
                    {assignUsersExtension && batchUserIds.length > 0 && (
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-6 px-1.5 text-[11px]"
                        onClick={openBatchAcctDialog}
                      >
                        {t("extensions.assignUsers.accountsBatch")}
                      </Button>
                    )}
                  </div>
                  {assignUsersCloudOptions.map((option) => {
                    const uid = Number(option.key.slice("cloud:".length));
                    const bound = assignUsersAssignments.find(
                      (a) =>
                        a.extension_id === assignUsersExtension?.id &&
                        a.user_id === uid,
                    );
                    return (
                      <div key={option.key}>
                        <label
                          htmlFor={`assign-users-${option.key}`}
                          className="flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 hover:bg-accent"
                        >
                          <Checkbox
                            id={`assign-users-${option.key}`}
                            checked={assignUsersTargets.includes(option.key)}
                            onCheckedChange={(checked) =>
                              toggleAssignUsersTarget(
                                option.key,
                                checked === true,
                              )
                            }
                          />
                          <span
                            className="min-w-0 flex-1 truncate text-sm font-medium"
                            title={option.name}
                          >
                            {option.name}
                          </span>
                          {bound?.account_ids &&
                            bound.account_ids.length > 0 && (
                              <span className="shrink-0 text-[11px] text-muted-foreground">
                                {t("extensions.assignUsers.accountsCount", {
                                  n: bound.account_ids.length,
                                })}
                              </span>
                            )}
                          {bound?.vps_login === true && (
                            <span className="shrink-0 rounded bg-primary/10 px-1 text-[11px] text-primary">
                              VPS
                            </span>
                          )}
                          <span className="shrink-0 text-xs text-muted-foreground">
                            {t("extensions.assignUsers.profileCount", {
                              n: option.count,
                            })}
                          </span>
                          {assignUsersExtension && (
                            <Button
                              variant="ghost"
                              size="sm"
                              className="h-6 shrink-0 px-1.5 text-[11px]"
                              onClick={() =>
                                void openAssignAcctDialog(
                                  uid,
                                  assignUsersCloudUsers.find(
                                    (u) => u.id === uid,
                                  )?.name ?? option.name,
                                )
                              }
                            >
                              {t("extensions.assignUsers.pickAccounts")}
                            </Button>
                          )}
                        </label>
                      </div>
                    );
                  })}
                </>
              )}
              {!assignUsersExtension &&
                assignUsersCreatorOptions.length > 0 && (
                  <>
                    <p className="px-2 pt-1 text-xs font-medium text-muted-foreground">
                      {t("extensions.assignUsers.creatorsSection")}
                    </p>
                    {assignUsersCreatorOptions.map((option) => (
                      <label
                        key={option.key}
                        htmlFor={`assign-users-${option.key}`}
                        className="flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 hover:bg-accent"
                      >
                        <Checkbox
                          id={`assign-users-${option.key}`}
                          checked={assignUsersTargets.includes(option.key)}
                          onCheckedChange={(checked) =>
                            toggleAssignUsersTarget(
                              option.key,
                              checked === true,
                            )
                          }
                        />
                        <span
                          className="min-w-0 flex-1 truncate text-sm font-medium"
                          title={option.email}
                        >
                          {option.email}
                        </span>
                        <span className="shrink-0 text-xs text-muted-foreground">
                          {t("extensions.assignUsers.profileCount", {
                            count: option.count,
                          })}
                        </span>
                      </label>
                    ))}
                  </>
                )}
              {!assignUsersExtension && assignUsersGroupOptions.length > 0 && (
                <>
                  <p className="px-2 pt-2 text-xs font-medium text-muted-foreground">
                    {t("extensions.assignUsers.groupsSection")}
                  </p>
                  {assignUsersGroupOptions.map((option) => (
                    <label
                      key={option.key}
                      htmlFor={`assign-users-${option.key}`}
                      className="flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 hover:bg-accent"
                    >
                      <Checkbox
                        id={`assign-users-${option.key}`}
                        checked={assignUsersTargets.includes(option.key)}
                        onCheckedChange={(checked) =>
                          toggleAssignUsersTarget(option.key, checked === true)
                        }
                      />
                      <span
                        className="min-w-0 flex-1 truncate text-sm font-medium"
                        title={option.name}
                      >
                        {option.name}
                      </span>
                      <span className="shrink-0 text-xs text-muted-foreground">
                        {t("extensions.assignUsers.profileCount", {
                          count: option.count,
                        })}
                      </span>
                    </label>
                  ))}
                </>
              )}
            </div>
          )}
          <DialogFooter>
            <Button variant="outline" onClick={() => closeAssignUsers()}>
              {t("common.buttons.cancel")}
            </Button>
            <Button
              disabled={
                isAssigningUsers ||
                (assignUsersExtension
                  ? assignUsersDiff.toAssign.length === 0 &&
                    assignUsersDiff.toUnassign.length === 0
                  : assignUsersTargets.length === 0)
              }
              onClick={() => void handleAssignGroupToUsers()}
            >
              {isAssigningUsers
                ? t("extensions.assignUsers.assigning")
                : assignUsersExtension &&
                    assignUsersDiff.toAssign.length === 0 &&
                    assignUsersDiff.toUnassign.length > 0
                  ? t("extensions.assignUsers.unassignAction")
                  : assignUsersExtension &&
                      assignUsersDiff.toAssign.length > 0 &&
                      assignUsersDiff.toUnassign.length > 0
                    ? t("extensions.assignUsers.saveAction")
                    : t("extensions.assignUsers.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 账号范围选择：快速搜索 + 勾选账号子集（含 VPS 未启动账号） */}
      <Dialog
        open={assignAcctDialogUser !== null}
        onOpenChange={(open) => {
          if (!open) setAssignAcctDialogUser(null);
        }}
      >
        <DialogContent className="w-[680px] max-w-[95vw] p-0 overflow-hidden">
          <DialogHeader className="border-b px-4 py-3">
            <DialogTitle className="flex items-center gap-2 text-sm font-semibold">
              <LuUsers className="size-4" />
              {assignAcctDialogUser
                ? t("extensions.assignUsers.accountsTitle", {
                    name: assignAcctDialogUser.name,
                  })
                : ""}
            </DialogTitle>
          </DialogHeader>
          <div className="flex items-center gap-2 border-b px-4 py-2">
            <LuSearch className="size-3.5 shrink-0 text-muted-foreground" />
            <Input
              value={assignAcctQuery}
              onChange={(e) => setAssignAcctQuery(e.target.value)}
              placeholder={t("extensions.assignUsers.accountsSearch")}
              className="h-8 text-xs"
            />
            <span className="shrink-0 text-xs text-muted-foreground">
              {t("extensions.assignUsers.profileCount", {
                n: assignAcctMatches.length,
              })}
            </span>
          </div>
          <div className="max-h-[420px] overflow-y-auto px-4 py-2">
            {(() => {
              const uid = assignAcctDialogUser?.id;
              const acct =
                uid === undefined ? undefined : assignUsersAcctLists[uid];
              if (acct === undefined || acct === "loading") {
                return (
                  <p className="py-8 text-center text-xs text-muted-foreground">
                    {t("extensions.assignUsers.accountsLoading")}
                  </p>
                );
              }
              if (acct === "error") {
                return (
                  <p className="py-8 text-center text-xs text-destructive">
                    {t("extensions.assignUsers.accountsError")}
                  </p>
                );
              }
              if (acct.length === 0) {
                return (
                  <p className="py-8 text-center text-xs text-muted-foreground">
                    {t("extensions.assignUsers.accountsEmpty")}
                  </p>
                );
              }
              const scope =
                uid === undefined ? undefined : assignUsersAccountScopes[uid];
              const allSelected = scope ? scope.accounts === null : true;
              const vpsLogin = scope ? scope.vpsLogin === true : false;
              const matches = assignAcctMatches;
              return (
                <>
                  <label
                    htmlFor="assign-acct-all"
                    className="flex cursor-pointer items-center gap-2 py-1.5 text-xs font-medium"
                  >
                    <Checkbox
                      id="assign-acct-all"
                      checked={allSelected}
                      onCheckedChange={(v) =>
                        uid !== undefined &&
                        setAcctScope(uid, {
                          accounts: v === true ? null : [],
                          vpsLogin,
                        })
                      }
                    />
                    {t("extensions.assignUsers.accountsAll")}
                  </label>
                  <label
                    htmlFor="assign-acct-vps"
                    className="flex cursor-pointer items-center gap-2 pb-1.5 text-xs font-medium"
                  >
                    <Checkbox
                      id="assign-acct-vps"
                      checked={vpsLogin}
                      onCheckedChange={(v) =>
                        uid !== undefined &&
                        setAcctScope(uid, {
                          accounts: scope ? scope.accounts : null,
                          vpsLogin: v === true,
                        })
                      }
                    />
                    {t("extensions.assignUsers.vpsLogin")}
                  </label>
                  <div className="space-y-0.5">
                    {matches.map((a) => (
                      <label
                        key={a.id}
                        htmlFor={`assign-acct-${a.id}`}
                        className="flex cursor-pointer items-center gap-2 rounded px-1 py-1 text-xs hover:bg-accent/50"
                      >
                        <Checkbox
                          id={`assign-acct-${a.id}`}
                          checked={
                            allSelected ||
                            (scope?.accounts ?? []).includes(a.id)
                          }
                          onCheckedChange={(v) => {
                            if (uid === undefined) return;
                            const cur = allSelected
                              ? acct.map((x) => x.id)
                              : [...(scope?.accounts ?? [])];
                            const next =
                              v === true
                                ? cur.concat(a.id)
                                : cur.filter((x) => x !== a.id);
                            setAcctScope(uid, {
                              accounts:
                                next.length === acct.length ? null : next,
                              vpsLogin,
                            });
                          }}
                        />
                        <span
                          className="min-w-0 flex-1 truncate"
                          title={a.label}
                        >
                          {a.label}
                        </span>
                      </label>
                    ))}
                  </div>
                  <p className="pt-2 text-[11px] text-muted-foreground">
                    {t("extensions.assignUsers.accountsHint")}
                  </p>
                </>
              );
            })()}
          </div>
          <div className="flex items-center justify-end border-t px-4 py-3">
            <Button
              size="sm"
              className="h-8 text-xs"
              onClick={() => setAssignAcctDialogUser(null)}
            >
              {t("extensions.assignUsers.accountsDone")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* 复制分配：把源插件的用户/账号范围绑定整份复制到当前插件 */}
      <Dialog
        open={copyAssignExtension !== null}
        onOpenChange={(open) => {
          if (!open) setCopyAssignExtension(null);
        }}
      >
        <DialogContent className="w-[440px] max-w-[95vw] p-0 overflow-hidden">
          <DialogHeader className="border-b px-4 py-3">
            <DialogTitle className="flex items-center gap-2 text-sm font-semibold">
              <LuCopy className="size-4" />
              {t("extensions.copyAssign.title")}
            </DialogTitle>
            <DialogDescription>{copyAssignExtension?.name}</DialogDescription>
          </DialogHeader>
          <div className="px-4 py-3">
            <div className="text-xs font-medium">
              {t("extensions.copyAssign.sourceLabel")}
            </div>
            <Select value={copySourceId} onValueChange={setCopySourceId}>
              <SelectTrigger className="mt-1 h-8 text-xs">
                <SelectValue
                  placeholder={t("extensions.copyAssign.sourceLabel")}
                />
              </SelectTrigger>
              <SelectContent>
                {extensions
                  .filter(
                    (e) =>
                      e.id !== copyAssignExtension?.id &&
                      (copySourceCounts[e.id] ?? 0) > 0,
                  )
                  .map((e) => (
                    <SelectItem key={e.id} value={e.id} className="text-xs">
                      {e.name}（{copySourceCounts[e.id]}）
                    </SelectItem>
                  ))}
              </SelectContent>
            </Select>
            {copyAssignExtension !== null &&
              extensions.filter(
                (e) =>
                  e.id !== copyAssignExtension?.id &&
                  (copySourceCounts[e.id] ?? 0) > 0,
              ).length === 0 && (
                <p className="pt-2 text-[11px] text-muted-foreground">
                  {t("extensions.copyAssign.noSource")}
                </p>
              )}
          </div>
          <div className="flex items-center justify-end gap-2 border-t px-4 py-3">
            <Button
              variant="outline"
              size="sm"
              className="h-8 text-xs"
              onClick={() => setCopyAssignExtension(null)}
            >
              {t("common.buttons.cancel")}
            </Button>
            <Button
              size="sm"
              className="h-8 text-xs"
              disabled={!copySourceId || isCopyingAssign}
              onClick={() => void handleCopyAssign()}
            >
              {isCopyingAssign
                ? t("extensions.copyAssign.copying")
                : t("extensions.copyAssign.confirm")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* 批量选账号：为所有已勾选用户一次筛选（如设备编号=素材）并勾选其账号 */}
      <Dialog
        open={batchAcctOpen}
        onOpenChange={(open) => {
          if (!open) setBatchAcctOpen(false);
        }}
      >
        <DialogContent className="w-[680px] max-w-[95vw] p-0 overflow-hidden">
          <DialogHeader className="border-b px-4 py-3">
            <DialogTitle className="flex items-center gap-2 text-sm font-semibold">
              <LuUsers className="size-4" />
              {t("extensions.assignUsers.accountsBatchTitle", {
                n: batchUserIds.length,
              })}
            </DialogTitle>
          </DialogHeader>
          <div className="flex items-center gap-2 border-b px-4 py-2">
            <LuSearch className="size-3.5 shrink-0 text-muted-foreground" />
            <Input
              value={batchQuery}
              onChange={(e) => setBatchQuery(e.target.value)}
              placeholder={t("extensions.assignUsers.accountsSearch")}
              className="h-8 text-xs"
            />
            <label
              htmlFor="batch-acct-vps"
              className="flex shrink-0 cursor-pointer items-center gap-1.5 text-xs"
            >
              <Checkbox
                id="batch-acct-vps"
                checked={batchVpsChecked}
                onCheckedChange={(v) => setBatchVps(v === true)}
              />
              {t("extensions.assignUsers.vpsLogin")}
            </label>
            <Button
              variant="outline"
              size="sm"
              className="h-8 shrink-0 text-xs"
              onClick={selectAllBatchMatches}
            >
              {t("extensions.assignUsers.selectAllMatches")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-8 shrink-0 text-xs"
              onClick={clearBatchMatches}
            >
              {t("extensions.assignUsers.clearMatches")}
            </Button>
          </div>
          <div className="max-h-[420px] overflow-y-auto px-4 py-2">
            {batchUserIds.map((uid) => {
              const list = assignUsersAcctLists[uid];
              const uname =
                assignUsersCloudUsers.find((u) => u.id === uid)?.name ??
                `#${uid}`;
              const q = batchQuery.trim().toLowerCase();
              const matches =
                list && list !== "loading" && list !== "error"
                  ? q
                    ? list.filter((a) => a.label.toLowerCase().includes(q))
                    : list
                  : [];
              const scope = assignUsersAccountScopes[uid];
              const allSelected = scope ? scope.accounts === null : false;
              return (
                <div key={uid} className="mb-3">
                  <p className="py-1 text-xs font-semibold">
                    {uname}
                    <span className="ml-2 font-normal text-muted-foreground">
                      {list === "loading" || list === undefined
                        ? t("extensions.assignUsers.accountsLoading")
                        : list === "error"
                          ? t("extensions.assignUsers.accountsError")
                          : t("extensions.assignUsers.accountsCount", {
                              n: matches.length,
                            })}
                    </span>
                  </p>
                  {matches.length > 0 && (
                    <div className="space-y-0.5">
                      {matches.map((a) => (
                        <label
                          key={a.id}
                          htmlFor={`batch-acct-${uid}-${a.id}`}
                          className="flex cursor-pointer items-center gap-2 rounded px-1 py-0.5 text-xs hover:bg-accent/50"
                        >
                          <Checkbox
                            id={`batch-acct-${uid}-${a.id}`}
                            checked={
                              allSelected ||
                              (scope?.accounts ?? []).includes(a.id)
                            }
                            onCheckedChange={(v) => {
                              const cur = allSelected
                                ? matches.map((x) => x.id)
                                : [...(scope?.accounts ?? [])];
                              const next =
                                v === true
                                  ? cur.concat(a.id)
                                  : cur.filter((x) => x !== a.id);
                              setAcctScope(uid, {
                                accounts:
                                  next.length === matches.length ? null : next,
                                vpsLogin: scope ? scope.vpsLogin : false,
                              });
                            }}
                          />
                          <span
                            className="min-w-0 flex-1 truncate"
                            title={a.label}
                          >
                            {a.label}
                          </span>
                        </label>
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
          <div className="flex items-center justify-end border-t px-4 py-3">
            <Button
              size="sm"
              className="h-8 text-xs"
              onClick={() => setBatchAcctOpen(false)}
            >
              {t("extensions.assignUsers.accountsDone")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {/* Delete extension confirmation */}
      <DeleteConfirmationDialog
        isOpen={extensionToDelete !== null}
        onClose={() => {
          setExtensionToDelete(null);
        }}
        onConfirm={handleDeleteExtension}
        title={t("extensions.deleteConfirmTitle")}
        description={t("extensions.deleteConfirmDescription", {
          name: extensionToDelete?.name ?? "",
        })}
        isLoading={isDeleting}
      />

      {/* Delete group confirmation */}
      <DeleteConfirmationDialog
        isOpen={groupToDelete !== null}
        onClose={() => {
          setGroupToDelete(null);
        }}
        onConfirm={handleDeleteGroup}
        title={t("extensions.deleteGroupConfirmTitle")}
        description={t("extensions.deleteGroupConfirmDescription", {
          name: groupToDelete?.name ?? "",
        })}
        isLoading={isDeleting}
      />

      {/* Bulk delete extensions confirmation */}
      <DeleteConfirmationDialog
        isOpen={bulkExtDeleteOpen}
        onClose={() => {
          setBulkExtDeleteOpen(false);
        }}
        onConfirm={handleBulkDeleteExtensions}
        title={t("extensions.bulkDelete.extensionsTitle")}
        description={t("extensions.bulkDelete.extensionsDescription", {
          count: selectedExtensions.length,
          names: selectedExtensions.map((ext) => ext.name).join(", "),
        })}
        confirmButtonText={t("extensions.bulkDelete.confirmButton")}
        isLoading={isDeleting}
      />

      {/* Bulk delete groups confirmation */}
      <DeleteConfirmationDialog
        isOpen={bulkGroupDeleteOpen}
        onClose={() => {
          setBulkGroupDeleteOpen(false);
        }}
        onConfirm={handleBulkDeleteGroups}
        title={t("extensions.bulkDelete.groupsTitle")}
        description={t("extensions.bulkDelete.groupsDescription", {
          count: selectedGroups.length,
          names: selectedGroups.map((group) => group.name).join(", "),
        })}
        confirmButtonText={t("extensions.bulkDelete.confirmButton")}
        isLoading={isDeleting}
      />

      {/* Bulk action bars — only mount the active tab's bar; an always-
          mounted DataTableActionBar (even with visible=false) keeps an
          AnimatePresence wrapper alive that intermittently captured pointer
          input on the proxy/extension subpages. */}
      {isOpen && activeTab === "extensions" && (
        <DataTableActionBar table={extTable}>
          <DataTableActionBarSelection table={extTable} />
          <DataTableActionBarAction
            tooltip={t("common.buttons.delete")}
            variant="destructive"
            size="icon"
            className="border-destructive bg-destructive hover:bg-destructive"
            onClick={() => {
              setBulkExtDeleteOpen(true);
            }}
          >
            <LuTrash2 />
          </DataTableActionBarAction>
        </DataTableActionBar>
      )}
    </>
  );
}
