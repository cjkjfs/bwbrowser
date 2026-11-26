"use client";

import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowRightLeft,
  LuEye,
  LuFolderOpen,
  LuGlobe,
  LuLayoutGrid,
  LuList,
  LuLoaderCircle,
  LuMonitorSmartphone,
  LuMousePointerClick,
  LuRefreshCw,
  LuTrash2,
} from "react-icons/lu";
import { toast } from "sonner";
import {
  FileTransferPage,
  type FileTransferTarget,
} from "@/components/file-transfer-page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { confirmDialog } from "@/components/ui/confirm-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import { useBwbrowserPermissions } from "@/hooks/use-bwbrowser-permissions";
import { translateBackendError } from "@/lib/backend-errors";
import { cn } from "@/lib/utils";

interface RemoteHardware {
  cpu: {
    name: string;
    logical_cores: number;
    physical_cores: number | null;
    frequency_mhz: number;
    usage_percent: number;
  };
  memory: { total_bytes: number; used_bytes: number };
  disks: {
    name: string;
    mount_point: string;
    total_bytes: number;
    available_bytes: number;
  }[];
  gpus: { name: string; memory_bytes: number | null }[];
}

/** 云端 remote_api.php 的 clients 记录（等同 remote_app.php 的机器列表）。 */
interface RemoteMachine {
  id: number;
  client_uuid: string;
  hostname: string;
  /** 被控端 BW 版本号（注册时上报，老机器未上报为空） */
  app_version: string;
  os_info: string;
  hardware: RemoteHardware | null;
  /** 静态配置变更后的旧完整硬件快照（云端保留，查看对比后清除） */
  hardware_prev: RemoteHardware | null;
  /** 检测到配置变更的时间（未变更过为空） */
  hardware_changed_at: string;
  /** 与 hardware_prev 配套：true 时列表显示「配置变更」提示 */
  config_changed: boolean;
  note: string;
  local_ips: string[];
  public_ip: string;
  connect_host: string;
  port: number;
  status: string;
  user_username: string;
  user_real_name: string;
}

interface RemoteMachinesResult {
  machines: RemoteMachine[];
  relay_url: string;
}

/** 后端解析出的连接目标：局域网优先，失败回退中继控制页。 */
interface RemoteTarget {
  method: "lan" | "relay";
  url: string;
  host: string;
  port: number;
  hostname: string;
}

