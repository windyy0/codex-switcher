export interface DesktopTransitionOutcome {
  canSwitch: boolean;
  reopenToken: string | null;
}

export async function finishDesktopTransition(
  outcome: DesktopTransitionOutcome,
  completeAccountChange: (() => Promise<boolean>) | null,
  finishReopen: (token: string | null) => Promise<void>,
): Promise<boolean> {
  if (!outcome.canSwitch) return false;
  if (completeAccountChange && !(await completeAccountChange())) return false;
  await finishReopen(outcome.reopenToken);
  return true;
}
