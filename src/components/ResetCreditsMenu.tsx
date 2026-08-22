import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AccountResetCredits } from "../types";
import {
  formatResetCreditDate,
  formatResetCreditDateTime,
  getAvailableResetCredits,
} from "../lib/resetCredits";

function getResetCreditsTone(resetCredits: AccountResetCredits | null): {
  container: string;
  badge: string;
  text: string;
} {
  const fallback = {
    container: "border-sky-200 bg-sky-50/70 dark:border-sky-800 dark:bg-sky-950/30",
    badge: "border-sky-200 bg-sky-100 text-sky-700 dark:border-sky-700 dark:bg-sky-900/50 dark:text-sky-300",
    text: "text-sky-700/80 dark:text-sky-300/80",
  };

  if (!resetCredits?.next_expires_at) return fallback;

  const expiry = new Date(resetCredits.next_expires_at);
  if (Number.isNaN(expiry.getTime())) return fallback;

  const remainingMs = expiry.getTime() - Date.now();
  const dayMs = 24 * 60 * 60 * 1000;

  if (remainingMs <= 3 * dayMs) {
    return {
      container: "border-red-200 bg-red-50/70 dark:border-red-800 dark:bg-red-950/30",
      badge: "border-red-200 bg-red-100 text-red-700 dark:border-red-700 dark:bg-red-900/50 dark:text-red-300",
      text: "text-red-700/80 dark:text-red-300/80",
    };
  }

  if (remainingMs <= 10 * dayMs) {
    return {
      container: "border-amber-200 bg-amber-50/70 dark:border-amber-800 dark:bg-amber-950/30",
      badge: "border-amber-200 bg-amber-100 text-amber-700 dark:border-amber-700 dark:bg-amber-900/50 dark:text-amber-300",
      text: "text-amber-700/80 dark:text-amber-300/80",
    };
  }

  return fallback;
}

