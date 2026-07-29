import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AccountInfo, UsageInfo } from "../types";
import { openExternalUrl } from "../lib/platform";

interface ReauthAccountModalProps {
  account: AccountInfo | null;
  onClose: () => void;
  onStart: (
    accountName: string,
    targetAccountId: string
  ) => Promise<{ auth_url: string }>;
  onComplete: (targetAccountId: string) => Promise<unknown>;
  onValidate: (accountId: string) => Promise<UsageInfo>;
  onReportPageError: (accountId: string, errorText: string) => Promise<unknown>;
  onCancel: () => Promise<void>;
}

type ReauthPhase =
  | "idle"
  | "starting"
  | "waiting"
  | "validating"
  | "success"
  | "warning"
  | "reported";

export function ReauthAccountModal({
  account,
  onClose,
  onStart,
  onComplete,
  onValidate,
  onReportPageError,
  onCancel,
}: ReauthAccountModalProps) {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<ReauthPhase>("idle");
  const [authUrl, setAuthUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pageErrorText, setPageErrorText] = useState("");
  const [pageError, setPageError] = useState<string | null>(null);
  const [isReportingPageError, setIsReportingPageError] = useState(false);
  const attemptRef = useRef(0);
  const reportingPageErrorRef = useRef(false);

  useEffect(() => {
    attemptRef.current += 1;
    setPhase("idle");
    setAuthUrl("");
    setError(null);
    setPageErrorText("");
    setPageError(null);
    setIsReportingPageError(false);
    reportingPageErrorRef.current = false;
  }, [account?.id]);

  if (!account) return null;

  const close = () => {
    attemptRef.current += 1;
    if (phase === "starting" || phase === "waiting") {
      void onCancel().catch((cancelError) => {
        console.error("Failed to cancel reauthorization:", cancelError);
      });
    }
    onClose();
  };

  const start = async () => {
    const attempt = ++attemptRef.current;
    setPhase("starting");
    setError(null);
    setAuthUrl("");
    setPageErrorText("");
    setPageError(null);

    try {
      const info = await onStart(account.name, account.id);
      if (attemptRef.current !== attempt) return;
      setAuthUrl(info.auth_url);
      setPhase("waiting");
      void openExternalUrl(info.auth_url).catch((openError) => {
        console.warn("Could not open the authorization URL automatically:", openError);
      });

      await onComplete(account.id);
      if (attemptRef.current !== attempt) return;
      setPhase("validating");

      try {
        const usage = await onValidate(account.id);
        if (attemptRef.current !== attempt) return;
        if (usage.error) {
          setError(usage.error);
          setPhase("warning");
        } else {
          setPhase("success");
        }
      } catch (validationError) {
        if (attemptRef.current !== attempt) return;
        setError(
          validationError instanceof Error
            ? validationError.message
            : String(validationError)
        );
        setPhase("warning");
      }
    } catch (reauthError) {
      if (attemptRef.current !== attempt) return;
      if (reportingPageErrorRef.current) return;
      setError(reauthError instanceof Error ? reauthError.message : String(reauthError));
      setPhase("idle");
    }
  };

  const pastePageError = async () => {
    setPageError(null);
    try {
      const text = await navigator.clipboard.readText();
      if (!text.trim()) {
        setPageError(t("reauth.clipboardEmpty"));
        return;
      }
      setPageErrorText(text);
    } catch {
      setPageError(t("reauth.clipboardReadFailed"));
    }
  };

  const reportPageError = async (reportedText: string) => {
    if (isReportingPageError || !reportedText.trim()) return;
    reportingPageErrorRef.current = true;
    setIsReportingPageError(true);
    setPageError(null);

    try {
      await onReportPageError(account.id, reportedText);
      attemptRef.current += 1;
      setAuthUrl("");
      setPhase("reported");
    } catch (reportError) {
      setPageError(reportError instanceof Error ? reportError.message : String(reportError));
    } finally {
      reportingPageErrorRef.current = false;
      setIsReportingPageError(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      role="dialog"
      aria-modal="true"
      aria-labelledby="reauth-account-title"
    >
      <div className="mx-4 max-h-[90vh] w-full max-w-lg overflow-y-auto rounded-2xl border border-gray-200 bg-white shadow-xl dark:border-gray-700 dark:bg-gray-900">
        <div className="flex items-start justify-between gap-4 border-b border-gray-100 p-5 dark:border-gray-800">
          <div>
            <h2
              id="reauth-account-title"
              className="text-lg font-semibold text-gray-900 dark:text-gray-100"
            >
              {t("reauth.title")}
            </h2>
            <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">{account.name}</p>
          </div>
          <button
            type="button"
            onClick={close}
            className="text-gray-400 transition-colors hover:text-gray-600 dark:hover:text-gray-300"
            aria-label={t("common.close")}
          >
            ✕
          </button>
        </div>

        <div className="space-y-4 p-5">
          <p className="text-sm leading-6 text-gray-600 dark:text-gray-300">
            {t("reauth.description")}
          </p>

          {(phase === "waiting" || phase === "validating") && (
            <div className="rounded-xl border border-blue-200 bg-blue-50 p-3 text-sm text-blue-800 dark:border-blue-800 dark:bg-blue-950/35 dark:text-blue-200">
              {phase === "waiting" ? t("reauth.waiting") : t("reauth.validating")}
            </div>
          )}

          {phase === "success" && (
            <div className="rounded-xl border border-emerald-200 bg-emerald-50 p-3 text-sm text-emerald-800 dark:border-emerald-800 dark:bg-emerald-950/35 dark:text-emerald-200">
              {t("reauth.success")}
            </div>
          )}

          {phase === "reported" && (
            <div className="rounded-xl border border-red-200 bg-red-50 p-3 text-sm text-red-800 dark:border-red-800 dark:bg-red-950/35 dark:text-red-200">
              {t("reauth.pageErrorRecorded")}
            </div>
          )}

          {phase === "warning" && (
            <div className="rounded-xl border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800 dark:border-amber-800 dark:bg-amber-950/35 dark:text-amber-200">
              <p className="font-medium">{t("reauth.savedButValidationFailed")}</p>
              {error && <p className="mt-1 break-words text-xs leading-5">{error}</p>}
            </div>
          )}

          {phase === "idle" && error && (
            <div
              className="rounded-xl border border-red-200 bg-red-50 p-3 text-sm text-red-700 dark:border-red-800 dark:bg-red-950/35 dark:text-red-200"
              role="alert"
            >
              {t("reauth.failed", { error })}
            </div>
          )}

          {authUrl && phase === "waiting" && (
            <>
              <div className="space-y-2">
                <label className="text-xs font-medium text-gray-500 dark:text-gray-400">
                  {t("reauth.loginLink")}
                </label>
                <div className="flex gap-2">
                  <input
                    readOnly
                    value={authUrl}
                    className="min-w-0 flex-1 rounded-lg border border-gray-200 bg-gray-50 px-3 py-2 text-xs text-gray-600 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-300"
                  />
                  <button
                    type="button"
                    onClick={() => void openExternalUrl(authUrl)}
                    className="shrink-0 rounded-lg bg-gray-900 px-3 py-2 text-xs font-medium text-white dark:bg-gray-100 dark:text-gray-900"
                  >
                    {t("common.open")}
                  </button>
                </div>
              </div>

              <div className="space-y-3 rounded-xl border border-amber-200 bg-amber-50 p-3 dark:border-amber-800 dark:bg-amber-950/30">
                <div>
                  <p className="text-sm font-semibold text-amber-900 dark:text-amber-200">
                    {t("reauth.pageErrorTitle")}
                  </p>
                  <p className="mt-1 text-xs leading-5 text-amber-800 dark:text-amber-300">
                    {t("reauth.pageErrorHelp")}
                  </p>
                </div>

                <button
                  type="button"
                  onClick={() => void reportPageError("account_deactivated")}
                  disabled={isReportingPageError}
                  className="w-full rounded-lg bg-red-600 px-3 py-2 text-sm font-medium text-white transition-colors hover:bg-red-700 disabled:opacity-50"
                >
                  {isReportingPageError
                    ? t("reauth.recordingPageError")
                    : t("reauth.reportAccountDeactivated")}
                </button>

                <div className="space-y-2 border-t border-amber-200 pt-3 dark:border-amber-800">
                  <div className="flex items-center justify-between gap-2">
                    <label className="text-xs font-medium text-amber-900 dark:text-amber-200">
                      {t("reauth.pastePageError")}
                    </label>
                    <button
                      type="button"
                      onClick={() => void pastePageError()}
                      disabled={isReportingPageError}
                      className="rounded-md border border-amber-300 bg-white/70 px-2 py-1 text-[11px] font-medium text-amber-800 hover:bg-white disabled:opacity-50 dark:border-amber-700 dark:bg-black/10 dark:text-amber-200"
                    >
                      {t("reauth.pasteFromClipboard")}
                    </button>
                  </div>
                  <textarea
                    value={pageErrorText}
                    onChange={(event) => setPageErrorText(event.target.value)}
                    rows={4}
                    placeholder={t("reauth.pageErrorPlaceholder")}
                    className="w-full resize-y rounded-lg border border-amber-200 bg-white px-3 py-2 text-xs leading-5 text-gray-700 outline-none focus:border-amber-400 dark:border-amber-800 dark:bg-gray-900 dark:text-gray-200"
                  />
                  <button
                    type="button"
                    onClick={() => void reportPageError(pageErrorText)}
                    disabled={isReportingPageError || !pageErrorText.trim()}
                    className="w-full rounded-lg border border-amber-300 bg-white px-3 py-2 text-xs font-medium text-amber-800 hover:bg-amber-100 disabled:opacity-50 dark:border-amber-700 dark:bg-gray-900 dark:text-amber-200 dark:hover:bg-amber-950/50"
                  >
                    {isReportingPageError
                      ? t("reauth.recordingPageError")
                      : t("reauth.submitPageError")}
                  </button>
                </div>

                {pageError && (
                  <p className="break-words text-xs leading-5 text-red-700 dark:text-red-300">
                    {pageError}
                  </p>
                )}
              </div>
            </>
          )}
        </div>

        <div className="flex justify-end gap-3 border-t border-gray-100 p-5 dark:border-gray-800">
          <button
            type="button"
            onClick={close}
            className="rounded-lg bg-gray-100 px-4 py-2.5 text-sm font-medium text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
          >
            {phase === "success" || phase === "warning" || phase === "reported"
              ? t("common.close")
              : t("common.cancel")}
          </button>
          {(phase === "idle" || phase === "warning") && (
            <button
              type="button"
              onClick={() => void start()}
              className="rounded-lg bg-gray-900 px-4 py-2.5 text-sm font-medium text-white hover:bg-gray-800 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-gray-200"
            >
              {phase === "warning" ? t("reauth.tryAgain") : t("reauth.start")}
            </button>
          )}
          {phase === "starting" && (
            <button
              type="button"
              disabled
              className="rounded-lg bg-gray-900 px-4 py-2.5 text-sm font-medium text-white opacity-60 dark:bg-gray-100 dark:text-gray-900"
            >
              {t("reauth.starting")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
