"use client";

import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuEye,
  LuFolderOpen,
  LuGlobe,
  LuLoaderCircle,
  LuMonitorSmartphone,
  LuMousePointerClick,
  LuRefreshCw,
  LuWifi,
} from "react-icons/lu";
import { toast } from "sonner";
import {
  FileTransferPage,
  type FileTransferTarget,
} from "@/components/file-transfer-page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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

/** 被控端默认端口（与后端 RemoteAgentConfig 默认值一致）。 */
const DEFAULT_AGENT_PORT = 8765;

/** 云端 remote_api.php 的 clients 记录（等同 remote_app.php 的机器列表）。 */
interface RemoteMachine {
  id: number;
  client_uuid: string;
  hostname: string;
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

/** 局域网 /discover 扫到的一台被控端（remote_duli 控制端的扫描结果）。 */
interface LanMachine {
  host: string;
  name: string;
  os: string;
  width: number;
  height: number;
  port: number;
}

/** 后端解析出的连接目标：局域网优先，失败回退中继控制页。 */
interface RemoteTarget {
  method: "lan" | "relay";
  url: string;
  host: string;
  port: number;
  hostname: string;
}

/** 表格里的一行：云端机器与局域网发现的机器合并后的统一视图。 */
interface MachineRow {
  key: string;
  hostname: string;
  /** 该机登录的爆文库账号：姓名（账号），局域网直连发现的机器没有此信息 */
  account: string;
  os: string;
  address: string;
  source: "lan" | "cloud";
  lanReachable: boolean;
  online: boolean;
  note: string;
  clientId: number | null;
  lanHost: string | null;
  lanPort: number | null;
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
  const [lanMachines, setLanMachines] = useState<LanMachine[]>([]);
  const [ranges, setRanges] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [isScanning, setIsScanning] = useState(false);
  const [connectingKey, setConnectingKey] = useState<string | null>(null);
  /** 打开双栏文件传输页的目标；为 null 时显示机器列表 */
  const [fileTarget, setFileTarget] = useState<FileTransferTarget | null>(null);
  const autoScanned = useRef(false);

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

  const scanLan = useCallback(
    async (override?: string) => {
      const text = (override ?? ranges).trim();
      setIsScanning(true);
      try {
        const found = await invoke<LanMachine[]>(
          "bwbrowser_remote_management_scan_lan",
          { ranges: text || null, port: DEFAULT_AGENT_PORT },
        );
        setLanMachines(found);
        toast.success(t("remoteManagement.scanFound", { n: found.length }));
      } catch (e) {
        toast.error(translateBackendError(t, e));
      } finally {
        setIsScanning(false);
      }
    },
    [ranges, t],
  );

  useEffect(() => {
    void loadMachines();
  }, [loadMachines]);

