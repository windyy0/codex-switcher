import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";

import { invokeBackend, isTauriRuntime } from "../lib/platform";
import type {
  AppSettings,
  CodexCloseBehavior,
  CodexReopenBehavior,
} from "../types";
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

  const update = async (patch: Partial<AppSettings>) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
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
      <div className="overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-sm dark:border-gray-800 dark:bg-gray-900">
        <div className="flex items-center justify-between gap-6 p-5">
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
            onChange={(value: CodexCloseBehavior) => void update({ codex_close_behavior: value })}
            ariaLabel={t("settings.codexCloseBehavior")}
            options={[
              { value: "ask", label: t("settings.codexCloseAsk") },
              { value: "graceful", label: t("settings.codexCloseGraceful") },
              { value: "force", label: t("settings.codexCloseForce") },
            ]}
          />
        </div>
        <div className="flex items-center justify-between gap-6 border-t border-gray-100 p-5 dark:border-gray-800">
          <div className="min-w-0">
            <div className="font-semibold text-gray-900 dark:text-gray-100">
              {t("settings.codexReopenBehavior")}
            </div>
            <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
              {t("settings.codexReopenBehaviorDescription")}
            </p>
          </div>
          <SelectMenu
            className="w-44 shrink-0"
            value={settings.codex_reopen_behavior}
            onChange={(value: CodexReopenBehavior) => void update({ codex_reopen_behavior: value })}
            ariaLabel={t("settings.codexReopenBehavior")}
            options={[
              { value: "ask", label: t("settings.codexReopenAsk") },
              { value: "always", label: t("settings.codexReopenAlways") },
              { value: "never", label: t("settings.codexReopenNever") },
            ]}
          />
        </div>
      </div>
    </section>
  );
}
