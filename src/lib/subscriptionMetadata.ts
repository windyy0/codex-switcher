export type SubscriptionMetadataFreshness = "fresh" | "cached" | "unknown";

export const SUBSCRIPTION_METADATA_FRESH_MS = 12 * 60 * 60 * 1000;

export function getSubscriptionMetadataFreshness(
  refreshedAt: string | null | undefined,
  now = Date.now(),
): SubscriptionMetadataFreshness {
  if (!refreshedAt) return "unknown";
  const timestamp = new Date(refreshedAt).getTime();
  if (!Number.isFinite(timestamp)) return "unknown";
  return now - timestamp <= SUBSCRIPTION_METADATA_FRESH_MS ? "fresh" : "cached";
}