  // 进页面自动扫一次局域网（remote_duli 的自动扫描），之后由按钮手动重扫。
  useEffect(() => {
    if (autoScanned.current) return;
    autoScanned.current = true;
    void scanLan("");
    // 只在首次挂载时触发；scanLan 依赖变化不应重扫。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scanLan]);

  // 合并云端机器与局域网发现的机器：能局域网直连的排前面（局域网优先）。
  const rows = useMemo<MachineRow[]>(() => {
    const lanByHost = new Map(lanMachines.map((m) => [m.host, m]));
    const matchedHosts = new Set<string>();

    const cloudRows: MachineRow[] = machines.map((m) => {
      const candidates = [m.connect_host, ...m.local_ips].filter(Boolean);
      const lanHost = candidates.find((ip) => lanByHost.has(ip)) ?? null;
      const lan = lanHost ? lanByHost.get(lanHost) : undefined;
      if (lanHost) matchedHosts.add(lanHost);
      const lanIps = m.local_ips.filter(Boolean);
      return {
        key: `cloud-${m.id}`,
        hostname: m.hostname || lan?.name || "",
        account:
          m.user_real_name && m.user_username
            ? `${m.user_real_name}（${m.user_username}）`
            : m.user_real_name || m.user_username || "",
        os: m.os_info || lan?.os || "",
        address: lanHost
          ? `${lanHost}:${lan?.port ?? m.port ?? DEFAULT_AGENT_PORT}`
          : lanIps[0] || m.public_ip || "",
        source: "cloud",
        lanReachable: Boolean(lanHost),
        online: m.status === "online",
        note: m.note,
        clientId: m.id,
        lanHost,
        lanPort: lan?.port ?? m.port ?? DEFAULT_AGENT_PORT,
      };
    });

    const lanRows: MachineRow[] = lanMachines
      .filter((m) => !matchedHosts.has(m.host))
      .map((m) => ({
        key: `lan-${m.host}`,
        hostname: m.name || m.host,
        account: "",
        os: m.os,
        address: `${m.host}:${m.port}`,
        source: "lan",
        lanReachable: true,
        online: true,
        note: "",
        clientId: null,
        lanHost: m.host,
        lanPort: m.port,
      }));

    return [...cloudRows, ...lanRows].sort((a, b) => {
      if (a.lanReachable !== b.lanReachable) return a.lanReachable ? -1 : 1;
      if (a.online !== b.online) return a.online ? -1 : 1;
      return 0;
    });
  }, [machines, lanMachines]);

  // 连接：局域网发现的机器直接开被控端内置 viewer；云端机器交给后端解析
  // （后端先探 /discover，可达即局域网直连，不可达才回退中继控制页）。
  // mode 决定 viewer 落到完全控制 / 仅查看 / 文件传输；viewer 在应用内窗口打开。
  const handleConnect = useCallback(
    async (row: MachineRow, mode: RemoteMode) => {
      // 文件传输用应用内双栏页（左本机 / 右远程），不打开被控端 viewer 窗口。
      if (mode === "files") {
        setFileTarget({
          hostName: row.hostname,
          clientId: row.clientId,
          lanHost: row.lanHost,
          lanPort: row.lanPort,
        });
        return;
      }
      setConnectingKey(`${row.key}:${mode}`);
      try {
        const title = `${t(MODE_LABEL_KEY[mode])} · ${
          row.hostname || row.address
        }`;
        const label = viewerWindowLabel(row.key, mode);
        if (row.source === "lan" && row.lanHost) {
          const port = row.lanPort ?? DEFAULT_AGENT_PORT;
          await openViewerWindow(
            label,
            `http://${row.lanHost}:${port}/?mode=${mode}`,
            title,
          );
          toast.success(
            t("remoteManagement.connectLanSuccess", {
              host: row.hostname || row.lanHost,
            }),
          );
          return;
        }
        if (row.clientId === null) return;
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

  const onlineCount = rows.filter((r) => r.online).length;
  const busy = isLoading || isScanning;

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
            disabled={busy}
          >
            <LuRefreshCw
              className={cn("w-4 h-4 mr-2", isLoading && "animate-spin")}
            />
            {t("remoteManagement.refresh")}
          </Button>
        </div>
      </div>

      {/* 局域网扫描工具条 */}
      <div className="flex items-center gap-2 px-6 py-3 border-b border-border">
        <Input
          value={ranges}
          onChange={(e) => setRanges(e.target.value)}
          placeholder={t("remoteManagement.rangePlaceholder")}
          className="h-8 max-w-xs text-xs"
          disabled={isScanning}
        />
        <Button
          size="sm"
          variant="outline"
          className="h-8 text-xs"
          onClick={() => void scanLan()}
          disabled={busy}
        >
          {isScanning ? (
            <LuLoaderCircle className="w-3.5 h-3.5 mr-1.5 animate-spin" />
          ) : (
            <LuWifi className="w-3.5 h-3.5 mr-1.5" />
          )}
          {isScanning
            ? t("remoteManagement.scanning")
            : t("remoteManagement.scan")}
        </Button>
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
              <TableHead>{t("remoteManagement.colSource")}</TableHead>
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
                  colSpan={7}
                  className="text-center py-12 text-muted-foreground"
                >
                  <LuLoaderCircle className="w-6 h-6 animate-spin mx-auto mb-2" />
                  {t("remoteManagement.loading")}
                </TableCell>
              </TableRow>
            ) : rows.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={7}
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
                    <div>{row.hostname || "—"}</div>
                    {row.account && (
                      <div className="text-xs font-normal text-muted-foreground">
                        {row.account}
                      </div>
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
                        {row.lanReachable ? (
                          <LuWifi className="w-3.5 h-3.5 text-success" />
                        ) : (
                          <LuGlobe className="w-3.5 h-3.5 text-muted-foreground" />
                        )}
                        {row.address}
                      </span>
                    ) : (
                      <span className="text-muted-foreground">—</span>
                    )}
                  </TableCell>
                  <TableCell>
                    <Badge
                      variant={row.source === "lan" ? "default" : "secondary"}
                    >
                      {row.source === "lan"
                        ? t("remoteManagement.sourceLan")
                        : t("remoteManagement.sourceCloud")}
                    </Badge>
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
