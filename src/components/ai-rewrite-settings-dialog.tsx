"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  LuCheck,
  LuChevronDown,
  LuDownload,
  LuLoader,
  LuRefreshCw,
  LuSearch,
  LuUnplug,
} from "react-icons/lu";
import { toast } from "sonner";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";

interface AiProxyFields {
  type: string;
  host: string;
  user: string;
  pass: string;
}

interface AiEndpointFields {
  provider: string;
  base_url: string;
  api_key: string;
  model: string;
  proxy: AiProxyFields;
}

interface AiApiConfigFields {
  asr: AiEndpointFields;
  llm: AiEndpointFields;
  translate: AiEndpointFields;
}

/** 本地 ASR（faster-whisper）配置：机器级偏好，存本地不上云 */
interface LocalAsrFields {
  enabled: boolean;
  pythonPath: string;
  model: string;
  language: string;
}

const EMPTY_LOCAL_ASR: LocalAsrFields = {
  enabled: false,
  pythonPath: "",
  model: "small",
  language: "",
};

/** faster-whisper 模型档位：标签 + 说明（大小/速度） */
const LOCAL_ASR_MODELS = [
  { value: "tiny", label: "tiny（最快，~75MB，精度低）" },
  { value: "base", label: "base（~140MB）" },
  { value: "small", label: "small（推荐，~500MB）" },
  { value: "medium", label: "medium（~1.5GB，较慢）" },
  { value: "large-v3", label: "large-v3（最准，~3GB，需 GPU）" },
] as const;

/** 本地转写「识别语言」下拉选项（value 传给 faster-whisper；auto = 留空自动检测） */
const LOCAL_ASR_LANGUAGES = [
  { value: "zh", label: "中文" },
  { value: "en", label: "英语" },
  { value: "ja", label: "日语" },
  { value: "ko", label: "韩语" },
  { value: "ru", label: "俄语" },
  { value: "es", label: "西班牙语" },
  { value: "pt", label: "葡萄牙语" },
  { value: "vi", label: "越南语" },
  { value: "th", label: "泰语" },
  { value: "id", label: "印尼语" },
] as const;

/** 后端模型预下载事件（ai-rewrite:model-download）载荷 */
interface ModelDownloadPayload {
  model: string;
  status: "downloading" | "done" | "error";
  percent?: number;
  done?: number;
  total?: number;
  speed?: number;
  message?: string;
}

/** 后端本地转写环境安装事件（ai-rewrite:env-install）载荷 */
interface EnvInstallPayload {
  status: "installing" | "done" | "error";
  stage?: string;
  percent?: number;
  message?: string;
}

/** 环境检测结果（ai_rewrite_check_local_env 返回子集） */
interface LocalEnvState {
  source: "custom" | "managed" | "system" | null;
  hasWhisper: boolean;
}

/** 模型下载字节数展示（模型体量以 MB/GB 计） */
function fmtModelBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0MB";
  const mb = n / 1024 / 1024;
  return mb >= 1024 ? `${(mb / 1024).toFixed(2)}GB` : `${mb.toFixed(0)}MB`;
}

/** 代理中心（云端）代理条目的前端子集 */
interface CloudProxyLite {
  id: string;
  name: string;
  proxy_type: string;
  host: string;
  port: number;
  username: string | null;
  password: string | null;
  vless_uri: string | null;
  country?: string | null;
  city?: string | null;
}

type EndpointKey = "llm" | "asr" | "translate";

const EMPTY_ENDPOINT: AiEndpointFields = {
  provider: "",
  base_url: "",
  api_key: "",
  model: "",
  proxy: { type: "", host: "", user: "", pass: "" },
};

const EMPTY_CONFIG: AiApiConfigFields = {
  asr: EMPTY_ENDPOINT,
  llm: EMPTY_ENDPOINT,
  translate: EMPTY_ENDPOINT,
};

/** 代理下拉的哨兵值（Radix Select 不接受空字符串 value） */
const PROXY_NONE = "__none__";
/** 服务器上已保存、但不属于代理中心的代理（保留原值，避免误清空） */
const PROXY_SAVED = "__saved__";

const PROVIDERS = [
  {
    value: "openai_compatible",
    label: "OpenAI 兼容（ChatGPT / DeepSeek / 中转 API 等）",
  },
  { value: "anthropic", label: "Anthropic（Claude）" },
  { value: "gemini", label: "Google Gemini" },
];

const ENDPOINTS: {
  key: EndpointKey;
  title: string;
  hint: string;
  modelPh?: string;
  baseUrlPh?: string;
  proxyLockedText?: string;
}[] = [
  {
    key: "llm",
    title: "文案模型（洗稿 / 爆点分析 / 打分）",
    hint: "用 Claude 时必须配代理线路，直连可能封号",
  },
  { key: "asr", title: "语音识别（ASR）", hint: "" },
  { key: "translate", title: "翻译（导出双语字幕）", hint: "" },
];

/** AI 接口可使用的代理类型；vless/trojan 由后端经本地 Xray 侧车中转 */
const USABLE_PROXY_TYPES = ["http", "https", "socks5", "vless", "trojan"];
/** 隧道协议：host 存完整 URI，认证由本地 Xray 侧车接管 */
const TUNNEL_TYPES = ["vless", "trojan"];

