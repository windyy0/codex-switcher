import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { invokeBackend } from "../lib/platform";
import type { AppSettings } from "../types";

function Toggle({ value, label, onChange }: { value: boolean; label: string; onChange: (next: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={value}
      aria-label={label}
      onClick={() => onChange(!value)}
      className={`relative h-7 w-12 shrink-0 rounded-full transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 focus-visible:ring-offset-2 dark:focus-visible:ring-offset-gray-900 ${value ? "bg-emerald-500" : "bg-gray-300 dark:bg-gray-700"}`}
    >
      <span aria-hidden="true" className={`absolute left-0 top-1 h-5 w-5 rounded-full bg-white shadow-sm transition-transform ${value ? "translate-x-6" : "translate-x-1"}`} />
    </button>
  );
}

export function TimeDisplaySettings() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    const unlisten = listen<AppSettings>("settings-changed", ({ payload }) => setSettings(payload));
    return () => { void unlisten.then((fn) => fn()); };
  }, []);

  const update = async (show_dual_clock: boolean) => {
    if (!settings) return;
    const next = { ...settings, show_dual_clock };
    setSettings(next);
    setSaving(true);
    try {
      setSettings(await invokeBackend<AppSettings>("set_app_settings", { settings: next }));
    } finally {
      setSaving(false);
    }
  };

  if (!settings) return null;

  return (
    <section aria-busy={saving}>
      <h3 className="mb-3 text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">
        {t("settings.timeSection")}
      </h3>
      <div className="flex items-center justify-between gap-6 rounded-2xl border border-gray-200 bg-white p-5 shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="min-w-0">
          <div className="font-semibold text-gray-900 dark:text-gray-100">{t("settings.timeDisplay")}</div>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">{t("settings.timeDisplayDescription")}</p>
        </div>
        <Toggle value={settings.show_dual_clock} label={t("settings.timeDisplay")} onChange={update} />
      </div>
    </section>
  );
}
