import { useEffect, useState } from "react";
import { invokeBackend } from "./lib/platform";
import type { AppSettings } from "./types";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";

export default function FloatingControls() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [tooltip, setTooltip] = useState<{ text: string; kind: "pin" | "close" } | null>(null);
  useEffect(() => {
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    const unlisten = listen<AppSettings>("settings-changed", ({ payload }) => setSettings(payload));
    return () => { void unlisten.then((fn) => fn()); };
  }, []);

  const update = async (patch: Partial<AppSettings["floating"]>) => {
    if (!settings) return;
    const next = { ...settings, floating: { ...settings.floating, ...patch } };
    setSettings(await invokeBackend<AppSettings>("set_app_settings", { settings: next }));
  };

  return <div className="relative h-full w-full" onMouseLeave={() => setTooltip(null)}>
    <div className="absolute right-0 top-0 flex items-center justify-end gap-1">
    <button aria-label={t("settings.unpin")} onMouseEnter={() => setTooltip({ text: t("settings.unpin"), kind: "pin" })} onClick={() => void update({ always_on_top: false, click_through: false })} className="flex h-7 w-7 items-center justify-center rounded-lg text-emerald-300 transition-colors hover:bg-white/10"><svg viewBox="0 0 24 24" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="m14.5 4.5 5 5-3 1-3.5 3.5.5 3.5-1 1-7-7 1-1 3.5.5 3.5-3.5 1-3Z" /><path d="m9.5 14.5-5 5" /></svg></button>
    <button aria-label={t("common.close")} onMouseEnter={() => setTooltip({ text: t("common.close"), kind: "close" })} onClick={() => void update({ visible: false })} className="flex h-7 w-7 items-center justify-center rounded-lg text-xs text-slate-400 transition-colors hover:bg-white/10 hover:text-white">×</button>
    </div>
    {tooltip && <div className="absolute right-0 top-8 max-w-[176px] whitespace-nowrap rounded-lg border border-white/10 bg-slate-950/95 px-2.5 py-1.5 text-[11px] font-medium leading-4 text-slate-100 shadow-xl backdrop-blur-xl">{tooltip.text}</div>}
  </div>;
}
