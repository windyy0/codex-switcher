import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  AccountResetCredits,
  AccountUsageStats,
  AccountWithUsage,
} from "../types";
import { invokeBackend } from "../lib/platform";

const RESET_CREDITS_REFRESH_INTERVAL_MS = 6 * 60 * 60 * 1000;

type ResetCreditsByAccount = Record<string, AccountResetCredits | null>;

export function useResetCredits(accounts: AccountWithUsage[]) {
  const [byAccount, setByAccount] = useState<ResetCreditsByAccount>({});
  const byAccountRef = useRef(byAccount);
  const lastFetchedAtRef = useRef<Record<string, number>>({});
  const inFlightRef = useRef(new Map<string, Promise<AccountResetCredits | null>>());

  const eligibleAccountIds = useMemo(
    () => accounts
      .filter((account) => account.auth_mode === "chat_g_p_t" && !account.disabled)
      .map((account) => account.id),
    [accounts],
  );
  const eligibleAccountKey = eligibleAccountIds.join("\0");

  const storeResetCredits = useCallback(
    (accountId: string, resetCredits: AccountResetCredits | null) => {
      lastFetchedAtRef.current[accountId] = Date.now();
      if (byAccountRef.current[accountId] === resetCredits) return;

      const next = { ...byAccountRef.current, [accountId]: resetCredits };
      byAccountRef.current = next;
      setByAccount(next);
    },
    [],
  );

  const refreshResetCredits = useCallback(
    async (accountId: string, force = false): Promise<AccountResetCredits | null> => {
      const lastFetchedAt = lastFetchedAtRef.current[accountId] ?? 0;
      if (!force && Date.now() - lastFetchedAt < RESET_CREDITS_REFRESH_INTERVAL_MS) {
        return byAccountRef.current[accountId] ?? null;
      }

      const existing = inFlightRef.current.get(accountId);
      if (existing) return existing;

      const request = invokeBackend<AccountUsageStats>("get_account_usage_stats", { accountId })
        .then((stats) => {
          const resetCredits = stats.account_id === accountId ? stats.reset_credits : null;
          storeResetCredits(accountId, resetCredits);
          return resetCredits;
        })
        .finally(() => {
          inFlightRef.current.delete(accountId);
        });

      inFlightRef.current.set(accountId, request);
      return request;
    },
    [storeResetCredits],
  );

  useEffect(() => {
    for (const accountId of eligibleAccountIds) {
      void refreshResetCredits(accountId).catch((error) => {
        console.warn("Failed to refresh account reset credits:", error);
      });
    }
  }, [eligibleAccountKey, refreshResetCredits]);

  useEffect(() => {
    if (eligibleAccountIds.length === 0) return;

    const timer = window.setInterval(() => {
      for (const accountId of eligibleAccountIds) {
        void refreshResetCredits(accountId, true).catch((error) => {
          console.warn("Failed to refresh account reset credits:", error);
        });
      }
    }, RESET_CREDITS_REFRESH_INTERVAL_MS);

    return () => window.clearInterval(timer);
  }, [eligibleAccountKey, refreshResetCredits]);

  return {
    resetCreditsByAccount: byAccount,
    refreshResetCredits,
    storeResetCredits,
  };
}
