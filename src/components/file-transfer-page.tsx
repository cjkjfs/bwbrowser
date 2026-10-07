"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  LuArrowLeft,
  LuArrowRight,
  LuArrowUp,
  LuDownload,
  LuFile,
  LuFolder,
  LuFolderPlus,
  LuHardDrive,
  LuLoaderCircle,
  LuPencil,
  LuRefreshCw,
  LuTrash2,
  LuUpload,
} from "react-icons/lu";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { translateBackendError } from "@/lib/backend-errors";
import { cn } from "@/lib/utils";

/** 被控端 viewer 的 file_list_result 结构（本机侧复用同一套，左右两栏渲染逻辑一致）。 */
interface FileEntry {
  name: string;
  is_dir: boolean;
  size: number;
}

interface FileListResult {
  success: boolean;
  entries?: FileEntry[];
  path: string;
  parent: string;
  is_drives: boolean;
  message?: string;
}

/** 后端解析出的传输目标（由 bwbrowser_remote_files_target 返回，原样回传复用）。 */
interface RemoteFileTarget {
  method: string;
  host: string;
  port: number;
  password: string;
  uuid: string;
  relay_url: string;
  hostname: string;
}

/** 打开文件传输页所需的机器信息。 */
export interface FileTransferTarget {
  hostName: string;
  /** 云端机器 id；纯局域网发现的机器为 null */
  clientId: number | null;
  lanHost: string | null;
  lanPort: number | null;
}

interface PaneState {
  path: string;
  parent: string;
  isDrives: boolean;
  entries: FileEntry[];
  loading: boolean;
}

const EMPTY_PANE: PaneState = {
  path: "",
  parent: "",
  isDrives: false,
  entries: [],
  loading: false,
};

