import { useCallback, useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type {
  AccountHealthDiagnostic,
  AccountHealthStatus,
  AccountResetCredits,
  AccountUsageStats as AccountUsageStatsInfo,
  AccountWithUsage,
  WarmupFailureInfo,
} from "../types";
import { accountHealthBlocksAccountActions } from "../lib/accountHealth";
import { getEffectivePlanType } from "../lib/accountPlan";
import { invokeBackend } from "../lib/platform";
import { AccountUsageStats } from "./AccountUsageStats";
import { ResetCreditsMenu } from "./ResetCreditsMenu";
import { UsageBar } from "./UsageBar";

const RESET_CREDITS_REFRESH_INTERVAL_MS = 6 * 60 * 60 * 1000;

interface AccountCardProps {
  account: AccountWithUsage;
  onSwitch: () => void;
  onWarmup: () => Promise<void>;
  onDelete: () => void;
  onRefresh: () => Promise<unknown>;
  onReauthorize?: () => void;
  onReportDeactivation?: () => void;
  onRevalidate?: () => Promise<unknown>;
  onRename: (newName: string) => Promise<void>;
  onToggleDisabled: () => Promise<void>;
  onEditApiConfig?: () => void;
  switching?: boolean;
  switchDisabled?: boolean;
  warmingUp?: boolean;
  masked?: boolean;
  onToggleMask?: () => void;
  autoWarmupEnabled?: boolean;
  autoWarmupManagedByAll?: boolean;
  autoWarmupLabel?: string;
  onToggleAutoWarmup?: () => void;
  warmupFailure?: WarmupFailureInfo;
  onDismissWarmupFailure?: () => void;
  statsDefaultOpen?: boolean;
  compact?: boolean;
}

function formatLastRefresh(date: Date | null, t: TFunction, locale: string): string {
  if (!date) return t("accountCard.never");
  const now = new Date();
  const diff = Math.floor((now.getTime() - date.getTime()) / 1000);
  if (diff < 5) return t("accountCard.justNow");
  if (diff < 60) return t("accountCard.secondsAgo", { count: diff });
  if (diff < 3600) return t("accountCard.minutesAgo", { count: Math.floor(diff / 60) });
  if (diff < 86400) return t("accountCard.hoursAgo", { count: Math.floor(diff / 3600) });
  return date.toLocaleDateString(locale);
}

function getSubscriptionStatus(timestamp: string | null | undefined, t: TFunction, locale: string): {
  label: string;
  className: string;
} {
  if (!timestamp) {
    return {
      label: t("accountCard.expiryUnavailable"),
      className: "text-gray-400 dark:text-gray-500",
    };
  }

  const expiryDate = new Date(timestamp);
  if (Number.isNaN(expiryDate.getTime())) {
    return {
      label: t("accountCard.expiryUnavailable"),
      className: "text-gray-400 dark:text-gray-500",
    };
  }
  const remainingMs = expiryDate.getTime() - Date.now();
  const formattedDate = new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    year: "numeric",
    ...(remainingMs <= 3 * 24 * 60 * 60 * 1000 ? { hour: "2-digit", minute: "2-digit" } : {}),
  }).format(expiryDate);

  if (remainingMs <= 0) {
    return {
      label: t("accountCard.expired", { date: formattedDate }),
      className: "text-red-500 dark:text-red-400",
    };
  }

  if (remainingMs <= 3 * 24 * 60 * 60 * 1000) {
    return {
      label: t("accountCard.until", { date: formattedDate }),
      className: "text-red-500 dark:text-red-400",
    };
  }

  if (remainingMs <= 7 * 24 * 60 * 60 * 1000) {
    return {
      label: t("accountCard.until", { date: formattedDate }),
      className: "text-amber-500 dark:text-amber-400",
    };
  }

  return {
    label: t("accountCard.until", { date: formattedDate }),
    className: "text-gray-400 dark:text-gray-500",
  };
}

