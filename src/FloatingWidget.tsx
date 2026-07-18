import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  currentMonitor,
  cursorPosition,
  getCurrentWindow,
  LogicalSize,
} from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { invokeBackend } from "./lib/platform";
import type { AccountInfo, AppSettings, UsageInfo } from "./types";

const REFRESH_MS = 60_000;
const FLOATING_HORIZONTAL_PADDING = 16;
const FLOATING_CARD_VERTICAL_PADDING = 24;
const FLOATING_BASE_CONTENT_WIDTH = 284;
const FLOATING_MIN_HEIGHT = 110;
const FLOATING_MIN_WIDTH = 180;
const FLOATING_DEFAULT_WIDTH = 300;
const FLOATING_DEFAULT_HEIGHT = 184;
const FLOATING_COMPACT_SIZE = 48;
const FLOATING_FULL_INSET = 8;
const COMPACT_EXPAND_DELAY_MS = 120;
const COMPACT_COLLAPSE_DELAY_MS = 300;
const PRESENTATION_TRANSITION_MS = 260;
const floatingWindow = getCurrentWindow();

type PresentationPhase = "compact" | "expanding" | "expanded" | "collapsing";
type TransitionAnchor = { right: boolean; bottom: boolean };

function remaining(used: number | null): number | null {
  return used == null || !Number.isFinite(used) ? null : Math.round(Math.max(0, Math.min(100, 100 - used)));
}

function usageTextTone(value: number | null): string {
  if (value == null) return "text-slate-300";
  if (value <= 10) return "text-rose-400";
  if (value <= 25) return "text-amber-300";
  return "text-emerald-300";
}

function resetLabel(timestamp: number, t: TFunction): string {
  const totalSeconds = Math.floor((timestamp * 1000 - Date.now()) / 1000);
  if (totalSeconds < 60) return t("usage.now");

  const days = Math.floor(totalSeconds / (24 * 60 * 60));
  const hours = Math.floor((totalSeconds % (24 * 60 * 60)) / (60 * 60));
  const minutes = Math.floor((totalSeconds % (60 * 60)) / 60);
  const parts: string[] = [];

  if (days > 0) parts.push(t("usage.days", { count: days }));
  if (hours > 0 && parts.length < 2) parts.push(t("usage.hours", { count: hours }));
  if (minutes > 0 && parts.length < 2) parts.push(t("usage.minutes", { count: minutes }));

  return parts.length > 0 ? parts.join(" ") : t("usage.now");
}

function exactResetParts(timestamp: number, locale: string): { date: string; weekday: string; time: string } {
  const value = new Date(timestamp * 1000);
  const date = new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "long",
    day: "numeric",
  }).format(value);
  const weekday = new Intl.DateTimeFormat(locale, {
    weekday: "short",
  }).format(value);
  const time = new Intl.DateTimeFormat(locale, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(value);
  return { date, weekday, time };
}

function UsageRow({ label, value }: { label: string; value: number | null }) {
  const tone = value == null ? "bg-slate-500" : value <= 10 ? "bg-rose-500" : value <= 25 ? "bg-amber-400" : "bg-emerald-400";
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between text-[12px]">
        <span className="text-slate-300">{label}</span>
        <span className="font-mono font-semibold text-white">{value == null ? "--" : `${value}%`}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-white/10">
        <div className={`h-full rounded-full transition-all ${tone}`} style={{ width: `${value ?? 0}%` }} />
      </div>
    </div>
  );
}

