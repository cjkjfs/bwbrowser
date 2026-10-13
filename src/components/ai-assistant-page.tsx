"use client";

import { invoke } from "@tauri-apps/api/core";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuBot,
  LuFolderOpen,
  LuLoader,
  LuPlus,
  LuSend,
  LuTrash2,
  LuX,
} from "react-icons/lu";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Textarea } from "@/components/ui/textarea";
import { translateBackendError } from "@/lib/backend-errors";
import { cn } from "@/lib/utils";

interface AiAssistantPageProps {
  embedded?: boolean;
  /** 融合进 AI 洗稿页的标签页里：去掉外层留白与标题行，清空按钮挪到输入框左侧。 */
  bare?: boolean;
}

interface AgentChatStep {
  kind: string;
  name: string;
  detail: string;
  is_error: boolean;
}

interface AgentChatReply {
  reply: string;
  steps: AgentChatStep[];
}

interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  steps?: AgentChatStep[];
}

/** 一个独立对话：标题取首条用户消息前 20 字，空串时 UI 显示「新对话」。 */
interface ChatSession {
  id: string;
  title: string;
  createdAt: number;
  messages: ChatMessage[];
}

const SESSIONS_KEY = "ai-assistant-sessions";
const ACTIVE_SESSION_KEY = "ai-assistant-active-session";
const LEGACY_HISTORY_KEY = "ai-assistant-history";
const SESSION_TITLE_MAX = 20;

/** 校验并规整消息数组；损坏条目直接丢弃。 */
function sanitizeMessages(parsed: unknown): ChatMessage[] {
  if (!Array.isArray(parsed)) return [];
  const valid: ChatMessage[] = [];
  for (const m of parsed) {
    if (!m || typeof m !== "object") continue;
    const c = m as Partial<ChatMessage>;
    if (
      (c.role === "user" || c.role === "assistant") &&
      typeof c.content === "string"
    ) {
      valid.push({
        id: c.id || newId(),
        role: c.role,
        content: c.content,
        ...(Array.isArray(c.steps) ? { steps: c.steps } : {}),
      });
    }
  }
  return valid;
}

function emptySession(): ChatSession {
  return { id: newId(), title: "", createdAt: Date.now(), messages: [] };
}

function sessionTitle(text: string): string {
  const t = text.replace(/\s+/g, " ").trim();
  return t.length > SESSION_TITLE_MAX ? `${t.slice(0, SESSION_TITLE_MAX)}…` : t;
}

/** 读取本地持久化的多会话；损坏即丢弃。旧版单列表历史迁移为第一个会话。 */
function loadSessions(): ChatSession[] {
  let legacy: ChatMessage[] = [];
  try {
    const raw = localStorage.getItem(LEGACY_HISTORY_KEY);
    if (raw) legacy = sanitizeMessages(JSON.parse(raw));
  } catch {
    legacy = [];
  }

  let sessions: ChatSession[] = [];
  try {
    const raw = localStorage.getItem(SESSIONS_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as unknown;
      if (Array.isArray(parsed)) {
        for (const s of parsed) {
          if (!s || typeof s !== "object") continue;
          const o = s as Partial<ChatSession>;
          if (typeof o.id !== "string" || !o.id) continue;
          sessions.push({
            id: o.id,
            title: typeof o.title === "string" ? o.title : "",
            createdAt:
              typeof o.createdAt === "number" ? o.createdAt : Date.now(),
            messages: sanitizeMessages(o.messages),
          });
        }
      }
    }
  } catch {
    sessions = [];
  }

  if (sessions.length === 0 && legacy.length > 0) {
    try {
      localStorage.removeItem(LEGACY_HISTORY_KEY);
    } catch {
      // 忽略：迁移失败只是下次再迁移一次，不影响使用
    }
    return [
      {
        id: newId(),
        title: sessionTitle(
          legacy.find((m) => m.role === "user")?.content ?? "",
        ),
        createdAt: Date.now(),
        messages: legacy,
      },
    ];
  }
  return sessions.length > 0 ? sessions : [emptySession()];
}

