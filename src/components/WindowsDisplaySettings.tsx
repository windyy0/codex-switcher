import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { invokeBackend } from "../lib/platform";
import type { AppSettings, FloatingField, TaskbarDoubleClickAction, TaskbarLayout } from "../types";
import { SelectMenu } from "./SelectMenu";

function Toggle({ value, label, onChange }: { value: boolean; label: string; onChange: (next: boolean) => void }) {
  return <button type="button" role="switch" aria-checked={value} aria-label={label} onClick={() => onChange(!value)} className={`relative h-7 w-12 shrink-0 rounded-full transition-colors ${value ? "bg-emerald-500" : "bg-gray-300 dark:bg-gray-700"}`}><span className={`absolute left-0 top-1 h-5 w-5 rounded-full bg-white shadow-sm transition-transform ${value ? "translate-x-6" : "translate-x-1"}`} /></button>;
}

export function WindowsDisplaySettings({ section }: { section: "floating" | "taskbar" }) {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [opacityDraft, setOpacityDraft] = useState<number | null>(null);
  const [offsetXDraft, setOffsetXDraft] = useState("0");
  const [offsetYDraft, setOffsetYDraft] = useState("0");
  const opacityTimer = useRef<number | null>(null);

  useEffect(() => {
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    const unlisten = listen<AppSettings>("settings-changed", ({ payload }) => setSettings(payload));
    return () => { void unlisten.then((fn) => fn()); };
  }, []);

  useEffect(() => {
    if (!settings) return;
    setOpacityDraft(Math.round(settings.floating.opacity * 100));
    setOffsetXDraft(String(settings.taskbar.offset_x));
    setOffsetYDraft(String(settings.taskbar.offset_y));
  }, [settings?.floating.opacity, settings?.taskbar.offset_x, settings?.taskbar.offset_y]);

  useEffect(() => () => {
    if (opacityTimer.current !== null) window.clearTimeout(opacityTimer.current);
  }, []);

  const save = useCallback(async (next: AppSettings) => {
    setSettings(next);
    setSaving(true);
    try { setSettings(await invokeBackend<AppSettings>("set_app_settings", { settings: next })); }
    finally { setSaving(false); }
  }, []);

  if (!settings) return null;
  const updateTaskbar = (patch: Partial<AppSettings["taskbar"]>) => void save({ ...settings, taskbar: { ...settings.taskbar, ...patch } });
  const updateFloating = (patch: Partial<AppSettings["floating"]>) => {
    const floating = { ...settings.floating, ...patch };
    if (patch.compact_mode === true) floating.click_through = false;
    else if (patch.click_through === true) floating.compact_mode = false;
    void save({ ...settings, floating });
  };
  const updateOpacity = (value: number) => {
    setOpacityDraft(value);
    const next = { ...settings, floating: { ...settings.floating, opacity: value / 100 } };
    setSettings(next);
    if (opacityTimer.current !== null) window.clearTimeout(opacityTimer.current);
    opacityTimer.current = window.setTimeout(() => {
      opacityTimer.current = null;
      void save(next);
    }, 40);
  };
  const commitOffset = (field: "offset_x" | "offset_y", draft: string) => {
    const value = Math.max(-5000, Math.min(5000, Number.parseInt(draft, 10) || 0));
    if (field === "offset_x") setOffsetXDraft(String(value));
    else setOffsetYDraft(String(value));
    updateTaskbar({ [field]: value });
  };
  const toggleField = (field: FloatingField) => {
    const fields = settings.floating.visible_fields.includes(field)
      ? settings.floating.visible_fields.filter((item) => item !== field)
      : [...settings.floating.visible_fields, field];
    if (fields.length) updateFloating({ visible_fields: fields });
  };

  return <div className="space-y-6 opacity-100 transition-opacity" aria-busy={saving}>
    {section === "taskbar" && <section>
      <h3 className="mb-3 text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">{t("settings.taskbarSection")}</h3>
      <div className="rounded-2xl border border-gray-200 bg-white shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="flex items-center justify-between gap-6 p-5"><div><div className="font-semibold text-gray-900 dark:text-gray-100">{t("settings.taskbarWidget")}</div><p className="mt-1 text-sm text-gray-500 dark:text-gray-400">{t("settings.taskbarDescription")}</p></div><Toggle value={settings.taskbar.enabled} label={t("settings.taskbarWidget")} onChange={(enabled) => updateTaskbar({ enabled })} /></div>
        <div className="grid gap-4 border-t border-gray-100 p-5 dark:border-gray-800 sm:grid-cols-2">
          <label className="min-w-0 space-y-2 text-sm text-gray-600 dark:text-gray-300"><span>{t("settings.taskbarLayout")}</span><SelectMenu className="w-full" value={settings.taskbar.layout} onChange={(value) => updateTaskbar({ layout: value as TaskbarLayout })} ariaLabel={t("settings.taskbarLayout")} options={[{ value: "detailed", label: t("settings.layoutDetailed") }, { value: "minimal", label: t("settings.layoutMinimal") }, { value: "compact", label: t("settings.layoutCompact") }]} /></label>
          <label className="min-w-0 space-y-2 text-sm text-gray-600 dark:text-gray-300"><span>{t("settings.doubleClickAction")}</span><SelectMenu className="w-full" value={settings.taskbar.double_click_action} onChange={(value) => updateTaskbar({ double_click_action: value as TaskbarDoubleClickAction })} ariaLabel={t("settings.doubleClickAction")} options={[{ value: "toggle_floating", label: t("settings.actionFloating") }, { value: "open_main", label: t("settings.actionMain") }]} /></label>
        </div>
        <div className="grid gap-4 border-t border-gray-100 p-5 dark:border-gray-800 sm:grid-cols-2">
          <label className="space-y-2 text-sm text-gray-600 dark:text-gray-300"><span>{t("settings.taskbarOffsetX")}</span><input type="text" inputMode="numeric" value={offsetXDraft} onChange={(event) => { if (/^-?\d*$/.test(event.target.value)) setOffsetXDraft(event.target.value); }} onBlur={() => commitOffset("offset_x", offsetXDraft)} onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }} className="h-10 w-full rounded-xl border border-gray-300 bg-white px-3 font-mono text-sm outline-none focus:border-emerald-500 dark:border-gray-700 dark:bg-gray-800" /></label>
          <label className="space-y-2 text-sm text-gray-600 dark:text-gray-300"><span>{t("settings.taskbarOffsetY")}</span><input type="text" inputMode="numeric" value={offsetYDraft} onChange={(event) => { if (/^-?\d*$/.test(event.target.value)) setOffsetYDraft(event.target.value); }} onBlur={() => commitOffset("offset_y", offsetYDraft)} onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }} className="h-10 w-full rounded-xl border border-gray-300 bg-white px-3 font-mono text-sm outline-none focus:border-emerald-500 dark:border-gray-700 dark:bg-gray-800" /></label>
          <p className="text-xs leading-5 text-gray-500 dark:text-gray-400 sm:col-span-2">{t("settings.taskbarOffsetDescription")}</p>
        </div>
        {settings.taskbar.last_error && <p className="border-t border-rose-100 bg-rose-50 px-5 py-3 text-xs text-rose-700 dark:border-rose-900 dark:bg-rose-950/30 dark:text-rose-300">{settings.taskbar.last_error}</p>}
      </div>
    </section>}
    {section === "floating" && <section>
      <h3 className="mb-3 text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">{t("settings.floatingSection")}</h3>
      <div className="overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="flex items-center justify-between gap-6 p-5"><div><div className="font-semibold text-gray-900 dark:text-gray-100">{t("settings.floatingWindow")}</div><p className="mt-1 text-sm text-gray-500 dark:text-gray-400">{t("settings.floatingDescription")}</p></div><Toggle value={settings.floating.visible} label={t("settings.floatingWindow")} onChange={(visible) => updateFloating({ enabled: true, visible })} /></div>
        <div className="grid gap-5 border-t border-gray-100 p-5 dark:border-gray-800 sm:grid-cols-2">
          <div className="space-y-3"><div className="flex items-center justify-between gap-3 text-sm"><span className="font-medium text-gray-800 dark:text-gray-100">{t("settings.alwaysOnTop")}</span><Toggle value={settings.floating.always_on_top} label={t("settings.alwaysOnTop")} onChange={(always_on_top) => updateFloating({ always_on_top })} /></div><p className="text-xs leading-5 text-gray-500 dark:text-gray-400">{t("settings.alwaysOnTopDescription")}</p></div>
          <div className="space-y-3"><div className="flex items-center justify-between gap-3 text-sm"><span className="font-medium text-gray-800 dark:text-gray-100">{t("settings.clickThrough")}</span><Toggle value={settings.floating.click_through} label={t("settings.clickThrough")} onChange={(click_through) => updateFloating({ click_through })} /></div><p className="text-xs leading-5 text-gray-500 dark:text-gray-400">{t("settings.clickThroughDescription")}</p></div>
          <label className="text-sm sm:col-span-2"><span>{t("settings.opacity", { value: opacityDraft ?? Math.round(settings.floating.opacity * 100) })}</span><input className="mt-3 w-full accent-emerald-500" type="range" min="25" max="100" value={opacityDraft ?? Math.round(settings.floating.opacity * 100)} onInput={(event) => updateOpacity(Number(event.currentTarget.value))} /></label>
        </div>
        <div className="border-t border-gray-100 px-5 py-4 dark:border-gray-800"><div className="mb-3 text-sm font-medium">{t("settings.visibleFields")}</div><div className="flex flex-wrap gap-2">{(["account", "primary_usage", "primary_reset", "secondary_usage"] as FloatingField[]).map((field) => <button key={field} onClick={() => toggleField(field)} className={`rounded-full border px-3 py-1.5 text-xs transition-colors ${settings.floating.visible_fields.includes(field) ? "border-emerald-400 bg-emerald-50 text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300" : "border-gray-200 text-gray-500 dark:border-gray-700"}`}>{t(`settings.field_${field}`)}</button>)}</div></div>
      </div>
    </section>}
  </div>;
}
