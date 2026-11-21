"use client";

/**
 * 全局确认 / 提示对话框服务。
 *
 * 目标：替换 @tauri-apps/plugin-dialog 的系统原生 confirm/alert（Windows 灰白
 * 系统框，与应用 UI 不搭）以及 WebView2 裸 confirm()/alert()（样式丑且可能
 * 被吞）。全部走项目现有 shadcn Dialog 原语——动效、主题、圆角与主界面一致。
 *
 * 用法（与 plugin-dialog 的 confirm 保持近似签名，替换成本低）：
 *   import { confirmDialog, alertDialog } from "@/components/ui/confirm-dialog";
 *   const ok = await confirmDialog("确定删除？", { kind: "danger", okLabel: "删除" });
 *   await alertDialog(`导出成功:\n${path}`);
 *
 * kind 语义：info（主色图标，一般操作）→ warning（琥珀图标，注意类）→
 * danger（红色图标 + destructive 确认按钮，删除/清空类）。
 *
 * ConfirmDialogProvider 挂在 ClientProviders（全局一次）；模块级 confirmDialog/
 * alertDialog 通过 bridge 转发到 Provider 内的队列，同一时刻只显示一个，
 * 后续请求自动排队。
 */

import {
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { LuInfo, LuTriangleAlert } from "react-icons/lu";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

export type ConfirmDialogKind = "info" | "warning" | "danger";

export interface ConfirmDialogOptions {
  /** 标题栏文字；不传时 confirm 用「请确认」、alert 用「提示」（i18n）。 */
  title?: string;
  /** 确认按钮文字；默认 common.buttons.confirm。 */
  okLabel?: string;
  /** 取消按钮文字；默认 common.buttons.cancel。alertDialog 无取消按钮。 */
  cancelLabel?: string;
  /** 图标与确认按钮风格：info（主色）、warning（琥珀）、danger（红+destructive 按钮）。 */
  kind?: ConfirmDialogKind;
}

interface ConfirmDialogState {
  mode: "confirm" | "alert";
  title: string;
  body: string;
  okLabel: string;
  cancelLabel: string;
  kind: ConfirmDialogKind;
}

type Mode = "confirm" | "alert";

type BridgeRequestFn = (
  mode: Mode,
  body: string,
  options?: ConfirmDialogOptions,
) => Promise<boolean>;

/** Provider 挂载后注册的实际执行函数；未挂载时 confirmDialog 兜底返回 false。 */
let bridgeRequest: BridgeRequestFn | null = null;

/** 当前对话框的 resolve 函数；null = 无待处理对话框。 */
let activeResolve: ((ok: boolean) => void) | null = null;

/** 确认对话框：resolve(true)=确认、resolve(false)=取消/Esc/点外部。 */
export function confirmDialog(
  body: string,
  options?: ConfirmDialogOptions,
): Promise<boolean> {
  if (!bridgeRequest) return Promise.resolve(false);
  return bridgeRequest("confirm", body, options);
}

/** 提示对话框（单个确定按钮），resolve 后返回。 */
export function alertDialog(
  body: string,
  options?: ConfirmDialogOptions,
): Promise<void> {
  if (!bridgeRequest) return Promise.resolve();
  return bridgeRequest("alert", body, options).then(() => undefined);
}

export function ConfirmDialogProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ConfirmDialogState | null>(null);
  const busyRef = useRef(false);
  const queueRef = useRef<Array<() => void>>([]);
  const { t } = useTranslation();

  const pump = useCallback(() => {
    if (busyRef.current) return;
    const next = queueRef.current.shift();
    if (next) next();
  }, []);

  useEffect(() => {
    const request: BridgeRequestFn = (mode, body, options) =>
      new Promise<boolean>((resolve) => {
        const start = () => {
          busyRef.current = true;
          const resolveFn = (ok: boolean) => {
            if (activeResolve !== resolveFn) return;
            activeResolve = null;
            busyRef.current = false;
            setState(null);
            resolve(ok);
            // 关闭动效播放期间放行下一个请求，避免肉眼可见的排队延迟。
            setTimeout(pump, 120);
          };
          activeResolve = resolveFn;
          setState({
            mode,
            body,
            kind: options?.kind ?? (mode === "alert" ? "info" : "warning"),
            title:
              options?.title ??
              (mode === "alert"
                ? t("confirmDialog.infoTitle")
                : t("confirmDialog.confirmTitle")),
            okLabel: options?.okLabel ?? t("common.buttons.confirm"),
            cancelLabel: options?.cancelLabel ?? t("common.buttons.cancel"),
          });
        };
        if (busyRef.current) {
          queueRef.current.push(start);
        } else {
          start();
        }
      });

    bridgeRequest = request;
    return () => {
      bridgeRequest = null;
    };
  }, [pump, t]);

  return (
    <>
      {children}
      {state && <ConfirmDialogHost state={state} />}
    </>
  );
}

function ConfirmDialogHost({ state }: { state: ConfirmDialogState }) {
  const settle = (ok: boolean) => {
    activeResolve?.(ok);
  };

  const isDanger = state.kind === "danger";
  const isWarning = state.kind === "warning";
  const isAlert = state.mode === "alert";

  const Icon = isDanger || isWarning ? LuTriangleAlert : LuInfo;
  const iconClasses = isDanger
    ? "text-destructive"
    : isWarning
      ? "text-amber-500"
      : "text-primary";

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        // Esc / 点击遮罩 = 取消（alert 模式则视为关闭）。
        if (!open) settle(isAlert);
      }}
    >
      <DialogContent className="sm:max-w-md" hideClose={isAlert}>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Icon className={cn("size-5 shrink-0", iconClasses)} />
            {state.title}
          </DialogTitle>
          <DialogDescription className="whitespace-pre-line pt-1">
            {state.body}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          {!isAlert && (
            <Button
              type="button"
              variant="outline"
              onClick={() => settle(false)}
            >
              {state.cancelLabel}
            </Button>
          )}
          <Button
            type="button"
            variant={isDanger ? "destructive" : "default"}
            onClick={() => settle(!isAlert)}
          >
            {state.okLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
