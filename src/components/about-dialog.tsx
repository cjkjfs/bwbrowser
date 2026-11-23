"use client";

import { invoke } from "@tauri-apps/api/core";
import { useReducedMotion } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BwbrowserSnack } from "@/components/bwbrowser-snack";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { launchBwbrowserClone } from "@/lib/bwbrowser-physics";
import { Logo } from "./icons/logo";
import { RippleButton } from "./ui/ripple";

interface AboutDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

interface SystemInfo {
  app_version: string;
  os: string;
  arch: string;
  portable: boolean;
}

// Flywheel: each click adds spin; past this speed the bwbrowser escapes the
// dialog and bounces around the window (shared physics with the rail egg).
const SPIN_PER_CLICK = 540; // deg/s
const ESCAPE_VELOCITY = 2200; // deg/s
const SPIN_FRICTION = 1.1; // fraction of velocity lost per second

export function AboutDialog({ isOpen, onClose }: AboutDialogProps) {
  const { t } = useTranslation();
  const reducedMotion = useReducedMotion();
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [logoFlown, setLogoFlown] = useState(false);
  const [snackOpen, setSnackOpen] = useState(false);

  const logoRef = useRef<HTMLButtonElement>(null);
  const rotationRef = useRef(0);
  const velocityRef = useRef(0);
  const rafRef = useRef(0);
  const lastTimeRef = useRef(0);
  const cancelLaunchRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    invoke<SystemInfo>("get_system_info")
      .then(setSystemInfo)
      .catch(() => {
        setSystemInfo(null);
      });
  }, [isOpen]);

  const stopSpin = useCallback(() => {
    if (rafRef.current) cancelAnimationFrame(rafRef.current);
    rafRef.current = 0;
    velocityRef.current = 0;
  }, []);

  const spinFrame = useCallback(
    (time: number) => {
      const el = logoRef.current;
      if (!el) {
        rafRef.current = 0;
        return;
      }
      const dt = Math.min((time - lastTimeRef.current) / 1000, 0.05);
      lastTimeRef.current = time;

      rotationRef.current += velocityRef.current * dt;
      velocityRef.current *= Math.max(0, 1 - SPIN_FRICTION * dt);
      el.style.transform = `rotate(${rotationRef.current}deg)`;

      if (velocityRef.current >= ESCAPE_VELOCITY) {
        // The flywheel wins: the bwbrowser tears loose and joins the bounce sim,
        // keeping its spin.
        stopSpin();
        setLogoFlown(true);
        cancelLaunchRef.current = launchBwbrowserClone(el, {
          initialVX: Math.random() > 0.5 ? 420 : -420,
          initialVY: -750,
          spinSpeed: ESCAPE_VELOCITY,
          onExit: () => {
            cancelLaunchRef.current = null;
          },
        });
        return;
      }

      if (velocityRef.current > 5) {
        rafRef.current = requestAnimationFrame(spinFrame);
      } else {
        rafRef.current = 0;
        velocityRef.current = 0;
      }
    },
    [stopSpin],
  );

  const handleLogoClick = useCallback(() => {
    if (reducedMotion || logoFlown) return;
    velocityRef.current += SPIN_PER_CLICK;
    if (!rafRef.current) {
      lastTimeRef.current = performance.now();
      rafRef.current = requestAnimationFrame(spinFrame);
    }
  }, [reducedMotion, logoFlown, spinFrame]);

  const resetLogo = useCallback(() => {
    stopSpin();
    cancelLaunchRef.current?.();
    cancelLaunchRef.current = null;
    rotationRef.current = 0;
    if (logoRef.current) {
      logoRef.current.style.transform = "";
      logoRef.current.style.visibility = "";
    }
    setLogoFlown(false);
  }, [stopSpin]);

  const handleClose = useCallback(() => {
    resetLogo();
    setSnackOpen(false);
    onClose();
  }, [onClose, resetLogo]);

  useEffect(() => {
    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
      cancelLaunchRef.current?.();
    };
  }, []);

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open) handleClose();
      }}
    >
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>{t("about.title")}</DialogTitle>
        </DialogHeader>

        <div className="flex flex-col items-center gap-3 py-4">
          {snackOpen ? (
            <BwbrowserSnack />
          ) : (
            <button
              ref={logoRef}
              type="button"
              data-slot="about-logo"
              aria-label={t("header.bwbrowserLogo")}
              onClick={(event) => {
                if (event.shiftKey) {
                  resetLogo();
                  setSnackOpen(true);
                } else {
                  handleLogoClick();
                }
              }}
              onKeyDown={(event) => {
                if (
                  event.shiftKey &&
                  (event.key === "Enter" || event.key === " ")
                ) {
                  event.preventDefault();
                  resetLogo();
                  setSnackOpen(true);
                }
              }}
              className="grid size-16 cursor-pointer place-items-center rounded-full bg-transparent text-foreground select-none will-change-transform"
              style={logoFlown ? { visibility: "hidden" } : undefined}
            >
              <Logo className="size-14" />
            </button>
          )}

          <div className="text-center">
            <p className="text-lg font-semibold">BW Browser</p>
            {systemInfo && (
              <>
                <p className="text-sm text-muted-foreground">
                  {t("about.version", {
                    version: systemInfo.app_version,
                  })}
                  {systemInfo.portable && (
                    <span className="ml-1.5 rounded border border-border bg-muted px-1 py-px text-[10px] align-middle">
                      {t("about.portableBadge")}
                    </span>
                  )}
                </p>
                <p className="text-xs text-muted-foreground">
                  {systemInfo.os} {systemInfo.arch}
                </p>
              </>
            )}
          </div>

          <p className="text-center text-xs text-muted-foreground">
            {t("about.licenseNotice")}
          </p>
        </div>

        <div className="flex justify-end">
          <RippleButton variant="outline" onClick={handleClose}>
            {t("common.buttons.close")}
          </RippleButton>
        </div>
      </DialogContent>
    </Dialog>
  );
}
