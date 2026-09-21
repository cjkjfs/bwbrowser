"use client";

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useRef, useState } from "react";
import { LuCloud, LuLoader, LuLock, LuUser } from "react-icons/lu";
import { RxCross2 } from "react-icons/rx";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useBwbrowserAuth } from "@/hooks/use-bwbrowser-auth";
import { showErrorToast, showSuccessToast } from "@/lib/toast-utils";
import { cn } from "@/lib/utils";

interface BwbrowserLoginDialogProps {
  isOpen: boolean;
  onClose: (loginOccurred?: boolean) => void;
  hideCloseButton?: boolean;
}

const REMEMBER_KEY = "bwbrowser_remember_credentials";
const AUTO_LOGIN_KEY = "bwbrowser_auto_login";
const AUTO_LOGIN_COUNTDOWN = 3;

function getRememberedCredentials(): {
  username: string;
  password: string;
} | null {
  try {
    const raw = localStorage.getItem(REMEMBER_KEY);
    if (raw) return JSON.parse(raw);
  } catch {
    /* ignore */
  }
  return null;
}

function saveRememberedCredentials(username: string, password: string) {
  try {
    localStorage.setItem(REMEMBER_KEY, JSON.stringify({ username, password }));
  } catch {
    /* ignore */
  }
}

function clearRememberedCredentials() {
  try {
    localStorage.removeItem(REMEMBER_KEY);
  } catch {
    /* ignore */
  }
}

function getAutoLoginSetting(): boolean {
  try {
    return localStorage.getItem(AUTO_LOGIN_KEY) === "1";
  } catch {
    return false;
  }
}

function saveAutoLoginSetting(value: boolean) {
  try {
    localStorage.setItem(AUTO_LOGIN_KEY, value ? "1" : "0");
  } catch {
    /* ignore */
  }
}

