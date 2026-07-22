import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { AccountWithUsage } from "../types";

interface AccountRowProps {
  account: AccountWithUsage;
  layout?: "list" | "card";
  masked?: boolean;
  switching?: boolean;
  switchDisabled?: boolean;
  warmingUp?: boolean;
  onSwitch: () => void;
  onWarmup: () => Promise<void>;
  onRefresh: () => Promise<unknown>;
  onEnable: () => Promise<void>;
  onOpenDetails: () => void;
}

function remainingPercent(usedPercent: number | null | undefined): number | null {
  if (usedPercent === null || usedPercent === undefined) return null;
  return Math.max(0, Math.min(100, 100 - usedPercent));
}

function formatResetTime(resetAt: number | null | undefined, t: TFunction): string {
  if (!resetAt) return "";
  const diff = resetAt - Math.floor(Date.now() / 1000);
  if (diff <= 0) return t("usage.now");
  if (diff < 3600) return t("usage.minutes", { count: Math.max(1, Math.floor(diff / 60)) });
  if (diff < 86400) {
    return t("usage.hoursMinutes", {
      hours: Math.floor(diff / 3600),
      minutes: Math.floor((diff % 3600) / 60),
    });
  }
  return t("usage.days", { count: Math.floor(diff / 86400) });
}

function getQuotaTone(remaining: number): string {
  if (remaining <= 10) return "bg-red-500";
  if (remaining <= 30) return "bg-amber-500";
  return "bg-emerald-500";
}

function QuotaLine({
  label,
  usedPercent,
  resetsAt,
}: {
  label: string;
  usedPercent: number | null | undefined;
  resetsAt: number | null | undefined;
}) {
  const { t } = useTranslation();
  const remaining = remainingPercent(usedPercent);

  if (remaining === null) return null;
  const reset = formatResetTime(resetsAt, t);

  return (
    <div className="min-w-0">
      <div className="mb-1 flex items-center justify-between gap-3 text-[11px] leading-none">
        <span className="font-medium text-gray-600 dark:text-gray-300">
          {label} · {Math.round(remaining)}%
        </span>
        {reset && (
          <span className="truncate text-gray-400 dark:text-gray-500">
            {t("usage.resets", { time: reset })}
          </span>
        )}
      </div>
      <div className="h-1 overflow-hidden rounded-full bg-gray-100 dark:bg-gray-800">
        <div
          className={`h-full rounded-full ${getQuotaTone(remaining)}`}
          style={{ width: `${remaining}%` }}
        />
      </div>
    </div>
  );
}

function AccountQuota({ account }: { account: AccountWithUsage }) {
  const { t } = useTranslation();

  if (account.disabled) {
    return (
      <span className="inline-flex rounded-full bg-gray-100 px-2.5 py-1 text-xs font-medium text-gray-500 dark:bg-gray-800 dark:text-gray-400">
        {t("accountCard.disabled")}
      </span>
    );
  }

  if (account.auth_mode === "api_key") {
    return (
      <span className="text-xs text-gray-500 dark:text-gray-400">
        {t("usage.apiKeyManagedExternally")}
      </span>
    );
  }

  if (account.usageLoading && !account.usage) {
    return (
      <div className="space-y-2 animate-pulse">
        <div className="h-2.5 w-28 rounded bg-gray-200 dark:bg-gray-700" />
        <div className="h-1 rounded bg-gray-100 dark:bg-gray-800" />
      </div>
    );
  }

  if (!account.usage || account.usage.error) {
    return (
      <span className="text-xs text-gray-400 dark:text-gray-500">
        {t("usage.compactUnavailable")}
      </span>
    );
  }

  const hasWeekly = account.usage.secondary_used_percent !== null;
  const hasSession = account.usage.primary_used_percent !== null;
  if (!hasWeekly && !hasSession) {
    return (
      <span className="text-xs text-gray-400 dark:text-gray-500">
        {t("usage.compactUnavailable")}
      </span>
    );
  }

  return (
    <div className="space-y-1.5">
      {hasWeekly && (
        <QuotaLine
          label={t("usage.weekly")}
          usedPercent={account.usage.secondary_used_percent}
          resetsAt={account.usage.secondary_resets_at}
        />
      )}
      {hasSession && (
        <QuotaLine
          label={t("usage.fiveHour")}
          usedPercent={account.usage.primary_used_percent}
          resetsAt={account.usage.primary_resets_at}
        />
      )}
    </div>
  );
}