function BlurredText({ children, blur }: { children: React.ReactNode; blur: boolean }) {
  return (
    <span
      className={`transition-all duration-200 select-none ${blur ? "blur-sm" : ""}`}
      style={blur ? { userSelect: "none" } : undefined}
    >
      {children}
    </span>
  );
}

function healthStatusLabel(status: AccountHealthStatus, t: TFunction): string {
  switch (status) {
    case "healthy":
      return t("accountHealth.healthy");
    case "reauth_required":
      return t("accountHealth.reauthRequired");
    case "account_deactivated":
      return t("accountHealth.accountDeactivated");
    case "workspace_deactivated":
      return t("accountHealth.workspaceDeactivated");
    case "limited":
      return t("accountHealth.limited");
    case "transient_error":
      return t("accountHealth.transientError");
    default:
      return t("accountHealth.unknown");
  }
}

function healthSourceLabel(
  diagnostic: AccountHealthDiagnostic,
  t: TFunction
): string {
  switch (diagnostic.source) {
    case "usage":
      return t("accountHealth.sourceUsage");
    case "accounts_check":
      return t("accountHealth.sourceAccountsCheck");
    case "token_refresh":
      return t("accountHealth.sourceTokenRefresh");
    case "oauth":
      return t("accountHealth.sourceOAuth");
    case "oauth_user_report":
      return t("accountHealth.sourceOAuthUserReport");
    case "deactivation_email":
      return t("accountHealth.sourceDeactivationEmail");
  }
}

function HealthDiagnosticDetails({
  diagnostic,
  locale,
  t,
}: {
  diagnostic: AccountHealthDiagnostic;
  locale: string;
  t: TFunction;
}) {
  return (
    <div className="rounded-lg border border-black/5 bg-white/55 px-3 py-2 text-[11px] leading-5 dark:border-white/10 dark:bg-black/10">
      <div className="font-semibold">{healthStatusLabel(diagnostic.status, t)}</div>
      <div>{t("accountHealth.source")}: {healthSourceLabel(diagnostic, t)}</div>
      {diagnostic.http_status !== null && (
        <div>HTTP: {diagnostic.http_status}</div>
      )}
      {diagnostic.error_code && (
        <div>{t("accountHealth.errorCode")}: {diagnostic.error_code}</div>
      )}
      {diagnostic.message && (
        <div className="break-words">{t("accountHealth.originalMessage")}: {diagnostic.message}</div>
      )}
      {diagnostic.deactivated_at && (
        <div>
          {t("accountHealth.deactivatedAt")}: {" "}
          {new Date(diagnostic.deactivated_at).toLocaleString(locale)}
        </div>
      )}
      <div>
        {t("accountHealth.firstSeen")}:{" "}
        {new Date(diagnostic.first_seen_at).toLocaleString(locale)}
      </div>
      <div>
        {t("accountHealth.lastSeen")}:{" "}
        {new Date(diagnostic.last_seen_at).toLocaleString(locale)}
      </div>
      <div>{t("accountHealth.occurrences", { count: diagnostic.occurrence_count })}</div>
    </div>
  );
}

