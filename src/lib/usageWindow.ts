export const MONTHLY_WINDOW_MINUTES = 30 * 24 * 60;
export const MONTHLY_WINDOW_MINUTES_THRESHOLD = 28 * 24 * 60;

export function isMonthlyWindow(windowMinutes: number | null | undefined): boolean {
  return (
    windowMinutes != null &&
    Number.isFinite(windowMinutes) &&
    windowMinutes >= MONTHLY_WINDOW_MINUTES_THRESHOLD
  );
}
