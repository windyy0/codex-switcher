export const OPENAI_TIME_ZONE = "America/Los_Angeles";

export function formatClockTime(
  date: Date,
  timeZone?: string,
  locale?: string,
): string {
  return new Intl.DateTimeFormat(locale, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    ...(timeZone ? { timeZone } : {}),
  }).format(date);
}

export function formatClockDate(
  date: Date,
  timeZone?: string,
  locale?: string,
): string {
  return new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    weekday: "short",
    ...(timeZone ? { timeZone } : {}),
  }).format(date);
}

export function formatClockOffset(
  date: Date,
  timeZone?: string,
  locale?: string,
): string {
  const parts = new Intl.DateTimeFormat(locale, {
    timeZoneName: "shortOffset",
    ...(timeZone ? { timeZone } : {}),
  }).formatToParts(date);
  return parts.find((part) => part.type === "timeZoneName")?.value ?? "Local";
}