export function AccountCard({
  account,
  onSwitch,
  onWarmup,
  onDelete,
  onRefresh,
  onReauthorize,
  onReportDeactivation,
  onRevalidate,
  onRename,
  onToggleDisabled,
  onEditApiConfig,
  switching,
  switchDisabled,
  warmingUp,
  masked = false,
  onToggleMask,
  autoWarmupEnabled = false,
  autoWarmupManagedByAll = false,
  autoWarmupLabel,
  onToggleAutoWarmup,
  warmupFailure,
  onDismissWarmupFailure,
  statsDefaultOpen,
  compact = false,
}: AccountCardProps) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en-US";
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [isRevalidating, setIsRevalidating] = useState(false);
  const [isTogglingDisabled, setIsTogglingDisabled] = useState(false);
  const lastRefresh = account.usageUpdatedAt ? new Date(account.usageUpdatedAt) : null;
  const [isEditing, setIsEditing] = useState(false);
  const [editName, setEditName] = useState(account.name);
  const [resetCredits, setResetCredits] = useState<AccountResetCredits | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const managementMenuRef = useRef<HTMLDetailsElement>(null);
  const resetRequestSeq = useRef(0);

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isEditing]);

  const handleRefresh = async () => {
    setIsRefreshing(true);
    try {
      await onRefresh();
    } catch (error) {
      console.error("Failed to refresh account usage:", error);
    } finally {
      setIsRefreshing(false);
    }
  };

  const handleRevalidate = async () => {
    if (!onRevalidate) return;
    setIsRevalidating(true);
    try {
      await onRevalidate();
    } catch (error) {
      console.error("Failed to revalidate account:", error);
    } finally {
      setIsRevalidating(false);
    }
  };

  const handleRename = async () => {
    const trimmed = editName.trim();
    if (trimmed && trimmed !== account.name) {
      try {
        await onRename(trimmed);
      } catch {
        setEditName(account.name);
      }
    } else {
      setEditName(account.name);
    }
    setIsEditing(false);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      handleRename();
    } else if (e.key === "Escape") {
      setEditName(account.name);
      setIsEditing(false);
    }
  };

  const handleToggleDisabled = async () => {
    setIsTogglingDisabled(true);
    try {
      await onToggleDisabled();
    } catch (error) {
      console.error("Failed to change account disabled state:", error);
    } finally {
      setIsTogglingDisabled(false);
    }
  };

  const startRenaming = () => {
    if (masked) return;
    setEditName(account.name);
    setIsEditing(true);
  };

  const effectivePlanType = getEffectivePlanType(account);
  const planDisplay = effectivePlanType
    ? effectivePlanType.charAt(0).toUpperCase() + effectivePlanType.slice(1)
    : account.auth_mode === "api_key"
      ? t("accountCard.apiKey")
      : t("common.unknown");

  const planColors: Record<string, string> = {
    pro: "bg-indigo-50 text-indigo-700 border-indigo-200 dark:bg-indigo-900/30 dark:text-indigo-300 dark:border-indigo-700",
    plus: "bg-emerald-50 text-emerald-700 border-emerald-200 dark:bg-emerald-900/30 dark:text-emerald-300 dark:border-emerald-700",
    team: "bg-blue-50 text-blue-700 border-blue-200 dark:bg-blue-900/30 dark:text-blue-300 dark:border-blue-700",
    enterprise: "bg-amber-50 text-amber-700 border-amber-200 dark:bg-amber-900/30 dark:text-amber-300 dark:border-amber-700",
    free: "bg-gray-50 text-gray-600 border-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:border-gray-700",
    api_key: "bg-orange-50 text-orange-700 border-orange-200 dark:bg-orange-900/30 dark:text-orange-300 dark:border-orange-700",
  };

  const isApiKeyAccount = account.auth_mode === "api_key";
  const healthStatus = account.health?.status;
  const healthBlocksActions = accountHealthBlocksAccountActions(account);
  const healthHasProblem = Boolean(healthStatus && healthStatus !== "healthy");
  const showHealthPanel = Boolean(
    account.health && (healthHasProblem || account.health_history.length > 0)
  );
  const healthTone = healthStatus === "account_deactivated" || healthStatus === "workspace_deactivated"
    ? "border-red-200 bg-red-50 text-red-800 dark:border-red-900/70 dark:bg-red-950/35 dark:text-red-200"
    : healthStatus === "healthy"
      ? "border-emerald-200 bg-emerald-50 text-emerald-800 dark:border-emerald-900/70 dark:bg-emerald-950/30 dark:text-emerald-200"
      : "border-amber-200 bg-amber-50 text-amber-800 dark:border-amber-900/70 dark:bg-amber-950/35 dark:text-amber-200";
  const planKey = effectivePlanType?.toLowerCase() || (isApiKeyAccount ? "api_key" : "free");
  const planColorClass = planColors[planKey] || planColors.free;
  const supportsWarmup = !isApiKeyAccount && !account.disabled && !healthBlocksActions;
  const subscriptionStatus = getSubscriptionStatus(account.subscription_expires_at, t, locale);
  const compactResetCredits = !account.is_active;

  const loadResetCredits = useCallback(async () => {
    const requestId = ++resetRequestSeq.current;

    if (account.auth_mode !== "chat_g_p_t" || account.disabled) {
      setResetCredits(null);
      return;
    }

    try {
      const stats = await invokeBackend<AccountUsageStatsInfo>("get_account_usage_stats", {
        accountId: account.id,
      });
      if (requestId !== resetRequestSeq.current) return;
      setResetCredits(stats.account_id === account.id ? stats.reset_credits : null);
    } catch {
      if (requestId !== resetRequestSeq.current) return;
      setResetCredits(null);
    }
  }, [account.auth_mode, account.disabled, account.id]);

  const handleStatsLoaded = useCallback(
    (stats: AccountUsageStatsInfo | null) => {
      setResetCredits(stats?.account_id === account.id ? stats.reset_credits : null);
    },
    [account.id]
  );

  useEffect(() => {
    setResetCredits(null);

    if (account.auth_mode !== "chat_g_p_t" || account.disabled) {
      resetRequestSeq.current += 1;
      return;
    }

    void loadResetCredits();
    const timer = window.setInterval(() => {
      void loadResetCredits();
    }, RESET_CREDITS_REFRESH_INTERVAL_MS);

    return () => {
      resetRequestSeq.current += 1;
      window.clearInterval(timer);
    };
  }, [loadResetCredits]);


  return (
    <div
      className={`relative rounded-xl border transition-all duration-200 ${compact ? "p-4" : "p-5"} ${
        account.disabled
          ? "border-gray-200 bg-gray-50/80 dark:border-gray-800 dark:bg-gray-950/50"
          : healthStatus === "account_deactivated" || healthStatus === "workspace_deactivated"
          ? "border-red-300 bg-white dark:border-red-800 dark:bg-gray-900"
          : healthStatus === "reauth_required"
          ? "border-amber-300 bg-white dark:border-amber-800 dark:bg-gray-900"
          : account.is_active
          ? "bg-white dark:bg-gray-900 border-emerald-400 shadow-sm"
          : "bg-white dark:bg-gray-900 border-gray-200 dark:border-gray-700 hover:border-gray-300 dark:hover:border-gray-600"
      }`}
    >
      {/* Header */}
      <div className="flex items-start justify-between mb-3">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 mb-1">
            {account.is_active && (
              <span className="flex h-2 w-2">
                <span className="animate-ping absolute inline-flex h-2 w-2 rounded-full bg-green-400 opacity-75"></span>
                <span className="relative inline-flex rounded-full h-2 w-2 bg-green-500"></span>
              </span>
            )}
            {isEditing ? (
              <input
                ref={inputRef}
                type="text"
                value={editName}
                onChange={(e) => setEditName(e.target.value)}
                onBlur={handleRename}
                onKeyDown={handleKeyDown}
                className="font-semibold text-gray-900 dark:text-gray-100 bg-gray-100 dark:bg-gray-800 px-2 py-0.5 rounded border border-gray-300 dark:border-gray-700 focus:outline-none focus:border-gray-500 dark:focus:border-gray-500 w-full"
              />
            ) : (
              <h3
                className="min-w-0 truncate font-semibold text-gray-900 cursor-pointer hover:text-gray-600 dark:text-gray-100 dark:hover:text-gray-300"
                onClick={startRenaming}
                data-tooltip={masked ? undefined : t("accountCard.rename")}
              >
                <BlurredText blur={masked}>{account.name}</BlurredText>
              </h3>
            )}
            {!isEditing && (
              <button
                onClick={startRenaming}
                disabled={masked}
                className="shrink-0 rounded p-1 text-gray-400 transition-colors hover:bg-gray-100 hover:text-gray-700 disabled:cursor-not-allowed disabled:opacity-40 dark:text-gray-500 dark:hover:bg-gray-800 dark:hover:text-gray-200"
                aria-label={t("accountCard.rename")}
                data-tooltip={masked ? t("accountCard.showInfo") : t("accountCard.rename")}
              >
                <svg className="h-3.5 w-3.5" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true">
                  <path d="M4 13.5V16h2.5L15 7.5 12.5 5 4 13.5Z" strokeLinecap="round" strokeLinejoin="round" />
                  <path d="m11.5 6 2.5 2.5" strokeLinecap="round" />
                </svg>
              </button>
            )}
          </div>
          {account.email && (
            <p className="text-sm text-gray-500 dark:text-gray-400 truncate">
              <BlurredText blur={masked}>{account.email}</BlurredText>
            </p>
          )}
        </div>

        <div className="flex max-w-[60%] flex-wrap items-center justify-end gap-2">
          {/* Eye toggle */}
          {onToggleMask && (
            <button
              onClick={onToggleMask}
              className="p-1 text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 transition-colors"
              data-tooltip={masked ? t("accountCard.showInfo") : t("accountCard.hideInfo")}
            >
              {masked ? (
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21" />
                </svg>
              ) : (
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                </svg>
              )}
            </button>
          )}
          {/* Plan badge */}
          <span
            className={`px-2.5 py-1 text-xs font-medium rounded-full border ${planColorClass}`}
          >
            {planDisplay}
          </span>
          {account.disabled && (
            <span className="rounded-full border border-gray-300 bg-gray-100 px-2.5 py-1 text-xs font-medium text-gray-500 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-400">
              {t("accountCard.disabled")}
            </span>
          )}
          {healthHasProblem && account.health && (
            <span className={`rounded-full border px-2.5 py-1 text-xs font-medium ${
              healthStatus === "account_deactivated" || healthStatus === "workspace_deactivated"
                ? "border-red-200 bg-red-50 text-red-700 dark:border-red-800 dark:bg-red-950/40 dark:text-red-300"
                : "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-800 dark:bg-amber-950/40 dark:text-amber-300"
            }`}>
              {healthStatusLabel(account.health.status, t)}
            </span>
          )}
          <ResetCreditsMenu
            compact={compactResetCredits}
            resetCredits={resetCredits}
          />
        </div>
      </div>

      {warmupFailure && (
        <div className="mb-3 rounded-lg border border-red-200 bg-red-50 px-3 py-2.5 text-red-800 dark:border-red-900/70 dark:bg-red-950/35 dark:text-red-200">
          <div className="flex items-start gap-2">
            <span className="mt-0.5 shrink-0" aria-hidden="true">⚠</span>
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                <span className="text-xs font-semibold">
                  {warmupFailure.modelUnavailable
                    ? t("accountCard.warmupModelUnavailable")
                    : t("accountCard.warmupFailed")}
                </span>
                <span className="text-[11px] text-red-500 dark:text-red-400">
                  {new Date(warmupFailure.failedAt).toLocaleString(locale)}
                </span>
              </div>
              <p className="mt-1 break-words text-xs leading-5 text-red-700 dark:text-red-300">
                {warmupFailure.error}
              </p>
              {warmupFailure.modelUnavailable && (
                <p className="mt-1 text-xs font-medium text-red-700 dark:text-red-300">
                  {t("accountCard.autoWarmupPaused")}
                </p>
              )}
            </div>
            {onDismissWarmupFailure && (
              <button
                type="button"
                onClick={onDismissWarmupFailure}
                className="shrink-0 rounded p-1 text-red-400 transition-colors hover:bg-red-100 hover:text-red-700 dark:text-red-500 dark:hover:bg-red-900/50 dark:hover:text-red-200"
                aria-label={t("common.dismiss")}
                data-tooltip={t("common.dismiss")}
              >
                <svg className="h-3.5 w-3.5" viewBox="0 0 20 20" fill="none" stroke="currentColor" aria-hidden="true">
                  <path d="M5 5l10 10M15 5 5 15" strokeWidth="1.8" strokeLinecap="round" />
                </svg>
              </button>
            )}
          </div>
        </div>
      )}

      {!isApiKeyAccount && showHealthPanel && account.health && (
        <div className={`mb-3 rounded-lg border px-3 py-2.5 text-xs ${healthTone}`}>
          <div className="min-w-0">
            <div className="font-semibold">{healthStatusLabel(account.health.status, t)}</div>
            <div className="mt-1 opacity-80">
              {t("accountHealth.detectedAt", {
                time: new Date(account.health.last_seen_at).toLocaleString(locale),
              })}
            </div>
            {account.health.deactivated_at && (
              <div className="mt-1 font-medium">
                {t("accountHealth.deactivatedAt")}: {" "}
                {new Date(account.health.deactivated_at).toLocaleString(locale)}
              </div>
            )}
            {account.health.message && healthHasProblem && (
              <p className="mt-1 break-words leading-5 opacity-90">{account.health.message}</p>
            )}
          </div>
          <div className="mt-2 flex flex-wrap justify-end gap-2">
            {account.health.status === "reauth_required" && onReauthorize && (
              <button
                type="button"
                onClick={onReauthorize}
                className="rounded-lg bg-amber-600 px-3 py-1.5 font-medium text-white hover:bg-amber-700"
              >
                {t("accountHealth.reauthorize")}
              </button>
            )}
            {account.health.status !== "healthy" &&
              account.health.status !== "reauth_required" &&
              onRevalidate && (
                <button
                  type="button"
                  onClick={() => void handleRevalidate()}
                  disabled={isRevalidating}
                  className="rounded-lg border border-current/20 bg-white/60 px-3 py-1.5 font-medium hover:bg-white disabled:opacity-50 dark:bg-black/10 dark:hover:bg-black/20"
                >
                  {isRevalidating
                    ? t("accountHealth.revalidating")
                    : t("accountHealth.revalidate")}
                </button>
              )}
          </div>
          <details className="mt-2">
            <summary className="cursor-pointer select-none font-medium">
              {t("accountHealth.viewDiagnostics")}
            </summary>
            <div className="mt-2 space-y-2">
              <HealthDiagnosticDetails diagnostic={account.health} locale={locale} t={t} />
              {[...account.health_history].reverse().map((diagnostic, index) => (
                <HealthDiagnosticDetails
                  key={`${diagnostic.first_seen_at}-${diagnostic.status}-${index}`}
                  diagnostic={diagnostic}
                  locale={locale}
                  t={t}
                />
              ))}
            </div>
          </details>
        </div>
      )}

      {account.disabled ? (
        <div className="mb-3 rounded-lg border border-dashed border-gray-300 bg-white/60 px-3 py-2.5 text-xs text-gray-500 dark:border-gray-700 dark:bg-gray-900/50 dark:text-gray-400">
          {t("accountCard.disabledDescription")}
        </div>
      ) : isApiKeyAccount ? (
        <div className="mb-3 rounded-lg border border-gray-200 bg-gray-50/80 px-3 py-2.5 text-xs leading-5 text-gray-600 dark:border-gray-700 dark:bg-gray-800/60 dark:text-gray-400">
          {t("accountCard.apiUsageManagedExternally")}
        </div>
      ) : healthHasProblem ? null : (
        <>
          {/* Usage */}
          <div className="mb-3">
            <UsageBar usage={account.usage} loading={isRefreshing || account.usageLoading} />
          </div>

          {/* Last refresh time */}
          <div className="flex flex-wrap items-center justify-between gap-2 text-xs mb-3">
            <div className="text-gray-400 dark:text-gray-500">
              {t("accountCard.lastUpdated", { time: formatLastRefresh(lastRefresh, t, locale) })}
            </div>
          <div className={`text-right ${subscriptionStatus.className}`}>
            {subscriptionStatus.label}
          </div>
          </div>

          <AccountUsageStats
            accountId={account.id}
            enabled
            defaultOpen={statsDefaultOpen ?? account.is_active}
            onStatsLoaded={handleStatsLoaded}
          />
        </>
      )}

      {/* Actions */}
      <div className="mt-3 flex flex-wrap gap-2">
        {account.is_active ? (
          <button
            disabled
            className="min-w-32 flex-1 px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 dark:bg-gray-800 text-gray-500 dark:text-gray-400 border border-gray-200 dark:border-gray-700 cursor-default"
          >
            ✓ {t("accountCard.active")}
          </button>
        ) : (
          <button
            onClick={onSwitch}
            disabled={account.disabled || healthBlocksActions || switching || switchDisabled}
            className={`min-w-32 flex-1 px-4 py-2 text-sm font-medium rounded-lg transition-colors disabled:opacity-50 ${
              account.disabled || healthBlocksActions || switchDisabled
                ? "bg-gray-200 dark:bg-gray-800 text-gray-400 dark:text-gray-500 cursor-not-allowed"
                : "bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900"
            }`}
            data-tooltip={
              account.disabled
                ? t("accountCard.enableBeforeSwitch")
                : healthBlocksActions
                  ? t("accountHealth.resolveBeforeSwitch")
                : switchDisabled
                  ? t("accountCard.closeProcesses")
                  : undefined
            }
          >
            {account.disabled
              ? t("accountCard.disabled")
              : healthBlocksActions
                ? t("accountHealth.unavailable")
              : switching
                ? t("accountCard.switching")
                : switchDisabled
                  ? t("accountCard.codexRunning")
                  : t("accountCard.switch")}
          </button>
        )}
        {supportsWarmup && <button
          onClick={() => {
            void onWarmup();
          }}
          disabled={warmingUp}
          className={`px-3 py-2 text-sm rounded-lg transition-colors ${
            warmingUp
              ? "bg-amber-100 dark:bg-amber-900/30 text-amber-500 dark:text-amber-300"
              : "bg-amber-50 dark:bg-amber-900/20 hover:bg-amber-100 dark:hover:bg-amber-900/40 text-amber-700 dark:text-amber-300"
          }`}
          data-tooltip={warmingUp ? t("accountCard.warmupSending") : t("accountCard.warmupSend")}
        >
          ⚡
        </button>}
        {supportsWarmup && onToggleAutoWarmup && (
          <button
            onClick={onToggleAutoWarmup}
            disabled={autoWarmupManagedByAll}
            className={`px-3 py-2 text-xs font-medium rounded-lg transition-colors whitespace-nowrap ${
              autoWarmupEnabled
                ? "bg-emerald-50 dark:bg-emerald-900/20 text-emerald-700 dark:text-emerald-300"
                : "bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300"
            } disabled:opacity-60`}
            data-tooltip={
              warmupFailure?.modelUnavailable
                ? t("accountCard.autoWarmupPaused")
                : autoWarmupManagedByAll
                ? t("accountCard.autoAll")
                : autoWarmupEnabled
                  ? t("accountCard.autoDisable")
                : t("accountCard.autoEnable")
            }
          >
            {warmupFailure?.modelUnavailable
              ? t("warmup.autoPaused")
              : autoWarmupLabel ??
                (autoWarmupEnabled ? t("warmup.autoOn") : t("warmup.autoOff"))}
          </button>
        )}
        {!isApiKeyAccount && !account.disabled && <button
          onClick={handleRefresh}
          disabled={isRefreshing}
          className={`px-3 py-2 text-sm rounded-lg transition-colors ${
            isRefreshing
              ? "bg-gray-200 dark:bg-gray-800 text-gray-400 dark:text-gray-500"
              : "bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300"
          }`}
          data-tooltip={t("accountCard.refreshUsage")}
        >
          <span className={isRefreshing ? "animate-spin inline-block" : ""}>↻</span>
        </button>}
        {!compact && isApiKeyAccount && onEditApiConfig && (
          <button
            onClick={onEditApiConfig}
            className="px-3 py-2 text-sm rounded-lg bg-blue-50 dark:bg-blue-900/20 hover:bg-blue-100 dark:hover:bg-blue-900/40 text-blue-700 dark:text-blue-300 transition-colors"
            data-tooltip={t("accountCard.apiConfig")}
          >
            ⚙
          </button>
        )}
        {compact ? (
          <details ref={managementMenuRef} className="relative">
            <summary
              className="flex h-9 w-9 cursor-pointer list-none items-center justify-center rounded-lg bg-gray-100 text-gray-600 transition-colors hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:hover:bg-gray-700 [&::-webkit-details-marker]:hidden"
              aria-label={t("accountCard.manage")}
              data-tooltip={t("accountCard.manage")}
            >
              <svg className="h-4 w-4" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
                <circle cx="4" cy="10" r="1.4" />
                <circle cx="10" cy="10" r="1.4" />
                <circle cx="16" cy="10" r="1.4" />
              </svg>
            </summary>
            <div className="absolute bottom-11 right-0 z-20 w-44 rounded-xl border border-gray-200 bg-white p-1.5 shadow-xl dark:border-gray-700 dark:bg-gray-900">
              {isApiKeyAccount && onEditApiConfig && (
                <button
                  type="button"
                  onClick={() => {
                    managementMenuRef.current?.removeAttribute("open");
                    onEditApiConfig();
                  }}
                  className="w-full rounded-lg px-3 py-2 text-left text-sm text-gray-700 transition-colors hover:bg-gray-100 dark:text-gray-200 dark:hover:bg-gray-800"
                >
                  {t("accountCard.apiConfig")}
                </button>
              )}
              {!isApiKeyAccount && onReportDeactivation && (
                <button
                  type="button"
                  onClick={() => {
                    managementMenuRef.current?.removeAttribute("open");
                    onReportDeactivation();
                  }}
                  className="w-full rounded-lg px-3 py-2 text-left text-sm text-red-600 transition-colors hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950/40"
                >
                  {t("accountCard.reportDeactivation")}
                </button>
              )}
              <button
                type="button"
                onClick={() => {
                  managementMenuRef.current?.removeAttribute("open");
                  void handleToggleDisabled();
                }}
                disabled={isTogglingDisabled}
                className="w-full rounded-lg px-3 py-2 text-left text-sm text-gray-700 transition-colors hover:bg-gray-100 disabled:opacity-50 dark:text-gray-200 dark:hover:bg-gray-800"
              >
                {account.disabled ? t("accountCard.enableAccount") : t("accountCard.disableAccount")}
              </button>
              <button
                type="button"
                onClick={() => {
                  managementMenuRef.current?.removeAttribute("open");
                  onDelete();
                }}
                className="w-full rounded-lg px-3 py-2 text-left text-sm text-red-600 transition-colors hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950/40"
              >
                {t("accountCard.remove")}
              </button>
            </div>
          </details>
        ) : (
          <>
            {!isApiKeyAccount && onReportDeactivation && (
              <button
                type="button"
                onClick={onReportDeactivation}
                className="whitespace-nowrap rounded-lg bg-red-50 px-3 py-2 text-xs font-medium text-red-600 transition-colors hover:bg-red-100 dark:bg-red-900/20 dark:text-red-300 dark:hover:bg-red-900/40"
                data-tooltip={t("accountCard.reportDeactivation")}
              >
                {t("accountCard.reportDeactivation")}
              </button>
            )}
            <button
              onClick={() => void handleToggleDisabled()}
              disabled={isTogglingDisabled}
              className={`whitespace-nowrap rounded-lg px-3 py-2 text-xs font-medium transition-colors disabled:opacity-50 ${
                account.disabled
                  ? "bg-emerald-50 text-emerald-700 hover:bg-emerald-100 dark:bg-emerald-900/20 dark:text-emerald-300 dark:hover:bg-emerald-900/40"
                  : "bg-gray-100 text-gray-600 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:hover:bg-gray-700"
              }`}
              data-tooltip={
                account.disabled
                  ? t("accountCard.enableAccount")
                  : t("accountCard.disableAccount")
              }
            >
              {account.disabled ? t("accountCard.enable") : t("accountCard.disable")}
            </button>
            <button
              onClick={onDelete}
              className="px-3 py-2 text-sm rounded-lg bg-red-50 dark:bg-red-900/20 hover:bg-red-100 dark:hover:bg-red-900/40 text-red-600 dark:text-red-300 transition-colors"
              data-tooltip={t("accountCard.remove")}
            >
              ✕
            </button>
          </>
        )}
      </div>
    </div>
  );
}
