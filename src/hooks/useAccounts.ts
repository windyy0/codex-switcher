import { useState, useEffect, useCallback, useRef } from "react";
import type {
  AccountInfo,
  UsageInfo,
  AccountWithUsage,
  WarmupSummary,
  ImportAccountsSummary,
} from "../types";
import { invokeBackend, isTauriRuntime, type FileSource } from "../lib/platform";

export function useAccounts() {
  const [accounts, setAccounts] = useState<AccountWithUsage[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const accountsRef = useRef<AccountWithUsage[]>([]);
  const maxConcurrentUsageRequests = 10;

  useEffect(() => {
    accountsRef.current = accounts;
  }, [accounts]);

  const buildUsageError = useCallback(
    (accountId: string, message: string, planType: string | null): UsageInfo => ({
      account_id: accountId,
      plan_type: planType,
      primary_used_percent: null,
      primary_window_minutes: null,
      primary_resets_at: null,
      secondary_used_percent: null,
      secondary_window_minutes: null,
      secondary_resets_at: null,
      has_credits: null,
      unlimited_credits: null,
      credits_balance: null,
      error: message,
    }),
    []
  );

  // Push freshly polled usage down to the tray (single poller feeds the tray menu).
  const reportUsageToTray = useCallback((usages: UsageInfo[]) => {
    if (!isTauriRuntime() || usages.length === 0) return;
    void invokeBackend("report_usage", { usages }).catch(() => {});
  }, []);

  const runWithConcurrency = useCallback(
    async <T,>(
      items: T[],
      worker: (item: T) => Promise<void>,
      concurrency: number
    ) => {
      if (items.length === 0) return;
      const limit = Math.min(Math.max(concurrency, 1), items.length);
      let index = 0;
      const runners = Array.from({ length: limit }, async () => {
        while (true) {
          const current = index++;
          if (current >= items.length) return;
          await worker(items[current]);
        }
      });
      await Promise.allSettled(runners);
    },
    []
  );

  const loadAccounts = useCallback(async (preserveUsage = false) => {
    try {
      setLoading(true);
      setError(null);
      const accountList = await invokeBackend<AccountInfo[]>("list_accounts");
      
      if (preserveUsage) {
        // Preserve existing usage data when just updating account info
        setAccounts((prev) => {
          const usageMap = new Map(
            prev.map((a) => [a.id, {
              usage: a.usage,
              usageLoading: a.usageLoading,
              usageUpdatedAt: a.usageUpdatedAt,
            }])
          );
          return accountList.map((a) => ({
            ...a,
            usage: a.auth_mode === "chat_g_p_t" && !a.disabled
              ? usageMap.get(a.id)?.usage
              : undefined,
            usageLoading: a.auth_mode === "chat_g_p_t" && !a.disabled
              ? usageMap.get(a.id)?.usageLoading
              : false,
            usageUpdatedAt: a.auth_mode === "chat_g_p_t" && !a.disabled
              ? usageMap.get(a.id)?.usageUpdatedAt
              : undefined,
          }));
        });
      } else {
        setAccounts(accountList.map((a) => ({ ...a, usageLoading: false })));
      }
      return accountList;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshUsage = useCallback(
    async (
      accountList?: AccountInfo[] | AccountWithUsage[],
      options?: { refreshMetadata?: boolean }
    ) => {
      try {
        let list = accountList ?? accountsRef.current;
        if (list.length === 0) {
          return;
        }

        // API key providers do not expose the ChatGPT subscription/rate-limit
        // endpoints. Keep them out of both metadata and usage polling instead
        // of manufacturing a recurring "usage unavailable" failure.
        list = list.filter(
          (account) => account.auth_mode === "chat_g_p_t" && !account.disabled
        );

        setAccounts((prev) =>
          prev.map((account) =>
            account.auth_mode === "api_key" || account.disabled
              ? { ...account, usage: undefined, usageLoading: false, usageUpdatedAt: undefined }
              : account
          )
        );

        if (list.length === 0) {
          return;
        }

        if (options?.refreshMetadata) {
          await runWithConcurrency(
            list,
            async (account) => {
              await invokeBackend<AccountInfo>("refresh_account_metadata", {
                accountId: account.id,
              });
            },
            maxConcurrentUsageRequests
          );

          list = (await loadAccounts(true)).filter(
            (account) => account.auth_mode === "chat_g_p_t"
          );
        }

        const accountIds = list.map((account) => account.id);
        const accountIdSet = new Set(accountIds);
        const usageResults = new Map<string, UsageInfo>();

        setAccounts((prev) =>
          prev.map((account) =>
            accountIdSet.has(account.id)
              ? { ...account, usageLoading: true }
              : account
          )
        );

        await runWithConcurrency(
          list,
          async (account) => {
            try {
              const usage = await invokeBackend<UsageInfo>("get_usage", {
                accountId: account.id,
                forceRefresh: Boolean(options?.refreshMetadata),
              });
              usageResults.set(account.id, usage);
            } catch (err) {
              console.error("Failed to refresh usage:", err);
              const message = err instanceof Error ? err.message : String(err);
              usageResults.set(
                account.id,
                buildUsageError(account.id, message, account.plan_type ?? null)
              );
            }
          },
          maxConcurrentUsageRequests
        );

        const refreshedAt = Date.now();
        setAccounts((prev) =>
          prev.map((account) => {
            const usage = usageResults.get(account.id);
            if (!usage) return account;
            return {
              ...account,
              usage,
              usageLoading: false,
              usageUpdatedAt: usage.error ? account.usageUpdatedAt : refreshedAt,
            };
          })
        );

        reportUsageToTray(Array.from(usageResults.values()));
      } catch (err) {
        console.error("Failed to refresh usage:", err);
        throw err;
      }
    },
    [buildUsageError, loadAccounts, maxConcurrentUsageRequests, reportUsageToTray, runWithConcurrency]
  );

  const refreshSingleUsage = useCallback(async (
    accountId: string,
    options?: { refreshMetadata?: boolean }
  ) => {
    const account = accountsRef.current.find((candidate) => candidate.id === accountId);
    if (account?.disabled) {
      throw new Error("Account is disabled");
    }
    if (account?.auth_mode === "api_key") {
      setAccounts((prev) =>
        prev.map((candidate) =>
          candidate.id === accountId
            ? { ...candidate, usage: undefined, usageLoading: false }
            : candidate
        )
      );
      throw new Error("Usage refresh is not supported for API key accounts");
    }

    try {
      if (options?.refreshMetadata) {
        try {
          await invokeBackend<AccountInfo>("refresh_account_metadata", { accountId });
          await loadAccounts(true);
        } catch (err) {
          // Subscription metadata is supplemental to the usage display. Keep the
          // per-account refresh usable when this backend endpoint is unavailable.
          console.warn("Failed to refresh account metadata; continuing with usage refresh:", err);
        }
      }

      setAccounts((prev) =>
        prev.map((a) =>
          a.id === accountId ? { ...a, usageLoading: true } : a
        )
      );
      const usage = await invokeBackend<UsageInfo>("get_usage", { accountId, forceRefresh: true });
      const refreshedAt = Date.now();
      setAccounts((prev) =>
        prev.map((a) =>
          a.id === accountId
            ? {
                ...a,
                usage,
                usageLoading: false,
                usageUpdatedAt: usage.error ? a.usageUpdatedAt : refreshedAt,
              }
            : a
        )
      );
      reportUsageToTray([usage]);
      return usage;
    } catch (err) {
      console.error("Failed to refresh single usage:", err);
      const message = err instanceof Error ? err.message : String(err);
      setAccounts((prev) =>
        prev.map((a) =>
          a.id === accountId
            ? {
                ...a,
                usage: buildUsageError(accountId, message, a.plan_type ?? null),
                usageLoading: false,
              }
            : a
        )
      );
      throw err;
    }
  }, [buildUsageError, loadAccounts, reportUsageToTray]);

  const warmupAccount = useCallback(async (accountId: string) => {
    try {
      await invokeBackend("warmup_account", { accountId });
    } catch (err) {
      console.error("Failed to warm up account:", err);
      throw err;
    }
  }, []);

  const warmupAllAccounts = useCallback(async () => {
    try {
      return await invokeBackend<WarmupSummary>("warmup_all_accounts");
    } catch (err) {
      console.error("Failed to warm up all accounts:", err);
      throw err;
    }
  }, []);

  const switchAccount = useCallback(
    async (accountId: string) => {
      try {
        await invokeBackend("switch_account", { accountId });
        void loadAccounts(true).catch((err) => {
          console.error("Account switched but the list could not be reloaded:", err);
        });
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts]
  );

  const deleteAccount = useCallback(
    async (accountId: string) => {
      try {
        await invokeBackend("delete_account", { accountId });
        void loadAccounts().catch((err) => {
          console.error("Account deleted but the list could not be reloaded:", err);
        });
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts]
  );

  const renameAccount = useCallback(
    async (accountId: string, newName: string) => {
      try {
        await invokeBackend("rename_account", { accountId, newName });
        void loadAccounts(true).catch((err) => {
          console.error("Account renamed but the list could not be reloaded:", err);
        });
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts]
  );

  const setAccountDisabled = useCallback(
    async (accountId: string, disabled: boolean) => {
      await invokeBackend<AccountInfo>("set_account_disabled", { accountId, disabled });
      await loadAccounts(true);
    },
    [loadAccounts]
  );

  const importFromFile = useCallback(
    async (source: FileSource, name: string) => {
      try {
        if (typeof source === "string") {
          await invokeBackend<AccountInfo>("add_account_from_file", { path: source, name });
        } else {
          const contents = await source.text();
          await invokeBackend<AccountInfo>("add_account_from_auth_json_text", {
            name,
            contents,
          });
        }
        void loadAccounts()
          .then((accountList) => refreshUsage(accountList))
          .catch((err) => console.error("Failed to refresh accounts after import:", err));
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts, refreshUsage]
  );

  const addApiAccount = useCallback(
    async (name: string, apiKey: string, config: string) => {
      await invokeBackend<AccountInfo>("add_api_account", {
        name,
        apiKey,
        config: config.trim() || null,
      });
      void loadAccounts()
        .then((accountList) => refreshUsage(accountList))
        .catch((err) => console.error("Failed to refresh accounts after API account creation:", err));
    },
    [loadAccounts, refreshUsage]
  );

  const startOAuthLogin = useCallback(async (accountName: string) => {
    try {
      const info = await invokeBackend<{ auth_url: string; callback_port: number }>(
        "start_login",
        { accountName }
      );
      return info;
    } catch (err) {
      throw err;
    }
  }, []);

  const completeOAuthLogin = useCallback(async () => {
    try {
      const account = await invokeBackend<AccountInfo>("complete_login");
      void loadAccounts()
        .then((accountList) => refreshUsage(accountList))
        .catch((err) => console.error("Failed to refresh accounts after login:", err));
      return account;
    } catch (err) {
      throw err;
    }
  }, [loadAccounts, refreshUsage]);

  const exportAccountsSlimText = useCallback(async () => {
    try {
      return await invokeBackend<string>("export_accounts_slim_text");
    } catch (err) {
      throw err;
    }
  }, []);

  const importAccountsSlimText = useCallback(
    async (payload: string) => {
      try {
        const summary = await invokeBackend<ImportAccountsSummary>("import_accounts_slim_text", {
          payload,
        });
        void loadAccounts()
          .then((accountList) => refreshUsage(accountList))
          .catch((err) => console.error("Slim import succeeded but account refresh failed:", err));
        return summary;
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts, refreshUsage]
  );

  const exportAccountsFullEncryptedFile = useCallback(
    async (path: string) => {
      try {
        await invokeBackend("export_accounts_full_encrypted_file", { path });
      } catch (err) {
        throw err;
      }
    },
    []
  );

  const importAccountsFullEncryptedFile = useCallback(
    async (path: string) => {
      try {
        const summary = await invokeBackend<ImportAccountsSummary>(
          "import_accounts_full_encrypted_file",
          { path }
        );
        void loadAccounts()
          .then((accountList) => refreshUsage(accountList))
          .catch((err) => console.error("Full import succeeded but account refresh failed:", err));
        return summary;
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts, refreshUsage]
  );

  const cancelOAuthLogin = useCallback(async () => {
    try {
      await invokeBackend("cancel_login");
    } catch (err) {
      console.error("Failed to cancel login:", err);
    }
  }, []);

  const loadMaskedAccountIds = useCallback(async () => {
    try {
      return await invokeBackend<string[]>("get_masked_account_ids");
    } catch (err) {
      console.error("Failed to load masked account IDs:", err);
      return [];
    }
  }, []);

  const saveMaskedAccountIds = useCallback(async (ids: string[]) => {
    try {
      await invokeBackend("set_masked_account_ids", { ids });
    } catch (err) {
      console.error("Failed to save masked account IDs:", err);
    }
  }, []);

  useEffect(() => {
    void loadAccounts()
      .then((accountList) => refreshUsage(accountList))
      .catch((err) => console.error("Failed to load accounts:", err));
    
    // Auto-refresh usage every 60 seconds (same as official Codex CLI)
    const interval = setInterval(() => {
      refreshUsage().catch(() => {});
    }, 60000);
    
    return () => clearInterval(interval);
  }, [loadAccounts, refreshUsage]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    void (async () => {
      if (!("__TAURI_INTERNALS__" in window)) return;
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen("accounts-changed", () => {
        void loadAccounts(true).catch((err) => {
          console.error("Failed to reload accounts after change:", err);
        });
      });
    })();

    return () => unlisten?.();
  }, [loadAccounts]);

  return {
    accounts,
    loading,
    error,
    loadAccounts,
    refreshUsage,
    refreshSingleUsage,
    warmupAccount,
    warmupAllAccounts,
    switchAccount,
    deleteAccount,
    renameAccount,
    setAccountDisabled,
    importFromFile,
    addApiAccount,
    exportAccountsSlimText,
    importAccountsSlimText,
    exportAccountsFullEncryptedFile,
    importAccountsFullEncryptedFile,
    startOAuthLogin,
    completeOAuthLogin,
    cancelOAuthLogin,
    loadMaskedAccountIds,
    saveMaskedAccountIds,
  };
}