/** 进入子目录：磁盘列表里的条目本身就是完整根路径，其余按分隔符拼接。 */
function childPath(base: string, name: string): string {
  if (!base) return name;
  return base.endsWith("/") || base.endsWith("\\")
    ? `${base}${name}`
    : `${base}/${name}`;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[i]}`;
}

type EditMode =
  | { kind: "mkdir" }
  | { kind: "rename"; name: string }
  | { kind: "delete"; name: string }
  | null;

interface FilePaneProps {
  title: string;
  icon: React.ReactNode;
  pane: PaneState;
  selected: FileEntry | null;
  onSelect: (entry: FileEntry | null) => void;
  onOpen: (entry: FileEntry) => void;
  onUp: () => void;
  onRefresh: () => void;
  onMkdir: (name: string) => Promise<void>;
  onRename: (name: string, newName: string) => Promise<void>;
  onDelete: (name: string) => Promise<void>;
  /** 传输按钮等额外操作 */
  extraActions?: React.ReactNode;
  disabled?: boolean;
}

function FilePane({
  title,
  icon,
  pane,
  selected,
  onSelect,
  onOpen,
  onUp,
  onRefresh,
  onMkdir,
  onRename,
  onDelete,
  extraActions,
  disabled,
}: FilePaneProps) {
  const { t } = useTranslation();
  const [edit, setEdit] = useState<EditMode>(null);
  const [draft, setDraft] = useState("");

  const startMkdir = () => {
    setDraft("");
    setEdit({ kind: "mkdir" });
  };
  const startRename = () => {
    if (!selected) return;
    setDraft(selected.name);
    setEdit({ kind: "rename", name: selected.name });
  };
  const cancelEdit = () => {
    setEdit(null);
    setDraft("");
  };

  const commitEdit = async () => {
    if (!edit) return;
    const value = draft.trim();
    if (edit.kind !== "delete" && !value) return;
    if (edit.kind === "mkdir") await onMkdir(value);
    else if (edit.kind === "rename") await onRename(edit.name, value);
    else await onDelete(edit.name);
    cancelEdit();
  };

  return (
    <div className="flex min-w-0 flex-1 flex-col">
      <div className="flex items-center gap-2 border-b border-border px-3 py-2">
        <span className="flex items-center gap-1.5 text-sm font-medium">
          {icon}
          {title}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-xs"
            onClick={onUp}
            disabled={disabled || pane.loading}
            title={t("fileTransfer.up")}
          >
            <LuArrowUp className="w-3.5 h-3.5" />
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-xs"
            onClick={onRefresh}
            disabled={disabled || pane.loading}
            title={t("fileTransfer.refresh")}
          >
            <LuRefreshCw
              className={cn("w-3.5 h-3.5", pane.loading && "animate-spin")}
            />
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-xs"
            onClick={startMkdir}
            disabled={disabled || pane.isDrives}
            title={t("fileTransfer.newFolder")}
          >
            <LuFolderPlus className="w-3.5 h-3.5" />
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-xs"
            onClick={startRename}
            disabled={disabled || !selected}
            title={t("fileTransfer.rename")}
          >
            <LuPencil className="w-3.5 h-3.5" />
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 px-2 text-xs text-destructive"
            onClick={() =>
              selected && setEdit({ kind: "delete", name: selected.name })
            }
            disabled={disabled || !selected}
            title={t("fileTransfer.delete")}
          >
            <LuTrash2 className="w-3.5 h-3.5" />
          </Button>
        </div>
      </div>

      <div className="truncate border-b border-border bg-muted/30 px-3 py-1 text-xs text-muted-foreground">
        {pane.isDrives || !pane.path ? t("fileTransfer.drives") : pane.path}
      </div>

      {edit && (
        <div className="flex items-center gap-2 border-b border-border bg-accent/40 px-3 py-2">
          {edit.kind === "delete" ? (
            <>
              <span className="text-xs">
                {t("fileTransfer.confirmDelete", { name: edit.name })}
              </span>
              <Button
                size="sm"
                variant="destructive"
                className="ml-auto h-7 px-2 text-xs"
                onClick={() => void commitEdit()}
              >
                {t("fileTransfer.confirm")}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="h-7 px-2 text-xs"
                onClick={cancelEdit}
              >
                {t("fileTransfer.cancel")}
              </Button>
            </>
          ) : (
            <>
              <Input
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void commitEdit();
                  if (e.key === "Escape") cancelEdit();
                }}
                placeholder={
                  edit.kind === "mkdir"
                    ? t("fileTransfer.folderName")
                    : t("fileTransfer.newName")
                }
                className="h-7 flex-1 text-xs"
              />
              <Button
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => void commitEdit()}
              >
                {t("fileTransfer.confirm")}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="h-7 px-2 text-xs"
                onClick={cancelEdit}
              >
                {t("fileTransfer.cancel")}
              </Button>
            </>
          )}
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        {pane.loading ? (
          <div className="flex items-center justify-center py-10 text-muted-foreground">
            <LuLoaderCircle className="w-5 h-5 animate-spin" />
          </div>
        ) : pane.entries.length === 0 ? (
          <div className="py-10 text-center text-xs text-muted-foreground">
            {t("fileTransfer.empty")}
          </div>
        ) : (
          pane.entries.map((entry) => {
            const active = selected?.name === entry.name;
            return (
              <button
                key={entry.name}
                type="button"
                onClick={() => onSelect(entry)}
                onDoubleClick={() => onOpen(entry)}
                className={cn(
                  "flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-accent",
                  active && "bg-accent",
                )}
              >
                {entry.is_dir ? (
                  pane.isDrives ? (
                    <LuHardDrive className="w-3.5 h-3.5 shrink-0 text-muted-foreground" />
                  ) : (
                    <LuFolder className="w-3.5 h-3.5 shrink-0 text-muted-foreground" />
                  )
                ) : (
                  <LuFile className="w-3.5 h-3.5 shrink-0 text-muted-foreground" />
                )}
                <span className="min-w-0 flex-1 truncate">{entry.name}</span>
                {!entry.is_dir && (
                  <span className="shrink-0 text-muted-foreground">
                    {formatSize(entry.size)}
                  </span>
                )}
              </button>
            );
          })
        )}
      </div>

      {extraActions && (
        <div className="flex items-center gap-2 border-t border-border px-3 py-2">
          {extraActions}
        </div>
      )}
    </div>
  );
}

export function FileTransferPage({
  target,
  onClose,
}: {
  target: FileTransferTarget;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [remote, setRemote] = useState<RemoteFileTarget | null>(null);
  const [password, setPassword] = useState("");
  const [needPassword, setNeedPassword] = useState(false);
  const [resolving, setResolving] = useState(true);
  const [resolveError, setResolveError] = useState<string | null>(null);

  const [local, setLocal] = useState<PaneState>({
    ...EMPTY_PANE,
    loading: true,
  });
  const [remotePane, setRemotePane] = useState<PaneState>({
    ...EMPTY_PANE,
    loading: true,
  });
  const [localSel, setLocalSel] = useState<FileEntry | null>(null);
  const [remoteSel, setRemoteSel] = useState<FileEntry | null>(null);
  const [busy, setBusy] = useState<"upload" | "download" | null>(null);
  const started = useRef(false);

  const loadLocal = useCallback(
    async (path: string) => {
      setLocal((s) => ({ ...s, loading: true }));
      try {
        const r = await invoke<FileListResult>("bwbrowser_local_files_list", {
          path,
        });
        setLocal({
          path: r.path ?? path,
          parent: r.parent ?? "",
          isDrives: Boolean(r.is_drives),
          entries: r.entries ?? [],
          loading: false,
        });
        setLocalSel(null);
      } catch (e) {
        toast.error(translateBackendError(t, e));
        setLocal((s) => ({ ...s, loading: false }));
      }
    },
    [t],
  );

  const loadRemote = useCallback(
    async (tgt: RemoteFileTarget, path: string) => {
      setRemotePane((s) => ({ ...s, loading: true }));
      try {
        const r = await invoke<FileListResult>("bwbrowser_remote_files_list", {
          target: tgt,
          path,
        });
        if (r.success === false) {
          toast.error(t("fileTransfer.operationFailed"), {
            description: r.message,
          });
          setRemotePane((s) => ({ ...s, loading: false }));
          return;
        }
        setRemotePane({
          path: r.path ?? path,
          parent: r.parent ?? "",
          isDrives: Boolean(r.is_drives),
          entries: r.entries ?? [],
          loading: false,
        });
        setRemoteSel(null);
      } catch (e) {
        toast.error(translateBackendError(t, e));
        setRemotePane((s) => ({ ...s, loading: false }));
      }
    },
    [t],
  );

  const resolveTarget = useCallback(
    async (pwd: string): Promise<RemoteFileTarget | null> => {
      setResolving(true);
      setResolveError(null);
      try {
        const r = await invoke<RemoteFileTarget>(
          "bwbrowser_remote_files_target",
          {
            clientId: target.clientId,
            host: target.lanHost,
            port: target.lanPort,
            password: pwd || null,
          },
        );
        setRemote(r);
        setNeedPassword(!r.password);
        return r;
      } catch (e) {
        setResolveError(translateBackendError(t, e));
        return null;
      } finally {
        setResolving(false);
      }
    },
    [target, t],
  );

  const reconnect = useCallback(async () => {
    const r = await resolveTarget(password);
    if (r) await loadRemote(r, "");
  }, [resolveTarget, password, loadRemote]);

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void (async () => {
      const home = await invoke<string>("bwbrowser_local_files_home");
      await loadLocal(home);
      const r = await resolveTarget("");
      if (r) await loadRemote(r, "");
    })();
  }, [loadLocal, resolveTarget, loadRemote]);

  /** 目录操作返回 file_op_result（不抛错），失败时给出可读提示 */
  const applyOpResult = useCallback(
    async (promise: Promise<unknown>): Promise<boolean> => {
      try {
        const r = (await promise) as { success?: boolean; message?: string };
        if (r && r.success === false) {
          toast.error(t("fileTransfer.operationFailed"), {
            description: r.message,
          });
          return false;
        }
        return true;
      } catch (e) {
        toast.error(translateBackendError(t, e));
        return false;
      }
    },
    [t],
  );

  const upload = useCallback(async () => {
    if (!remote) return;
    if (!localSel || localSel.is_dir) {
      toast.error(t("fileTransfer.selectFileFirst"));
      return;
    }
    if (remotePane.isDrives || !remotePane.path) {
      toast.error(t("fileTransfer.selectRemoteFolder"));
      return;
    }
    setBusy("upload");
    try {
      await invoke("bwbrowser_remote_files_upload", {
        target: remote,
        localPath: childPath(local.path, localSel.name),
        remoteDir: remotePane.path,
      });
      toast.success(t("fileTransfer.uploadSuccess", { name: localSel.name }));
      await loadRemote(remote, remotePane.path);
    } catch (e) {
      toast.error(translateBackendError(t, e));
    } finally {
      setBusy(null);
    }
  }, [remote, localSel, local.path, remotePane, loadRemote, t]);

  const download = useCallback(async () => {
    if (!remote) return;
    if (!remoteSel || remoteSel.is_dir) {
      toast.error(t("fileTransfer.selectFileFirst"));
      return;
    }
    if (local.isDrives || !local.path) {
      toast.error(t("fileTransfer.selectLocalFolder"));
      return;
    }
    setBusy("download");
    try {
      const res = await invoke<{ name: string }>(
        "bwbrowser_remote_files_download",
        {
          target: remote,
          remotePath: childPath(remotePane.path, remoteSel.name),
          localDir: local.path,
        },
      );
      toast.success(t("fileTransfer.downloadSuccess", { name: res.name }));
      await loadLocal(local.path);
    } catch (e) {
      toast.error(translateBackendError(t, e));
    } finally {
      setBusy(null);
    }
  }, [remote, remoteSel, remotePane.path, local, loadLocal, t]);

  const methodLabel =
    remote?.method === "relay"
      ? t("fileTransfer.viaRelay")
      : t("fileTransfer.viaLan");

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-3 border-b border-border px-4 py-3">
        <Button
          size="sm"
          variant="ghost"
          className="h-8 px-2 text-xs"
          onClick={onClose}
        >
          <LuArrowLeft className="w-4 h-4 mr-1" />
          {t("fileTransfer.back")}
        </Button>
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm font-semibold">
            {target.hostName || remote?.hostname || target.lanHost}
          </span>
          <Badge variant="secondary">{t("fileTransfer.title")}</Badge>
          {remote && <Badge variant="outline">{methodLabel}</Badge>}
        </div>
        <div className="ml-auto flex items-center gap-2">
          {needPassword && (
            <>
              <Input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder={t("fileTransfer.password")}
                className="h-8 w-40 text-xs"
              />
              <Button
                size="sm"
                variant="outline"
                className="h-8 text-xs"
                onClick={() => void reconnect()}
                disabled={resolving}
              >
                {t("fileTransfer.connect")}
              </Button>
            </>
          )}
          {!needPassword && remote && (
            <Button
              size="sm"
              variant="ghost"
              className="h-8 px-2 text-xs"
              onClick={() => void reconnect()}
              disabled={resolving}
              title={t("fileTransfer.retry")}
            >
              <LuRefreshCw
                className={cn("w-3.5 h-3.5", resolving && "animate-spin")}
              />
            </Button>
          )}
        </div>
      </div>

      {resolveError && (
        <div className="border-b border-border bg-destructive/10 px-4 py-2 text-xs text-destructive">
          {t("fileTransfer.connectFailed")}: {resolveError}
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <FilePane
          title={t("fileTransfer.localPane")}
          icon={<LuHardDrive className="w-4 h-4" />}
          pane={local}
          selected={localSel}
          onSelect={setLocalSel}
          onOpen={(e) => {
            if (e.is_dir) void loadLocal(childPath(local.path, e.name));
          }}
          onUp={() => void loadLocal(local.parent)}
          onRefresh={() => void loadLocal(local.path)}
          onMkdir={async (name) => {
            if (
              await applyOpResult(
                invoke("bwbrowser_local_files_mkdir", {
                  path: local.path,
                  name,
                }),
              )
            ) {
              await loadLocal(local.path);
            }
          }}
          onRename={async (name, newName) => {
            if (
              await applyOpResult(
                invoke("bwbrowser_local_files_rename", {
                  path: childPath(local.path, name),
                  newName,
                }),
              )
            ) {
              await loadLocal(local.path);
            }
          }}
          onDelete={async (name) => {
            if (
              await applyOpResult(
                invoke("bwbrowser_local_files_delete", {
                  path: childPath(local.path, name),
                }),
              )
            ) {
              await loadLocal(local.path);
            }
          }}
          extraActions={
            <Button
              size="sm"
              className="h-7 text-xs"
              onClick={() => void upload()}
              disabled={
                !remote || busy !== null || !localSel || localSel.is_dir
              }
            >
              {busy === "upload" ? (
                <LuLoaderCircle className="w-3.5 h-3.5 mr-1 animate-spin" />
              ) : (
                <LuUpload className="w-3.5 h-3.5 mr-1" />
              )}
              {t("fileTransfer.upload")}
            </Button>
          }
        />

        <div className="w-px shrink-0 bg-border" />

        <FilePane
          title={t("fileTransfer.remotePane")}
          icon={<LuArrowRight className="w-4 h-4" />}
          pane={remotePane}
          selected={remoteSel}
          onSelect={setRemoteSel}
          onOpen={(e) => {
            if (e.is_dir && remote)
              void loadRemote(remote, childPath(remotePane.path, e.name));
          }}
          onUp={() => remote && void loadRemote(remote, remotePane.parent)}
          onRefresh={() => remote && void loadRemote(remote, remotePane.path)}
          onMkdir={async (name) => {
            if (!remote) return;
            if (
              await applyOpResult(
                invoke("bwbrowser_remote_files_mkdir", {
                  target: remote,
                  path: remotePane.path,
                  name,
                }),
              )
            ) {
              await loadRemote(remote, remotePane.path);
            }
          }}
          onRename={async (name, newName) => {
            if (!remote) return;
            if (
              await applyOpResult(
                invoke("bwbrowser_remote_files_rename", {
                  target: remote,
                  path: childPath(remotePane.path, name),
                  newName,
                }),
              )
            ) {
              await loadRemote(remote, remotePane.path);
            }
          }}
          onDelete={async (name) => {
            if (!remote) return;
            if (
              await applyOpResult(
                invoke("bwbrowser_remote_files_delete", {
                  target: remote,
                  path: childPath(remotePane.path, name),
                }),
              )
            ) {
              await loadRemote(remote, remotePane.path);
            }
          }}
          disabled={!remote}
          extraActions={
            <Button
              size="sm"
              variant="outline"
              className="h-7 text-xs"
              onClick={() => void download()}
              disabled={
                !remote || busy !== null || !remoteSel || remoteSel.is_dir
              }
            >
              {busy === "download" ? (
                <LuLoaderCircle className="w-3.5 h-3.5 mr-1 animate-spin" />
              ) : (
                <LuDownload className="w-3.5 h-3.5 mr-1" />
              )}
              {t("fileTransfer.download")}
            </Button>
          }
        />
      </div>
    </div>
  );
}
