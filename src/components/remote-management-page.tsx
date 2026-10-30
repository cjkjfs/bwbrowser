"use client";

import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { confirm as dialogConfirm } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuEye,
  LuFolderOpen,
  LuGlobe,
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
import { translateBackendError } from "@/lib/backend-errors";
import { cn } from "@/lib/utils";

/** 云端 remote_api.php 的 clients 记录（等同 remote_app.php 的机器列表）。 */
interface RemoteMachine {
  id: number;
  client_uuid: string;
  hostname: string;
  /** 被控端 BW 版本号（注册时上报，老机器未上报为空） */
  app_version: string;
  os_info: string;
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
  address: string;
  online: boolean;
  note: string;
  clientId: number;
}

const ALL_COMPANIES = "all";

/** 操作列的三种连接方式，对应被控端 viewer 的 `?mode=`。 */
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
    useBwbrowserCompany(isSuperAdmin, isLoggedIn);

  const [machines, setMachines] = useState<RemoteMachine[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [connectingKey, setConnectingKey] = useState<string | null>(null);
  /** 打开双栏文件传输页的目标；为 null 时显示机器列表 */
  const [fileTarget, setFileTarget] = useState<FileTransferTarget | null>(null);

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
      const ok = await dialogConfirm(
        t("remoteManagement.deleteConfirm", { name: row.hostname }),
        { kind: "warning" },
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

      {/* 机器列表 */}
      <div className="flex-1 overflow-auto">
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
            {isLoading && rows.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={6}
                  className="text-center py-12 text-muted-foreground"
                >
                  <LuLoaderCircle className="w-6 h-6 animate-spin mx-auto mb-2" />
                  {t("remoteManagement.loading")}
                </TableCell>
              </TableRow>
            ) : rows.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={6}
                  className="text-center py-12 text-muted-foreground"
                >
                  {t("remoteManagement.empty")}
                </TableCell>
              </TableRow>
            ) : (
              rows.map((row, idx) => (
                <TableRow key={row.key}>
                  <TableCell className="text-muted-foreground text-sm">
                    {idx + 1}
                  </TableCell>
                  <TableCell className="font-medium">
                    <div className="flex items-center gap-1.5">
                      {row.hostname || "—"}
                      {row.appVersion && (
                        <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[10px] font-normal text-muted-foreground">
                          v{row.appVersion.replace(/^v/, "")}
                        </span>
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
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
