import type { WarmupFailureInfo } from "../types";

export const WARMUP_FAILURES_STORAGE_KEY = "codex-switcher:warmup-failures";

export type WarmupFailureLedger = Record<string, WarmupFailureInfo>;

export function readStoredWarmupFailures(): WarmupFailureLedger {
  if (typeof window === "undefined") return {};

  try {
    const parsed = JSON.parse(
      window.localStorage.getItem(WARMUP_FAILURES_STORAGE_KEY) ?? "{}"
    );
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};

    const entries = Object.entries(parsed).filter(
      (entry): entry is [string, WarmupFailureInfo] => {
        const value = entry[1];
        return Boolean(
          value &&
            typeof value === "object" &&
            !Array.isArray(value) &&
            "error" in value &&
            typeof value.error === "string" &&
            "failedAt" in value &&
            typeof value.failedAt === "number" &&
            "modelUnavailable" in value &&
            typeof value.modelUnavailable === "boolean"
        );
      }
    );
    return Object.fromEntries(entries);
  } catch {
    return {};
  }
}

export function isWarmupModelUnavailable(error: string): boolean {
  const normalized = error.toLowerCase();
  if (
    normalized.includes("model_not_found") ||
    normalized.includes("model_deprecated") ||
    normalized.includes("model_retired") ||
    /warm-up failed with status (404|410)\b/.test(normalized)
  ) {
    return true;
  }

  const unavailableTerms =
    "(?:unavailable|not found|does not exist|no longer available|deprecated|retired|unsupported|decommissioned|disabled)";
  if (
    new RegExp(`model.{0,80}${unavailableTerms}`, "i").test(normalized) ||
    new RegExp(`${unavailableTerms}.{0,80}model`, "i").test(normalized)
  ) {
    return true;
  }

  return /模型.{0,40}(不可用|不存在|已下线|下线|已停用|已弃用|不支持)/.test(error);
}

export function warmupErrorFingerprint(error: string): string {
  return error.toLowerCase().replace(/\s+/g, " ").trim();
}

export function truncateNotificationText(text: string, maxLength = 180): string {
  const characters = Array.from(text);
  if (characters.length <= maxLength) return text;
  return `${characters.slice(0, maxLength).join("")}...`;
}
