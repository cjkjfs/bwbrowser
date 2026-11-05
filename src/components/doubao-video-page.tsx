"use client";

// doubao-video-page.tsx - 豆包视频：账号池 / 视频任务 / 生成结果 / 设置
// 账号池只显示云端 doubao 平台的账号（doubao_load_cloud_accounts 载入）；
// 任务由 doubao_scheduler 在后端调度，CDP 注入执行。

import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  confirm as dialogConfirm,
  open as openFileDialog,
} from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuCloud,
  LuDownload,
  LuExternalLink,
  LuImages,
  LuPlay,
  LuPlus,
  LuQrCode,
  LuRefreshCw,
  LuRotateCcw,
  LuShieldCheck,
  LuTrash2,
  LuX,
} from "react-icons/lu";
import {
  AnimatedTabs,
  AnimatedTabsList,
  AnimatedTabsTrigger,
} from "@/components/ui/animated-tabs";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import { Textarea } from "@/components/ui/textarea";
import { translateBackendError } from "@/lib/backend-errors";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";

interface DoubaoAccount {
  id: string;
  display_name: string;
  doubao_user_id: string | null;
  doubao_nickname: string | null;
  login_account: string | null;
  profile_id: string | null;
  server_id: number | null;
  phone: string | null;
  proxy_node: string | null;
  status: string;
  enabled: boolean;
  last_verified_at: number | null;
  last_error: string | null;
  video_quota_used: number;
  video_quota_date: string | null;
  video_limited_until: number | null;
  created_at: number;
  updated_at: number;
}

interface DoubaoMaterial {
  id: string;
  name: string;
  path: string;
  size: number;
  created_at: number;
  category: string;
  cloud_url: string;
}

interface DoubaoMaterialUpload {
  id: string;
  name: string;
  ok: boolean;
  message: string;
  cloud_url: string;
}

interface DoubaoVideoTask {
  id: string;
  account_id: string | null;
  prompt: string;
  model: string;
  ratio: string;
  duration: number;
  mode: string;
  image_paths: string[];
  status: string;
  result_url: string | null;
  backup_result_url: string | null;
  fallback_result_url: string | null;
  cover_url: string | null;
  local_file: string | null;
  local_thumb: string | null;
  error_message: string | null;
  created_at: number;
  updated_at: number;
  completed_at: number | null;
}

interface DoubaoSettings {
  max_concurrency: number;
  quota_enabled: boolean;
  daily_quota: number;
  quota_reset_time: string;
  scheduler_strategy: string;
  default_model: string;
  default_duration: number;
  default_ratio: string;
  video_timeout: number;
  download_dir: string | null;
  sync_enabled: boolean;
  sync_api_url: string;
  sync_username: string;
  sync_password: string;
}

const MODELS = [
  { value: "seedance_v2.0_mini", label: "Seedance 极速 (mini)" },
  { value: "seedance_v2.0_std", label: "Seedance 标准 (std)" },
  { value: "seedance_v2.0", label: "Seedance 高品质" },
];
const RATIOS = ["1:1", "3:4", "4:3", "9:16", "16:9", "21:9"];
const DURATIONS = [5, 10, 15];

const STATUS_LABELS: Record<string, string> = {
  queued: "排队中",
  starting: "准备中",
  generating: "生成中",
  verifying: "人机验证",
  awaiting_confirm: "待确认",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
};

const STATUS_BADGE: Record<string, string> = {
  queued: "bg-muted text-muted-foreground",
  starting: "bg-primary/10 text-primary",
  generating: "bg-primary/10 text-primary",
  verifying: "bg-warning/10 text-warning",
  awaiting_confirm: "bg-warning/10 text-warning",
  completed: "bg-success/10 text-success",
  failed: "bg-destructive/10 text-destructive",
  cancelled: "bg-muted text-muted-foreground",
};

const TASK_ACTIVE_STATES = ["queued", "starting", "generating", "verifying"];

// cookie_health 的状态字符串 → 登录状态列的徽标（与云端账号页同源）
const HEALTH_BADGE: Record<string, { label: string; className: string }> = {
  valid: { label: "登录有效", className: "bg-success/10 text-success" },
  expired: {
    label: "Cookie 失效",
    className: "bg-destructive/10 text-destructive",
  },
  missing: { label: "无 Cookie", className: "bg-warning/10 text-warning" },
  unknown: { label: "未检测", className: "" },
};

// 豆包说额度用到哪天，调度器就把账号锁到哪天；没锁或已过期就不显示
function quotaLockedUntil(account: DoubaoAccount): number | null {
  const until = account.video_limited_until;
  if (!until || until * 1000 <= Date.now()) return null;
  return until;
}

function modelLabel(value: string) {
  return MODELS.find((m) => m.value === value)?.label ?? value;
}
function formatTime(unix: number | null) {
  if (!unix) return "—";
  return new Date(unix * 1000).toLocaleString();
}

