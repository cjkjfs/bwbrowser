"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import {
  LuCheck,
  LuChevronDown,
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
