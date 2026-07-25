import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { invokeBackend } from "../lib/platform";
import type { AppSettings } from "../types";

const LOCAL_TIME_ZONE = Intl.DateTimeFormat().resolvedOptions().timeZone;

function formatTime(date: Date, timeZone?: string): string {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    ...(timeZone ? { timeZone } : {}),
  }).format(date);
}

function formatDate(date: Date, timeZone?: string): string {
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    weekday: "short",
    ...(timeZone ? { timeZone } : {}),
  }).format(date);
}

function formatTimeZone(date: Date): string {
  const parts = new Intl.DateTimeFormat(undefined, {
    timeZoneName: "shortOffset",
  }).formatToParts(date);
  return parts.find((part) => part.type === "timeZoneName")?.value ?? "Local";
}

export function TimeDisplay() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [now, setNow] = useState(() => new Date());
  const localTimeZone = useMemo(() => LOCAL_TIME_ZONE || "Local", []);

  useEffect(() => {
    void invokeBackend<AppSettings>("get_app_settings").then(setSettings);
    const unlisten = listen<AppSettings>("settings-changed", ({ payload }) => setSettings(payload));
    return () => { void unlisten.then((fn) => fn()); };
  }, []);

  useEffect(() => {
    if (!settings?.show_dual_clock) return;
    const timer = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(timer);
  }, [settings?.show_dual_clock]);

  if (!settings?.show_dual_clock) return null;

  const localTime = formatTime(now);
  const openAiTime = formatTime(now, "UTC");
  const localDate = formatDate(now);
  const openAiDate = formatDate(now, "UTC");
  const localOffset = formatTimeZone(now);

  return (
    <div
      className="absolute left-1/2 top-1/2 hidden -translate-x-1/2 -translate-y-1/2 items-center gap-1 rounded-xl border border-gray-200/80 bg-gray-50/95 p-1 shadow-sm sm:flex dark:border-gray-700/80 dark:bg-gray-800/95"
      title={`${t("header.localTime")}: ${localDate} ${localTime} (${localTimeZone}, ${localOffset}) · ${t("header.openaiTime")}: ${openAiDate} ${openAiTime} (UTC)`}
      aria-label={t("header.timeDisplay")}
    >
      <div className="min-w-[142px] rounded-lg bg-white px-2.5 py-1 dark:bg-gray-900">
        <div className="flex items-center justify-center gap-1.5">
          <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" aria-hidden="true" />
          <span className="text-[10px] font-semibold uppercase tracking-wide text-gray-500 dark:text-gray-400">
            {t("header.localTime")}
          </span>
          <span className="font-mono text-xs font-semibold tabular-nums text-gray-800 dark:text-gray-100">{localTime}</span>
        </div>
        <div className="mt-0.5 text-center text-[10px] leading-3 text-gray-400 dark:text-gray-500">{localDate}</div>
      </div>
      <div className="min-w-[142px] rounded-lg bg-gray-100/80 px-2.5 py-1 dark:bg-gray-950/70">
        <div className="flex items-center justify-center gap-1.5">
          <span className="h-1.5 w-1.5 rounded-full bg-sky-500" aria-hidden="true" />
          <span className="text-[10px] font-semibold uppercase tracking-wide text-gray-500 dark:text-gray-400">
            {t("header.openaiTime")}
          </span>
          <span className="font-mono text-xs font-semibold tabular-nums text-gray-800 dark:text-gray-100">{openAiTime}</span>
        </div>
        <div className="mt-0.5 text-center text-[10px] leading-3 text-gray-400 dark:text-gray-500">{openAiDate}</div>
      </div>
    </div>
  );
}
