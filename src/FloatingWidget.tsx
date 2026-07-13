import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import { invokeBackend } from "./lib/platform";
import type { AccountInfo, AppSettings, UsageInfo } from "./types";

const REFRESH_MS = 60_000;

function remaining(used: number | null): number | null {
  return used == null || !Number.isFinite(used) ? null : Math.round(Math.max(0, Math.min(100, 100 - used)));
}

function resetLabel(timestamp: number | null): string {
  if (timestamp == null) return "--";
  const minutes = Math.max(0, Math.ceil((timestamp * 1000 - Date.now()) / 60_000));
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return minutes % 60 ? `${hours}h ${minutes % 60}m` : `${hours}h`;
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
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [account, setAccount] = useState<AccountInfo | null>(null);
  const [usage, setUsage] = useState<UsageInfo | null>(null);
  const [offline, setOffline] = useState(false);
  const [controlTooltip, setControlTooltip] = useState<"pin" | "close" | null>(null);
  const [viewport, setViewport] = useState(() => ({ width: window.innerWidth, height: window.innerHeight }));
  const dragTimer = useRef<number | null>(null);

  const refresh = useCallback(async () => {
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
    } catch { setOffline(true); }
  }, []);

  useEffect(() => {
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    void refresh();
    const timer = window.setInterval(() => void refresh(), REFRESH_MS);
    const unlistenSettings = listen<AppSettings>("floating-settings-changed", ({ payload }) => setSettings(payload));
    const unlistenAccounts = listen("accounts-changed", () => void refresh());
    return () => { window.clearInterval(timer); void unlistenSettings.then((fn) => fn()); void unlistenAccounts.then((fn) => fn()); };
  }, [refresh]);

  useEffect(() => {
    const updateViewport = () => setViewport({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener("resize", updateViewport);
    return () => window.removeEventListener("resize", updateViewport);
  }, []);

  const fields = useMemo(() => new Set(settings?.floating.visible_fields ?? []), [settings]);
  const primary = remaining(usage?.primary_used_percent ?? null);
  const secondary = remaining(usage?.secondary_used_percent ?? null);
  const contentScale = Math.max(
    0.5,
    Math.min(2.5, Math.min((viewport.width - 16) / 284, (viewport.height - 16) / 168))
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
  const hide = async () => {
    if (settings) {
      const next = { ...settings, floating: { ...settings.floating, visible: false } };
      setSettings(await invokeBackend<AppSettings>("set_app_settings", { settings: next }));
    } else {
      await getCurrentWindow().hide();
    }
  };
  const pin = async () => {
    if (!settings) return;
    const next = { ...settings, floating: { ...settings.floating, always_on_top: true, click_through: true } };
    setSettings(await invokeBackend<AppSettings>("set_app_settings", { settings: next }));
  };
  const cancelLongPress = () => {
    if (dragTimer.current !== null) window.clearTimeout(dragTimer.current);
    dragTimer.current = null;
  };
  const startLongPress = (event: React.PointerEvent<HTMLDivElement>) => {
    if (settings?.floating.always_on_top || (event.target as HTMLElement).closest("button")) return;
    cancelLongPress();
    dragTimer.current = window.setTimeout(() => {
      dragTimer.current = null;
      void getCurrentWindow().startDragging();
    }, 20);
  };

  return (
    <div className="h-full w-full p-2" style={{ opacity: settings?.floating.opacity ?? 0.92 }}>
      <div className={`relative h-full select-none overflow-hidden rounded-[20px] bg-slate-950/90 px-4 py-3 text-white backdrop-blur-xl ${settings?.floating.always_on_top ? "" : "cursor-move"}`} onPointerDown={startLongPress} onPointerUp={cancelLongPress} onPointerCancel={cancelLongPress} onPointerLeave={cancelLongPress}>
        <div className="origin-top-left" style={{ width: `${100 / contentScale}%`, height: `${100 / contentScale}%`, transform: `scale(${contentScale})` }}>
        <div className="mb-3 flex h-7 items-center justify-between pr-16">
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
            {fields.has("primary_usage") && <UsageRow label={t("usage.fiveHour")} value={primary} />}
            {fields.has("secondary_usage") && <UsageRow label={t("usage.weekly")} value={secondary} />}
            {fields.has("primary_reset") && <div className="text-right text-[11px] text-slate-400">{t("usage.resets", { time: resetLabel(usage?.primary_resets_at ?? null) })}</div>}
          </div>
        )}
        </div>
        {!settings?.floating.always_on_top && <div className="absolute right-4 top-3 flex items-center gap-1">
          <button aria-label={t("settings.pinAndPassThrough")} onMouseEnter={() => setControlTooltip("pin")} onMouseLeave={() => setControlTooltip(null)} className="flex h-7 w-7 items-center justify-center rounded-lg text-slate-400 transition-colors hover:bg-white/10 hover:text-white" onClick={() => void pin()}><PinIcon /></button>
          <button aria-label={t("common.close")} onMouseEnter={() => setControlTooltip("close")} onMouseLeave={() => setControlTooltip(null)} className="flex h-7 w-7 items-center justify-center rounded-lg text-slate-400 transition-colors hover:bg-white/10 hover:text-white" onClick={() => void hide()}>×</button>
        </div>}
        {!settings?.floating.always_on_top && controlTooltip && <div className="pointer-events-none absolute top-11 z-20 whitespace-nowrap rounded-lg border border-white/10 bg-slate-950/95 px-2.5 py-1.5 text-[11px] font-medium text-slate-100 shadow-xl backdrop-blur-xl" style={{ right: controlTooltip === "pin" ? 62 : 30, transform: "translateX(50%)" }}>{controlTooltip === "pin" ? t("settings.pinAndPassThrough") : t("common.close")}</div>}
        {!settings?.floating.always_on_top && <button aria-label={t("settings.resizeFloating")} className="absolute bottom-2 right-2 h-5 w-5 cursor-se-resize touch-none" onPointerDown={(event) => { event.stopPropagation(); void getCurrentWindow().startResizeDragging("SouthEast"); }}><span className="absolute bottom-0.5 right-0.5 h-2.5 w-2.5 border-b-2 border-r-2 border-white/35" /></button>}
      </div>
    </div>
  );
}

function PinIcon() {
  return <svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="m14.5 4.5 5 5-3 1-3.5 3.5.5 3.5-1 1-7-7 1-1 3.5.5 3.5-3.5 1-3Z" /><path d="m9.5 14.5-5 5" /></svg>;
}