export default function FloatingWidget() {
  const { t, i18n } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [account, setAccount] = useState<AccountInfo | null>(null);
  const [usage, setUsage] = useState<UsageInfo | null>(null);
  const [usageReady, setUsageReady] = useState(false);
  const [offline, setOffline] = useState(false);
  const [controlTooltip, setControlTooltip] = useState<"top" | "compact" | "through" | "close" | null>(null);
  const [resetTooltipVisible, setResetTooltipVisible] = useState(false);
  const [previewExpanded, setPreviewExpanded] = useState(false);
  const [presentationPhase, setPresentationPhase] = useState<PresentationPhase>(() =>
    window.innerWidth <= FLOATING_COMPACT_SIZE + 2 ? "compact" : "expanded"
  );
  const [transitionProgress, setTransitionProgress] = useState(() =>
    window.innerWidth <= FLOATING_COMPACT_SIZE + 2 ? 0 : 1
  );
  const [transitionAnchor, setTransitionAnchor] = useState<TransitionAnchor>({ right: false, bottom: false });
  const [viewport, setViewport] = useState(() => ({ width: window.innerWidth, height: window.innerHeight }));
  const dragTimer = useRef<number | null>(null);
  const expandTimer = useRef<number | null>(null);
  const collapseTimer = useRef<number | null>(null);
  const pointerInside = useRef(false);
  const dragging = useRef(false);
  const hoverArmed = useRef(true);
  const waitForPhysicalLeave = useRef(false);
  const resizeGeneration = useRef(0);
  const windowPresentation = useRef<"compact" | "expanded" | null>(null);
  const presentationTarget = useRef<"compact" | "expanded" | null>(null);
  const expandedSize = useRef<[number, number]>([FLOATING_DEFAULT_WIDTH, FLOATING_DEFAULT_HEIGHT]);
  const contentRef = useRef<HTMLDivElement | null>(null);

  const refresh = useCallback(async () => {
    setUsageReady(false);
    try {
      const active = await invokeBackend<AccountInfo | null>("get_active_account_info");
      setAccount(active);
      if (!active) { setUsage(null); setOffline(false); return; }
      if (active.auth_mode === "api_key") {
        setUsage(null);
        setOffline(false);
        return;
      }
      const next = await invokeBackend<UsageInfo>("get_usage", { accountId: active.id });
      setOffline(Boolean(next.error));
      if (!next.error) setUsage(next);
    } catch {
      setOffline(true);
    } finally {
      setUsageReady(true);
    }
  }, []);

  useEffect(() => {
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    void refresh();
    const timer = window.setInterval(() => void refresh(), REFRESH_MS);
    const unlistenSettings = listen<AppSettings>("floating-settings-changed", ({ payload }) => setSettings(payload));
    const unlistenAppSettings = listen<AppSettings>("settings-changed", ({ payload }) => setSettings(payload));
    const unlistenAccounts = listen("accounts-changed", () => void refresh());
    return () => {
      window.clearInterval(timer);
      if (expandTimer.current !== null) window.clearTimeout(expandTimer.current);
      if (collapseTimer.current !== null) window.clearTimeout(collapseTimer.current);
      void unlistenSettings.then((fn) => fn());
      void unlistenAppSettings.then((fn) => fn());
      void unlistenAccounts.then((fn) => fn());
    };
  }, [refresh]);

  useEffect(() => {
    const updateViewport = () => setViewport({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener("resize", updateViewport);
    return () => window.removeEventListener("resize", updateViewport);
  }, []);

  const fields = useMemo(() => new Set(settings?.floating.visible_fields ?? []), [settings]);
  const primary = remaining(usage?.primary_used_percent ?? null);
  const secondary = remaining(usage?.secondary_used_percent ?? null);
  const hasPrimaryWindow = Boolean(
    usage &&
      (usage.primary_used_percent != null ||
        usage.primary_window_minutes != null ||
        usage.primary_resets_at != null)
  );
  const hasSecondaryWindow = Boolean(
    usage &&
      (usage.secondary_used_percent != null ||
        usage.secondary_window_minutes != null ||
        usage.secondary_resets_at != null)
  );
  const activeReset = hasPrimaryWindow
    ? usage?.primary_resets_at != null
      ? { kind: "session" as const, timestamp: usage.primary_resets_at }
      : null
    : hasSecondaryWindow && usage?.secondary_resets_at != null
      ? { kind: "weekly" as const, timestamp: usage.secondary_resets_at }
      : null;
  const showPrimaryUsage = fields.has("primary_usage") && primary !== null;
  const showSecondaryUsage = fields.has("secondary_usage") && secondary !== null;
  const showActiveReset = fields.has("primary_reset") && activeReset !== null;
  const exactReset = activeReset
    ? exactResetParts(activeReset.timestamp, i18n.resolvedLanguage ?? "en-US")
    : null;
  const compactMode = Boolean(settings?.floating.compact_mode);
  const clickThrough = Boolean(settings?.floating.click_through);
  const showCompactView = compactMode && !previewExpanded;
  const prefersReducedMotion = useMemo(
    () => window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    []
  );
  const compactUsage = primary !== null
    ? { value: primary, label: t("usage.compactFiveHour", { percent: `${primary}%` }) }
    : secondary !== null
      ? { value: secondary, label: t("usage.compactWeekly", { percent: `${secondary}%` }) }
      : { value: null, label: t("usage.compactUnavailable") };
  const contentScale = Math.max(
    0.5,
    Math.min(2.5, (viewport.width - FLOATING_HORIZONTAL_PADDING) / FLOATING_BASE_CONTENT_WIDTH)
  );
  const isApiKeyAccount = account?.auth_mode === "api_key";
  const planKey = account?.plan_type?.toLowerCase() ?? (isApiKeyAccount ? "api_key" : "free");
  const planDisplay = account?.plan_type
    ? account.plan_type.charAt(0).toUpperCase() + account.plan_type.slice(1)
    : account?.auth_mode === "api_key"
      ? t("accountCard.apiKey")
      : null;
  const planColors: Record<string, string> = {
    pro: "bg-indigo-400/15 text-indigo-300 border-indigo-400/30",
    plus: "bg-emerald-400/15 text-emerald-300 border-emerald-400/30",
    team: "bg-blue-400/15 text-blue-300 border-blue-400/30",
    enterprise: "bg-amber-400/15 text-amber-300 border-amber-400/30",
    free: "bg-slate-400/15 text-slate-300 border-slate-400/30",
    api_key: "bg-orange-400/15 text-orange-300 border-orange-400/30",
  };

  const resizeWindowAnchored = useCallback(async (
    width: number,
    height: number,
    minWidth: number,
    minHeight: number,
    duration = 0,
    onPrepared?: (anchor: TransitionAnchor, startWidth: number, startHeight: number) => void,
    onFrame?: (width: number, height: number) => void,
    insetForSize?: (width: number, height: number) => number
  ): Promise<boolean> => {
    const generation = ++resizeGeneration.current;
    try {
      const [oldPosition, oldSize, monitor, scaleFactor] = await Promise.all([
        floatingWindow.outerPosition(),
        floatingWindow.outerSize(),
        currentMonitor(),
        floatingWindow.scaleFactor(),
      ]);
      if (generation !== resizeGeneration.current) return false;

      const work = monitor?.workArea;
      const workRight = work ? work.position.x + work.size.width : 0;
      const workBottom = work ? work.position.y + work.size.height : 0;
      const anchorRight = Boolean(
        work &&
          Math.abs(workRight - (oldPosition.x + oldSize.width)) <
            Math.abs(oldPosition.x - work.position.x)
      );
      const anchorBottom = Boolean(
        work &&
          Math.abs(workBottom - (oldPosition.y + oldSize.height)) <
            Math.abs(oldPosition.y - work.position.y)
      );
      const anchor = { right: anchorRight, bottom: anchorBottom };
      const startWidth = oldSize.width / scaleFactor;
      const startHeight = oldSize.height / scaleFactor;
      const startInset = (insetForSize?.(startWidth, startHeight) ?? 0) * scaleFactor;
      const fixedLeft = oldPosition.x + startInset;
      const fixedTop = oldPosition.y + startInset;
      const fixedRight = oldPosition.x + oldSize.width - startInset;
      const fixedBottom = oldPosition.y + oldSize.height - startInset;

      const placeAtSize = async (logicalWidth: number, logicalHeight: number) => {
        const physicalWidth = Math.round(logicalWidth * scaleFactor);
        const physicalHeight = Math.round(logicalHeight * scaleFactor);
        const inset = (insetForSize?.(logicalWidth, logicalHeight) ?? 0) * scaleFactor;
        let x = anchorRight ? fixedRight + inset - physicalWidth : fixedLeft - inset;
        let y = anchorBottom ? fixedBottom + inset - physicalHeight : fixedTop - inset;
        if (work) {
          const maxX = Math.max(work.position.x, workRight - physicalWidth);
          const maxY = Math.max(work.position.y, workBottom - physicalHeight);
          x = Math.min(maxX, Math.max(work.position.x, x));
          y = Math.min(maxY, Math.max(work.position.y, y));
        }
        await invokeBackend<void>("set_floating_bounds", {
          x: Math.round(x),
          y: Math.round(y),
          width: physicalWidth,
          height: physicalHeight,
        });
        return generation === resizeGeneration.current;
      };

      if (duration <= 0) {
        await floatingWindow.setMinSize(new LogicalSize(minWidth, minHeight));
        if (generation !== resizeGeneration.current) return false;
        if (!await placeAtSize(width, height)) return false;
      } else {
        const shrinking = width < startWidth || height < startHeight;
        if (shrinking) {
          await floatingWindow.setMinSize(new LogicalSize(minWidth, minHeight));
          if (generation !== resizeGeneration.current) return false;
        }
        onPrepared?.(anchor, startWidth, startHeight);
        await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
        const startedAt = performance.now();

        while (generation === resizeGeneration.current) {
          const progress = Math.min(1, (performance.now() - startedAt) / duration);
          const eased = progress < 0.5
            ? 4 * progress * progress * progress
            : 1 - Math.pow(-2 * progress + 2, 3) / 2;
          const nextWidth = startWidth + (width - startWidth) * eased;
          const nextHeight = startHeight + (height - startHeight) * eased;
          onFrame?.(nextWidth, nextHeight);
          if (!await placeAtSize(nextWidth, nextHeight)) return false;
          if (progress >= 1) break;
          await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
        }
        if (generation !== resizeGeneration.current) return false;
        await floatingWindow.setMinSize(new LogicalSize(minWidth, minHeight));
      }

      return generation === resizeGeneration.current;
    } catch (error) {
      console.error("Failed to resize floating window:", error);
      return false;
    }
  }, []);

  useEffect(() => {
    const savedSize = settings?.floating.size;
    if (!savedSize) return;
    expandedSize.current = [
      Math.max(FLOATING_MIN_WIDTH, savedSize[0]),
      Math.max(FLOATING_MIN_HEIGHT, savedSize[1]),
    ];
  }, [settings?.floating.size]);

  useEffect(() => {
    if (!settings) return;
    if ((!compactMode || clickThrough) && previewExpanded) setPreviewExpanded(false);
  }, [clickThrough, compactMode, previewExpanded, settings]);

  useEffect(() => {
    if (!settings) return;
    const presentation = showCompactView ? "compact" : "expanded";
    if (windowPresentation.current === presentation) return;
    windowPresentation.current = presentation;
    presentationTarget.current = presentation;

    const initialPresentation = presentationPhase === presentation && (
      (presentation === "compact" && viewport.width <= FLOATING_COMPACT_SIZE + 2) ||
      (presentation === "expanded" && viewport.width >= FLOATING_MIN_WIDTH)
    );
    const duration = prefersReducedMotion || initialPresentation ? 0 : PRESENTATION_TRANSITION_MS;
    const transition = async () => {
      const [savedWidth, savedHeight] = expandedSize.current;
      const expandedTarget = {
        width: Math.max(FLOATING_MIN_WIDTH, savedWidth),
        height: Math.max(FLOATING_MIN_HEIGHT, savedHeight),
        minWidth: FLOATING_MIN_WIDTH,
        minHeight: FLOATING_MIN_HEIGHT,
      };
      const target = presentation === "compact"
        ? {
            width: FLOATING_COMPACT_SIZE,
            height: FLOATING_COMPACT_SIZE,
            minWidth: FLOATING_COMPACT_SIZE,
            minHeight: FLOATING_COMPACT_SIZE,
          }
        : expandedTarget;
      const progressForSize = (width: number, height: number) => {
        const widthProgress = (width - FLOATING_COMPACT_SIZE) /
          (expandedTarget.width - FLOATING_COMPACT_SIZE);
        const heightProgress = (height - FLOATING_COMPACT_SIZE) /
          (expandedTarget.height - FLOATING_COMPACT_SIZE);
        return Math.max(0, Math.min(1, (widthProgress + heightProgress) / 2));
      };

      if (duration <= 0) {
        setTransitionProgress(presentation === "compact" ? 0 : 1);
        setPresentationPhase(presentation);
      }
      const completed = await resizeWindowAnchored(
        target.width,
        target.height,
        target.minWidth,
        target.minHeight,
        duration,
        duration > 0
          ? (anchor, startWidth, startHeight) => {
              setTransitionAnchor(anchor);
              setTransitionProgress(progressForSize(startWidth, startHeight));
              setPresentationPhase(presentation === "compact" ? "collapsing" : "expanding");
            }
          : undefined,
        duration > 0
          ? (width, height) => setTransitionProgress(progressForSize(width, height))
          : undefined,
        (width, height) => FLOATING_FULL_INSET * progressForSize(width, height)
      );
      if (completed && presentationTarget.current === presentation) {
        setTransitionProgress(presentation === "compact" ? 0 : 1);
        setPresentationPhase(presentation);
      }
    };
    void transition();
  }, [
    prefersReducedMotion,
    presentationPhase,
    resizeWindowAnchored,
    settings,
    showCompactView,
    viewport.width,
  ]);

  useLayoutEffect(() => {
    const content = contentRef.current;
    if (
      !content ||
      !settings ||
      !usageReady ||
      presentationPhase !== "expanded" ||
      viewport.width < FLOATING_MIN_WIDTH
    ) return;

    const desiredHeight = Math.max(
      FLOATING_MIN_HEIGHT,
      Math.ceil(
        FLOATING_HORIZONTAL_PADDING +
          FLOATING_CARD_VERTICAL_PADDING +
          content.scrollHeight * contentScale
      )
    );
    if (Math.abs(viewport.height - desiredHeight) <= 1) return;

    void resizeWindowAnchored(
      viewport.width,
      desiredHeight,
      FLOATING_MIN_WIDTH,
      FLOATING_MIN_HEIGHT
    );
  }, [
    activeReset?.kind,
    contentScale,
    fields,
    isApiKeyAccount,
    resizeWindowAnchored,
    showActiveReset,
    presentationPhase,
    showPrimaryUsage,
    showSecondaryUsage,
    settings,
    t,
    usageReady,
    viewport.height,
    viewport.width,
  ]);

  const hide = async () => {
    if (settings) {
      await updateFloating({ visible: false });
    } else {
      await floatingWindow.hide();
    }
  };
  const updateFloating = async (patch: Partial<AppSettings["floating"]>) => {
    const current = await invokeBackend<AppSettings>("get_app_settings");
    const floating = { ...current.floating, ...patch };
    if (patch.compact_mode === true) floating.click_through = false;
    else if (patch.click_through === true) floating.compact_mode = false;
    const next = { ...current, floating };
    const saved = await invokeBackend<AppSettings>("set_app_settings", { settings: next });
    setSettings(saved);
    return saved;
  };
  const topmostActionLabel = settings?.floating.always_on_top
    ? t("settings.unpin")
    : t("settings.alwaysOnTop");
  const compactActionLabel = compactMode
    ? t("settings.disableCompactMode")
    : t("settings.enableCompactMode");

  const clearExpandTimer = () => {
    if (expandTimer.current !== null) window.clearTimeout(expandTimer.current);
    expandTimer.current = null;
  };
  const clearCollapseTimer = () => {
    if (collapseTimer.current !== null) window.clearTimeout(collapseTimer.current);
    collapseTimer.current = null;
  };
  const scheduleCollapse = () => {
    clearCollapseTimer();
    if (!compactMode || dragging.current) return;
    collapseTimer.current = window.setTimeout(() => {
      collapseTimer.current = null;
      if (!pointerInside.current && !dragging.current) setPreviewExpanded(false);
    }, COMPACT_COLLAPSE_DELAY_MS);
  };
  const confirmPhysicalLeave = async () => {
    try {
      const [cursor, position, size] = await Promise.all([
        cursorPosition(),
        floatingWindow.outerPosition(),
        floatingWindow.outerSize(),
      ]);
      const outside =
        cursor.x < position.x ||
        cursor.x >= position.x + size.width ||
        cursor.y < position.y ||
        cursor.y >= position.y + size.height;
      if (!outside) return;
    } catch {
      // If cursor geometry is unavailable, a native mouse-leave is the best signal available.
    }
    waitForPhysicalLeave.current = false;
    hoverArmed.current = true;
  };
  const handleMouseEnter = () => {
    pointerInside.current = true;
    clearCollapseTimer();
    if (compactMode && presentationPhase === "collapsing") {
      clearExpandTimer();
      setPreviewExpanded(true);
      return;
    }
    if (
      !compactMode ||
      clickThrough ||
      previewExpanded ||
      waitForPhysicalLeave.current ||
      !hoverArmed.current
    ) return;
    clearExpandTimer();
    expandTimer.current = window.setTimeout(() => {
      expandTimer.current = null;
      if (pointerInside.current && hoverArmed.current && !waitForPhysicalLeave.current) {
        setPreviewExpanded(true);
      }
    }, COMPACT_EXPAND_DELAY_MS);
  };
  const handleMouseLeave = () => {
    pointerInside.current = false;
    clearExpandTimer();
    if (waitForPhysicalLeave.current) {
      void confirmPhysicalLeave();
      return;
    }
    hoverArmed.current = true;
    if (previewExpanded) scheduleCollapse();
  };
  const toggleCompactMode = async () => {
    clearExpandTimer();
    clearCollapseTimer();
    setPreviewExpanded(false);
    if (!compactMode) {
      hoverArmed.current = false;
      waitForPhysicalLeave.current = true;
    } else {
      hoverArmed.current = false;
      waitForPhysicalLeave.current = false;
    }
    await updateFloating({ compact_mode: !compactMode });
  };
  const cancelLongPress = () => {
    if (dragTimer.current !== null) window.clearTimeout(dragTimer.current);
    dragTimer.current = null;
  };
  const beginDragging = async () => {
    dragging.current = true;
    clearCollapseTimer();
    try {
      await floatingWindow.startDragging();
    } finally {
      dragging.current = false;
      if (!pointerInside.current) scheduleCollapse();
    }
  };
  const startLongPress = (event: React.PointerEvent<HTMLDivElement>) => {
    if (settings?.floating.click_through || (event.target as HTMLElement).closest("button")) return;
    cancelLongPress();
    dragTimer.current = window.setTimeout(() => {
      dragTimer.current = null;
      void beginDragging();
    }, 20);
  };

  const compactText = compactUsage.value == null ? "--" : `${compactUsage.value}%`;
  const crossfadePosition = Math.max(0, Math.min(1, (transitionProgress - 0.18) / 0.52));
  const expandedOpacity = crossfadePosition * crossfadePosition * (3 - 2 * crossfadePosition);
  const compactOpacity = 1 - expandedOpacity;
  const transitionInset = FLOATING_FULL_INSET * transitionProgress;
  const transitionRadius = 14 + 6 * transitionProgress;
  const transitionBackgroundOpacity = 0.95 - 0.05 * transitionProgress;
  const transitionBorderOpacity = 0.1 * (1 - transitionProgress);

  if (presentationPhase === "compact") {
    return (
      <div
        className="h-full w-full"
        style={{ opacity: settings?.floating.opacity ?? 0.92 }}
        onMouseEnter={handleMouseEnter}
        onMouseLeave={handleMouseLeave}
      >
        <div
          role="img"
          aria-label={compactUsage.label}
          className={`flex h-full w-full select-none items-center justify-center overflow-hidden rounded-[14px] border border-white/10 bg-slate-950/95 font-mono text-[13px] font-bold tracking-tight ${usageTextTone(compactUsage.value)} cursor-move`}
          onPointerDown={startLongPress}
          onPointerUp={cancelLongPress}
          onPointerCancel={cancelLongPress}
          onPointerLeave={cancelLongPress}
        >
          {compactText}
        </div>
      </div>
    );
  }

  return (
    <div
      className={`relative h-full w-full p-2 ${presentationPhase === "expanding" || presentationPhase === "collapsing" ? "overflow-hidden rounded-[14px]" : ""}`}
      style={{ opacity: settings?.floating.opacity ?? 0.92 }}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      {(presentationPhase === "expanding" || presentationPhase === "collapsing") && (
        <div
          aria-hidden="true"
          className="pointer-events-none absolute"
          style={{
            inset: transitionInset,
            border: `1px solid rgb(255 255 255 / ${transitionBorderOpacity})`,
            borderRadius: transitionRadius,
            backgroundColor: `rgb(2 6 23 / ${transitionBackgroundOpacity})`,
          }}
        />
      )}
      <div
        className={`relative h-full select-none overflow-hidden rounded-[20px] px-4 py-3 text-white ${presentationPhase === "expanded" ? "bg-slate-950/90 backdrop-blur-xl" : "bg-transparent"} ${settings?.floating.click_through ? "" : "cursor-move"}`}
        style={presentationPhase === "expanded" ? undefined : { opacity: expandedOpacity }}
        onPointerDown={startLongPress}
        onPointerUp={cancelLongPress}
        onPointerCancel={cancelLongPress}
        onPointerLeave={cancelLongPress}
      >
        <div ref={contentRef} className="origin-top-left" style={{ width: `${100 / contentScale}%`, transform: `scale(${contentScale})` }}>
        <div className="mb-3 flex h-7 items-center justify-between pr-32">
          <div className="flex min-w-0 items-center gap-2">
            <span className={`h-2 w-2 shrink-0 rounded-full ${isApiKeyAccount ? "bg-orange-400" : offline ? "bg-rose-400" : "bg-emerald-400"}`} />
            {fields.has("account") && <span className="truncate text-sm font-semibold">{account?.name ?? "Codex"}</span>}
            {fields.has("account") && planDisplay && <span className={`shrink-0 rounded-full border px-2 py-0.5 text-[10px] font-medium ${planColors[planKey] ?? planColors.free}`}>{planDisplay}</span>}
          </div>
        </div>
        {isApiKeyAccount ? (
          <div className="rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-[11px] leading-4 text-slate-400">
            {t("usage.apiKeyManagedExternally")}
          </div>
        ) : (
          <div className="space-y-3">
            {showPrimaryUsage && <UsageRow label={t("usage.fiveHour")} value={primary} />}
            {showSecondaryUsage && <UsageRow label={t("usage.weekly")} value={secondary} />}
            {showActiveReset && activeReset && (
              <div className="relative flex justify-end" onMouseLeave={() => setResetTooltipVisible(false)}>
                <button
                  type="button"
                  aria-describedby={resetTooltipVisible ? "floating-reset-tooltip" : undefined}
                  aria-expanded={resetTooltipVisible}
                  className="flex cursor-help items-center gap-1.5 rounded-lg border border-transparent px-2 py-1 text-right text-[11px] text-slate-400 outline-none transition-all hover:border-white/10 hover:bg-white/[0.06] hover:text-slate-200 focus-visible:border-sky-400/40 focus-visible:bg-white/[0.06] focus-visible:text-slate-200"
                  onMouseEnter={() => setResetTooltipVisible(true)}
                  onFocus={() => setResetTooltipVisible(true)}
                  onBlur={() => setResetTooltipVisible(false)}
                >
                  <ClockIcon />
                  {t(
                    activeReset.kind === "session"
                      ? "usage.sessionResetCountdown"
                      : "usage.weeklyResetCountdown",
                    { time: resetLabel(activeReset.timestamp, t) }
                  )}
                </button>
                {resetTooltipVisible && exactReset && (
                  <div
                    id="floating-reset-tooltip"
                    role="tooltip"
                    className="pointer-events-none absolute bottom-full right-0 z-30 mb-1.5 w-[198px] max-w-[calc(100vw-32px)] origin-bottom-right animate-[tooltip-in_120ms_ease-out] rounded-[10px] border border-sky-400/20 bg-slate-950/95 px-2.5 py-2 text-left shadow-[0_12px_32px_rgba(0,0,0,0.5)] ring-1 ring-black/30 backdrop-blur-xl"
                  >
                    <div className="flex items-center gap-1.5 text-[10px] font-medium tracking-[0.08em] text-sky-300">
                      <CalendarIcon />
                      {t("usage.exactReset")}
                    </div>
                    <div className="mt-1.5 flex items-center justify-between gap-2">
                      <div className="min-w-0 whitespace-nowrap text-[11px] font-semibold leading-4 text-white">
                        {exactReset.date}
                        <span className="ml-1.5 font-normal text-slate-400">{exactReset.weekday}</span>
                      </div>
                      <span className="shrink-0 rounded-md border border-sky-400/15 bg-sky-400/10 px-1.5 py-0.5 font-mono text-[11px] font-semibold leading-4 text-sky-200">
                        {exactReset.time}
                      </span>
                    </div>
                    <span className="absolute -bottom-1 right-5 h-2 w-2 rotate-45 border-b border-r border-sky-400/20 bg-slate-950" />
                  </div>
                )}
              </div>
            )}
          </div>
        )}
        </div>
        {!settings?.floating.click_through && <div className="absolute right-4 top-3 flex items-center gap-1">
          <button aria-label={topmostActionLabel} onMouseEnter={() => setControlTooltip("top")} onMouseLeave={() => setControlTooltip(null)} className={`flex h-7 w-7 items-center justify-center rounded-lg transition-colors hover:bg-white/10 ${settings?.floating.always_on_top ? "text-emerald-300" : "text-slate-400 hover:text-white"}`} onClick={() => void updateFloating({ always_on_top: !settings?.floating.always_on_top })}><PinIcon /></button>
          <button aria-label={compactActionLabel} onMouseEnter={() => setControlTooltip("compact")} onMouseLeave={() => setControlTooltip(null)} className={`flex h-7 w-7 items-center justify-center rounded-lg transition-colors hover:bg-white/10 ${compactMode ? "text-sky-300" : "text-slate-400 hover:text-white"}`} onClick={() => void toggleCompactMode()}><CompactIcon /></button>
          <button aria-label={t("settings.enableClickThrough")} onMouseEnter={() => setControlTooltip("through")} onMouseLeave={() => setControlTooltip(null)} className="flex h-7 w-7 items-center justify-center rounded-lg text-slate-400 transition-colors hover:bg-white/10 hover:text-white" onClick={() => void updateFloating({ click_through: true })}><PointerIcon /></button>
          <button aria-label={t("common.close")} onMouseEnter={() => setControlTooltip("close")} onMouseLeave={() => setControlTooltip(null)} className="flex h-7 w-7 items-center justify-center rounded-lg text-slate-400 transition-colors hover:bg-white/10 hover:text-white" onClick={() => void hide()}>×</button>
        </div>}
        {!settings?.floating.click_through && controlTooltip && <div className="pointer-events-none absolute right-4 top-11 z-20 whitespace-nowrap rounded-lg border border-white/10 bg-slate-950/95 px-2.5 py-1.5 text-[11px] font-medium text-slate-100 shadow-xl backdrop-blur-xl">{controlTooltip === "top" ? topmostActionLabel : controlTooltip === "compact" ? compactActionLabel : controlTooltip === "through" ? t("settings.enableClickThrough") : t("common.close")}</div>}
        {!settings?.floating.click_through && !compactMode && <button aria-label={t("settings.resizeFloating")} className="absolute bottom-2 right-2 h-5 w-5 cursor-e-resize touch-none" onPointerDown={(event) => { event.stopPropagation(); void floatingWindow.startResizeDragging("East"); }}><span className="absolute bottom-0.5 right-0.5 h-3 w-1.5 border-r-2 border-white/35" /></button>}
      </div>
      {(presentationPhase === "expanding" || presentationPhase === "collapsing") && (
        <div
          aria-hidden="true"
          className={`pointer-events-none absolute z-50 flex h-12 w-12 select-none items-center justify-center font-mono text-[13px] font-bold tracking-tight ${usageTextTone(compactUsage.value)} ${transitionAnchor.right ? "right-0" : "left-0"} ${transitionAnchor.bottom ? "bottom-0" : "top-0"}`}
          style={{ opacity: compactOpacity }}
        >
          {compactText}
        </div>
      )}
    </div>
  );
}

function PinIcon() {
  return <svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="m14.5 4.5 5 5-3 1-3.5 3.5.5 3.5-1 1-7-7 1-1 3.5.5 3.5-3.5 1-3Z" /><path d="m9.5 14.5-5 5" /></svg>;
}

function CompactIcon() {
  return <svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"><rect x="5" y="5" width="14" height="14" rx="3" /><path d="M9 12h6" /></svg>;
}

function PointerIcon() {
  return <svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"><path d="m5 3 12 9-5.5 1.2L9 19 5 3Z" /><path d="m13 17 3 4" /></svg>;
}

function ClockIcon() {
  return <svg viewBox="0 0 24 24" className="h-3 w-3 shrink-0" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="12" r="8" /><path d="M12 8v4l2.5 1.5" /></svg>;
}

function CalendarIcon() {
  return <svg viewBox="0 0 24 24" className="h-3.5 w-3.5 shrink-0" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"><rect x="4" y="5.5" width="16" height="14" rx="2" /><path d="M8 3.5v4M16 3.5v4M4 10h16" /></svg>;
}