function newId(): string {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export function AiAssistantPage({
  embedded = false,
  bare = false,
}: AiAssistantPageProps) {
  const { t } = useTranslation();
  const [sessions, setSessions] = useState<ChatSession[]>(loadSessions);
  const [activeId, setActiveId] = useState<string>(() => {
    try {
      return localStorage.getItem(ACTIVE_SESSION_KEY) ?? "";
    } catch {
      return "";
    }
  });
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  /** 用户通过原生选择器授权给助手的本地文件夹（只读）。 */
  const [allowedFolders, setAllowedFolders] = useState<string[]>([]);
  const scrollRef = useRef<HTMLDivElement>(null);

  // 无效的 activeId 回落到第一个会话；active 是唯一消息来源。
  const active =
    sessions.find((s) => s.id === activeId) ?? sessions[0] ?? emptySession();
  const messages = active.messages;

  useEffect(() => {
    try {
      localStorage.setItem(SESSIONS_KEY, JSON.stringify(sessions));
    } catch {
      // 忽略：隐私模式等场景下 localStorage 不可用，对话仅存于内存
    }
  }, [sessions]);

  useEffect(() => {
    try {
      localStorage.setItem(ACTIVE_SESSION_KEY, active.id);
    } catch {
      // 忽略
    }
  }, [active.id]);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);

  const send = useCallback(async () => {
    const text = input.trim();
    if (!text || busy) return;
    const userMsg: ChatMessage = { id: newId(), role: "user", content: text };
    const history = [...messages, userMsg];
    const sessionId = active.id;
    setSessions((prev) =>
      prev.map((s) =>
        s.id === sessionId
          ? { ...s, title: s.title || sessionTitle(text), messages: history }
          : s,
      ),
    );
    setInput("");
    setBusy(true);
    try {
      const reply = await invoke<AgentChatReply>("ai_agent_chat", {
        messages: history.map((m) => ({ role: m.role, content: m.content })),
        allowedFolders,
      });
      setSessions((prev) =>
        prev.map((s) =>
          s.id === sessionId
            ? {
                ...s,
                messages: [
                  ...history,
                  {
                    id: newId(),
                    role: "assistant",
                    content: reply.reply,
                    steps: reply.steps,
                  },
                ],
              }
            : s,
        ),
      );
    } catch (e) {
      toast.error(translateBackendError(t, e));
    } finally {
      setBusy(false);
    }
  }, [input, busy, messages, active.id, allowedFolders, t]);

  const pickFolder = useCallback(async () => {
    const picked = await open({ directory: true, multiple: true });
    if (!picked) return;
    const folders = Array.isArray(picked) ? picked : [picked];
    setAllowedFolders((prev) => {
      const next = new Set(prev);
      for (const f of folders) next.add(f);
      return [...next];
    });
  }, []);

  const removeFolder = useCallback((folder: string) => {
    setAllowedFolders((prev) => prev.filter((f) => f !== folder));
  }, []);

  const clear = useCallback(async () => {
    const ok = await confirm(t("aiAssistant.clearBody"), {
      title: t("aiAssistant.clearTitle"),
      kind: "warning",
    }).catch(() => false);
    if (!ok) return;
    setSessions((prev) =>
      prev.map((s) =>
        s.id === active.id ? { ...s, title: "", messages: [] } : s,
      ),
    );
    toast.success(t("aiAssistant.cleared"));
  }, [t, active.id]);

  const newSession = useCallback(() => {
    const s = emptySession();
    setSessions((prev) => [s, ...prev]);
    setActiveId(s.id);
    setInput("");
  }, []);

  const deleteSession = useCallback(
    async (id: string) => {
      const target = sessions.find((s) => s.id === id);
      if (!target || (busy && id === active.id)) return;
      // 最后一个会话不真删：清空内容，界面始终保留一个对话（无破坏性，免确认）。
      if (sessions.length === 1) {
        setSessions([{ ...target, title: "", messages: [] }]);
        return;
      }
      if (target.messages.length > 0) {
        const ok = await confirm(t("aiAssistant.deleteSessionBody"), {
          title: t("aiAssistant.deleteSessionTitle"),
          kind: "warning",
        }).catch(() => false);
        if (!ok) return;
      }
      const idx = sessions.findIndex((s) => s.id === id);
      const next = sessions.filter((s) => s.id !== id);
      setSessions(next);
      if (id === activeId) {
        setActiveId((next[idx] ?? next[next.length - 1]).id);
      }
    },
    [sessions, activeId, active.id, busy, t],
  );

  const hasMessages = messages.length > 0;

  return (
    <div
      className={cn(
        "flex min-h-0 flex-1 flex-col gap-4 overflow-hidden",
        bare ? "" : embedded ? "p-4" : "p-6",
      )}
    >
      {!bare && (
        <div className="flex items-start justify-between gap-4">
          <div className="flex items-center gap-3">
            <div className="grid size-9 shrink-0 place-items-center rounded-lg bg-primary/10">
              <LuBot className="size-5 text-primary" />
            </div>
            <div>
              <h2 className="text-base font-semibold text-foreground">
                {t("aiAssistant.title")}
              </h2>
              <p className="text-xs text-muted-foreground">
                {t("aiAssistant.description")}
              </p>
            </div>
          </div>
          {hasMessages && (
            <Button
              variant="ghost"
              size="sm"
              onClick={clear}
              className="gap-1.5 text-muted-foreground"
            >
              <LuTrash2 className="size-4" />
              {t("aiAssistant.clear")}
            </Button>
          )}
        </div>
      )}

      <div className="flex items-center gap-1 overflow-x-auto">
        {sessions.map((s) => {
          const isActive = s.id === active.id;
          return (
            <div
              key={s.id}
              className={cn(
                "group flex max-w-44 shrink-0 items-center gap-1 rounded-md border px-2 py-1 text-xs",
                isActive
                  ? "border-primary/40 bg-primary/10 text-foreground"
                  : "border-border bg-card text-muted-foreground hover:text-foreground",
              )}
            >
              <button
                type="button"
                className="truncate"
                onClick={() => setActiveId(s.id)}
                title={s.title || t("aiAssistant.defaultSessionTitle")}
              >
                {s.title || t("aiAssistant.defaultSessionTitle")}
              </button>
              <button
                type="button"
                onClick={() => void deleteSession(s.id)}
                aria-label={t("aiAssistant.deleteSession")}
                title={t("aiAssistant.deleteSession")}
                className={cn(
                  "shrink-0 transition-opacity",
                  isActive
                    ? "opacity-50 hover:opacity-100"
                    : "opacity-0 group-hover:opacity-70 hover:!opacity-100",
                )}
              >
                <LuX className="size-3" />
              </button>
            </div>
          );
        })}
        <Button
          variant="ghost"
          size="icon"
          onClick={newSession}
          aria-label={t("aiAssistant.newSession")}
          title={t("aiAssistant.newSession")}
          className="size-7 shrink-0 text-muted-foreground"
        >
          <LuPlus className="size-3.5" />
        </Button>
      </div>

      <ScrollArea className="min-h-0 flex-1 rounded-lg border border-border bg-card/30">
        <div ref={scrollRef} className="h-full overflow-y-auto p-4">
          {!hasMessages && !busy && (
            <div className="flex h-full min-h-40 flex-col items-center justify-center gap-2 text-center">
              <LuBot className="size-10 text-muted-foreground/40" />
              <p className="text-sm font-medium text-muted-foreground">
                {t("aiAssistant.emptyTitle")}
              </p>
              <p className="max-w-sm text-xs text-muted-foreground/80">
                {t("aiAssistant.emptyBody")}
              </p>
            </div>
          )}
          <div className="flex flex-col gap-3">
            {messages.map((m) => (
              <div
                key={m.id}
                className={cn(
                  "flex",
                  m.role === "user" ? "justify-end" : "justify-start",
                )}
              >
                <div
                  className={cn(
                    "max-w-[85%] rounded-lg px-3 py-2 text-sm",
                    m.role === "user"
                      ? "bg-primary text-primary-foreground"
                      : "border border-border bg-card text-foreground",
                  )}
                >
                  {m.steps && m.steps.length > 0 && (
                    <div className="mb-2 flex flex-col gap-1 border-b border-border/60 pb-2">
                      {m.steps.map((s, i) => (
                        <div key={i} className="flex items-start gap-1.5">
                          <Badge
                            variant={s.is_error ? "destructive" : "secondary"}
                            className="shrink-0 text-[10px]"
                          >
                            {s.name}
                          </Badge>
                          {s.detail && (
                            <span
                              className="line-clamp-2 text-[11px] text-muted-foreground"
                              title={s.detail}
                            >
                              {s.detail}
                            </span>
                          )}
                        </div>
                      ))}
                    </div>
                  )}
                  <p className="whitespace-pre-wrap break-words">{m.content}</p>
                </div>
              </div>
            ))}
            {busy && (
              <div className="flex justify-start">
                <div className="flex items-center gap-2 rounded-lg border border-border bg-card px-3 py-2 text-sm text-muted-foreground">
                  <LuLoader className="size-4 animate-spin" />
                  {t("aiAssistant.thinking")}
                </div>
              </div>
            )}
          </div>
        </div>
      </ScrollArea>

      {allowedFolders.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          {allowedFolders.map((folder) => (
            <Badge
              key={folder}
              variant="secondary"
              className="max-w-72 gap-1 font-normal"
            >
              <span className="truncate" title={folder}>
                {folder}
              </span>
              <button
                type="button"
                onClick={() => removeFolder(folder)}
                aria-label={t("aiAssistant.removeFolder")}
                className="shrink-0 text-muted-foreground hover:text-foreground"
              >
                <LuX className="size-3" />
              </button>
            </Badge>
          ))}
        </div>
      )}

      <div className="flex items-end gap-2">
        <Button
          variant="ghost"
          size="icon"
          onClick={() => void pickFolder()}
          aria-label={t("aiAssistant.pickFolder")}
          title={t("aiAssistant.pickFolder")}
          className="shrink-0 text-muted-foreground"
        >
          <LuFolderOpen className="size-4" />
        </Button>
        {bare && hasMessages && (
          <Button
            variant="ghost"
            size="icon"
            onClick={clear}
            aria-label={t("aiAssistant.clear")}
            title={t("aiAssistant.clear")}
            className="shrink-0 text-muted-foreground"
          >
            <LuTrash2 className="size-4" />
          </Button>
        )}
        <Textarea
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (
              e.key === "Enter" &&
              !e.shiftKey &&
              !e.nativeEvent.isComposing
            ) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder={t("aiAssistant.placeholder")}
          className="min-h-11 flex-1 resize-none"
          rows={1}
          disabled={busy}
        />
        <Button
          size="icon"
          onClick={() => void send()}
          disabled={busy || !input.trim()}
          aria-label={t("aiAssistant.send")}
        >
          {busy ? (
            <LuLoader className="size-4 animate-spin" />
          ) : (
            <LuSend className="size-4" />
          )}
        </Button>
      </div>
    </div>
  );
}
