export interface DeactivationEmailDetails {
  email: string | null;
  rawDate: string | null;
  deactivatedAt: string | null;
  isDeactivationNotice: boolean;
}

function cleanMarkdownLine(line: string): string {
  return line.trim().replace(/^\*+/, "").replace(/\*+$/, "").trim();
}

function findLabeledValue(text: string, labels: string[]): string | null {
  const lines = text.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    const line = cleanMarkdownLine(lines[index]);
    const asciiSeparator = line.indexOf(":");
    const fullWidthSeparator = line.indexOf("：");
    const separator = [asciiSeparator, fullWidthSeparator]
      .filter((offset) => offset >= 0)
      .sort((left, right) => left - right)[0];
    if (separator === undefined) continue;

    const label = line.slice(0, separator).trim().toLocaleLowerCase();
    if (!labels.some((candidate) => candidate.toLocaleLowerCase() === label)) continue;

    const inlineValue = line.slice(separator + 1).trim();
    if (inlineValue) return inlineValue;
    for (let nextIndex = index + 1; nextIndex < lines.length; nextIndex += 1) {
      const nextValue = cleanMarkdownLine(lines[nextIndex]);
      if (nextValue) return nextValue;
    }
  }
  return null;
}

export function extractDeactivationEmailDetails(text: string): DeactivationEmailDetails {
  const rawDate = findLabeledValue(text, ["时间", "time", "date"]);
  const parsedDate = rawDate ? new Date(rawDate) : null;
  const deactivatedAt = parsedDate && !Number.isNaN(parsedDate.getTime())
    ? parsedDate.toISOString()
    : null;
  const email = text.match(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i)?.[0] ?? null;
  const normalized = text.toLocaleLowerCase();
  const isDeactivationNotice = [
    "account_deactivated",
    "account has been deactivated",
    "account is deactivated",
    "access deactivated",
    "workspace_deactivated",
    "workspace has been deactivated",
    "workspace is deactivated",
  ].some((phrase) => normalized.includes(phrase));

  return { email, rawDate, deactivatedAt, isDeactivationNotice };
}
