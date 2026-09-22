import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";

import { invokeBackend, isTauriRuntime } from "../lib/platform";
import type { AppSettings, CodexCloseBehavior } from "../types";
import { SelectMenu } from "./SelectMenu";

export function CodexCloseSettings() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    const unlisten = listen<AppSettings>("settings-changed", ({ payload }) => {
      setSettings(payload);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const update = async (codex_close_behavior: CodexCloseBehavior) => {
    if (!settings) return;
    const next = { ...settings, codex_close_behavior };
    setSettings(next);
    setSaving(true);
    try {
      setSettings(await invokeBackend<AppSettings>("set_app_settings", { settings: next }));
    } finally {
      setSaving(false);
    }
  };

  if (!isTauriRuntime() || !settings) return null;

  return (
    <section aria-busy={saving}>
      <h3 className="mb-3 text-xs font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">
        {t("settings.codexProcessSection")}
      </h3>
      <div className="flex items-center justify-between gap-6 rounded-2xl border border-gray-200 bg-white p-5 shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="min-w-0">
          <div className="font-semibold text-gray-900 dark:text-gray-100">
            {t("settings.codexCloseBehavior")}
          </div>
          <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
            {t("settings.codexCloseBehaviorDescription")}
          </p>
        </div>
        <SelectMenu
          className="w-44 shrink-0"
          value={settings.codex_close_behavior}
          onChange={(value) => void update(value)}
          ariaLabel={t("settings.codexCloseBehavior")}
          options={[
            { value: "ask", label: t("settings.codexCloseAsk") },
            { value: "graceful", label: t("settings.codexCloseGraceful") },
            { value: "force", label: t("settings.codexCloseForce") },
          ]}
        />
      </div>
    </section>
  );
}
