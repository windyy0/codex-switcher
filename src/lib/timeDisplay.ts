export const OPENAI_TIME_ZONE = "America/Los_Angeles";

export function formatClockDateTime(date: Date, timeZone?: string): string {
  const parts = new Intl.DateTimeFormat("en-GB", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
    ...(timeZone ? { timeZone } : {}),
  }).formatToParts(date);
  const value = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((part) => part.type === type)?.value ?? "";

  return `${value("year")}-${value("month")}-${value("day")} ${value("hour")}:${value("minute")}`;
}

export function formatClockUtcOffset(date: Date, timeZone?: string): string {
  const offset = formatClockOffset(date, timeZone, "en-US").replace(/^GMT/, "UTC");
  return /^UTC[+-]0(?::00)?$/.test(offset) ? "UTC" : offset;
}

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
