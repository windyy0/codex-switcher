import type { AccountInfo } from "../types";

type AccountPlanSource = Pick<
  AccountInfo,
  "auth_mode" | "plan_type" | "subscription_expires_at"
>;

export function getEffectivePlanType(
  account: AccountPlanSource,
  now = Date.now(),
): string | null {
  if (account.auth_mode === "chat_g_p_t" && account.subscription_expires_at) {
    const expiresAt = new Date(account.subscription_expires_at).getTime();
    if (!Number.isNaN(expiresAt) && expiresAt <= now) {
      return "free";
    }
  }

  return account.plan_type;
}
