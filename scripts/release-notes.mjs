import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDirectory, "..");
const changelogPath = path.join(root, "CHANGELOG.md");

export function normalizeVersion(input) {
  return input.trim().replace(/^v/i, "");
}

export function extractReleaseNotes(contents, versionInput) {
  const version = normalizeVersion(versionInput);
  const escapedVersion = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const headingPattern = new RegExp(
    `^## \\[${escapedVersion}\\](?:\\s+-\\s+[^\\r\\n]+)?\\s*$`,
    "m"
  );
  const match = headingPattern.exec(contents);
  if (!match) {
    throw new Error(`CHANGELOG.md 中找不到版本 ${version}`);
  }

  const bodyStart = match.index + match[0].length;
  const remaining = contents.slice(bodyStart);
  const nextHeading = /^## \[/m.exec(remaining);
  const body = (nextHeading ? remaining.slice(0, nextHeading.index) : remaining).trim();
  if (!body) {
    throw new Error(`CHANGELOG.md 中的版本 ${version} 没有更新内容`);
  }

  return body;
}

export function finalizeUnreleasedSection(
  contents,
  versionInput,
  releaseDate = new Date().toISOString().slice(0, 10)
) {
  const version = normalizeVersion(versionInput);
  const unreleasedHeading = /^## \[未发布\]\s*$/m.exec(contents);
  if (!unreleasedHeading) {
    throw new Error("CHANGELOG.md 必须包含“## [未发布]”章节");
  }

  const bodyStart = unreleasedHeading.index + unreleasedHeading[0].length;
  const remaining = contents.slice(bodyStart);
  const nextVersionHeading = /^## \[/m.exec(remaining);
  const unreleasedBody = (
    nextVersionHeading ? remaining.slice(0, nextVersionHeading.index) : remaining
  ).trim();
  if (!/^\s*-\s+\S/m.test(unreleasedBody)) {
    throw new Error("CHANGELOG.md 的“未发布”章节没有可发布的更新条目");
  }

  const history = nextVersionHeading
    ? remaining.slice(nextVersionHeading.index).trimStart()
    : "";
  const prefix = contents.slice(0, unreleasedHeading.index);
  const releasedSection = [
    "## [未发布]",
    "",
    `## [${version}] - ${releaseDate}`,
    "",
    unreleasedBody,
    "",
    history,
  ]
    .join("\n")
    .trimEnd();

  return `${prefix}${releasedSection}\n`;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const version = process.argv[2];
  if (!version) {
    console.error("Usage: node scripts/release-notes.mjs <version>");
    process.exit(1);
  }

  try {
    const contents = fs.readFileSync(changelogPath, "utf8");
    process.stdout.write(`${extractReleaseNotes(contents, version)}\n`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