function proxyKind(p: CloudProxyLite): string {
  return p.proxy_type.toLowerCase();
}

/** 代理条目实际生效的地址：隧道协议是完整 URI，其余是 host:port */
function proxyTargetHost(p: CloudProxyLite): string {
  return TUNNEL_TYPES.includes(proxyKind(p))
    ? p.vless_uri || `${p.host}:${p.port}`
    : `${p.host}:${p.port}`;
}

/** 下拉选项 label：隧道协议显示协议名，避免完整 URI 过长 */
function proxyOptionLabel(p: CloudProxyLite): string {
  const kind = proxyKind(p);
  if (kind === "vless") return `${p.name}（VLESS）`;
  if (kind === "trojan") return `${p.name}（Trojan）`;
  return `${p.name}（${p.host}:${p.port}）`;
}

/** 服务器已保存代理的展示地址（截断超长 URI） */
function savedProxyLabel(host: string): string {
  return host.length > 48 ? `${host.slice(0, 48)}…` : host;
}

/** 代理线路触发按钮的展示文案 */
function proxyModeLabel(
  mode: string,
  proxies: CloudProxyLite[],
  savedProxy: AiProxyFields,
): string {
  if (mode === PROXY_NONE) return "不使用代理（直连）";
  if (mode === PROXY_SAVED)
    return `当前保存的代理（${savedProxyLabel(savedProxy.host)}）`;
  const p = proxies.find((x) => x.id === mode);
  return p ? proxyOptionLabel(p) : "请选择代理线路";
}

function normalizeEndpoint(e?: Partial<AiEndpointFields>): AiEndpointFields {
  return {
    provider: e?.provider ?? "",
    base_url: e?.base_url ?? "",
    api_key: e?.api_key ?? "",
    model: e?.model ?? "",
    proxy: {
      type: e?.proxy?.type ?? "",
      host: e?.proxy?.host ?? "",
      user: e?.proxy?.user ?? "",
      pass: e?.proxy?.pass ?? "",
    },
  };
}

function normalizeConfig(raw: Partial<AiApiConfigFields>): AiApiConfigFields {
  return {
    asr: normalizeEndpoint(raw.asr),
    llm: normalizeEndpoint(raw.llm),
    translate: normalizeEndpoint(raw.translate),
  };
}

/** 按已保存的代理反推下拉选中项：匹配代理中心条目 → 该条目；有 host → 保留原值；否则不使用 */
function guessProxyMode(ep: AiEndpointFields, proxies: CloudProxyLite[]) {
  if (!ep.proxy.host.trim()) return PROXY_NONE;
  const hit = proxies.find((p) => {
    // 隧道协议按完整 URI 匹配（认证由本地 Xray 侧车接管，不比对账号）
    if (TUNNEL_TYPES.includes(proxyKind(p))) {
      return proxyTargetHost(p) === ep.proxy.host.trim();
    }
    return (
      `${p.host}:${p.port}` === ep.proxy.host.trim() &&
      (p.username ?? "") === ep.proxy.user
    );
  });
  return hit ? hit.id : PROXY_SAVED;
}

interface EndpointSectionProps {
  title: string;
  hint: string;
  ep: AiEndpointFields;
  mode: string;
  proxies: CloudProxyLite[];
  savedProxy: AiProxyFields;
  proxyLocked?: boolean;
  modelPh?: string;
  baseUrlPh?: string;
  proxyLockedText?: string;
  onEndpointChange: (ep: AiEndpointFields) => void;
  onOpenPicker: () => void;
}