export function BwbrowserLoginDialog({
  isOpen,
  onClose,
  hideCloseButton = false,
}: BwbrowserLoginDialogProps) {
  const { login, isLoggedIn } = useBwbrowserAuth();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [isLoggingIn, setIsLoggingIn] = useState(false);
  const [remember, setRemember] = useState(true);
  const [autoLogin, setAutoLogin] = useState(false);
  const [countdown, setCountdown] = useState<number | null>(null);
  const autoLoginStartedRef = useRef(false);
  const countdownTimerRef = useRef<number | null>(null);
  const isLoggingInRef = useRef(isLoggingIn);
  isLoggingInRef.current = isLoggingIn;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const clearCountdown = useCallback(() => {
    if (countdownTimerRef.current !== null) {
      clearInterval(countdownTimerRef.current);
      countdownTimerRef.current = null;
    }
    setCountdown(null);
  }, []);

  const doLogin = async (
    user: string,
    pass: string,
    opts?: { remember?: boolean; autoLogin?: boolean },
  ) => {
    if (isLoggingIn) return;
    clearCountdown();
    setIsLoggingIn(true);
    const shouldRemember = opts?.remember ?? remember;
    const shouldAutoLogin = opts?.autoLogin ?? autoLogin;
    try {
      await login(user, pass);
      if (shouldRemember) {
        saveRememberedCredentials(user, pass);
      } else {
        clearRememberedCredentials();
      }
      if (shouldAutoLogin && shouldRemember) {
        saveAutoLoginSetting(true);
      } else {
        saveAutoLoginSetting(false);
      }
      showSuccessToast("登录成功");
      onCloseRef.current(true);
    } catch (error) {
      console.error("Bwbrowser login failed:", error);
      showErrorToast(String(error));
    } finally {
      setIsLoggingIn(false);
    }
  };

  const doLoginRef = useRef(doLogin);
  doLoginRef.current = doLogin;

  const handleLogin = async () => {
    const trimmedUser = username.trim();
    const trimmedPass = password.trim();
    if (!trimmedUser || !trimmedPass) return;
    await doLogin(trimmedUser, trimmedPass);
  };

  const handleCancelAutoLogin = () => {
    clearCountdown();
    autoLoginStartedRef.current = false;
  };

  const handleQuitApp = useCallback(async () => {
    try {
      await invoke("confirm_quit");
    } catch (e) {
      console.error("Failed to quit app:", e);
    }
  }, []);

  const handleOpenChange = (open: boolean) => {
    if (!open) {
      if (hideCloseButton) return;
      clearCountdown();
      autoLoginStartedRef.current = false;
      onClose(false);
    }
  };

  useEffect(() => {
    if (!isOpen) {
      autoLoginStartedRef.current = false;
      clearCountdown();
      return;
    }
    if (autoLoginStartedRef.current) return;

    const remembered = getRememberedCredentials();
    const shouldAutoLogin = getAutoLoginSetting();
    setAutoLogin(shouldAutoLogin);

    if (remembered?.username && remembered.password) {
      setUsername(remembered.username);
      setPassword(remembered.password);
      setRemember(true);

      if (shouldAutoLogin && !isLoggingInRef.current) {
        autoLoginStartedRef.current = true;
        setCountdown(AUTO_LOGIN_COUNTDOWN);

        let remaining = AUTO_LOGIN_COUNTDOWN;
        countdownTimerRef.current = window.setInterval(() => {
          remaining -= 1;
          if (remaining <= 0) {
            clearCountdown();
            if (isLoggedIn) {
              onCloseRef.current(true);
            } else {
              void doLoginRef.current(
                remembered.username,
                remembered.password,
                {
                  remember: true,
                  autoLogin: true,
                },
              );
            }
          } else {
            setCountdown(remaining);
          }
        }, 1000);
      }
    } else {
      setRemember(false);
    }

    return () => {
      clearCountdown();
    };
  }, [isOpen, isLoggedIn, clearCountdown]);

  useEffect(() => {
    return () => {
      clearCountdown();
    };
  }, [clearCountdown]);

  const isAutoLoginMode = countdown !== null && !isLoggingIn;

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent
        className="max-w-md !p-0 overflow-hidden"
        hideClose
        dismissible={false}
      >
        {!hideCloseButton && (
          <button
            type="button"
            onClick={() => void handleQuitApp()}
            className="absolute top-4 right-4 z-50 cursor-pointer rounded-xs opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:outline-none disabled:pointer-events-none [&_svg]:size-4"
            title="退出程序"
          >
            <RxCross2 />
            <span className="sr-only">退出程序</span>
          </button>
        )}

        {isAutoLoginMode ? (
          /* 自动登录倒计时界面 */
          <div className="flex flex-col items-center justify-center px-8 py-16 min-h-[420px]">
            {/* 云图标 */}
            <div className="w-16 h-16 rounded-2xl bg-primary flex items-center justify-center mb-6 shadow-lg shadow-primary/30">
              <LuCloud className="w-8 h-8 text-primary-foreground" />
            </div>

            {/* 品牌名 */}
            <h1 className="text-2xl font-bold text-foreground mb-2">
              BwBrowser
            </h1>

            {/* 欢迎语 */}
            <p className="text-sm text-muted-foreground mb-8">
              欢迎回来，{username}
            </p>

            {/* 倒计时 */}
            <div className="flex items-center gap-2 mb-8">
              <LuLoader className="w-4 h-4 text-primary animate-spin" />
              <span className="text-sm text-primary font-medium">
                {countdown} 秒后自动登录
              </span>
            </div>

            {/* 取消按钮 */}
            <button
              type="button"
              onClick={handleCancelAutoLogin}
              className="px-6 py-2 text-sm text-muted-foreground border border-border rounded-lg hover:bg-muted hover:text-foreground transition-colors"
            >
              取消自动登录
            </button>
          </div>
        ) : (
          /* 登录表单界面 */
          <div className="flex flex-col items-center px-8 pt-12 pb-8">
            {/* Logo */}
            <div className="w-16 h-16 rounded-full bg-gradient-to-br from-blue-500 to-purple-600 flex items-center justify-center mb-4 shadow-lg">
              <span className="text-2xl font-bold text-white">B</span>
            </div>

            {/* 品牌名 */}
            <h1 className="text-2xl font-bold text-foreground mb-1">
              BwBrowser
            </h1>

            {/* 标语 */}
            <p className="text-xs text-muted-foreground mb-8">
              云端同步 · 团队协作 · 指纹隔离
            </p>

            {/* 表单 */}
            <div className="w-full space-y-4">
              {/* 用户名 */}
              <div className="space-y-1.5">
                <Label
                  htmlFor="bwbrowser-username"
                  className="text-xs text-muted-foreground"
                >
                  用户名
                </Label>
                <div className="relative">
                  <LuUser className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground pointer-events-none" />
                  <Input
                    id="bwbrowser-username"
                    placeholder="请输入用户名"
                    value={username}
                    onChange={(e) => {
                      setUsername(e.target.value);
                      if (countdown !== null) handleCancelAutoLogin();
                    }}
                    onKeyDown={(e) => {
                      if (
                        e.key === "Enter" &&
                        username.trim() &&
                        password.trim()
                      ) {
                        void handleLogin();
                      }
                    }}
                    autoComplete="username"
                    autoFocus={!isLoggingIn && countdown === null}
                    disabled={isLoggingIn}
                    className="pl-9"
                  />
                </div>
              </div>

              {/* 密码 */}
              <div className="space-y-1.5">
                <Label
                  htmlFor="bwbrowser-password"
                  className="text-xs text-muted-foreground"
                >
                  密码
                </Label>
                <div className="relative">
                  <LuLock className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground pointer-events-none" />
                  <Input
                    id="bwbrowser-password"
                    type="password"
                    placeholder="请输入密码"
                    value={password}
                    onChange={(e) => {
                      setPassword(e.target.value);
                      if (countdown !== null) handleCancelAutoLogin();
                    }}
                    onKeyDown={(e) => {
                      if (
                        e.key === "Enter" &&
                        username.trim() &&
                        password.trim()
                      ) {
                        void handleLogin();
                      }
                    }}
                    autoComplete="current-password"
                    disabled={isLoggingIn}
                    className="pl-9"
                  />
                </div>
              </div>

              {/* 记住密码 + 自动登录 */}
              <div className="flex items-center justify-between">
                <div className="flex items-center space-x-2">
                  <Checkbox
                    id="bwbrowser-remember"
                    checked={remember}
                    onCheckedChange={(checked) => {
                      const val = checked === true;
                      setRemember(val);
                      if (!val) setAutoLogin(false);
                      if (countdown !== null) handleCancelAutoLogin();
                    }}
                  />
                  <Label
                    htmlFor="bwbrowser-remember"
                    className="text-xs cursor-pointer"
                  >
                    记住密码
                  </Label>
                </div>
                <div className="flex items-center space-x-2">
                  <Checkbox
                    id="bwbrowser-auto-login"
                    checked={autoLogin}
                    disabled={!remember}
                    onCheckedChange={(checked) => {
                      const val = checked === true;
                      setAutoLogin(val);
                      if (val && !remember) setRemember(true);
                      if (countdown !== null) handleCancelAutoLogin();
                    }}
                  />
                  <Label
                    htmlFor="bwbrowser-auto-login"
                    className={cn(
                      "text-xs cursor-pointer",
                      !remember && "text-muted-foreground opacity-60",
                    )}
                  >
                    自动登录
                  </Label>
                </div>
              </div>

              {/* 登录按钮 */}
              <button
                type="button"
                onClick={() => void handleLogin()}
                disabled={isLoggingIn || !username.trim() || !password.trim()}
                className="w-full h-10 bg-primary text-primary-foreground rounded-lg font-medium text-sm hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:pointer-events-none flex items-center justify-center gap-2"
              >
                {isLoggingIn && <LuLoader className="w-4 h-4 animate-spin" />}
                {isLoggingIn ? "登录中..." : "登录"}
              </button>

              {/* 注册链接 */}
              <p className="text-center text-xs text-muted-foreground">
                没有账号？
                <button
                  type="button"
                  onClick={() => void openUrl("https://yacm.xin/")}
                  className="text-primary hover:underline ml-1"
                >
                  立即注册
                </button>
              </p>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
