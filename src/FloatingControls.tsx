import { useEffect, useState } from "react";
import { invokeBackend } from "./lib/platform";
import type { AppSettings } from "./types";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";

export default function FloatingControls() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [tooltip, setTooltip] = useState<"top" | "compact" | "through" | "close" | null>(null);
  useEffect(() => {
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    const unlisten = listen<AppSettings>("settings-changed", ({ payload }) => setSettings(payload));
    return () => { void unlisten.then((fn) => fn()); };
  }, []);

  const update = async (patch: Partial<AppSettings["floating"]>) => {
    const current = await invokeBackend<AppSettings>("get_app_settings");
    const floating = { ...current.floating, ...patch };
    if (patch.compact_mode === true) floating.click_through = false;
    else if (patch.click_through === true) floating.compact_mode = false;
    const next = { ...current, floating };
    setSettings(await invokeBackend<AppSettings>("set_app_settings", { settings: next }));
  };

  const topmostLabel = settings?.floating.always_on_top
    ? t("settings.unpin")
    : t("settings.alwaysOnTop");
  const clickThroughLabel = settings?.floating.click_through
    ? t("settings.disableClickThrough")
    : t("settings.enableClickThrough");
  const compactLabel = settings?.floating.compact_mode
    ? t("settings.disableCompactMode")
    : t("settings.enableCompactMode");
  const tooltipText = tooltip === "top"
    ? topmostLabel
    : tooltip === "compact"
      ? compactLabel
      : tooltip === "through"
        ? clickThroughLabel
        : t("common.close");

  return <div className="relative h-full w-full" onMouseLeave={() => setTooltip(null)}>
    <div className="absolute right-0 top-0 flex items-center justify-end gap-1">
    <button aria-label={topmostLabel} onMouseEnter={() => setTooltip("top")} onClick={() => void update({ always_on_top: !settings?.floating.always_on_top })} className={`flex h-7 w-7 items-center justify-center rounded-lg transition-colors hover:bg-white/10 ${settings?.floating.always_on_top ? "text-emerald-300" : "text-slate-400 hover:text-white"}`}><svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="m14.5 4.5 5 5-3 1-3.5 3.5.5 3.5-1 1-7-7 1-1 3.5.5 3.5-3.5 1-3Z" /><path d="m9.5 14.5-5 5" /></svg></button>
    <button aria-label={compactLabel} onMouseEnter={() => setTooltip("compact")} onClick={() => void update({ compact_mode: !settings?.floating.compact_mode })} className={`flex h-7 w-7 items-center justify-center rounded-lg transition-colors hover:bg-white/10 ${settings?.floating.compact_mode ? "text-sky-300" : "text-slate-400 hover:text-white"}`}><svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"><rect x="5" y="5" width="14" height="14" rx="3" /><path d="M9 12h6" /></svg></button>
    <button aria-label={clickThroughLabel} onMouseEnter={() => setTooltip("through")} onClick={() => void update({ click_through: !settings?.floating.click_through })} className={`flex h-7 w-7 items-center justify-center rounded-lg transition-colors hover:bg-white/10 ${settings?.floating.click_through ? "text-sky-300" : "text-slate-400 hover:text-white"}`}><svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"><path d="m5 3 12 9-5.5 1.2L9 19 5 3Z" /><path d="m13 17 3 4" /></svg></button>
    <button aria-label={t("common.close")} onMouseEnter={() => setTooltip("close")} onClick={() => void update({ visible: false })} className="flex h-7 w-7 items-center justify-center rounded-lg text-xs text-slate-400 transition-colors hover:bg-white/10 hover:text-white">×</button>
    </div>
    {tooltip && <div className="absolute right-0 top-8 max-w-[176px] whitespace-nowrap rounded-lg border border-white/10 bg-slate-950/95 px-2.5 py-1.5 text-[11px] font-medium leading-4 text-slate-100 shadow-xl backdrop-blur-xl">{tooltipText}</div>}
  </div>;
}
