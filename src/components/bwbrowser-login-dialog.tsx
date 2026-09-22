"use client";

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  LuLoader,
  LuLock,
  LuMaximize2,
  LuMinimize,
  LuMinimize2,
  LuUser,
  LuX,
} from "react-icons/lu";
import { Checkbox } from "@/components/ui/checkbox";
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
  const [isMaximized, setIsMaximized] = useState(false);
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
      try {
        await getCurrentWindow().maximize();
      } catch (e) {
        console.error("Failed to maximize window:", e);
      }
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

  useEffect(() => {
    if (!isOpen) return;
    const win = getCurrentWindow();
    win.isMaximized().then(setIsMaximized).catch(() => {});
    const unlisten = win.onResized(() => {
      win.isMaximized().then(setIsMaximized).catch(() => {});
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [isOpen]);

  const handleMinimize = useCallback(async () => {
    try {
      await getCurrentWindow().minimize();
    } catch (e) {
      console.error("Failed to minimize window:", e);
    }
  }, []);

  const handleToggleMaximize = useCallback(async () => {
    try {
      await getCurrentWindow().toggleMaximize();
    } catch (e) {
      console.error("Failed to toggle maximize:", e);
    }
  }, []);

  if (!isOpen) return null;

  const isAutoLoginMode = countdown !== null && !isLoggingIn;

  return (
    <div className="fixed inset-0 z-9999 flex flex-col bg-background">
      <div
        data-tauri-drag-region
        onDoubleClick={() => void handleToggleMaximize()}
        className="h-10 w-full shrink-0 flex items-center justify-end px-3 select-none"
      >
        <div className="absolute top-3 right-3 flex items-center gap-1">
        <button
          type="button"
          onClick={() => void handleMinimize()}
          className="cursor-pointer rounded-md p-2 text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
          title="最小化"
        >
          <LuMinimize className="w-4 h-4" />
        </button>
        <button
          type="button"
          onClick={() => void handleToggleMaximize()}
          className="cursor-pointer rounded-md p-2 text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
          title={isMaximized ? "还原" : "最大化"}
        >
          {isMaximized ? (
            <LuMinimize2 className="w-4 h-4" />
          ) : (
            <LuMaximize2 className="w-4 h-4" />
          )}
        </button>
        {!hideCloseButton && (
          <button
            type="button"
            onClick={() => void handleQuitApp()}
            className="cursor-pointer rounded-md p-2 text-muted-foreground hover:bg-destructive hover:text-destructive-foreground transition-colors"
            title="退出程序"
          >
            <LuX className="w-4 h-4" />
            <span className="sr-only">退出程序</span>
          </button>
        )}
        </div>
      </div>

      <div className="flex-1 flex items-center justify-center overflow-auto">
      {isAutoLoginMode ? (
        <div className="flex flex-col items-center justify-center px-8 py-16 min-h-[420px]">
          <img src="/logo.png" alt="BwBrowser" className="w-16 h-16 rounded-2xl mb-6 shadow-lg" />

          <h1 className="text-2xl font-bold text-foreground mb-2">
            BwBrowser
          </h1>

          <p className="text-sm text-muted-foreground mb-8">
            欢迎回来，{username}
          </p>

          <div className="flex items-center gap-2 mb-8">
            <LuLoader className="w-4 h-4 text-primary animate-spin" />
            <span className="text-sm text-primary font-medium">
              {countdown} 秒后自动登录
            </span>
          </div>

          <button
            type="button"
            onClick={handleCancelAutoLogin}
            className="px-6 py-2 text-sm text-muted-foreground border border-border rounded-lg hover:bg-muted hover:text-foreground transition-colors"
          >
            取消自动登录
          </button>
        </div>
      ) : (
        <div className="flex flex-col items-center px-8 pt-12 pb-8 w-full max-w-md">
          <img src="/logo.png" alt="BwBrowser" className="w-16 h-16 rounded-2xl mb-4 shadow-lg" />

          <h1 className="text-2xl font-bold text-foreground mb-1">
            BwBrowser
          </h1>

          <p className="text-xs text-muted-foreground mb-8">
            云端同步 · 团队协作 · 指纹隔离
          </p>

          <div className="w-full space-y-4">
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

            <button
              type="button"
              onClick={() => void handleLogin()}
              disabled={isLoggingIn || !username.trim() || !password.trim()}
              className="w-full h-10 bg-primary text-primary-foreground rounded-lg font-medium text-sm hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:pointer-events-none flex items-center justify-center gap-2"
            >
              {isLoggingIn && <LuLoader className="w-4 h-4 animate-spin" />}
              {isLoggingIn ? "登录中..." : "登录"}
            </button>

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
      </div>
    </div>
  );
}
