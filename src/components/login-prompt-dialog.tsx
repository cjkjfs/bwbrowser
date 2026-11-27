"use client";

/**
 * 登录公告弹窗（2026-11-27）。
 *
 * 数据源：权限组「登录提示」（users.login_prompt_enabled / login_prompt_text，
 * me 接口透出 login_prompt_enabled / login_prompt_text / login_prompt_version）。
 *
 * 弹出策略：enabled=1 且 text 非空才弹；localStorage 记「已读版本号」
 * （login_prompt_version = 内容 md5），内容没变不再弹，管理员改了内容
 * （版本号变化）下一次登录重新弹。每次会话最多弹一次。
 */

import { LuMegaphone } from "react-icons/lu";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";

const READ_VERSION_KEY = "bwbrowser_login_prompt_read_version";

export function shouldShowLoginPrompt(
  enabled: boolean,
  text: string,
  version: string,
): boolean {
  if (!enabled || !text.trim()) return false;
  if (!version) return true; // 老服务端无版本号：只要开着就弹
  try {
    const read = localStorage.getItem(READ_VERSION_KEY);
    return read !== version;
  } catch {
    return true;
  }
}

export function markLoginPromptRead(version: string) {
  try {
    if (version) localStorage.setItem(READ_VERSION_KEY, version);
  } catch {
    // localStorage 不可用时静默：本次会话仍然弹
  }
}

export function LoginPromptDialog({
  open,
  text,
  onClose,
}: {
  open: boolean;
  text: string;
  version: string;
  onClose: () => void;
}) {
  if (!open) return null;
  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent className="max-w-md">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-orange-100 dark:bg-orange-950">
            <LuMegaphone className="h-5 w-5 text-orange-600 dark:text-orange-400" />
          </div>
          <div className="min-w-0 flex-1">
            <DialogTitle className="text-base">公告</DialogTitle>
            <DialogDescription className="sr-only">
              管理员发布的登录公告
            </DialogDescription>
          </div>
        </div>
        <div className="mt-2 max-h-72 overflow-y-auto whitespace-pre-wrap text-sm leading-relaxed text-foreground/90">
          {text}
        </div>
        <div className="mt-4 flex justify-end">
          <Button onClick={onClose}>我知道了</Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