function formatSize(bytes: number) {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

const emptySettings: DoubaoSettings = {
  max_concurrency: 1,
  quota_enabled: true,
  daily_quota: 5,
  quota_reset_time: "00:00",
  scheduler_strategy: "least_used",
  default_model: "seedance_v2.0_mini",
  default_duration: 5,
  default_ratio: "1:1",
  video_timeout: 420,
  download_dir: null,
  sync_enabled: false,
  sync_api_url: "",
  sync_username: "",
  sync_password: "",
};

// ===== 账号池 =====

function AccountsTab() {
  const { t } = useTranslation();
  const [accounts, setAccounts] = useState<DoubaoAccount[]>([]);
  const [loginStates, setLoginStates] = useState<
    Record<string, { state: string; message: string }>
  >({});
  const [checkingId, setCheckingId] = useState<string | null>(null);
  const [healthStates, setHealthStates] = useState<
    Record<string, { status: string; message: string; checked_at: number }>
  >({});
  const [loadingCloud, setLoadingCloud] = useState(false);
  const [openingId, setOpeningId] = useState<string | null>(null);

  const LOGIN_ACTIVE_STATES = ["launching", "waiting_for_scan", "verifying"];
  const isLoggingIn = (accountId: string) =>
    LOGIN_ACTIVE_STATES.includes(loginStates[accountId]?.state ?? "");

  const poolAccounts = useMemo(
    () => accounts.filter((a) => a.server_id != null),
    [accounts],
  );

  const LOGIN_STATE_LABELS: Record<string, string> = {
    launching: "正在启动浏览器窗口",
    waiting_for_scan: "请在浏览器窗口中扫码登录豆包",
    verifying: "已检测到登录，正在确认账号",
  };

  const refresh = useCallback(async () => {
    try {
      setAccounts(await invoke<DoubaoAccount[]>("doubao_list_accounts"));
    } catch (err) {
      showErrorToast(
        `加载账号池失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const loadHealthStates = useCallback(async () => {
    try {
      const states = await invoke<
        Array<{
          account_id: string;
          status: string;
          message: string;
          checked_at: number;
        }>
      >("doubao_account_health_states");
      const next: Record<
        string,
        { status: string; message: string; checked_at: number }
      > = {};
      for (const s of states) next[s.account_id] = s;
      setHealthStates(next);
    } catch {
      // 健康状态加载失败静默，列显示「未检测」
    }
  }, []);

  useEffect(() => {
    void loadHealthStates();
  }, [loadHealthStates]);

  useEffect(() => {
    const unlisten = listen<{
      state: string;
      message: string;
      account_id: string;
    }>("doubao-login:progress", (event) => {
      const p = event.payload;
      if (["succeeded", "failed", "cancelled", "timed_out"].includes(p.state)) {
        setLoginStates((prev) => {
          const next = { ...prev };
          delete next[p.account_id];
          return next;
        });
      } else {
        setLoginStates((prev) => ({
          ...prev,
          [p.account_id]: { state: p.state, message: p.message },
        }));
      }
      if (p.state === "succeeded") {
        showSuccessToast(p.message);
        void refresh();
      } else if (p.state === "failed" || p.state === "timed_out") {
        showErrorToast(p.message);
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const startLogin = async (account: DoubaoAccount) => {
    try {
      await invoke("doubao_start_login", { accountId: account.id });
      void refresh();
    } catch (err) {
      showErrorToast(
        `登录失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const cancelLogin = async (accountId: string) => {
    try {
      await invoke("doubao_cancel_login", { accountId });
    } catch (err) {
      showErrorToast(
        `取消失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const checkHealth = async (account: DoubaoAccount) => {
    setCheckingId(account.id);
    try {
      const result = await invoke<{
        account_id: string;
        status: string;
        message: string;
        checked_at: number;
      }>("doubao_check_account_health", { accountId: account.id });
      setHealthStates((prev) => ({ ...prev, [result.account_id]: result }));
      if (result.status === "valid") {
        showSuccessToast(
          result.message || `账号 ${account.display_name} Cookie 健康`,
        );
      } else {
        showErrorToast(
          result.message || `账号 ${account.display_name} Cookie 需重新登录`,
        );
      }
    } catch (err) {
      showErrorToast(`检测失败: ${translateBackendError(t, err)}`);
    } finally {
      setCheckingId(null);
    }
  };

  const pullVideos = async (account: DoubaoAccount) => {
    try {
      const result = await invoke<{ total: number; new: number }>(
        "doubao_pull_account_videos",
        { accountId: account.id },
      );
      showSuccessToast(
        `拉取完成：共 ${result.total} 个，新增 ${result.new} 个`,
      );
    } catch (err) {
      showErrorToast(
        `拉取失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const syncFromCloud = useCallback(
    async (silent: boolean) => {
      setLoadingCloud(true);
      try {
        const result = await invoke<{
          loaded: number;
          created: number;
          updated: number;
          bound: number;
          skipped: number;
        }>("doubao_load_cloud_accounts");
        if (!silent) {
          const parts = [
            `云端 ${result.loaded} 个`,
            `新增 ${result.created}`,
            `更新 ${result.updated}`,
          ];
          if (result.skipped > 0) parts.push(`跳过 ${result.skipped}`);
          showSuccessToast(`载入云端账号完成：${parts.join("，")}`);
        } else if (result.created > 0) {
          // 静默同步只在真的多了账号时出声，否则每次进页面都刷一条
          showSuccessToast(`云端账号已同步：新增 ${result.created} 个`);
        }
        await refresh();
        void loadHealthStates();
      } catch (err) {
        if (!silent) {
          showErrorToast(`载入云端账号失败: ${translateBackendError(t, err)}`);
        }
      } finally {
        setLoadingCloud(false);
      }
    },
    [refresh, loadHealthStates, t],
  );

  useEffect(() => {
    // 账号池是云端镜像：进页面先静默同步一次，新增的云端账号不用手动点按钮
    void syncFromCloud(true);
  }, [syncFromCloud]);

  const remove = async (account: DoubaoAccount) => {
    const ok = await dialogConfirm(`确定删除账号「${account.display_name}」？`);
    if (!ok) return;
    try {
      await invoke("doubao_delete_account", { id: account.id });
      showSuccessToast("已删除");
      void refresh();
    } catch (err) {
      showErrorToast(
        `删除失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const openBrowser = async (account: DoubaoAccount) => {
    setOpeningId(account.id);
    try {
      await invoke("doubao_open_browser", { accountId: account.id });
      showSuccessToast(`已打开「${account.display_name}」的浏览器窗口`);
    } catch (err) {
      showErrorToast(`打开浏览器失败: ${translateBackendError(t, err)}`);
    } finally {
      setOpeningId(null);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex items-center justify-between">
        <p className="text-xs text-muted-foreground">
          账号池只显示云端 doubao 平台的账号，豆包登录态保存在账号对应 BW
          环境的数据目录里。
        </p>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={loadingCloud}
            onClick={() => void syncFromCloud(false)}
          >
            <LuCloud className="size-3.5" />
            {loadingCloud ? "载入中..." : "载入云端账号"}
          </Button>
          <Button variant="outline" size="sm" onClick={() => void refresh()}>
            <LuRefreshCw className="size-3.5" />
            刷新
          </Button>
        </div>
      </div>

      {poolAccounts.length === 0 ? (
        <div className="grid flex-1 place-items-center rounded-lg border border-dashed border-border text-sm text-muted-foreground">
          还没有云端账号，点「载入云端账号」拉取 doubao 平台的账号
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto rounded-lg border border-border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>手机编号</TableHead>
                <TableHead>账号名称</TableHead>
                <TableHead>登录账号</TableHead>
                <TableHead>登录状态</TableHead>
                <TableHead>代理</TableHead>
                <TableHead className="text-right">操作</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {poolAccounts.map((account) => (
                <TableRow key={account.id}>
                  <TableCell className="font-mono text-xs text-muted-foreground">
                    {account.phone || "-"}
                  </TableCell>
                  <TableCell>
                    <span className="block max-w-40 truncate text-sm font-medium">
                      {account.display_name}
                    </span>
                  </TableCell>
                  <TableCell className="max-w-40 truncate text-xs text-muted-foreground">
                    {account.login_account || "-"}
                  </TableCell>
                  <TableCell>
                    {(() => {
                      const health = healthStates[account.id];
                      const badge =
                        HEALTH_BADGE[health?.status ?? "unknown"] ??
                        HEALTH_BADGE.unknown;
                      return (
                        <Badge
                          variant={badge.className ? "default" : "outline"}
                          className={cn("text-[10px]", badge.className)}
                          title={
                            health?.message
                              ? `${health.message}${
                                  health.checked_at
                                    ? ` · ${new Date(health.checked_at * 1000).toLocaleString()}`
                                    : ""
                                }`
                              : "未检测"
                          }
                        >
                          {badge.label}
                        </Badge>
                      );
                    })()}
                    {quotaLockedUntil(account) && (
                      <Badge
                        variant="outline"
                        className="ml-1.5 text-[10px] text-warning"
                        title={`${account.last_error ?? "豆包额度已用完"} · ${formatTime(
                          quotaLockedUntil(account),
                        )} 恢复`}
                      >
                        额度受限
                      </Badge>
                    )}
                    {account.last_error && !quotaLockedUntil(account) && (
                      <span
                        className="ml-1.5 text-xs text-destructive"
                        title={account.last_error}
                      >
                        !
                      </span>
                    )}
                    {isLoggingIn(account.id) && (
                      <span className="mt-1 block max-w-40 text-[11px] leading-4 text-muted-foreground">
                        {LOGIN_STATE_LABELS[
                          loginStates[account.id]?.state ?? ""
                        ] ?? loginStates[account.id]?.message}
                      </span>
                    )}
                  </TableCell>
                  <TableCell className="max-w-32 truncate text-xs text-muted-foreground">
                    {account.proxy_node || "-"}
                  </TableCell>
                  <TableCell>
                    <div className="flex justify-end gap-1.5">
                      {isLoggingIn(account.id) && (
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => void cancelLogin(account.id)}
                        >
                          取消登录
                        </Button>
                      )}
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={account.status !== "active"}
                        onClick={() => void pullVideos(account)}
                      >
                        <LuDownload className="size-3.5" />
                        拉取视频
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={isLoggingIn(account.id)}
                        onClick={() => void startLogin(account)}
                      >
                        <LuQrCode className="size-3.5" />
                        {isLoggingIn(account.id) ? "登录中" : "扫码登录"}
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={checkingId === account.id}
                        onClick={() => void checkHealth(account)}
                      >
                        <LuShieldCheck className="size-3.5" />
                        {checkingId === account.id ? "检测中..." : "检测"}
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        title="打开浏览器查看（无环境会自动创建）"
                        disabled={openingId === account.id}
                        onClick={() => void openBrowser(account)}
                      >
                        <LuExternalLink className="size-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        onClick={() => void remove(account)}
                      >
                        <LuTrash2 className="size-4 text-destructive" />
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </div>
  );
}

// ===== 视频任务 =====

function TasksTab({ accounts }: { accounts: DoubaoAccount[] }) {
  const [tasks, setTasks] = useState<DoubaoVideoTask[]>([]);
  const [formAccountId, setFormAccountId] = useState<string>("auto");
  const [prompt, setPrompt] = useState("");
  const [model, setModel] = useState("seedance_v2.0_mini");
  const [ratio, setRatio] = useState("1:1");
  const [duration, setDuration] = useState("5");
  const [imagePaths, setImagePaths] = useState<string[]>([]);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [libraryMaterials, setLibraryMaterials] = useState<DoubaoMaterial[]>(
    [],
  );
  const [librarySelected, setLibrarySelected] = useState<string[]>([]);
  const [libraryPreviewId, setLibraryPreviewId] = useState<string | null>(null);
  const [libraryFilter, setLibraryFilter] = useState(ALL_CATEGORIES);
  const [libraryQuery, setLibraryQuery] = useState("");
  const [creating, setCreating] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setTasks(await invoke<DoubaoVideoTask[]>("doubao_list_tasks"));
    } catch {
      // 轮询失败静默，下个周期重试
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    const unlisten = listen("doubao-task:progress", () => void refresh());
    return () => {
      window.clearInterval(timer);
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const cancelTask = async (task: DoubaoVideoTask) => {
    try {
      const requested = await invoke<boolean>("doubao_cancel_task", {
        taskId: task.id,
      });
      showSuccessToast(
        requested ? "已请求取消，等待当前步骤结束" : "任务已结束，无需取消",
      );
      void refresh();
    } catch (err) {
      showErrorToast(
        `取消失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const retryTask = async (task: DoubaoVideoTask) => {
    try {
      await invoke("doubao_retry_task", { taskId: task.id });
      showSuccessToast("已重新排队");
      void refresh();
    } catch (err) {
      showErrorToast(
        `重试失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const pickImages = async () => {
    const picked = await openFileDialog({
      multiple: true,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (!picked) return;
    const paths = (Array.isArray(picked) ? picked : [picked]).filter(
      (p): p is string => typeof p === "string",
    );
    setImagePaths((prev) => [...prev, ...paths].slice(0, 9));
  };

  const openLibrary = async () => {
    try {
      setLibraryMaterials(
        await invoke<DoubaoMaterial[]>("doubao_list_materials"),
      );
      setLibrarySelected([]);
      setLibraryPreviewId(null);
      setLibraryFilter(ALL_CATEGORIES);
      setLibraryQuery("");
      setLibraryOpen(true);
    } catch (err) {
      showErrorToast(
        `加载素材库失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const toggleLibraryItem = (id: string) =>
    setLibrarySelected((prev) =>
      prev.includes(id)
        ? prev.filter((x) => x !== id)
        : prev.length >= 9
          ? prev
          : [...prev, id],
    );

  const visibleLibraryMaterials = libraryMaterials.filter((m) => {
    const keyword = libraryQuery.trim().toLocaleLowerCase();
    return (
      (libraryFilter === ALL_CATEGORIES || m.category === libraryFilter) &&
      (!keyword ||
        `${m.name} ${m.category}`.toLocaleLowerCase().includes(keyword))
    );
  });
  const libraryPreview =
    libraryMaterials.find((m) => m.id === libraryPreviewId) ??
    visibleLibraryMaterials[0] ??
    null;

  const confirmLibraryPick = () => {
    const paths = libraryMaterials
      .filter((m) => librarySelected.includes(m.id))
      .map((m) => m.path);
    setImagePaths((prev) => {
      const next = [...prev];
      for (const p of paths) {
        if (!next.includes(p) && next.length < 9) next.push(p);
      }
      return next;
    });
    setLibraryOpen(false);
  };

  const create = async () => {
    if (!prompt.trim()) {
      showErrorToast("请填写视频描述（Prompt）");
      return;
    }
    setCreating(true);
    try {
      await invoke("doubao_create_task", {
        accountId: formAccountId === "auto" ? null : formAccountId,
        prompt: prompt.trim(),
        model,
        ratio,
        duration: Number(duration),
        imagePaths,
      });
      showSuccessToast("任务已创建");
      setPrompt("");
      setImagePaths([]);
      void refresh();
    } catch (err) {
      showErrorToast(
        `创建失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      setCreating(false);
    }
  };

  const remove = async (task: DoubaoVideoTask) => {
    const ok = await dialogConfirm("确定删除该任务？");
    if (!ok) return;
    try {
      await invoke("doubao_delete_task", { id: task.id });
      void refresh();
    } catch (err) {
      showErrorToast(
        `删除失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const accountName = (id: string | null) =>
    accounts.find((a) => a.id === id)?.display_name ?? "自动分配";

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4">
      <div className="space-y-3 rounded-lg border border-border bg-card/50 p-4">
        <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
          <div className="space-y-1.5">
            <Label className="text-xs">账号</Label>
            <Select value={formAccountId} onValueChange={setFormAccountId}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="auto">自动分配</SelectItem>
                {accounts
                  .filter((a) => a.enabled)
                  .map((a) => (
                    <SelectItem key={a.id} value={a.id}>
                      {a.display_name}
                    </SelectItem>
                  ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">模型</Label>
            <Select value={model} onValueChange={setModel}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {MODELS.map((m) => (
                  <SelectItem key={m.value} value={m.value}>
                    {m.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">比例</Label>
            <Select value={ratio} onValueChange={setRatio}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {RATIOS.map((r) => (
                  <SelectItem key={r} value={r}>
                    {r}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">时长（秒）</Label>
            <Select value={duration} onValueChange={setDuration}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {DURATIONS.map((d) => (
                  <SelectItem key={d} value={String(d)}>
                    {d} 秒
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
        <div className="space-y-1.5">
          <Label className="text-xs">视频描述（Prompt）</Label>
          <Textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            rows={3}
            placeholder="描述要生成的视频画面，越具体越好"
          />
        </div>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => void pickImages()}
            >
              <LuPlus className="size-3.5" />
              添加参考图（{imagePaths.length}/9）
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void openLibrary()}
            >
              <LuImages className="size-3.5" />
              从素材库选择
            </Button>
            {imagePaths.length > 0 && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setImagePaths([])}
              >
                清空图片
              </Button>
            )}
            <span className="text-xs text-muted-foreground">
              {imagePaths.length > 0 ? "图生视频" : "文生视频"}
            </span>
          </div>
          <Button size="sm" disabled={creating} onClick={() => void create()}>
            创建任务
          </Button>
        </div>
        {imagePaths.length > 0 && (
          <div className="flex flex-wrap gap-1 text-[10px] text-muted-foreground">
            {imagePaths.map((p, i) => (
              <span
                key={`${p}-${i}`}
                className="max-w-48 truncate rounded bg-muted px-1.5 py-0.5"
                title={p}
              >
                {p}
              </span>
            ))}
          </div>
        )}
      </div>

      <Dialog open={libraryOpen} onOpenChange={setLibraryOpen}>
        <DialogContent className="max-w-4xl">
          <DialogHeader>
            <DialogTitle>从素材库选择参考图</DialogTitle>
          </DialogHeader>
          {libraryMaterials.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">
              素材库是空的，先到「素材库」页导入图片
            </p>
          ) : (
            <div className="flex flex-wrap items-center gap-2">
              <Input
                className="h-8 w-56 text-xs"
                placeholder="搜索素材名称"
                value={libraryQuery}
                onChange={(e) => setLibraryQuery(e.target.value)}
              />
              <Select value={libraryFilter} onValueChange={setLibraryFilter}>
                <SelectTrigger className="h-8 w-40 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={ALL_CATEGORIES}>全部分类</SelectItem>
                  <SelectItem value="">未分类</SelectItem>
                  {Array.from(
                    new Set(
                      libraryMaterials
                        .map((m) => m.category)
                        .filter((category) => category !== ""),
                    ),
                  ).map((category) => (
                    <SelectItem key={category} value={category}>
                      {category}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}
          {libraryMaterials.length > 0 && (
            <div className="grid min-h-0 gap-3 md:grid-cols-[240px_1fr]">
              <div className="space-y-2 rounded-lg border border-border bg-muted/30 p-3">
                {libraryPreview ? (
                  <>
                    {/* biome-ignore lint/performance/noImgElement: local asset:// file, next/image optimization unavailable */}
                    <img
                      src={convertFileSrc(libraryPreview.path)}
                      alt={libraryPreview.name}
                      className="max-h-56 w-full rounded object-contain"
                    />
                    <p className="truncate text-sm">{libraryPreview.name}</p>
                    <p className="text-xs text-muted-foreground">
                      {libraryPreview.category || "未分类"} ·{" "}
                      {formatSize(libraryPreview.size)}
                    </p>
                  </>
                ) : (
                  <p className="py-12 text-center text-xs text-muted-foreground">
                    没有匹配的素材
                  </p>
                )}
              </div>
              <div className="grid max-h-[50vh] auto-rows-min content-start grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-2 overflow-y-auto pr-1">
                {visibleLibraryMaterials.map((m) => {
                  const selected = librarySelected.includes(m.id);
                  return (
                    <div
                      key={m.id}
                      className={cn(
                        "relative overflow-hidden rounded-md border bg-muted",
                        selected ? "border-primary" : "border-border",
                      )}
                    >
                      <button
                        type="button"
                        title={m.name}
                        className="block w-full text-left"
                        onClick={() => setLibraryPreviewId(m.id)}
                      >
                        {/* biome-ignore lint/performance/noImgElement: local asset:// file, next/image optimization unavailable */}
                        <img
                          src={convertFileSrc(m.path)}
                          alt={m.name}
                          className="aspect-[4/3] w-full object-cover"
                          loading="lazy"
                        />
                        <span className="block truncate bg-card px-2 py-1.5 text-xs">
                          {m.name}
                        </span>
                      </button>
                      <Checkbox
                        aria-label={`选择 ${m.name}`}
                        checked={selected}
                        disabled={!selected && librarySelected.length >= 9}
                        onCheckedChange={() => toggleLibraryItem(m.id)}
                        className="absolute left-1.5 top-1.5 bg-background/90"
                      />
                      {selected && (
                        <span className="pointer-events-none absolute right-1.5 top-1.5 grid size-5 place-items-center rounded-full bg-primary text-[10px] text-primary-foreground">
                          {librarySelected.indexOf(m.id) + 1}
                        </span>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
          <DialogFooter>
            <span className="mr-auto text-xs text-muted-foreground">
              已选 {librarySelected.length}/9
            </span>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setLibraryOpen(false)}
            >
              取消
            </Button>
            <Button
              size="sm"
              disabled={librarySelected.length === 0}
              onClick={confirmLibraryPick}
            >
              确定
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {tasks.length === 0 ? (
        <div className="grid flex-1 place-items-center rounded-lg border border-dashed border-border text-sm text-muted-foreground">
          还没有任务
        </div>
      ) : (
        <div className="flex-1 space-y-2 overflow-y-auto">
          {tasks.map((task) => (
            <div
              key={task.id}
              className="rounded-lg border border-border bg-card/50 p-3"
            >
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge
                      variant="secondary"
                      className={cn("text-[10px]", STATUS_BADGE[task.status])}
                    >
                      {STATUS_LABELS[task.status] ?? task.status}
                    </Badge>
                    <span className="text-xs text-muted-foreground">
                      {accountName(task.account_id)} · {modelLabel(task.model)}{" "}
                      · {task.ratio} · {task.duration}秒 ·{" "}
                      {task.mode === "i2v" ? "图生" : "文生"}
                    </span>
                    <span className="text-xs text-muted-foreground">
                      {formatTime(task.created_at)}
                    </span>
                  </div>
                  <p className="mt-1 line-clamp-2 text-sm">{task.prompt}</p>
                  {task.error_message && (
                    <p className="mt-1 text-xs text-destructive">
                      {task.error_message}
                    </p>
                  )}
                </div>
                {TASK_ACTIVE_STATES.includes(task.status) ? (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => void cancelTask(task)}
                  >
                    <LuX className="size-3.5" />
                    取消
                  </Button>
                ) : (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => void retryTask(task)}
                  >
                    <LuRotateCcw className="size-3.5" />
                    重试
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="icon"
                  onClick={() => void remove(task)}
                >
                  <LuTrash2 className="size-4 text-destructive" />
                </Button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ===== 生成结果 =====

function ResultsTab() {
  const [tasks, setTasks] = useState<DoubaoVideoTask[]>([]);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const all = await invoke<DoubaoVideoTask[]>("doubao_list_tasks");
      setTasks(
        all.filter(
          (t) =>
            t.status === "completed" ||
            t.status === "failed" ||
            t.result_url ||
            t.local_file,
        ),
      );
    } catch {
      // 静默
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    const unlisten = listen("doubao-task:progress", () => void refresh());
    return () => {
      window.clearInterval(timer);
      void unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const copyUrl = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      showSuccessToast("已复制链接");
    } catch {
      showErrorToast("复制失败");
    }
  };

  const remove = async (task: DoubaoVideoTask) => {
    try {
      const files = await invoke<number>("doubao_delete_result", {
        id: task.id,
        deleteFiles: !!(task.local_file || task.local_thumb),
      });
      setPendingDelete(null);
      showSuccessToast(files > 0 ? `已删除记录和 ${files} 个文件` : "已删除");
      void refresh();
    } catch (err) {
      showErrorToast(
        `删除失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  useEffect(() => {
    if (!pendingDelete) return;
    const timer = window.setTimeout(() => setPendingDelete(null), 4000);
    return () => window.clearTimeout(timer);
  }, [pendingDelete]);

  const download = async (task: DoubaoVideoTask) => {
    try {
      const next = await invoke<DoubaoVideoTask>("doubao_download_task_video", {
        taskId: task.id,
      });
      showSuccessToast(`已下载：${next.local_file ?? ""}`);
      void refresh();
    } catch (err) {
      showErrorToast(
        `下载失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const openLocal = async (path: string) => {
    try {
      await openPath(path);
    } catch (err) {
      showErrorToast(
        `打开失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  if (tasks.length === 0) {
    return (
      <div className="grid flex-1 place-items-center rounded-lg border border-dashed border-border text-sm text-muted-foreground">
        还没有生成结果
      </div>
    );
  }

  return (
    <div className="grid flex-1 auto-rows-min gap-2 overflow-y-auto md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
      {tasks.map((task) => {
        const url =
          task.result_url ?? task.backup_result_url ?? task.fallback_result_url;
        const file = task.local_file;
        const thumb = task.local_thumb
          ? convertFileSrc(task.local_thumb)
          : task.cover_url && /^https?:/i.test(task.cover_url)
            ? task.cover_url
            : null;
        const folder = file ? file.split(/[/\\]/).slice(0, -1).join("/") : null;
        return (
          <div
            key={task.id}
            className="flex items-center gap-2 rounded-lg border border-border bg-card p-2"
          >
            <div className="grid size-14 shrink-0 place-items-center overflow-hidden rounded bg-muted text-muted-foreground">
              {thumb ? (
                // biome-ignore lint/performance/noImgElement: local asset:// file, next/image optimization unavailable
                <img
                  src={thumb}
                  alt={task.prompt}
                  loading="lazy"
                  className="size-full object-cover"
                />
              ) : (
                <LuPlay className="size-4" />
              )}
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5">
                <Badge
                  variant="secondary"
                  className={cn(
                    "px-1 py-0 text-[10px]",
                    STATUS_BADGE[task.status],
                  )}
                >
                  {STATUS_LABELS[task.status] ?? task.status}
                </Badge>
                <span className="truncate text-[11px] text-muted-foreground">
                  {formatTime(task.completed_at ?? task.created_at)}
                </span>
              </div>
              <p className="truncate text-xs" title={task.prompt}>
                {task.prompt}
              </p>
              {task.error_message && (
                <p
                  className="truncate text-[11px] text-destructive"
                  title={task.error_message}
                >
                  {task.error_message}
                </p>
              )}
              <div className="mt-0.5 flex items-center gap-1">
                {file ? (
                  <>
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-6 px-1.5 text-[11px]"
                      onClick={() => void openLocal(file)}
                    >
                      打开
                    </Button>
                    {folder && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-6 px-1.5 text-[11px]"
                        onClick={() => void openLocal(folder)}
                      >
                        文件夹
                      </Button>
                    )}
                  </>
                ) : (
                  url && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-6 px-1.5 text-[11px]"
                      onClick={() => void download(task)}
                    >
                      下载
                    </Button>
                  )
                )}
                {url && (
                  <>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-6 px-1.5 text-[11px]"
                      onClick={() => void copyUrl(url)}
                    >
                      复制
                    </Button>
                    <a href={url} target="_blank" rel="noreferrer">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-6 px-1.5 text-[11px]"
                      >
                        链接
                      </Button>
                    </a>
                  </>
                )}
                {pendingDelete === task.id ? (
                  <Button
                    variant="destructive"
                    size="sm"
                    className="ml-auto h-6 shrink-0 px-1.5 text-[11px]"
                    onClick={() => void remove(task)}
                  >
                    {task.local_file || task.local_thumb
                      ? "删记录+文件"
                      : "确认删除"}
                  </Button>
                ) : (
                  <Button
                    variant="ghost"
                    size="icon"
                    className="ml-auto size-6 shrink-0 text-muted-foreground hover:text-destructive"
                    onClick={() => setPendingDelete(task.id)}
                  >
                    <LuTrash2 className="size-3.5" />
                  </Button>
                )}
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ===== 素材库 =====

// 「全部」是筛选哨值，真正的分类名不会等于它；未分类就是后端里的空串
const ALL_CATEGORIES = "__all__";

function MaterialsTab() {
  const [materials, setMaterials] = useState<DoubaoMaterial[]>([]);
  const [importing, setImporting] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [filter, setFilter] = useState<string>(ALL_CATEGORIES);
  const [preview, setPreview] = useState<DoubaoMaterial | null>(null);
  const [draftCategory, setDraftCategory] = useState("");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string[]>([]);

  const refresh = useCallback(async () => {
    try {
      setMaterials(await invoke<DoubaoMaterial[]>("doubao_list_materials"));
    } catch (err) {
      showErrorToast(
        `加载素材库失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const categories = useMemo(
    () =>
      Array.from(
        new Set(materials.map((m) => m.category).filter((c) => c !== "")),
      ).sort((a, b) => a.localeCompare(b, "zh-Hans-CN")),
    [materials],
  );

  const visible = useMemo(() => {
    const keyword = query.trim().toLocaleLowerCase();
    return materials.filter(
      (m) =>
        (filter === ALL_CATEGORIES || m.category === filter) &&
        (!keyword ||
          `${m.name} ${m.category}`.toLocaleLowerCase().includes(keyword)),
    );
  }, [materials, filter, query]);

  const upload = async (ids: string[]) => {
    if (ids.length === 0) {
      showSuccessToast("这一类素材都已经传过云端了");
      return;
    }
    setUploading(true);
    try {
      const results = await invoke<DoubaoMaterialUpload[]>(
        "doubao_upload_materials",
        { ids },
      );
      const ok = results.filter((r) => r.ok).length;
      const failed = results.find((r) => !r.ok);
      if (failed) {
        showErrorToast(
          `已上传 ${ok} 张，「${failed.name}」失败：${failed.message}`,
        );
      } else {
        showSuccessToast(`已上传 ${ok} 张到云端`);
      }
      const uploaded = results.find(
        (r) => r.ok && r.id === preview?.id && r.cloud_url !== "",
      );
      if (uploaded) {
        setPreview((p) => (p ? { ...p, cloud_url: uploaded.cloud_url } : p));
      }
      void refresh();
    } catch (err) {
      showErrorToast(
        `上传失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      setUploading(false);
    }
  };

  const saveCategory = async () => {
    if (!preview) return;
    try {
      await invoke("doubao_set_material_category", {
        id: preview.id,
        category: draftCategory,
      });
      setPreview(null);
      void refresh();
    } catch (err) {
      showErrorToast(
        `分类保存失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const importFiles = async () => {
    const picked = await openFileDialog({
      multiple: true,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    if (!picked) return;
    const paths = (Array.isArray(picked) ? picked : [picked]).filter(
      (p): p is string => typeof p === "string",
    );
    if (paths.length === 0) return;
    setImporting(true);
    try {
      const imported = await invoke<DoubaoMaterial[]>(
        "doubao_import_materials",
        {
          paths,
          category: filter === ALL_CATEGORIES ? "" : filter,
        },
      );
      showSuccessToast(
        imported.length > 0
          ? `已导入 ${imported.length} 张素材`
          : "所选图片均已在素材库中",
      );
      void refresh();
    } catch (err) {
      showErrorToast(
        `导入失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      setImporting(false);
    }
  };

  const remove = async (material: DoubaoMaterial) => {
    const ok = await dialogConfirm(
      `删除素材「${material.name}」？已创建的任务不受影响。`,
    );
    if (!ok) return;
    try {
      await invoke("doubao_delete_material", { id: material.id });
      void refresh();
    } catch (err) {
      showErrorToast(
        `删除失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  };

  const openPreview = (material: DoubaoMaterial) => {
    setDraftCategory(material.category);
    setPreview(material);
  };

  const toggleSelected = (id: string) =>
    setSelected((current) =>
      current.includes(id)
        ? current.filter((item) => item !== id)
        : [...current, id],
    );

  const removeSelected = async () => {
    const ok = await dialogConfirm(
      `删除选中的 ${selected.length} 张素材？已创建的任务不受影响。`,
    );
    if (!ok) return;
    try {
      for (const id of selected) {
        await invoke("doubao_delete_material", { id });
      }
      setSelected([]);
      setPreview(null);
      await refresh();
    } catch (err) {
      showErrorToast(
        `删除失败: ${err instanceof Error ? err.message : String(err)}`,
      );
      await refresh();
    }
  };

  const chips = [
    { value: ALL_CATEGORIES, label: `全部 ${materials.length}` },
    {
      value: "",
      label: `未分类 ${materials.filter((m) => m.category === "").length}`,
    },
    ...categories.map((c) => ({
      value: c,
      label: `${c} ${materials.filter((m) => m.category === c).length}`,
    })),
  ];

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <Input
          className="h-7 w-44 text-xs"
          placeholder="搜索素材名称或分类"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-xs"
          disabled={visible.length === 0}
          onClick={() =>
            setSelected(
              visible.every((m) => selected.includes(m.id))
                ? selected.filter((id) => !visible.some((m) => m.id === id))
                : Array.from(
                    new Set([...selected, ...visible.map((m) => m.id)]),
                  ),
            )
          }
        >
          {visible.every((m) => selected.includes(m.id)) ? "取消全选" : "全选"}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-xs text-destructive"
          disabled={selected.length === 0}
          onClick={() => void removeSelected()}
        >
          批量删除（{selected.length}）
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-xs"
          disabled={uploading || selected.length === 0}
          onClick={() => void upload(selected)}
        >
          {uploading ? "上传中..." : "上传所选到云端"}
        </Button>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {chips.map((chip) => (
          <button
            key={chip.value || "__none__"}
            type="button"
            onClick={() => setFilter(chip.value)}
            className={cn(
              "rounded-full border px-2 py-0.5 text-[11px]",
              filter === chip.value
                ? "border-primary text-primary"
                : "border-border text-muted-foreground hover:text-foreground",
            )}
          >
            {chip.label}
          </button>
        ))}
        <Input
          className="ml-auto h-7 w-28 text-xs"
          placeholder="新分类名"
          list="doubao-material-categories"
          value={filter === ALL_CATEGORIES ? "" : filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-xs"
          disabled={importing}
          onClick={() => void importFiles()}
        >
          {importing ? "导入中..." : "导入图片"}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-7 text-xs"
          disabled={uploading || visible.length === 0}
          onClick={() =>
            void upload(
              visible.filter((m) => m.cloud_url === "").map((m) => m.id),
            )
          }
        >
          {uploading ? "上传中..." : "上传本类到云端"}
        </Button>
      </div>

      <datalist id="doubao-material-categories">
        {categories.map((c) => (
          <option key={c} value={c} />
        ))}
      </datalist>

      {visible.length === 0 ? (
        <div className="grid flex-1 place-items-center rounded-lg border border-dashed border-border text-sm text-muted-foreground">
          这个分类下还没有素材，点「导入图片」添加参考图
        </div>
      ) : (
        <div className="grid flex-1 auto-rows-min content-start gap-2 overflow-y-auto rounded-lg border border-border bg-card/50 p-2 grid-cols-[repeat(auto-fill,minmax(130px,1fr))]">
          {visible.map((m) => {
            const checked = selected.includes(m.id);
            return (
              <div
                key={m.id}
                className={cn(
                  "group relative overflow-hidden rounded-md border bg-muted",
                  checked ? "border-primary" : "border-border",
                )}
              >
                <button
                  type="button"
                  title={`${m.name} · ${formatSize(m.size)}${m.category ? ` · ${m.category}` : ""}`}
                  className="block w-full overflow-hidden"
                  onClick={() => openPreview(m)}
                >
                  {/* biome-ignore lint/performance/noImgElement: local asset:// file, next/image optimization unavailable */}
                  <img
                    src={convertFileSrc(m.path)}
                    alt={m.name}
                    loading="lazy"
                    className="aspect-[4/3] w-full object-cover"
                  />
                  <span className="block truncate bg-card px-2 py-1.5 text-left text-xs">
                    {m.name}
                  </span>
                </button>
                <Checkbox
                  aria-label={`选择 ${m.name}`}
                  checked={checked}
                  onCheckedChange={() => toggleSelected(m.id)}
                  className="absolute left-1.5 top-1.5 bg-background/90"
                />
                {m.cloud_url !== "" && (
                  <span className="pointer-events-none absolute bottom-8 left-0 rounded-br bg-background/85 px-1 text-[9px] text-success">
                    云端
                  </span>
                )}
                <button
                  type="button"
                  aria-label={`删除 ${m.name}`}
                  className="absolute right-1.5 top-1.5 hidden rounded bg-background/90 p-1 text-muted-foreground group-hover:block hover:text-destructive"
                  onClick={() => void remove(m)}
                >
                  <LuTrash2 className="size-3.5" />
                </button>
              </div>
            );
          })}
        </div>
      )}

      <Dialog
        open={preview !== null}
        onOpenChange={(open) => !open && setPreview(null)}
      >
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle className="truncate text-sm">
              {preview?.name}
            </DialogTitle>
          </DialogHeader>
          {preview && (
            <div className="space-y-3">
              {/* biome-ignore lint/performance/noImgElement: local asset:// file, next/image optimization unavailable */}
              <img
                src={convertFileSrc(preview.path)}
                alt={preview.name}
                className="mx-auto max-h-[55vh] w-auto rounded border border-border object-contain"
              />
              <p className="text-xs text-muted-foreground">
                {formatSize(preview.size)} · {formatTime(preview.created_at)}
                {preview.cloud_url !== "" && " · 已传回云端"}
              </p>
              <div className="flex items-center gap-2">
                <Label className="shrink-0 text-xs">分类</Label>
                <Input
                  className="h-8 flex-1 text-xs"
                  list="doubao-material-categories"
                  value={draftCategory}
                  placeholder="留空即未分类"
                  onChange={(e) => setDraftCategory(e.target.value)}
                />
                <Button
                  variant="outline"
                  size="sm"
                  className="h-8 text-xs"
                  onClick={() => void saveCategory()}
                >
                  保存
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-8 text-xs"
                  disabled={uploading}
                  onClick={() => void upload([preview.id])}
                >
                  {uploading ? "上传中..." : "传回云端"}
                </Button>
              </div>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

// ===== 设置 =====

function SettingsTab() {
  const [settings, setSettings] = useState<DoubaoSettings>(emptySettings);
  const [saving, setSaving] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    invoke<DoubaoSettings>("doubao_get_settings")
      .then((s) => {
        setSettings({ ...emptySettings, ...s });
        setLoaded(true);
      })
      .catch(() => setLoaded(true));
  }, []);

  const update = <K extends keyof DoubaoSettings>(
    key: K,
    value: DoubaoSettings[K],
  ) => setSettings((prev) => ({ ...prev, [key]: value }));

  const save = async () => {
    setSaving(true);
    try {
      const saved = await invoke<DoubaoSettings>("doubao_save_settings", {
        settings,
      });
      setSettings({ ...emptySettings, ...saved });
      showSuccessToast("设置已保存");
    } catch (err) {
      showErrorToast(
        `保存失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      setSaving(false);
    }
  };

  if (!loaded) return null;

  return (
    <div className="flex-1 space-y-5 overflow-y-auto">
      <div className="space-y-3 rounded-lg border border-border bg-card/50 p-4">
        <h3 className="text-sm font-medium">调度</h3>
        <div className="grid grid-cols-2 gap-3 md:grid-cols-3">
          <div className="space-y-1.5">
            <Label className="text-xs">最大并发账号数</Label>
            <Input
              type="number"
              min={1}
              max={10}
              value={settings.max_concurrency}
              onChange={(e) =>
                update("max_concurrency", Number(e.target.value) || 1)
              }
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">调度策略</Label>
            <Select
              value={settings.scheduler_strategy}
              onValueChange={(v) => update("scheduler_strategy", v)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="least_used">优先用量最少的账号</SelectItem>
                <SelectItem value="round_robin">轮询分配</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">生成超时（秒）</Label>
            <Input
              type="number"
              min={60}
              max={3600}
              value={settings.video_timeout}
              onChange={(e) =>
                update("video_timeout", Number(e.target.value) || 420)
              }
            />
          </div>
        </div>
      </div>

      <div className="space-y-3 rounded-lg border border-border bg-card/50 p-4">
        <h3 className="text-sm font-medium">每日额度</h3>
        <div className="flex items-center gap-2">
          <Checkbox
            id="doubao-quota-enabled"
            checked={settings.quota_enabled}
            onCheckedChange={(checked) =>
              update("quota_enabled", checked === true)
            }
          />
          <Label htmlFor="doubao-quota-enabled" className="text-sm font-normal">
            启用每日额度限制
          </Label>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <div className="space-y-1.5">
            <Label className="text-xs">每账号每日上限</Label>
            <Input
              type="number"
              min={1}
              max={100}
              value={settings.daily_quota}
              onChange={(e) =>
                update("daily_quota", Number(e.target.value) || 5)
              }
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">额度重置时间</Label>
            <Input
              type="time"
              value={settings.quota_reset_time}
              onChange={(e) => update("quota_reset_time", e.target.value)}
            />
          </div>
        </div>
      </div>

      <div className="space-y-3 rounded-lg border border-border bg-card/50 p-4">
        <h3 className="text-sm font-medium">默认生成参数</h3>
        <div className="grid grid-cols-2 gap-3 md:grid-cols-3">
          <div className="space-y-1.5">
            <Label className="text-xs">默认模型</Label>
            <Select
              value={settings.default_model}
              onValueChange={(v) => update("default_model", v)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {MODELS.map((m) => (
                  <SelectItem key={m.value} value={m.value}>
                    {m.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">默认比例</Label>
            <Select
              value={settings.default_ratio}
              onValueChange={(v) => update("default_ratio", v)}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {RATIOS.map((r) => (
                  <SelectItem key={r} value={r}>
                    {r}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">默认时长（秒）</Label>
            <Select
              value={String(settings.default_duration)}
              onValueChange={(v) => update("default_duration", Number(v))}
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {DURATIONS.map((d) => (
                  <SelectItem key={d} value={String(d)}>
                    {d} 秒
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
      </div>

      <div className="space-y-3 rounded-lg border border-border bg-card/50 p-4">
        <h3 className="text-sm font-medium">下载目录</h3>
        <Input
          value={settings.download_dir ?? ""}
          onChange={(e) => update("download_dir", e.target.value || null)}
          placeholder="留空使用默认目录（数据目录/doubao/downloads）"
        />
      </div>

      <div className="flex justify-end">
        <Button disabled={saving} onClick={() => void save()}>
          保存设置
        </Button>
      </div>
    </div>
  );
}

// ===== 页面 =====

export function DoubaoVideoPage() {
  const [accounts, setAccounts] = useState<DoubaoAccount[]>([]);
  const [active, setActive] = useState("accounts");

  useEffect(() => {
    const load = () =>
      invoke<DoubaoAccount[]>("doubao_list_accounts")
        .then(setAccounts)
        .catch(() => {});
    load();
    // 打开豆包视频页才唤醒调度器：把重启前遗留的排队/生成中任务接上
    invoke<number>("doubao_resume_tasks")
      .then((n) => {
        if (n > 0) showSuccessToast(`已恢复 ${n} 个未完成任务`);
      })
      .catch(() => {});
    const timer = window.setInterval(load, 5000);
    return () => window.clearInterval(timer);
  }, []);

  const enabledAccounts = useMemo(
    () => accounts.filter((a) => a.enabled),
    [accounts],
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col p-6">
      <AnimatedTabs
        value={active}
        onValueChange={setActive}
        className="flex min-h-0 flex-1 flex-col"
      >
        <AnimatedTabsList className="mb-4 shrink-0">
          <AnimatedTabsTrigger value="accounts">账号池</AnimatedTabsTrigger>
          <AnimatedTabsTrigger value="tasks">视频任务</AnimatedTabsTrigger>
          <AnimatedTabsTrigger value="materials">素材库</AnimatedTabsTrigger>
          <AnimatedTabsTrigger value="results">生成结果</AnimatedTabsTrigger>
          <AnimatedTabsTrigger value="settings">设置</AnimatedTabsTrigger>
        </AnimatedTabsList>
        <div
          className={cn(
            "flex min-h-0 flex-1 flex-col",
            active !== "accounts" && "hidden",
          )}
        >
          <AccountsTab />
        </div>
        <div
          className={cn(
            "flex min-h-0 flex-1 flex-col",
            active !== "tasks" && "hidden",
          )}
        >
          <TasksTab accounts={enabledAccounts} />
        </div>
        <div
          className={cn(
            "flex min-h-0 flex-1 flex-col",
            active !== "materials" && "hidden",
          )}
        >
          <MaterialsTab />
        </div>
        <div
          className={cn(
            "flex min-h-0 flex-1 flex-col",
            active !== "results" && "hidden",
          )}
        >
          <ResultsTab />
        </div>
        <div
          className={cn(
            "flex min-h-0 flex-1 flex-col",
            active !== "settings" && "hidden",
          )}
        >
          <SettingsTab />
        </div>
      </AnimatedTabs>
    </div>
  );
}