export function ResetCreditsMenu({
  variant,
  resetCredits,
}: {
  variant: "card" | "compact" | "list";
  resetCredits: AccountResetCredits | null;
}) {
  const { t, i18n } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const popupId = useId();
  const availableCredits = getAvailableResetCredits(resetCredits);
  const count = availableCredits.length;
  const countLabel = t("accountCard.resetCount", { count });
  const locale = i18n.resolvedLanguage ?? "en-US";
  const compact = variant !== "card";
  const nextExpiry = formatResetCreditDateTime(
    availableCredits[0]?.expires_at ?? null,
    { compact, locale },
  );
  const nextExpiryLabel =
    nextExpiry === "No expiry"
      ? t("accountCard.resetNoExpiry")
      : nextExpiry === "Expiry unavailable"
        ? t("accountCard.resetExpiryUnavailable")
        : t(compact ? "accountCard.closest" : "accountCard.closestExpires", {
            date: nextExpiry,
          });
  const tone = getResetCreditsTone(resetCredits);
  const listExpiryValue = formatResetCreditDate(availableCredits[0]?.expires_at ?? null, {
    locale,
  });
  const listExpiry = listExpiryValue === "No expiry"
    ? t("accountCard.resetNoExpiry")
    : listExpiryValue === "Expiry unavailable"
      ? t("accountCard.resetExpiryUnavailable")
      : listExpiryValue;

  useEffect(() => {
    if (!isOpen) return;

    const handlePointerDown = (event: MouseEvent) => {
      if (!wrapperRef.current?.contains(event.target as Node)) {
        setIsOpen(false);
      }
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setIsOpen(false);
      buttonRef.current?.focus();
    };

    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [isOpen]);

  useEffect(() => {
    setIsOpen(false);
  }, [resetCredits]);

  if (count === 0) return null;

  return (
    <div ref={wrapperRef} className={`relative min-w-0 max-w-full ${variant === "list" ? "group h-full justify-self-end" : ""}`}>
      <button
        ref={buttonRef}
        type="button"
        aria-expanded={variant === "list" ? undefined : isOpen}
        aria-controls={popupId}
        aria-haspopup="dialog"
        onClick={variant === "list" ? undefined : () => setIsOpen((open) => !open)}
        className={variant === "list"
          ? "flex h-full min-w-0 flex-col justify-center rounded-lg px-1.5 py-1.5 text-left transition-colors hover:bg-gray-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500 md:text-right dark:hover:bg-gray-800"
          : compact
            ? `flex min-w-0 max-w-full items-center gap-1.5 rounded-full border px-2 py-1 text-[11px] leading-none transition-colors hover:brightness-95 focus:outline-none focus:ring-2 focus:ring-sky-400/60 ${tone.container} ${tone.text}`
            : `flex max-w-full items-center gap-2 rounded-lg border px-2 py-1.5 text-xs transition-colors hover:brightness-95 focus:outline-none focus:ring-2 focus:ring-sky-400/60 ${tone.container}`}
        title={variant === "list"
          ? undefined
          : `${countLabel} · ${nextExpiryLabel} · ${t("accountCard.resetDetails")}`}
      >
        {variant === "list" ? (
          <>
            <span className="w-full truncate text-[11px] font-medium uppercase tracking-wide text-gray-400 dark:text-gray-500">
              {countLabel}
            </span>
            <span className={`mt-1 w-full truncate text-xs font-medium ${tone.text}`}>
              {listExpiry}
            </span>
          </>
        ) : (
          <>
            <span
              className={compact
                ? "shrink-0 whitespace-nowrap font-semibold"
                : `whitespace-nowrap rounded-full border px-2.5 py-0.5 font-medium ${tone.badge}`}
            >
              {countLabel}
            </span>
            <span className={`truncate ${compact ? "" : tone.text}`}>
              · {nextExpiryLabel}
            </span>
          </>
        )}
        {variant !== "list" && (
          <svg
            className={`h-3 w-3 shrink-0 transition-transform ${isOpen ? "rotate-180" : ""}`}
            viewBox="0 0 12 12"
            fill="none"
            aria-hidden="true"
          >
            <path
              d="m3 4.5 3 3 3-3"
              stroke="currentColor"
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth="1.5"
            />
          </svg>
        )}
      </button>

      {(variant === "list" || isOpen) && (
        <div
          id={popupId}
          role="dialog"
          aria-label={t("accountCard.resetDetails")}
          className={`absolute right-0 top-full z-30 mt-2 w-80 max-w-[calc(100vw-3rem)] overflow-hidden rounded-xl border border-gray-200 bg-white text-left shadow-xl transition-all dark:border-gray-700 dark:bg-gray-900 ${
            variant === "list"
              ? "pointer-events-none invisible translate-y-1 opacity-0 group-hover:pointer-events-auto group-hover:visible group-hover:translate-y-0 group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:visible group-focus-within:translate-y-0 group-focus-within:opacity-100"
              : ""
          }`}
        >
          <div className="flex items-center justify-between border-b border-gray-100 px-3 py-2.5 dark:border-gray-800">
            <span className="text-xs font-semibold text-gray-900 dark:text-gray-100">
              {t("accountCard.availableResets")}
            </span>
            <span className="text-[11px] text-gray-500 dark:text-gray-400">
              {count}
            </span>
          </div>
          <div className="max-h-64 overflow-y-auto py-1">
            {availableCredits.map((credit, index) => {
              const expiry = formatResetCreditDateTime(credit.expires_at, { locale });
              const expiryLabel =
                expiry === "No expiry"
                  ? t("accountCard.resetNoExpiry")
                  : expiry === "Expiry unavailable"
                    ? t("accountCard.resetExpiryUnavailable")
                    : t("accountCard.resetExpires", { date: expiry });

              return (
                <div
                  key={credit.id}
                  className="flex items-start gap-2.5 px-3 py-2.5 text-xs"
                >
                  <span
                    className={`flex h-5 w-5 shrink-0 items-center justify-center rounded-full font-semibold ${tone.badge}`}
                  >
                    {index + 1}
                  </span>
                  <div className="min-w-0">
                    <div className="truncate font-medium text-gray-800 dark:text-gray-200">
                      {credit.title?.trim() || t("accountCard.resetItem", { count: index + 1 })}
                    </div>
                    <div className="mt-0.5 text-[11px] text-gray-500 dark:text-gray-400">
                      {expiryLabel}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          <div className="border-t border-gray-100 px-3 py-2 text-[10px] text-gray-400 dark:border-gray-800 dark:text-gray-500">
            {t("accountCard.resetTimesLocal")}
          </div>
        </div>
      )}
    </div>
  );
}