/** 表格里的一行：云端机器列表的统一视图。 */
interface MachineRow {
  key: string;
  hostname: string;
  appVersion: string;
  /** 该机登录的爆文库账号：姓名（账号） */
  account: string;
  os: string;
  hardware: RemoteHardware | null;
  hardwarePrev: RemoteHardware | null;
  hardwareChangedAt: string;
  configChanged: boolean;
  address: string;
  online: boolean;
  note: string;
  clientId: number;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(value >= 100 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function usagePercent(used: number, total: number): string {
  if (total <= 0) return "—";
  return `${Math.round((used / total) * 100)}%`;
}

function HardwareDetails({ hardware }: { hardware: RemoteHardware | null }) {
  const { t } = useTranslation();
  if (!hardware) {
    return (
      <div className="text-xs text-primary-foreground/80">
        {t("remoteManagement.hardwareUnavailable")}
      </div>
    );
  }
  const { cpu, memory, disks, gpus } = hardware;
  const cores = cpu.physical_cores
    ? `${cpu.physical_cores} / ${cpu.logical_cores}`
    : String(cpu.logical_cores || "—");
  const diskUsed = (disk: RemoteHardware["disks"][number]) =>
    Math.max(0, disk.total_bytes - disk.available_bytes);
  return (
    <div className="space-y-2.5 text-left text-xs">
      <div className="border-b border-primary-foreground/20 pb-1.5 font-medium">
        {t("remoteManagement.hardwareTitle")}
      </div>
      <div>
        <div className="font-medium">{t("remoteManagement.hardwareCpu")}</div>
        <div>{cpu.name || "—"}</div>
        <div className="mt-0.5 text-primary-foreground/80">
          {t("remoteManagement.hardwareCores", { cores })} ·{" "}
          {cpu.frequency_mhz > 0
            ? `${(cpu.frequency_mhz / 1000).toFixed(2)} GHz`
            : "—"}{" "}
          · {t("remoteManagement.hardwareUsage", { usage: cpu.usage_percent })}
        </div>
      </div>
      <div>
        <div className="font-medium">
          {t("remoteManagement.hardwareMemory")}
        </div>
        <div>
          {formatBytes(memory.used_bytes)} / {formatBytes(memory.total_bytes)} ·{" "}
          {usagePercent(memory.used_bytes, memory.total_bytes)}
        </div>
      </div>
      <div>
        <div className="font-medium">{t("remoteManagement.hardwareDisk")}</div>
        {disks.length ? (
          disks.map((disk) => (
            <div
              key={`${disk.mount_point}:${disk.name}`}
              className="mt-0.5 break-all"
            >
              <div>
                {[disk.mount_point, disk.name].filter(Boolean).join(" · ")}
              </div>
              <div className="text-primary-foreground/80">
                {formatBytes(diskUsed(disk))} / {formatBytes(disk.total_bytes)}{" "}
                · {usagePercent(diskUsed(disk), disk.total_bytes)} ·{" "}
                {t("remoteManagement.hardwareAvailable", {
                  available: formatBytes(disk.available_bytes),
                })}
              </div>
            </div>
          ))
        ) : (
          <div className="text-primary-foreground/80">—</div>
        )}
      </div>
      <div>
        <div className="font-medium">{t("remoteManagement.hardwareGpu")}</div>
        {gpus.length ? (
          gpus.map((gpu) => (
            <div key={gpu.name}>
              {gpu.name}
              {gpu.memory_bytes ? ` · ${formatBytes(gpu.memory_bytes)}` : ""}
            </div>
          ))
        ) : (
          <div className="text-primary-foreground/80">—</div>
        )}
      </div>
    </div>
  );
}

/** 静态配置对比的一行：label + 变更前值 + 当前值（changed 时高亮）。 */
interface ConfigDiffLine {
  label: string;
  prev: string;
  curr: string;
  changed: boolean;
}

type Translate = (key: string, opts?: Record<string, unknown>) => string;

/** 生成新旧配置的静态项对比（CPU/内存总量/磁盘容量/GPU，不含占用率等动态值）。 */
function buildConfigDiffLines(
  prev: RemoteHardware | null,
  curr: RemoteHardware | null,
  t: Translate,
): ConfigDiffLine[] {
  const lines: ConfigDiffLine[] = [];

  const cpuText = (hw: RemoteHardware | null) => {
    const cpu = hw?.cpu;
    if (!cpu) return "—";
    const cores = cpu.physical_cores
      ? `${cpu.physical_cores} / ${cpu.logical_cores}`
      : String(cpu.logical_cores || "—");
    return `${cpu.name || "—"}（${t("remoteManagement.hardwareCores", {
      cores,
    })}）`;
  };
  const cpuPrev = cpuText(prev);
  const cpuCurr = cpuText(curr);
  lines.push({
    label: t("remoteManagement.configDiffCpu"),
    prev: cpuPrev,
    curr: cpuCurr,
    changed: cpuPrev !== cpuCurr,
  });

  const memPrev = prev?.memory?.total_bytes ?? 0;
  const memCurr = curr?.memory?.total_bytes ?? 0;
  lines.push({
    label: t("remoteManagement.configDiffMemory"),
    prev: formatBytes(memPrev),
    curr: formatBytes(memCurr),
    changed: memPrev !== memCurr,
  });

  // 硬盘按挂载点合并（容量或挂载点集合变化都算变更）
  const diskKeys = new Set<string>();
  for (const d of prev?.disks ?? []) diskKeys.add(d.mount_point || d.name);
  for (const d of curr?.disks ?? []) diskKeys.add(d.mount_point || d.name);
  const diskNone = t("remoteManagement.configDiffDiskNone");
  for (const key of [...diskKeys].sort()) {
    const dPrev = (prev?.disks ?? []).find(
      (d) => (d.mount_point || d.name) === key,
    );
    const dCurr = (curr?.disks ?? []).find(
      (d) => (d.mount_point || d.name) === key,
    );
    const prevText = dPrev
      ? `${key} · ${formatBytes(dPrev.total_bytes)}`
      : `${key} · ${diskNone}`;
    const currText = dCurr
      ? `${key} · ${formatBytes(dCurr.total_bytes)}`
      : `${key} · ${diskNone}`;
    lines.push({
      label: t("remoteManagement.configDiffDisk"),
      prev: prevText,
      curr: currText,
      changed: prevText !== currText,
    });
  }

  const gpuText = (hw: RemoteHardware | null) =>
    (hw?.gpus ?? [])
      .map((g) => g.name)
      .filter(Boolean)
      .sort()
      .join("、") || "—";
  const gpuPrev = gpuText(prev);
  const gpuCurr = gpuText(curr);
  lines.push({
    label: t("remoteManagement.configDiffGpu"),
    prev: gpuPrev,
    curr: gpuCurr,
    changed: gpuPrev !== gpuCurr,
  });
  return lines;
}

/** 「配置变更」对比弹窗内容：旧配置 → 新配置逐项对照，变化行高亮。 */
function ConfigDiffDetails({ row }: { row: MachineRow }) {
  const { t } = useTranslation();
  const lines = buildConfigDiffLines(row.hardwarePrev, row.hardware, t);
  const changedCount = lines.filter((l) => l.changed).length;
  return (
    <div className="text-sm">
      <div className="mb-3 grid grid-cols-[90px_1fr_1fr] gap-2 text-xs font-medium text-muted-foreground">
        <div />
        <div>{t("remoteManagement.configDiffPrevHeader")}</div>
        <div>{t("remoteManagement.configDiffCurrHeader")}</div>
      </div>
      <div className="space-y-2">
        {lines.map((line, i) => (
          <div
            key={`${line.label}:${i}`}
            className={cn(
              "grid grid-cols-[90px_1fr_1fr] gap-2 rounded-md px-2 py-1.5 text-xs",
              line.changed &&
                "bg-amber-500/10 text-amber-600 dark:text-amber-400",
            )}
          >
            <div className="font-medium">{line.label}</div>
            <div className="break-all">{line.prev}</div>
            <div className="break-all">{line.curr}</div>
          </div>
        ))}
      </div>
      <div className="mt-3 text-xs text-muted-foreground">
        {changedCount > 0
          ? t("remoteManagement.configDiffSummaryChanged", {
              count: changedCount,
            })
          : t("remoteManagement.configDiffSummaryUnchanged")}
      </div>
    </div>
  );
}

const ALL_COMPANIES =
  "all"; /** 操作列的三种连接方式，对应被控端 viewer 的 `?mode=`。 */
type RemoteMode = "full" | "view" | "files";

const ACTIONS: {
  mode: RemoteMode;
  labelKey: string;
  Icon: typeof LuMousePointerClick;
}[] = [
  {
    mode: "full",
    labelKey: "remoteManagement.fullControl",
    Icon: LuMousePointerClick,
  },
  { mode: "view", labelKey: "remoteManagement.viewOnly", Icon: LuEye },
  {
    mode: "files",
    labelKey: "remoteManagement.fileTransfer",
    Icon: LuFolderOpen,
  },
];

const MODE_LABEL_KEY = Object.fromEntries(
  ACTIONS.map((a) => [a.mode, a.labelKey]),
) as Record<RemoteMode, string>;

/** 应用内窗口标签只允许字母/数字/`-`/`:`/`_`，IP 里的点等字符需替换。 */
function viewerWindowLabel(key: string, mode: RemoteMode): string {
  return `remote-${key}-${mode}`.replace(/[^a-zA-Z0-9\-/:_]/g, "-");
}

/**
 * 在应用自己的窗口里打开被控端 viewer（不拉起系统浏览器）。
 * 同一目标 + 模式重复点击时聚焦已开窗口，不重复创建；
 * 窗口创建失败（如权限缺失）时回退系统浏览器，保证功能不丢。
 */
async function openViewerWindow(label: string, url: string, title: string) {
  const existing = await WebviewWindow.getByLabel(label);
  if (existing) {
    await existing.setFocus();
    return;
  }
  const win = new WebviewWindow(label, {
    url,
    title,
    width: 1280,
    height: 800,
    minWidth: 900,
    minHeight: 600,
    center: true,
    resizable: true,
  });
  win.once("tauri://error", () => {
    void openUrl(url);
  });
}

export function RemoteManagementPage() {
  const { t } = useTranslation();
  const { isLoggedIn } = useBwbrowserAuth();
  const { isSuperAdmin } = useBwbrowserPermissions();
  const { companies, selectedCompanyId, setSelectedCompanyId } =
    // 本页 null=「全部公司」语义，关闭「自动选中第一家公司」填充
    useBwbrowserCompany(isSuperAdmin, isLoggedIn, true, false);

  const [machines, setMachines] = useState<RemoteMachine[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [connectingKey, setConnectingKey] = useState<string | null>(null);
  /** 打开双栏文件传输页的目标；为 null 时显示机器列表 */
  const [fileTarget, setFileTarget] = useState<FileTransferTarget | null>(null);
  /** 「配置变更」对比弹窗的目标行；为 null 时不显示 */
  const [configDiffRow, setConfigDiffRow] = useState<MachineRow | null>(null);
  const [clearingDiff, setClearingDiff] = useState(false);
  /** 视图模式：卡片 / 列表（记住上次选择） */
  const [viewMode, setViewMode] = useState<"card" | "list">(() =>
    localStorage.getItem("remote_mgmt_view_mode") === "list" ? "list" : "card",
  );
  const switchViewMode = useCallback((mode: "card" | "list") => {
    setViewMode(mode);
    localStorage.setItem("remote_mgmt_view_mode", mode);
  }, []);

  const loadMachines = useCallback(async () => {
    setIsLoading(true);
    try {
      const result = await invoke<RemoteMachinesResult>(
        "bwbrowser_remote_management_machines",
        { companyId: selectedCompanyId },
      );
      setMachines(result.machines);
    } catch (e) {
      toast.error(translateBackendError(t, e));
      setMachines([]);
    } finally {
      setIsLoading(false);
    }
  }, [selectedCompanyId, t]);

  /** 删除无用/长期不在线的机器（服务端按远程管理权限校验）。 */
  const [deletingId, setDeletingId] = useState<number | null>(null);
  const handleDeleteMachine = useCallback(
    async (row: MachineRow) => {
      const ok = await confirmDialog(
        t("remoteManagement.deleteConfirm", { name: row.hostname }),
        { kind: "danger" },
      );
      if (!ok) return;
      setDeletingId(row.clientId);
      try {
        await invoke("bwbrowser_remote_management_delete", {
          clientId: row.clientId,
        });
        toast.success(t("remoteManagement.deleteSuccess"));
        void loadMachines();
      } catch (e) {
        toast.error(translateBackendError(t, e));
      } finally {
        setDeletingId(null);
      }
    },
    [loadMachines, t],
  );

  useEffect(() => {
    void loadMachines();
  }, [loadMachines]);

  /** 查看对比后清除该机器的变更提示（云端 hardware_prev 置空，标记已读） */
  const handleClearConfigChange = useCallback(async () => {
    if (!configDiffRow) return;
    setClearingDiff(true);
    try {
      await invoke("bwbrowser_remote_management_clear_config_change", {
        clientId: configDiffRow.clientId,
      });
      toast.success(t("remoteManagement.configDiffAckSuccess"));
      setConfigDiffRow(null);
      void loadMachines();
    } catch (e) {
      toast.error(translateBackendError(t, e));
    } finally {
      setClearingDiff(false);
    }
  }, [configDiffRow, loadMachines, t]);

  // 连接：交给后端解析（后端先探被控端的局域网地址 /discover，
  // 可达即局域网直连，不可达才回退中继控制页）。
  // mode 决定 viewer 落到完全控制 / 仅查看 / 文件传输；viewer 在应用内窗口打开。
  const handleConnect = useCallback(
    async (row: MachineRow, mode: RemoteMode) => {
      // 文件传输用应用内双栏页（左本机 / 右远程），不打开被控端 viewer 窗口。
      if (mode === "files") {
        setFileTarget({
          hostName: row.hostname,
          clientId: row.clientId,
        });
        return;
      }
      setConnectingKey(`${row.key}:${mode}`);
      try {
        const title = `${t(MODE_LABEL_KEY[mode])} · ${
          row.account || row.hostname || row.address
        }`;
        const label = viewerWindowLabel(row.key, mode);
        const target = await invoke<RemoteTarget>(
          "bwbrowser_remote_management_target",
          { clientId: row.clientId, mode },
        );
        await openViewerWindow(label, target.url, title);
        const name = target.hostname || row.hostname;
        toast.success(
          target.method === "lan"
            ? t("remoteManagement.connectLanSuccess", { host: name })
            : t("remoteManagement.connectRelaySuccess", { host: name }),
        );
      } catch (e) {
        toast.error(translateBackendError(t, e));
      } finally {
        setConnectingKey(null);
      }
    },
    [t],
  );

  const rows = useMemo<MachineRow[]>(() => {
    const merged: MachineRow[] = machines.map((m) => ({
      key: `cloud-${m.id}`,
      hostname: m.hostname || "",
      appVersion: m.app_version || "",
      account:
        m.user_real_name && m.user_username
          ? `${m.user_real_name}（${m.user_username}）`
          : m.user_real_name || m.user_username || "",
      os: m.os_info || "",
      hardware: m.hardware,
      hardwarePrev: m.hardware_prev,
      hardwareChangedAt: m.hardware_changed_at || "",
      configChanged: m.config_changed,
      address: m.local_ips.filter(Boolean)[0] || m.public_ip || "",
      online: m.status === "online",
      note: m.note,
      clientId: m.id,
    }));
    merged.sort((a, b) => {
      if (a.online !== b.online) return a.online ? -1 : 1;
      return 0;
    });

    // 同一账号在多台电脑登录会产生多行：组内只要有在线的，
    // 就用在线行替换掉离线行（全离线的组原样保留，避免账号直接消失）。
    const onlineByAccount = new Map<string, boolean>();
    for (const r of merged) {
      if (!r.account) continue;
      onlineByAccount.set(
        r.account,
        (onlineByAccount.get(r.account) ?? false) || r.online,
      );
    }
    return merged.filter(
      (r) => !r.account || r.online || !onlineByAccount.get(r.account),
    );
  }, [machines]);

  const onlineCount = rows.filter((r) => r.online).length;

  // 文件传输：整页替换机器列表，返回时回到列表。
  if (fileTarget) {
    return (
      <FileTransferPage
        target={fileTarget}
        onClose={() => setFileTarget(null)}
      />
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* 头部 */}
      <div className="flex items-center justify-between px-6 py-4 border-b border-border">
        <div className="flex items-center gap-3">
          <LuMonitorSmartphone className="w-5 h-5" />
          <h2 className="text-lg font-semibold">
            {t("remoteManagement.title")}
          </h2>
          <Badge variant="secondary" className="ml-2">
            {t("remoteManagement.onlineSummary", {
              online: onlineCount,
              total: rows.length,
            })}
          </Badge>
        </div>
        <div className="flex items-center gap-2">
          {/* 视图切换：卡片 / 列表 */}
          <div className="flex items-center rounded-lg border border-border p-0.5">
            <button
              type="button"
              className={cn(
                "flex h-7 items-center gap-1 rounded-md px-2 text-xs transition-colors",
                viewMode === "card"
                  ? "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => switchViewMode("card")}
            >
              <LuLayoutGrid className="w-3.5 h-3.5" />
              {t("remoteManagement.cardView")}
            </button>
            <button
              type="button"
              className={cn(
                "flex h-7 items-center gap-1 rounded-md px-2 text-xs transition-colors",
                viewMode === "list"
                  ? "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => switchViewMode("list")}
            >
              <LuList className="w-3.5 h-3.5" />
              {t("remoteManagement.listView")}
            </button>
          </div>
          {isSuperAdmin && companies.length > 0 && (
            <Select
              value={
                selectedCompanyId ? String(selectedCompanyId) : ALL_COMPANIES
              }
              onValueChange={(v) =>
                setSelectedCompanyId(v === ALL_COMPANIES ? null : Number(v))
              }
            >
              <SelectTrigger className="w-[160px] h-8 text-xs">
                <SelectValue
                  placeholder={t("remoteManagement.selectCompany")}
                />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL_COMPANIES}>
                  {t("remoteManagement.allCompanies")}
                </SelectItem>
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
            onClick={() => void loadMachines()}
            disabled={isLoading}
          >
            <LuRefreshCw
              className={cn("w-4 h-4 mr-2", isLoading && "animate-spin")}
            />
            {t("remoteManagement.refresh")}
          </Button>
        </div>
      </div>

      {/* 机器列表（卡片 / 列表双视图，均带序号） */}
      <div className="flex-1 overflow-auto px-6 py-4">
        {isLoading && rows.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 text-muted-foreground">
            <LuLoaderCircle className="w-6 h-6 animate-spin mb-2" />
            {t("remoteManagement.loading")}
          </div>
        ) : rows.length === 0 ? (
          <div className="flex items-center justify-center py-16 text-muted-foreground">
            {t("remoteManagement.empty")}
          </div>
        ) : viewMode === "card" ? (
          <div className="grid gap-4 grid-cols-[repeat(auto-fill,minmax(300px,1fr))]">
            {rows.map((row, idx) => (
              <div
                key={row.key}
                className={cn(
                  "flex flex-col rounded-xl border bg-card p-4 transition-shadow hover:shadow-md",
                  !row.online && "opacity-60",
                )}
              >
                {/* 头部：序号 + 主机名（硬件详情悬浮）+ 版本 + 在线状态 */}
                <div className="flex items-start justify-between gap-2">
                  <div className="flex items-center gap-1.5 min-w-0">
                    <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground tabular-nums">
                      {idx + 1}
                    </span>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          className="truncate font-medium cursor-help underline decoration-dotted underline-offset-4 outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        >
                          {row.hostname || "—"}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent
                        side="bottom"
                        sideOffset={8}
                        className="w-[360px] p-3"
                      >
                        <HardwareDetails hardware={row.hardware} />
                      </TooltipContent>
                    </Tooltip>
                    {row.appVersion && (
                      <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] font-normal text-muted-foreground">
                        v{row.appVersion.replace(/^v/, "")}
                      </span>
                    )}
                  </div>
                  <span
                    className={cn(
                      "inline-flex shrink-0 items-center gap-1.5 text-xs",
                      row.online ? "text-success" : "text-muted-foreground",
                    )}
                  >
                    <span
                      className={cn(
                        "size-2 rounded-full",
                        row.online ? "bg-success" : "bg-muted-foreground",
                      )}
                    />
                    {row.online
                      ? t("remoteManagement.online")
                      : t("remoteManagement.offline")}
                  </span>
                </div>

                {/* 信息区 */}
                <div className="mt-2.5 space-y-1 text-xs text-muted-foreground">
                  {row.configChanged && (
                    <button
                      type="button"
                      className="inline-flex items-center gap-1 rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-600 cursor-pointer hover:bg-amber-500/25 dark:text-amber-400"
                      onClick={() => setConfigDiffRow(row)}
                    >
                      <LuArrowRightLeft className="w-3 h-3 shrink-0" />
                      {t("remoteManagement.configChangedBadge")}
                      {row.hardwareChangedAt
                        ? ` · ${row.hardwareChangedAt.slice(5, 16)}`
                        : ""}
                    </button>
                  )}
                  {row.account && (
                    <button
                      type="button"
                      className="flex items-center gap-1 text-xs font-normal text-muted-foreground cursor-pointer hover:text-foreground hover:underline disabled:cursor-default disabled:hover:no-underline disabled:hover:text-muted-foreground"
                      title={t("remoteManagement.viewOnly")}
                      disabled={!row.online || connectingKey !== null}
                      onClick={() => void handleConnect(row, "view")}
                    >
                      <LuEye className="w-3 h-3 shrink-0" />
                      <span className="truncate">{row.account}</span>
                    </button>
                  )}
                  {row.os && (
                    <div className="truncate" title={row.os}>
                      {row.os}
                    </div>
                  )}
                  <div className="flex items-center gap-1.5">
                    <LuGlobe className="w-3 h-3 shrink-0" />
                    {row.address || "—"}
                  </div>
                  {row.note && (
                    <div
                      className="rounded bg-muted/60 px-2 py-1 break-all"
                      title={row.note}
                    >
                      {row.note}
                    </div>
                  )}
                </div>

                {/* 操作区 */}
                <div className="mt-3 flex flex-wrap items-center gap-1.5 border-t pt-3">
                  {ACTIONS.map(({ mode, labelKey, Icon }) => (
                    <Button
                      key={mode}
                      size="sm"
                      variant="outline"
                      className="h-7 px-2 text-xs"
                      disabled={!row.online || connectingKey !== null}
                      onClick={() => void handleConnect(row, mode)}
                    >
                      {connectingKey === `${row.key}:${mode}` ? (
                        <LuLoaderCircle className="w-3.5 h-3.5 mr-1 animate-spin" />
                      ) : (
                        <Icon className="w-3.5 h-3.5 mr-1" />
                      )}
                      {t(labelKey)}
                    </Button>
                  ))}
                  <Button
                    size="sm"
                    variant="outline"
                    className="ml-auto h-7 px-2 text-xs text-destructive hover:bg-destructive/10"
                    disabled={deletingId !== null}
                    onClick={() => void handleDeleteMachine(row)}
                  >
                    {deletingId === row.clientId ? (
                      <LuLoaderCircle className="w-3.5 h-3.5 animate-spin" />
                    ) : (
                      <LuTrash2 className="w-3.5 h-3.5" />
                    )}
                  </Button>
                </div>
              </div>
            ))}
          </div>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-12">#</TableHead>
                <TableHead>{t("remoteManagement.colHostname")}</TableHead>
                <TableHead>{t("remoteManagement.colOs")}</TableHead>
                <TableHead>{t("remoteManagement.colAddress")}</TableHead>
                <TableHead>{t("remoteManagement.colStatus")}</TableHead>
                <TableHead className="text-right">
                  {t("remoteManagement.colActions")}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((row, idx) => (
                <TableRow key={row.key}>
                  <TableCell className="text-muted-foreground text-sm tabular-nums">
                    {idx + 1}
                  </TableCell>
                  <TableCell className="font-medium">
                    <div className="flex items-center gap-1.5">
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            type="button"
                            className="cursor-help underline decoration-dotted underline-offset-4 outline-none focus-visible:ring-2 focus-visible:ring-ring"
                          >
                            {row.hostname || "—"}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent
                          side="right"
                          sideOffset={8}
                          className="w-[360px] p-3"
                        >
                          <HardwareDetails hardware={row.hardware} />
                        </TooltipContent>
                      </Tooltip>
                      {row.appVersion && (
                        <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] font-normal text-muted-foreground">
                          v{row.appVersion.replace(/^v/, "")}
                        </span>
                      )}
                      {row.configChanged && (
                        <button
                          type="button"
                          className="inline-flex shrink-0 items-center gap-1 rounded bg-amber-500/15 px-1 py-0.5 text-[10px] font-medium text-amber-600 cursor-pointer hover:bg-amber-500/25 dark:text-amber-400"
                          onClick={() => setConfigDiffRow(row)}
                        >
                          <LuArrowRightLeft className="w-3 h-3" />
                          {t("remoteManagement.configChangedBadge")}
                        </button>
                      )}
                    </div>
                    {row.account && (
                      <button
                        type="button"
                        className="flex items-center gap-1 text-xs font-normal text-muted-foreground cursor-pointer hover:text-foreground hover:underline disabled:cursor-default disabled:hover:no-underline disabled:hover:text-muted-foreground"
                        title={t("remoteManagement.viewOnly")}
                        disabled={!row.online || connectingKey !== null}
                        onClick={() => void handleConnect(row, "view")}
                      >
                        <LuEye className="w-3 h-3" />
                        {row.account}
                      </button>
                    )}
                    {row.note && (
                      <div className="text-xs font-normal text-muted-foreground">
                        {row.note}
                      </div>
                    )}
                  </TableCell>
                  <TableCell className="text-sm text-muted-foreground">
                    {row.os || "—"}
                  </TableCell>
                  <TableCell className="text-sm">
                    {row.address ? (
                      <span className="inline-flex items-center gap-1.5">
                        <LuGlobe className="w-3.5 h-3.5 text-muted-foreground" />
                        {row.address}
                      </span>
                    ) : (
                      <span className="text-muted-foreground">—</span>
                    )}
                  </TableCell>
                  <TableCell>
                    <span
                      className={cn(
                        "inline-flex items-center gap-1.5 text-sm",
                        row.online ? "text-success" : "text-muted-foreground",
                      )}
                    >
                      <span
                        className={cn(
                          "size-2 rounded-full",
                          row.online ? "bg-success" : "bg-muted-foreground",
                        )}
                      />
                      {row.online
                        ? t("remoteManagement.online")
                        : t("remoteManagement.offline")}
                    </span>
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex items-center justify-end gap-1">
                      {ACTIONS.map(({ mode, labelKey, Icon }) => (
                        <Button
                          key={mode}
                          size="sm"
                          variant="outline"
                          className="h-7 px-2 text-xs"
                          disabled={!row.online || connectingKey !== null}
                          onClick={() => void handleConnect(row, mode)}
                        >
                          {connectingKey === `${row.key}:${mode}` ? (
                            <LuLoaderCircle className="w-3.5 h-3.5 mr-1 animate-spin" />
                          ) : (
                            <Icon className="w-3.5 h-3.5 mr-1" />
                          )}
                          {t(labelKey)}
                        </Button>
                      ))}
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-7 px-2 text-xs text-destructive hover:bg-destructive/10"
                        disabled={deletingId !== null}
                        onClick={() => void handleDeleteMachine(row)}
                      >
                        {deletingId === row.clientId ? (
                          <LuLoaderCircle className="w-3.5 h-3.5 mr-1 animate-spin" />
                        ) : (
                          <LuTrash2 className="w-3.5 h-3.5 mr-1" />
                        )}
                        {t("remoteManagement.delete")}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>

      {/* 配置变更对比弹窗：旧配置 → 新配置逐项对照 */}
      <Dialog
        open={configDiffRow !== null}
        onOpenChange={(open) => {
          if (!open) setConfigDiffRow(null);
        }}
      >
        <DialogContent className="max-w-[640px]">
          <DialogHeader>
            <DialogTitle>{t("remoteManagement.configDiffTitle")}</DialogTitle>
            <DialogDescription>
              {t("remoteManagement.configDiffDesc", {
                name: configDiffRow?.hostname || "—",
                time: configDiffRow?.hardwareChangedAt || "—",
              })}
            </DialogDescription>
          </DialogHeader>
          {configDiffRow && <ConfigDiffDetails row={configDiffRow} />}
          <div className="flex justify-end gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={clearingDiff}
              onClick={() => setConfigDiffRow(null)}
            >
              {t("common.buttons.close")}
            </Button>
            <Button
              size="sm"
              disabled={clearingDiff}
              onClick={() => void handleClearConfigChange()}
            >
              {clearingDiff && (
                <LuLoaderCircle className="w-3.5 h-3.5 mr-1 animate-spin" />
              )}
              {t("remoteManagement.configDiffAck")}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
