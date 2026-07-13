import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  describeFileSource,
  isTauriRuntime,
  invokeBackend,
  openExternalUrl,
  pickAuthJsonFile,
  type FileSource,
} from "../lib/platform";

interface AddAccountModalProps {
  isOpen: boolean;
  onClose: () => void;
  onImportFile: (source: FileSource, name: string) => Promise<void>;
  onAddApi: (name: string, apiKey: string, config: string) => Promise<void>;
  onStartOAuth: (name: string) => Promise<{ auth_url: string }>;
  onCompleteOAuth: () => Promise<unknown>;
  onCancelOAuth: () => Promise<void>;
}

type Tab = "oauth" | "import" | "api";

export function AddAccountModal({
  isOpen,
  onClose,
  onImportFile,
  onAddApi,
  onStartOAuth,
  onCompleteOAuth,
  onCancelOAuth,
}: AddAccountModalProps) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<Tab>("oauth");
  const [name, setName] = useState("");
  const [fileSource, setFileSource] = useState<FileSource | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [apiConfig, setApiConfig] = useState("");
  const [detectedLocalFile, setDetectedLocalFile] = useState(false);
  const [localDetectionDone, setLocalDetectionDone] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [oauthPending, setOauthPending] = useState(false);
  const [authUrl, setAuthUrl] = useState<string>("");
  const [copied, setCopied] = useState<boolean>(false);
  const oauthAttemptRef = useRef(0);
  const isPrimaryDisabled = loading || (activeTab === "oauth" && oauthPending);
  const nonOauthSubmissionPending = loading && activeTab !== "oauth";
  const submissionInputLocked = loading || oauthPending;
  const tauriRuntime = isTauriRuntime();

  useEffect(() => {
    if (!isOpen || activeTab !== "import" || !tauriRuntime || localDetectionDone || fileSource) {
      return;
    }

    let cancelled = false;
    setLocalDetectionDone(true);
    void invokeBackend<string | null>("detect_local_auth_json")
      .then((path) => {
        if (path && !cancelled) {
          setFileSource(path);
          setDetectedLocalFile(true);
        }
      })
      .catch((err) => {
        console.error("Failed to detect local auth.json:", err);
      });

    return () => {
      cancelled = true;
    };
    // localDetectionDone intentionally acts as a one-shot guard and is reset
    // when the modal closes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab, fileSource, isOpen, tauriRuntime]);

  const resetForm = () => {
    setName("");
    setFileSource(null);
    setApiKey("");
    setApiConfig("");
    setDetectedLocalFile(false);
    setLocalDetectionDone(false);
    setError(null);
    setLoading(false);
    setOauthPending(false);
    setAuthUrl("");
  };

  const handleClose = () => {
    if (nonOauthSubmissionPending) return;
    oauthAttemptRef.current += 1;
    if (activeTab === "oauth" && (oauthPending || loading)) {
      void onCancelOAuth().catch((err) => {
        console.error("Failed to cancel login:", err);
      });
    }
    resetForm();
    onClose();
  };

  const handleOAuthLogin = async () => {
    if (!name.trim()) {
      setError(t("addAccount.nameRequired"));
      return;
    }

    const attemptId = ++oauthAttemptRef.current;
    try {
      setLoading(true);
      setError(null);
      const info = await onStartOAuth(name.trim());
      if (oauthAttemptRef.current !== attemptId) {
        void onCancelOAuth().catch((err) => {
          console.error("Failed to cancel stale login:", err);
        });
        return;
      }
      setAuthUrl(info.auth_url);
      setOauthPending(true);
      setLoading(false);

      // Wait for completion
      await onCompleteOAuth();
      if (oauthAttemptRef.current !== attemptId) return;
      handleClose();
    } catch (err) {
      if (oauthAttemptRef.current !== attemptId) return;
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
      setOauthPending(false);
    }
  };

  const handleSelectFile = async () => {
    if (submissionInputLocked) return;
    try {
      const selected = await pickAuthJsonFile(t("fileDialog.selectAuth"));
      if (selected) {
        setFileSource(selected);
        setDetectedLocalFile(false);
      }
    } catch (err) {
      console.error("Failed to open file dialog:", err);
    }
  };

  const handleImportFile = async () => {
    if (!name.trim()) {
      setError(t("addAccount.nameRequired"));
      return;
    }
    if (!fileSource) {
      setError(t("addAccount.fileRequired"));
      return;
    }

    const attemptId = ++oauthAttemptRef.current;
    try {
      setLoading(true);
      setError(null);
      await onImportFile(fileSource, name.trim());
      if (oauthAttemptRef.current !== attemptId) return;
      resetForm();
      onClose();
    } catch (err) {
      if (oauthAttemptRef.current !== attemptId) return;
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
    }
  };

  const handleAddApi = async () => {
    if (!name.trim()) {
      setError(t("addAccount.nameRequired"));
      return;
    }
    if (!apiKey.trim()) {
      setError(t("addAccount.apiKeyRequired"));
      return;
    }
    const attemptId = ++oauthAttemptRef.current;
    try {
      setLoading(true);
      setError(null);
      await onAddApi(name.trim(), apiKey.trim(), apiConfig);
      if (oauthAttemptRef.current !== attemptId) return;
      resetForm();
      onClose();
    } catch (err) {
      if (oauthAttemptRef.current !== attemptId) return;
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md mx-4 shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between p-5 border-b border-gray-100 dark:border-gray-800">
          <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">{t("addAccount.title")}</h2>
          <button
            onClick={handleClose}
            disabled={nonOauthSubmissionPending}
            className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 transition-colors disabled:opacity-50"
          >
            ✕
          </button>
        </div>

        {/* Tabs */}
        <div className="flex border-b border-gray-100 dark:border-gray-800">
          {(["oauth", "import", "api"] as Tab[]).map((tab) => (
            <button
              key={tab}
              disabled={nonOauthSubmissionPending}
              onClick={() => {
                if (tab !== "oauth" && activeTab === "oauth" && (oauthPending || loading)) {
                  oauthAttemptRef.current += 1;
                  void onCancelOAuth().catch((err) => {
                    console.error("Failed to cancel login:", err);
                  });
                  setOauthPending(false);
                  setLoading(false);
                  setAuthUrl("");
                }
                setActiveTab(tab);
                setError(null);
              }}
              className={`flex-1 px-4 py-3 text-sm font-medium transition-colors disabled:opacity-50 ${activeTab === tab
                  ? "text-gray-900 dark:text-gray-100 border-b-2 border-gray-900 dark:border-gray-100 -mb-px"
                  : "text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300"
                }`}
            >
              {tab === "oauth" ? t("addAccount.chatgptLogin") : tab === "api" ? t("addAccount.apiKey") : t("addAccount.importFile")}
            </button>
          ))}
        </div>

        {/* Content */}
        <div className="p-5 space-y-4">
          {/* Account Name (always shown) */}
          <div>
            <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
              {t("addAccount.name")}
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              disabled={submissionInputLocked}
              placeholder={t("addAccount.namePlaceholder")}
              className="w-full px-4 py-2.5 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-gray-900 dark:text-gray-100 placeholder-gray-400 dark:placeholder-gray-500 focus:outline-none focus:border-gray-400 dark:focus:border-gray-500 focus:ring-1 focus:ring-gray-400 dark:focus:ring-gray-500 transition-colors disabled:opacity-60"
            />
          </div>

          {/* Tab-specific content */}
          {activeTab === "oauth" && (
            <div className="text-sm text-gray-500 dark:text-gray-400">
              {oauthPending ? (
                <div className="text-center py-4">
                  <div className="animate-spin h-8 w-8 border-2 border-gray-900 dark:border-gray-100 border-t-transparent rounded-full mx-auto mb-3"></div>
                  <p className="text-gray-700 dark:text-gray-300 font-medium mb-2">{t("addAccount.waiting")}</p>
                  <p className="text-xs text-gray-500 dark:text-gray-400 mb-4">
                    {t("addAccount.openLink")}
                  </p>
                  <div className="flex items-center gap-2 mb-2 bg-gray-50 dark:bg-gray-800 p-2 rounded-lg border border-gray-200 dark:border-gray-700">
                    <input
                      type="text"
                      readOnly
                      value={authUrl}
                      className="flex-1 bg-transparent border-none text-xs text-gray-600 dark:text-gray-300 focus:outline-none focus:ring-0 truncate"
                    />
                    <button
                      onClick={() => {
                        void navigator.clipboard
                          .writeText(authUrl)
                          .then(() => {
                            setCopied(true);
                            setTimeout(() => setCopied(false), 2000);
                          })
                          .catch(() => {
                            setError(t("addAccount.copyLinkManually"));
                          });
                      }}
                      className={`px-3 py-1.5 border rounded text-xs font-medium transition-colors shrink-0 
                        ${copied
                          ? "bg-green-50 dark:bg-green-900/30 border-green-200 dark:border-green-700 text-green-700 dark:text-green-300"
                          : "bg-white dark:bg-gray-900 border-gray-200 dark:border-gray-700 text-gray-700 dark:text-gray-200 hover:bg-gray-50 dark:hover:bg-gray-800"
                        }`}
                    >
                      {copied ? t("addAccount.copied") : t("common.copy")}
                    </button>
                    <button
                      onClick={() => {
                        void openExternalUrl(authUrl);
                      }}
                      className="px-3 py-1.5 bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 border border-gray-900 dark:border-gray-100 rounded text-xs font-medium text-white dark:text-gray-900 transition-colors shrink-0"
                    >
                      {t("common.open")}
                    </button>
                  </div>
                  {!tauriRuntime && (
                    <p className="text-xs text-amber-600">
                      {t("addAccount.remoteOauthWarning")}
                    </p>
                  )}
                </div>
              ) : (
                <p>
                  {t("addAccount.loginHelp")}
                </p>
              )}
            </div>
          )}

          {activeTab === "import" && (
            <div>
              <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
                {t("addAccount.selectFile")}
              </label>
              <div className="flex gap-2">
                <div className="flex-1 px-4 py-2.5 bg-gray-50 dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-sm text-gray-600 dark:text-gray-300 truncate">
                  {fileSource ? describeFileSource(fileSource) : t("fileDialog.noFile")}
                </div>
                <button
                  onClick={handleSelectFile}
                  disabled={submissionInputLocked}
                  className="px-4 py-2.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 border border-gray-200 dark:border-gray-700 rounded-lg text-sm font-medium text-gray-700 dark:text-gray-200 transition-colors whitespace-nowrap disabled:opacity-60"
                >
                  {t("addAccount.browse")}
                </button>
              </div>
              <p className="text-xs text-gray-400 dark:text-gray-500 mt-2">
                {t("addAccount.importHelp")}
              </p>
              {detectedLocalFile && (
                <p className="text-xs text-green-600 dark:text-green-400 mt-2">
                  {t("addAccount.localAuthDetected")}
                </p>
              )}
            </div>
          )}

          {activeTab === "api" && (
            <div className="space-y-4">
              <div>
                <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">{t("addAccount.apiKeyLabel")}</label>
                <input
                  type="password"
                  value={apiKey}
                  onChange={(event) => setApiKey(event.target.value)}
                  disabled={submissionInputLocked}
                  placeholder="sk-..."
                  className="w-full px-4 py-2.5 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-gray-900 dark:text-gray-100 placeholder-gray-400 focus:outline-none focus:border-gray-400 disabled:opacity-60"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">{t("addAccount.apiConfigLabel")}</label>
                <textarea
                  value={apiConfig}
                  onChange={(event) => setApiConfig(event.target.value)}
                  disabled={submissionInputLocked}
                  placeholder={t("addAccount.apiConfigPlaceholder")}
                  spellCheck={false}
                  className="w-full h-36 font-mono text-xs p-3 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-gray-900 dark:text-gray-100 placeholder-gray-400 focus:outline-none focus:border-gray-400 disabled:opacity-60"
                />
                <p className="text-xs text-gray-400 dark:text-gray-500 mt-2">{t("addAccount.apiConfigHelp")}</p>
              </div>
            </div>
          )}

          {/* Error */}
          {error && (
            <div className="p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-700 rounded-lg text-red-600 dark:text-red-300 text-sm">
              {error}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex gap-3 p-5 border-t border-gray-100 dark:border-gray-800">
          <button
            onClick={handleClose}
            disabled={nonOauthSubmissionPending}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 transition-colors disabled:opacity-50"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={activeTab === "oauth" ? handleOAuthLogin : activeTab === "api" ? handleAddApi : handleImportFile}
            disabled={isPrimaryDisabled}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors disabled:opacity-50"
          >
            {loading
              ? t("addAccount.adding")
                : activeTab === "oauth"
                  ? t("addAccount.generateLink")
                  : activeTab === "api"
                    ? t("addAccount.addApi")
                    : t("common.import")}
          </button>
        </div>
      </div>
    </div>
  );
}
