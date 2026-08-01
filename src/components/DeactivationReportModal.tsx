import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AccountInfo } from "../types";
import { extractDeactivationEmailDetails } from "../lib/deactivationEmail";

interface DeactivationReportModalProps {
  account: AccountInfo | null;
  onClose: () => void;
  onReport: (accountId: string, emailText: string) => Promise<unknown>;
}

export function DeactivationReportModal({
  account,
  onClose,
  onReport,
}: DeactivationReportModalProps) {
  const { t, i18n } = useTranslation();
  const [emailText, setEmailText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const details = useMemo(() => extractDeactivationEmailDetails(emailText), [emailText]);
  const emailMismatch = Boolean(
    account?.email &&
      details.email &&
      account.email.toLocaleLowerCase() !== details.email.toLocaleLowerCase()
  );

  useEffect(() => {
    setEmailText("");
    setError(null);
    setSubmitting(false);
    setSubmitted(false);
  }, [account?.id]);

  if (!account) return null;

  const pasteFromClipboard = async () => {
    setError(null);
    try {
      const text = await navigator.clipboard.readText();
      if (!text.trim()) {
        setError(t("deactivationReport.clipboardEmpty"));
        return;
      }
      setEmailText(text);
    } catch {
      setError(t("deactivationReport.clipboardReadFailed"));
    }
  };

  const submit = async () => {
    if (!emailText.trim() || submitting || emailMismatch || !details.isDeactivationNotice) return;
    setSubmitting(true);
    setError(null);
    try {
      await onReport(account.id, emailText);
      setSubmitted(true);
    } catch (reportError) {
      setError(reportError instanceof Error ? reportError.message : String(reportError));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      role="dialog"
      aria-modal="true"
      aria-labelledby="deactivation-report-title"
    >
      <div className="mx-4 max-h-[90vh] w-full max-w-xl overflow-y-auto rounded-2xl border border-gray-200 bg-white shadow-xl dark:border-gray-700 dark:bg-gray-900">
        <div className="flex items-start justify-between gap-4 border-b border-gray-100 p-5 dark:border-gray-800">
          <div>
            <h2 id="deactivation-report-title" className="text-lg font-semibold text-gray-900 dark:text-gray-100">
              {t("deactivationReport.title")}
            </h2>
            <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
              {account.name}{account.email ? ` · ${account.email}` : ""}
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            disabled={submitting}
            className="text-gray-400 transition-colors hover:text-gray-600 disabled:opacity-50 dark:hover:text-gray-300"
            aria-label={t("common.close")}
          >
            ✕
          </button>
        </div>

        <div className="space-y-4 p-5">
          {submitted ? (
            <div className="rounded-xl border border-red-200 bg-red-50 p-4 text-sm text-red-800 dark:border-red-800 dark:bg-red-950/35 dark:text-red-200">
              {t("deactivationReport.recorded")}
            </div>
          ) : (
            <>
              <p className="text-sm leading-6 text-gray-600 dark:text-gray-300">
                {t("deactivationReport.help")}
              </p>
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <label className="text-xs font-medium text-gray-600 dark:text-gray-300">
                    {t("deactivationReport.emailContent")}
                  </label>
                  <button
                    type="button"
                    onClick={() => void pasteFromClipboard()}
                    disabled={submitting}
                    className="rounded-md border border-gray-200 bg-gray-50 px-2 py-1 text-[11px] font-medium text-gray-700 hover:bg-gray-100 disabled:opacity-50 dark:border-gray-700 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
                  >
                    {t("deactivationReport.pasteFromClipboard")}
                  </button>
                </div>
                <textarea
                  value={emailText}
                  onChange={(event) => {
                    setEmailText(event.target.value);
                    setError(null);
                  }}
                  rows={12}
                  disabled={submitting}
                  placeholder={t("deactivationReport.placeholder")}
                  className="w-full resize-y rounded-xl border border-gray-200 bg-white px-3 py-2 text-xs leading-5 text-gray-700 outline-none focus:border-gray-400 disabled:opacity-60 dark:border-gray-700 dark:bg-gray-950 dark:text-gray-200"
                />
              </div>

              {emailText.trim() && (
                <div className="rounded-xl border border-gray-200 bg-gray-50 p-3 text-xs leading-5 text-gray-700 dark:border-gray-700 dark:bg-gray-800/70 dark:text-gray-200">
                  <div className={details.isDeactivationNotice
                    ? "font-medium text-emerald-700 dark:text-emerald-300"
                    : "font-medium text-red-600 dark:text-red-300"}
                  >
                    {t(details.isDeactivationNotice
                      ? "deactivationReport.recognized"
                      : "deactivationReport.notRecognized")}
                  </div>
                  {details.email && (
                    <div className={emailMismatch ? "font-medium text-red-600 dark:text-red-300" : ""}>
                      {t("deactivationReport.detectedEmail", { email: details.email })}
                    </div>
                  )}
                  {details.deactivatedAt ? (
                    <div>
                      {t("deactivationReport.detectedDate", {
                        date: new Date(details.deactivatedAt).toLocaleString(i18n.resolvedLanguage ?? "en-US"),
                      })}
                    </div>
                  ) : details.rawDate ? (
                    <div className="font-medium text-amber-700 dark:text-amber-300">
                      {t("deactivationReport.unrecognizedDate", { date: details.rawDate })}
                    </div>
                  ) : null}
                  {emailMismatch && <div>{t("deactivationReport.emailMismatch")}</div>}
                </div>
              )}

              {error && (
                <div className="rounded-xl border border-red-200 bg-red-50 p-3 text-xs leading-5 text-red-700 dark:border-red-800 dark:bg-red-950/35 dark:text-red-200" role="alert">
                  {error}
                </div>
              )}
            </>
          )}
        </div>

        <div className="flex justify-end gap-3 border-t border-gray-100 p-5 dark:border-gray-800">
          <button
            type="button"
            onClick={onClose}
            disabled={submitting}
            className="rounded-lg bg-gray-100 px-4 py-2.5 text-sm font-medium text-gray-700 hover:bg-gray-200 disabled:opacity-50 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
          >
            {submitted ? t("common.close") : t("common.cancel")}
          </button>
          {!submitted && (
            <button
              type="button"
              onClick={() => void submit()}
              disabled={submitting || !emailText.trim() || emailMismatch || !details.isDeactivationNotice}
              className="rounded-lg bg-red-600 px-4 py-2.5 text-sm font-medium text-white hover:bg-red-700 disabled:opacity-50"
            >
              {submitting ? t("deactivationReport.recording") : t("deactivationReport.record")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