function EndpointSection({
  title,
  hint,
  ep,
  mode,
  proxies,
  savedProxy,
  proxyLocked,
  modelPh,
  baseUrlPh,
  proxyLockedText,
  onEndpointChange,
  onOpenPicker,
}: EndpointSectionProps) {
  const patch = (p: Partial<AiEndpointFields>) =>
    onEndpointChange({ ...ep, ...p });
  const [testing, setTesting] = useState(false);
  const [testOut, setTestOut] = useState("");

  return (
    <div className="rounded-lg border bg-card p-3">
      <div className="mb-2">
        <span className="text-sm font-medium">{title}</span>
        {hint && <span className="ml-2 text-xs text-warning">{hint}</span>}
      </div>
      <div className="grid grid-cols-2 gap-2">
        <div className="space-y-1">
          <Label className="text-xs text-muted-foreground">接口协议</Label>
          <Select
            value={ep.provider}
            onValueChange={(v) => patch({ provider: v })}
          >
            <SelectTrigger className="h-8 text-xs">
              <SelectValue placeholder="未设置" />
            </SelectTrigger>
            <SelectContent>
              {PROVIDERS.map((p) => (
                <SelectItem key={p.value} value={p.value} className="text-xs">
                  {p.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label className="text-xs text-muted-foreground">模型</Label>
          <Input
            className="h-8 text-xs"
            value={ep.model}
            onChange={(e) => patch({ model: e.target.value })}
            placeholder={modelPh ?? "例：claude-sonnet-4-5"}
          />
        </div>
        <div className="col-span-2 space-y-1">
          <Label className="text-xs text-muted-foreground">接口地址</Label>
          <Input
            className="h-8 text-xs"
            value={ep.base_url}
            onChange={(e) => patch({ base_url: e.target.value })}
            placeholder={baseUrlPh ?? "例：https://api.anthropic.com"}
          />
          <p className="text-[11px] leading-4 text-muted-foreground">
            ChatGPT 官方：填 https://api.openai.com/v1；中转
            API：填中转站地址；DeepSeek 等留空即可
          </p>
        </div>
        <div className="col-span-2 space-y-1">
          <Label className="text-xs text-muted-foreground">API Key</Label>
          <Input
            className="h-8 text-xs"
            type="password"
            value={ep.api_key}
            onChange={(e) => patch({ api_key: e.target.value })}
            placeholder="sk-..."
          />
        </div>
        <div className="col-span-2 space-y-1">
          <Label className="text-xs text-muted-foreground">代理线路</Label>
          {proxyLocked ? (
            <div className="flex h-8 items-center rounded-md border bg-muted/40 px-2 text-xs text-muted-foreground">
              {proxyLockedText ?? "跟随语音识别（ASR）的代理线路"}
            </div>
          ) : (
            <button
              type="button"
              onClick={onOpenPicker}
              className="flex h-8 w-full items-center justify-between rounded-md border bg-transparent px-2 text-xs hover:bg-muted/30"
            >
              <span className="truncate">
                {proxyModeLabel(mode, proxies, savedProxy)}
              </span>
              <LuChevronDown className="ml-2 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            </button>
          )}
          {!proxyLocked && proxies.length === 0 && (
            <span className="block text-[11px] text-muted-foreground">
              代理中心没有可用代理（支持 HTTP / SOCKS5 / VLESS / Trojan）
            </span>
          )}
        </div>
      </div>
      <div className="mt-2 flex items-start gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7 text-xs"
          disabled={testing}
          onClick={() => {
            setTesting(true);
            setTestOut("");
            invoke<string>("ai_rewrite_test_endpoint", {
              endpoint: {
                provider: ep.provider,
                model: ep.model,
                base_url: ep.base_url,
                api_key: ep.api_key,
                proxy: ep.proxy,
              },
            })
              .then((msg) => setTestOut(msg))
              .catch((e) =>
                setTestOut(e instanceof Error ? e.message : String(e)),
              )
              .finally(() => setTesting(false));
          }}
        >
          {testing ? "测试中…" : "🔌 测试连接"}
        </Button>
        {testOut && (
          <span
            className={
              "flex-1 whitespace-pre-wrap text-[11px] leading-4 " +
              (testOut.startsWith("✅") ? "text-success" : "text-destructive")
            }
          >
            {testOut}
          </span>
        )}
      </div>
    </div>
  );
}

/** 代理线路选择弹窗：与云端账号「设置代理」同款表格形式（搜索 + 节点 IP 列表 + 单选） */
function AiProxyPickerDialog({
  open,
  onOpenChange,
  title,
  proxies,
  value,
  savedProxy,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  proxies: CloudProxyLite[];
  value: string;
  savedProxy: AiProxyFields | null;
  onConfirm: (value: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState(value);

  useEffect(() => {
    if (open) {
      setPicked(value);
      setQuery("");
    }
  }, [open, value]);

  const keyword = query.trim().toLowerCase();
  const filtered = keyword
    ? proxies.filter((p) =>
        [p.name, p.host, String(p.port), p.proxy_type, p.country ?? ""]
          .join(" ")
          .toLowerCase()
          .includes(keyword),
      )
    : proxies;
  const savedAvailable = savedProxy !== null && savedProxy.host.trim() !== "";
  const pickedProxy = proxies.find((x) => x.id === picked);
  const pickedLabel =
    picked === PROXY_NONE
      ? "不使用代理（直连）"
      : picked === PROXY_SAVED && savedProxy
        ? `当前保存的代理（${savedProxyLabel(savedProxy.host)}）`
        : (pickedProxy && proxyOptionLabel(pickedProxy)) || "请选择代理线路";

  const rowClass = (selected: boolean) =>
    cn(
      "cursor-pointer border-b transition-colors",
      selected ? "bg-primary/5" : "hover:bg-muted/30",
    );

  const TypeBadge = ({ text }: { text: string }) => (
    <span className="inline-flex rounded bg-muted px-1.5 py-0.5 text-[11px] font-medium">
      {text}
    </span>
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[640px] max-w-[95vw] p-0 overflow-hidden">
        <DialogHeader className="border-b px-4 py-3">
          <DialogTitle className="flex items-center gap-2 text-sm font-semibold">
            {title}
          </DialogTitle>
          <DialogDescription className="text-xs">
            从代理中心选择一条线路，支持 HTTP / SOCKS5 / VLESS / Trojan
          </DialogDescription>
        </DialogHeader>

        {/* 搜索栏 */}
        <div className="flex items-center gap-2 border-b px-4 py-2">
          <div className="relative flex-1">
            <LuSearch className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索代理名称、IP、类型..."
              className="h-8 pl-7 text-xs"
            />
          </div>
          <span className="shrink-0 text-xs text-muted-foreground">
            共 {filtered.length} 个
          </span>
        </div>

        {/* 代理列表表格 */}
        <div className="max-h-[60vh] min-h-[280px] overflow-y-auto">
          {filtered.length === 0 && !savedAvailable ? (
            <div className="p-8 text-center text-sm text-muted-foreground">
              暂无可用代理，请先在代理中心添加
            </div>
          ) : (
            <table className="w-full">
              <thead className="sticky top-0 bg-muted/50 backdrop-blur-sm">
                <tr className="border-b">
                  <th className="w-8 px-3 py-2" />
                  <th className="px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    代理信息
                  </th>
                  <th className="w-20 px-2 py-2 text-left text-xs font-medium text-muted-foreground">
                    类型
                  </th>
                  <th className="w-12 px-2 py-2" />
                </tr>
              </thead>
              <tbody>
                {/* 不使用代理（直连）选项 */}
                <tr
                  onClick={() => setPicked(PROXY_NONE)}
                  className={rowClass(picked === PROXY_NONE)}
                >
                  <td className="px-3 py-2">
                    <Checkbox
                      checked={picked === PROXY_NONE}
                      onCheckedChange={() => setPicked(PROXY_NONE)}
                    />
                  </td>
                  <td className="px-2 py-2">
                    <div className="flex items-center gap-2">
                      <LuUnplug className="h-3.5 w-3.5 text-muted-foreground" />
                      <span className="text-xs font-medium">
                        不使用代理（直连）
                      </span>
                    </div>
                  </td>
                  <td className="px-2 py-2">
                    <TypeBadge text="直连" />
                  </td>
                  <td className="px-2 py-2">
                    {picked === PROXY_NONE && (
                      <LuCheck className="h-3.5 w-3.5 text-primary" />
                    )}
                  </td>
                </tr>
                {/* 服务器已保存、但不属于代理中心的代理（保留原值） */}
                {savedAvailable && savedProxy && (
                  <tr
                    onClick={() => setPicked(PROXY_SAVED)}
                    className={rowClass(picked === PROXY_SAVED)}
                  >
                    <td className="px-3 py-2">
                      <Checkbox
                        checked={picked === PROXY_SAVED}
                        onCheckedChange={() => setPicked(PROXY_SAVED)}
                      />
                    </td>
                    <td className="px-2 py-2">
                      <div className="flex flex-col">
                        <span className="text-xs font-medium">
                          当前保存的代理
                        </span>
                        <span className="font-mono text-[11px] text-muted-foreground">
                          {savedProxyLabel(savedProxy.host)}
                        </span>
                      </div>
                    </td>
                    <td className="px-2 py-2">
                      <TypeBadge text={savedProxy.type || "原值"} />
                    </td>
                    <td className="px-2 py-2">
                      {picked === PROXY_SAVED && (
                        <LuCheck className="h-3.5 w-3.5 text-primary" />
                      )}
                    </td>
                  </tr>
                )}
                {filtered.map((proxy) => {
                  const isSelected = picked === proxy.id;
                  const kind = proxyKind(proxy);
                  return (
                    <tr
                      key={proxy.id}
                      onClick={() => setPicked(proxy.id)}
                      className={rowClass(isSelected)}
                    >
                      <td className="px-3 py-2">
                        <Checkbox
                          checked={isSelected}
                          onCheckedChange={() => setPicked(proxy.id)}
                        />
                      </td>
                      <td className="px-2 py-2">
                        <div className="flex flex-col">
                          <div className="flex items-center gap-1">
                            {proxy.country && (
                              <span className="inline-flex shrink-0 rounded bg-muted px-1 py-px text-[10px] font-medium text-muted-foreground">
                                {proxy.country}
                              </span>
                            )}
                            <span className="text-xs font-medium">
                              {proxy.name}
                            </span>
                          </div>
                          <span className="font-mono text-[11px] text-muted-foreground">
                            {proxy.host}:{proxy.port}
                          </span>
                        </div>
                      </td>
                      <td className="px-2 py-2">
                        <TypeBadge text={kind} />
                      </td>
                      <td className="px-2 py-2">
                        {isSelected && (
                          <LuCheck className="h-3.5 w-3.5 text-primary" />
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </div>

        {/* 底部操作 */}
        <div className="flex items-center justify-between border-t px-4 py-2">
          <span className="truncate text-xs text-muted-foreground">
            已选择：{pickedLabel}
          </span>
          <div className="flex shrink-0 gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-7"
              onClick={() => onOpenChange(false)}
            >
              取消
            </Button>
            <Button size="sm" className="h-7" onClick={() => onConfirm(picked)}>
              确定
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

interface AiRewriteSettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function AiRewriteSettingsDialog({
  open,
  onOpenChange,
}: AiRewriteSettingsDialogProps) {
  const [cfg, setCfg] = useState<AiApiConfigFields>(EMPTY_CONFIG);
  const [proxyModes, setProxyModes] = useState<Record<EndpointKey, string>>({
    llm: PROXY_NONE,
    asr: PROXY_NONE,
    translate: PROXY_NONE,
  });
  const [savedProxies, setSavedProxies] = useState<
    Record<EndpointKey, AiProxyFields>
  >({
    llm: EMPTY_ENDPOINT.proxy,
    asr: EMPTY_ENDPOINT.proxy,
    translate: EMPTY_ENDPOINT.proxy,
  });
  const [proxies, setProxies] = useState<CloudProxyLite[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [proxyError, setProxyError] = useState("");
  /** 正在为哪个端点选择代理线路（null = 选择弹窗关闭） */
  const [pickerFor, setPickerFor] = useState<EndpointKey | null>(null);
  const [localAsr, setLocalAsr] = useState<LocalAsrFields>(EMPTY_LOCAL_ASR);
  /** 每个模型的缓存状态：true=已下载 false=未下载 undefined=检查中 */
  const [modelCached, setModelCached] = useState<Record<string, boolean>>({});
  /** 正在预下载的模型（null = 无下载任务） */
  const [dlModel, setDlModel] = useState<string | null>(null);
  const [dl, setDl] = useState({ percent: 0, done: 0, total: 0, speed: 0 });
  /** 去重防抖：同模型的缓存检查不并发重复 spawn */
  const checkingRef = useRef<Set<string>>(new Set());
  /** 本轮会话中自动安装/自动下载失败过的对象：失败不自动重试，手动点按钮可清除重试 */
  const envFailedRef = useRef(false);
  const dlFailedRef = useRef<Set<string>>(new Set());
  /** 本地转写环境状态：null = 未检测完；hasWhisper=false 时展示一键安装 */
  const [env, setEnv] = useState<LocalEnvState | null>(null);
  /** 正在安装本地转写环境（null = 无安装任务） */
  const [envInstall, setEnvInstall] = useState<{
    stage: string;
    percent: number;
    message: string;
  } | null>(null);

  /** 探测模型是否已在 HF 缓存（失败按未下载处理，交给下载按钮兜底） */
  const checkModel = useCallback((model: string) => {
    if (checkingRef.current.has(model)) return;
    checkingRef.current.add(model);
    invoke<{ cached: boolean }>("ai_rewrite_check_local_model", { model })
      .then((r) => setModelCached((s) => ({ ...s, [model]: !!r.cached })))
      .catch(() => setModelCached((s) => ({ ...s, [model]: false })))
      .finally(() => checkingRef.current.delete(model));
  }, []);

  /** 检测本机 Python + faster-whisper 环境（失败按未检测到处理） */
  const checkEnv = useCallback(async () => {
    try {
      const r = await invoke<LocalEnvState>("ai_rewrite_check_local_env");
      setEnv({
        source: r.source ?? null,
        hasWhisper: !!r.hasWhisper,
      });
    } catch {
      setEnv({ source: null, hasWhisper: false });
    }
  }, []);

  // 打开设置或切换模型档位时重新探测缓存状态
  useEffect(() => {
    if (open && localAsr.enabled) checkModel(localAsr.model);
  }, [open, localAsr.enabled, localAsr.model, checkModel]);

  // 打开设置或开启本地模型时检测本机转写环境
  useEffect(() => {
    if (open && localAsr.enabled) void checkEnv();
  }, [open, localAsr.enabled, checkEnv]);

  // 环境安装事件：进度/收尾（done 重新检测环境与模型缓存，error 弹出原因）
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    listen<EnvInstallPayload>("ai-rewrite:env-install", (event) => {
      const p = event.payload;
      if (p.status === "installing") {
        setEnvInstall({
          stage: p.stage ?? "",
          percent: p.percent ?? 0,
          message: p.message ?? "",
        });
      } else if (p.status === "done") {
        setEnvInstall(null);
        toast.success(p.message || "本地转写环境安装完成");
        void checkEnv();
        if (localAsr.enabled) checkModel(localAsr.model);
      } else {
        envFailedRef.current = true;
        setEnvInstall(null);
        toast.error(p.message || "本地转写环境安装失败");
      }
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});
    return () => {
      for (const fn of unlisteners) fn();
    };
  }, [checkEnv, localAsr.enabled, localAsr.model, checkModel]);

  // 模型预下载事件：进度/收尾（done 回填缓存状态，error 弹出原因）
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    listen<ModelDownloadPayload>("ai-rewrite:model-download", (event) => {
      const p = event.payload;
      if (p.status === "done") {
        setDlModel(null);
        setModelCached((s) => ({ ...s, [p.model]: true }));
        toast.success("模型下载完成，转写时可离线使用");
      } else if (p.status === "error") {
        dlFailedRef.current.add(p.model);
        setDlModel(null);
        toast.error(p.message || "模型下载失败");
      } else {
        setDl({
          percent: p.percent ?? 0,
          done: p.done ?? 0,
          total: p.total ?? 0,
          speed: p.speed ?? 0,
        });
      }
    })
      .then((fn) => unlisteners.push(fn))
      .catch(() => {});
    return () => {
      for (const fn of unlisteners) fn();
    };
  }, []);

  /** 启动模型预下载（后端互斥，一次只允许一个；manual=true 清除失败标记） */
  const startDownload = useCallback(
    (model: string, manual = false) => {
      if (manual) dlFailedRef.current.delete(model);
      if (dlModel || dlFailedRef.current.has(model)) return;
      setDl({ percent: 0, done: 0, total: 0, speed: 0 });
      setDlModel(model);
      invoke("ai_rewrite_download_local_model", { model }).catch((e) => {
        dlFailedRef.current.add(model);
        setDlModel(null);
        toast.error(e instanceof Error ? e.message : String(e));
      });
    },
    [dlModel],
  );

  /** 一键安装本地转写环境（后端互斥 + 幂等续装；manual=true 清除失败标记） */
  const startEnvInstall = useCallback(
    (manual = false) => {
      if (manual) envFailedRef.current = false;
      if (envInstall || envFailedRef.current) return;
      setEnvInstall({ stage: "准备安装", percent: 0, message: "" });
      invoke("ai_rewrite_install_local_env").catch((e) => {
        envFailedRef.current = true;
        setEnvInstall(null);
        toast.error(e instanceof Error ? e.message : String(e));
      });
    },
    [envInstall],
  );

  // 全自动链路：没环境自动装环境；环境就绪且当前模型未下载则自动下模型。
  // 触发源（检测完成/安装完成/缓存检查完成）经状态变化进入这里；
  // 失败标记阻断自动重试，避免网络问题导致死循环，按钮仍可手动重试。
  useEffect(() => {
    if (!open || !localAsr.enabled || envInstall || !env) return;
    if (!env.hasWhisper) {
      if (env.source !== "custom") startEnvInstall();
      return;
    }
    if (modelCached[localAsr.model] === false) startDownload(localAsr.model);
  }, [
    open,
    localAsr.enabled,
    localAsr.model,
    env,
    envInstall,
    modelCached,
    startEnvInstall,
    startDownload,
  ]);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError("");
    setProxyError("");
    let next: AiApiConfigFields;
    try {
      const saved = await invoke<Partial<AiApiConfigFields>>(
        "ai_rewrite_get_api_config",
      );
      next = normalizeConfig(saved);
      setCfg(next);
      setSavedProxies({
        llm: next.llm.proxy,
        asr: next.asr.proxy,
        translate: next.translate.proxy,
      });
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
      setLoading(false);
      return;
    }
    // 本地 ASR 偏好单独读：失败不影响云端配置编辑
    try {
      const local = await invoke<Partial<LocalAsrFields>>(
        "ai_rewrite_get_local_asr",
      );
      setLocalAsr({ ...EMPTY_LOCAL_ASR, ...local });
    } catch {
      setLocalAsr(EMPTY_LOCAL_ASR);
    }
    // 代理列表单独拉取：失败不影响配置编辑，只影响代理下拉
    let usable: CloudProxyLite[] = [];
    try {
      const cloud = await invoke<CloudProxyLite[]>("bwbrowser_list_proxies", {
        companyId: null,
      });
      usable = (cloud ?? []).filter((p) =>
        USABLE_PROXY_TYPES.includes(p.proxy_type.toLowerCase()),
      );
      setProxies(usable);
    } catch (e) {
      setProxyError(e instanceof Error ? e.message : String(e));
    }
    setProxyModes({
      llm: guessProxyMode(next.llm, usable),
      asr: guessProxyMode(next.asr, usable),
      translate: guessProxyMode(next.translate, usable),
    });
    setLoading(false);
  }, []);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  const setEndpoint = useCallback(
    (key: EndpointKey, ep: AiEndpointFields) =>
      setCfg((c) => ({ ...c, [key]: ep })),
    [],
  );

  const setMode = useCallback(
    (key: EndpointKey, mode: string) =>
      setProxyModes((m) => ({ ...m, [key]: mode })),
    [],
  );

  /** 应用选择弹窗的确认结果（直连 / 保留原值 / 代理中心条目） */
  const applyProxyChoice = (key: EndpointKey, value: string) => {
    setCfg((c) => {
      const ep = c[key];
      const savedProxy = savedProxies[key];
      let proxy: AiProxyFields;
      if (value === PROXY_NONE) {
        proxy = { type: "", host: "", user: "", pass: "" };
      } else if (value === PROXY_SAVED) {
        proxy = { ...savedProxy };
      } else {
        const p = proxies.find((x) => x.id === value);
        if (!p) return c;
        const kind = proxyKind(p);
        proxy = TUNNEL_TYPES.includes(kind)
          ? { type: kind, host: proxyTargetHost(p), user: "", pass: "" }
          : {
              type: kind === "socks5" ? "socks5" : "http",
              host: `${p.host}:${p.port}`,
              user: p.username ?? "",
              pass: p.password ?? "",
            };
      }
      return { ...c, [key]: { ...ep, proxy } };
    });
    setMode(key, value);
  };

  const save = async () => {
    // 与后端防封号保护一致：Claude 必须带代理线路
    const llm = cfg.llm;
    const overridingLlm =
      llm.provider.trim() || llm.base_url.trim() || llm.api_key.trim();
    if (
      overridingLlm &&
      /claude|anthropic/i.test(llm.provider) &&
      !llm.proxy.host.trim()
    ) {
      toast.error(
        "Claude API 必须走指定代理线路（直连可能封号），请先选择 LLM 代理",
      );
      return;
    }
    setSaving(true);
    try {
      await invoke("ai_rewrite_save_api_config", {
        config: {
          asr: cfg.asr,
          llm: cfg.llm,
          translate: cfg.translate,
        },
      });
      await invoke("ai_rewrite_save_local_asr", { cfg: localAsr });
      toast.success("已保存，云端后台与本程序同步更新");
      onOpenChange(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>接口设置</DialogTitle>
            <DialogDescription>
              与云端后台「AI
              接口配置」是同一份配置，任一侧修改保存后另一侧自动一致；代理线路从代理中心选择。
            </DialogDescription>
          </DialogHeader>

          {loading ? (
            <div className="flex items-center justify-center gap-2 py-10 text-sm text-muted-foreground">
              <LuLoader className="size-4 animate-spin" />
              正在从服务器读取配置…
            </div>
          ) : loadError ? (
            <div className="space-y-3 py-8 text-center">
              <p className="text-sm text-destructive">读取失败：{loadError}</p>
              <Button variant="outline" size="sm" onClick={() => void load()}>
                <LuRefreshCw className="size-3.5" />
                重试
              </Button>
            </div>
          ) : (
            <ScrollArea className="max-h-[60vh] pr-2">
              <div className="space-y-3">
                {proxyError && (
                  <div className="flex items-center justify-between gap-2 rounded-lg border border-warning/50 bg-warning/10 px-3 py-2 text-xs text-warning-foreground">
                    <span>代理中心列表加载失败：{proxyError}</span>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-6 px-2 text-xs"
                      onClick={() => void load()}
                    >
                      <LuRefreshCw className="size-3" />
                      重试
                    </Button>
                  </div>
                )}
                {/* 提取文案转写来源：云端按量计费 / 本地 faster-whisper 免费 */}
                <div className="rounded-lg border p-3">
                  <div className="text-sm font-medium">提取文案转写来源</div>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    只影响「提取文案 / 转写」这一步；分析、洗稿、打分仍走云端。
                  </p>
                  <div className="mt-2 space-y-1.5">
                    <div
                      className="flex cursor-pointer items-start gap-2 rounded-md px-2 py-1.5 hover:bg-accent"
                      role="button"
                      tabIndex={0}
                      onClick={() =>
                        setLocalAsr((s) => ({ ...s, enabled: false }))
                      }
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          setLocalAsr((s) => ({ ...s, enabled: false }));
                        }
                      }}
                    >
                      <Checkbox
                        className="mt-0.5"
                        checked={!localAsr.enabled}
                      />
                      <span className="text-xs">
                        <span className="font-medium">
                          云端 ASR（按量计费）
                        </span>
                        <span className="block text-muted-foreground">
                          服务器统一密钥，速度快、精度稳定，消耗 token
                        </span>
                      </span>
                    </div>
                    <div
                      className="flex cursor-pointer items-start gap-2 rounded-md px-2 py-1.5 hover:bg-accent"
                      role="button"
                      tabIndex={0}
                      onClick={() =>
                        setLocalAsr((s) => ({ ...s, enabled: true }))
                      }
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          setLocalAsr((s) => ({ ...s, enabled: true }));
                        }
                      }}
                    >
                      <Checkbox className="mt-0.5" checked={localAsr.enabled} />
                      <span className="text-xs">
                        <span className="font-medium">
                          本地模型 faster-whisper（免费）
                        </span>
                        <span className="block text-muted-foreground">
                          调用本机 Python 的 faster-whisper，不消耗
                          token；可先在下方点「下载」预下载模型，未下载时首次使用会自动下载
                        </span>
                      </span>
                    </div>
                  </div>
                  {localAsr.enabled && (
                    <div className="mt-2 space-y-2 rounded-md bg-muted/40 p-2.5">
                      <div className="space-y-1.5">
                        <div className="flex items-end gap-2">
                          <div className="flex-1 space-y-1">
                            <Label className="text-xs">模型</Label>
                            <Select
                              value={localAsr.model}
                              onValueChange={(v) =>
                                setLocalAsr((s) => ({ ...s, model: v }))
                              }
                              disabled={!!dlModel || !!envInstall}
                            >
                              <SelectTrigger className="h-7 text-xs">
                                <SelectValue />
                              </SelectTrigger>
                              <SelectContent>
                                {LOCAL_ASR_MODELS.map((m) => (
                                  <SelectItem key={m.value} value={m.value}>
                                    {m.label}
                                  </SelectItem>
                                ))}
                              </SelectContent>
                            </Select>
                          </div>
                          {dlModel === localAsr.model ? (
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-7 w-20 gap-1 text-xs"
                              disabled
                            >
                              <LuLoader className="size-3.5 animate-spin" />
                              {dl.percent}%
                            </Button>
                          ) : modelCached[localAsr.model] ? (
                            <span className="flex h-7 items-center gap-1 whitespace-nowrap px-1 text-xs text-success">
                              <LuCheck className="size-3.5" />
                              已下载
                            </span>
                          ) : (
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-7 w-20 gap-1 text-xs"
                              disabled={!!dlModel || !!envInstall}
                              onClick={() =>
                                startDownload(localAsr.model, true)
                              }
                            >
                              <LuDownload className="size-3.5" />
                              下载
                            </Button>
                          )}
                        </div>
                        {dlModel === localAsr.model && (
                          <div className="space-y-1 rounded-md border bg-background/50 p-2">
                            <div className="flex items-center justify-between text-xs text-muted-foreground">
                              <span>
                                正在下载模型 {dl.percent}%（下载后可离线转写）
                              </span>
                              <span>
                                {dl.total > 0
                                  ? `${fmtModelBytes(dl.done)} / ${fmtModelBytes(dl.total)}${dl.speed > 0 ? ` · ${fmtModelBytes(dl.speed)}/s` : ""}`
                                  : dl.done > 0
                                    ? `已下载 ${fmtModelBytes(dl.done)}${dl.speed > 0 ? ` · ${fmtModelBytes(dl.speed)}/s` : ""}`
                                    : "准备下载…"}
                              </span>
                            </div>
                            <Progress value={dl.percent} className="h-1.5" />
                          </div>
                        )}
                      </div>
                      {env && !env.hasWhisper && envInstall === null ? (
                        env.source === "custom" ? (
                          <div className="rounded-md border border-warning/50 bg-warning/10 p-2 text-xs text-warning-foreground">
                            当前配置的 Python 路径无法运行或缺少
                            faster-whisper，请修正或清空「Python 路径」后重试
                          </div>
                        ) : (
                          <div className="flex items-center justify-between gap-2 rounded-md border bg-background/50 p-2">
                            <span className="text-xs text-muted-foreground">
                              本机未检测到可用的转写环境，点「一键安装」自动完成（免费，约
                              200MB，装进程序数据目录，不影响系统）
                            </span>
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-7 shrink-0 gap-1 text-xs"
                              onClick={() => startEnvInstall(true)}
                            >
                              <LuDownload className="size-3.5" />
                              一键安装
                            </Button>
                          </div>
                        )
                      ) : null}
                      {envInstall && (
                        <div className="space-y-1 rounded-md border bg-background/50 p-2">
                          <div className="flex items-center justify-between text-xs text-muted-foreground">
                            <span>
                              正在安装本地转写环境：{envInstall.stage}
                              {envInstall.message
                                ? `（${envInstall.message}）`
                                : ""}
                            </span>
                            <span>{envInstall.percent}%</span>
                          </div>
                          <Progress
                            value={envInstall.percent}
                            className="h-1.5"
                          />
                        </div>
                      )}
                      <div className="space-y-1">
                        <Label className="text-xs">Python 路径（可选）</Label>
                        <Input
                          className="h-7 text-xs"
                          placeholder="留空 = 使用系统 PATH 中的 python"
                          value={localAsr.pythonPath}
                          onChange={(e) =>
                            setLocalAsr((s) => ({
                              ...s,
                              pythonPath: e.target.value,
                            }))
                          }
                        />
                      </div>
                      <div className="space-y-1">
                        <Label className="text-xs">识别语言（可选）</Label>
                        <Select
                          value={localAsr.language || "auto"}
                          onValueChange={(v) =>
                            setLocalAsr((s) => ({
                              ...s,
                              language: v === "auto" ? "" : v,
                            }))
                          }
                        >
                          <SelectTrigger className="h-7 text-xs">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="auto" className="text-xs">
                              自动检测
                            </SelectItem>
                            {LOCAL_ASR_LANGUAGES.map((l) => (
                              <SelectItem
                                key={l.value}
                                value={l.value}
                                className="text-xs"
                              >
                                {l.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                    </div>
                  )}
                </div>
                {ENDPOINTS.map((item) => (
                  <EndpointSection
                    key={item.key}
                    title={item.title}
                    hint={item.hint}
                    ep={cfg[item.key]}
                    mode={proxyModes[item.key]}
                    proxies={proxies}
                    savedProxy={savedProxies[item.key]}
                    proxyLocked={item.key === "translate"}
                    modelPh={item.modelPh}
                    baseUrlPh={item.baseUrlPh}
                    proxyLockedText={item.proxyLockedText}
                    onEndpointChange={(ep) => setEndpoint(item.key, ep)}
                    onOpenPicker={() => setPickerFor(item.key)}
                  />
                ))}
              </div>
            </ScrollArea>
          )}

          <DialogFooter>
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              取消
            </Button>
            <Button
              onClick={() => void save()}
              disabled={loading || saving || !!loadError}
            >
              {saving && <LuLoader className="size-4 animate-spin" />}
              保存并同步
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AiProxyPickerDialog
        open={pickerFor !== null}
        onOpenChange={(o) => !o && setPickerFor(null)}
        title={
          pickerFor === "llm"
            ? "选择文案模型代理线路"
            : "选择语音识别（ASR）代理线路"
        }
        proxies={proxies}
        value={pickerFor ? proxyModes[pickerFor] : PROXY_NONE}
        savedProxy={pickerFor ? savedProxies[pickerFor] : null}
        onConfirm={(v) => {
          if (pickerFor) applyProxyChoice(pickerFor, v);
          setPickerFor(null);
        }}
      />
    </>
  );
}