function planPresentation(account: AccountWithUsage, t: TFunction) {
  const plan = account.plan_type
    ? account.plan_type.charAt(0).toUpperCase() + account.plan_type.slice(1)
    : account.auth_mode === "api_key"
      ? t("accountCard.apiKey")
      : t("common.unknown");
  const key = account.plan_type?.toLowerCase() || (account.auth_mode === "api_key" ? "api_key" : "free");
  const colors: Record<string, string> = {
    pro: "border-indigo-200 bg-indigo-50 text-indigo-700 dark:border-indigo-700 dark:bg-indigo-900/30 dark:text-indigo-300",
    plus: "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-700 dark:bg-emerald-900/30 dark:text-emerald-300",
    team: "border-blue-200 bg-blue-50 text-blue-700 dark:border-blue-700 dark:bg-blue-900/30 dark:text-blue-300",
    enterprise: "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-700 dark:bg-amber-900/30 dark:text-amber-300",
    api_key: "border-orange-200 bg-orange-50 text-orange-700 dark:border-orange-700 dark:bg-orange-900/30 dark:text-orange-300",
    free: "border-gray-200 bg-gray-50 text-gray-600 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300",
  };
  return { plan, color: colors[key] || colors.free };
}

function formatExpiry(expiresAt: string | null | undefined, locale: string, t: TFunction) {
  if (!expiresAt) {
    return { label: t("accounts.expiryUnavailable"), tone: "text-gray-400 dark:text-gray-500" };
  }
  const expiry = new Date(expiresAt);
  if (Number.isNaN(expiry.getTime())) {
    return { label: t("accounts.expiryUnavailable"), tone: "text-gray-400 dark:text-gray-500" };
  }
  const label = new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" }).format(expiry);
  const remaining = expiry.getTime() - Date.now();
  const tone = remaining <= 3 * 86400000
    ? "text-red-600 dark:text-red-400"
    : remaining <= 7 * 86400000
      ? "text-amber-600 dark:text-amber-400"
      : "text-gray-600 dark:text-gray-300";
  return { label, tone };
}

