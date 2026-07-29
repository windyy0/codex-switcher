import type { AccountHealthStatus, AccountInfo } from "../types";

export function healthStatusBlocksAccountActions(
  status: AccountHealthStatus | null | undefined
): boolean {
  return (
    status === "reauth_required" ||
    status === "account_deactivated" ||
    status === "workspace_deactivated"
  );
}

export function accountHealthBlocksAccountActions(
  account: Pick<AccountInfo, "health">
): boolean {
  return healthStatusBlocksAccountActions(account.health?.status);
}