export function AccountRow({
  account,
  layout = "list",
  masked = false,
  switching = false,
  switchDisabled = false,
  warmingUp = false,
  onSwitch,
  onWarmup,
  onRefresh,
  onEnable,
  onOpenDetails,
}: AccountRowProps) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en-US";
  const plan = planPresentation(account, t);
  const isApiAccount = account.auth_mode === "api_key";
  const isCardLayout = layout === "card";
  const expiry = isApiAccount
    ? null
    : formatExpiry(account.subscription_expires_at, locale, t);
  const [refreshing, setRefreshing] = useState(false);
  const [enabling, setEnabling] = useState(false);
  const isRefreshing = refreshing || Boolean(account.usageLoading);

  const handleRefresh = async () => {
    if (isRefreshing) return;
    setRefreshing(true);
    try {
      await onRefresh();
    } catch (error) {
      console.error("Failed to refresh account usage:", error);
    } finally {
      setRefreshing(false);
    }
  };

  const handleEnable = async () => {
    if (enabling) return;
    setEnabling(true);
    try {
      await onEnable();
    } catch (error) {
      console.error("Failed to enable account:", error);
    } finally {
      setEnabling(false);
    }
  };

  return (
    <article
      className={isCardLayout
        ? `flex min-h-56 min-w-0 flex-col rounded-2xl border p-4 shadow-sm transition-colors ${
            account.is_active
              ? "border-emerald-300 bg-emerald-50/45 dark:border-emerald-800 dark:bg-emerald-950/15"
              : "border-gray-200 bg-white hover:border-gray-300 dark:border-gray-800 dark:bg-gray-900 dark:hover:border-gray-700"
          }`
        : `grid min-h-20 grid-cols-1 items-center gap-3 border-b border-gray-100 px-4 py-2.5 transition-colors last:border-b-0 dark:border-gray-800/80 ${
            isApiAccount
              ? "md:grid-cols-[minmax(0,1.15fr)_minmax(16rem,1.6fr)_auto]"
              : "md:grid-cols-[minmax(0,1.15fr)_minmax(16rem,1.6fr)_7rem_auto]"
          } ${
            account.is_active
              ? "bg-emerald-50/45 dark:bg-emerald-950/15"
              : "hover:bg-gray-50/80 dark:hover:bg-gray-800/35"
          }`}
    >
      <button
        type="button"
        onClick={onOpenDetails}
        className="min-w-0 text-left outline-none focus-visible:rounded-lg focus-visible:ring-2 focus-visible:ring-emerald-500"
      >
        <div className="flex min-w-0 items-center gap-2">
          {account.is_active && <span className="h-2 w-2 shrink-0 rounded-full bg-emerald-500" />}
          <span className={`truncate font-semibold text-gray-900 dark:text-gray-100 ${masked ? "select-none blur-sm" : ""}`}>
            {account.name}
          </span>
          <span className={`shrink-0 rounded-full border px-2 py-0.5 text-[11px] font-medium ${plan.color}`}>
            {plan.plan}
          </span>
        </div>
        {account.email && (
          <div className={`mt-1 truncate text-xs text-gray-500 dark:text-gray-400 ${masked ? "select-none blur-sm" : ""}`}>
            {account.email}
          </div>
        )}
      </button>

      <button
        type="button"
        onClick={onOpenDetails}
        className={`${isCardLayout
          ? "mt-4 min-h-[4.5rem] rounded-xl bg-gray-50 p-3 dark:bg-gray-950/65"
          : "h-full"
        } min-w-0 text-left outline-none focus-visible:rounded-lg focus-visible:ring-2 focus-visible:ring-emerald-500`}
      >
        <AccountQuota account={account} />
      </button>

      {expiry && (
        <button
          type="button"
          onClick={onOpenDetails}
          className={`${isCardLayout
            ? "mt-3 flex items-center justify-between gap-3 rounded-lg px-1"
            : "md:text-right"
          } min-w-0 text-left outline-none focus-visible:rounded-lg focus-visible:ring-2 focus-visible:ring-emerald-500`}
        >
          <div className="text-[11px] font-medium uppercase tracking-wide text-gray-400 dark:text-gray-500">
            {t("accounts.subscriptionExpiry")}
          </div>
          <div className={`${isCardLayout ? "" : "mt-1"} truncate text-xs font-medium ${expiry.tone}`}>{expiry.label}</div>
        </button>
      )}

      <div className={`flex items-center justify-end gap-2 ${
        isCardLayout
          ? "mt-auto border-t border-gray-100 pt-4 dark:border-gray-800"
          : isApiAccount
            ? "md:col-start-3"
            : "md:col-start-4"
      }`}>
        {!account.disabled && account.is_active ? (
          <span className="inline-flex h-9 items-center rounded-lg bg-emerald-50 px-3 text-xs font-semibold text-emerald-700 dark:bg-emerald-900/25 dark:text-emerald-300">
            ✓ {t("accountCard.active")}
          </span>
        ) : !account.disabled ? (
          <button
            type="button"
            onClick={onSwitch}
            disabled={switching || switchDisabled}
            className="h-9 min-w-20 rounded-lg bg-gray-900 px-3 text-xs font-semibold text-white transition-colors hover:bg-gray-800 disabled:cursor-not-allowed disabled:opacity-45 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-gray-200"
            data-tooltip={switchDisabled ? t("accountCard.closeProcesses") : undefined}
          >
            {switching ? t("accountCard.switching") : t("accountCard.switch")}
          </button>
        ) : null}

        {account.disabled && (
          <button
            type="button"
            onClick={() => void handleEnable()}
            disabled={enabling}
            className="h-9 rounded-lg bg-emerald-50 px-3 text-xs font-semibold text-emerald-700 transition-colors hover:bg-emerald-100 disabled:opacity-50 dark:bg-emerald-900/25 dark:text-emerald-300 dark:hover:bg-emerald-900/40"
          >
            {enabling ? t("common.saving") : t("accountCard.enable")}
          </button>
        )}

        {!account.disabled && account.auth_mode === "chat_g_p_t" && (
          <>
            <button
              type="button"
              onClick={() => void onWarmup()}
              disabled={warmingUp}
              className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-amber-50 text-amber-700 transition-colors hover:bg-amber-100 disabled:cursor-wait disabled:opacity-55 dark:bg-amber-900/20 dark:text-amber-300 dark:hover:bg-amber-900/40"
              aria-label={warmingUp ? t("accountCard.warmupSending") : t("accountCard.warmupSend")}
              data-tooltip={warmingUp ? t("accountCard.warmupSending") : t("accountCard.warmupSend")}
            >
              <svg className={`h-4 w-4 ${warmingUp ? "animate-pulse" : ""}`} viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
                <path d="M11.2 1.8 4.6 10h4.1l-.3 8.2 7-9.6h-4.2V1.8Z" />
              </svg>
            </button>
            <button
              type="button"
              onClick={() => void handleRefresh()}
              disabled={isRefreshing}
              className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-gray-100 text-gray-600 transition-colors hover:bg-gray-200 disabled:cursor-wait disabled:opacity-55 dark:bg-gray-800 dark:text-gray-300 dark:hover:bg-gray-700"
              aria-label={t("accountCard.refreshUsage")}
              data-tooltip={t("accountCard.refreshUsage")}
            >
              <svg className={`h-4 w-4 ${isRefreshing ? "animate-spin" : ""}`} viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
                <path d="M15.5 7A6 6 0 1 0 16 11" strokeLinecap="round" />
                <path d="M12.5 4.5h3.5V8" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </button>
          </>
        )}

      </div>
    </article>
  );
}
