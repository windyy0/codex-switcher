import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDirectory, "..");
const changelogPath = path.join(root, "CHANGELOG.md");
const VERSION_PATTERN = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const CHANGELOG_HEADING_PATTERN =
  /^## \[([^\]]+)\](?:[ \t]+-[ \t]+[^\r\n]+)?[ \t]*(?:\r?$)/gm;

export function normalizeVersion(input) {
  return input.trim().replace(/^v/i, "");
}

export function assertValidVersion(versionInput) {
  const version = normalizeVersion(versionInput);
  if (!VERSION_PATTERN.test(version)) {
    throw new Error(`无效的版本号：${versionInput}`);
  }
  return version;
}

export function formatLocalDate(date = new Date()) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function getHeadings(contents) {
  return [...contents.matchAll(CHANGELOG_HEADING_PATTERN)].map((match) => ({
    label: match[1],
    index: match.index,
    length: match[0].length,
  }));
}

export function assertValidChangelog(contents) {
  const headings = getHeadings(contents);
  const counts = new Map();
  for (const heading of headings) {
    counts.set(heading.label, (counts.get(heading.label) ?? 0) + 1);
  }

  const duplicates = [...counts.entries()]
    .filter(([, count]) => count > 1)
    .map(([label]) => label);
  if (duplicates.length > 0) {
    throw new Error(`CHANGELOG.md 包含重复章节：${duplicates.join("、")}`);
  }

  const unreleased = headings.filter((heading) => heading.label === "未发布");
  if (unreleased.length !== 1) {
    throw new Error("CHANGELOG.md 必须且只能包含一个“## [未发布]”章节");
  }

  return headings;
}

function getSectionBody(contents, headings, headingIndex) {
  const heading = headings[headingIndex];
  const bodyStart = heading.index + heading.length;
  const nextHeading = headings[headingIndex + 1];
  return contents
    .slice(bodyStart, nextHeading?.index ?? contents.length)
    .trim();
}

export function extractReleaseNotes(contents, versionInput) {
  const version = assertValidVersion(versionInput);
  const headings = assertValidChangelog(contents);
  const headingIndex = headings.findIndex((heading) => heading.label === version);
  if (headingIndex === -1) {
    throw new Error(`CHANGELOG.md 中找不到版本 ${version}`);
  }

  const body = getSectionBody(contents, headings, headingIndex);
  if (!body) {
    throw new Error(`CHANGELOG.md 中的版本 ${version} 没有更新内容`);
  }

  return body;
}

export function formatReleaseNotesForPublication(
  contents,
  versionInput,
  repository = process.env.GITHUB_REPOSITORY ?? "windyy0/codex-switcher"
) {
  const version = assertValidVersion(versionInput);
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error(`无效的 GitHub 仓库：${repository}`);
  }

  const notes = extractReleaseNotes(contents, version);
  return `${notes}\n\n完整更新记录：https://github.com/${repository}/blob/v${version}/CHANGELOG.md`;
}

export function finalizeUnreleasedSection(
  contents,
  versionInput,
  releaseDate = formatLocalDate()
) {
  const version = assertValidVersion(versionInput);
  if (!/^\d{4}-\d{2}-\d{2}$/.test(releaseDate)) {
    throw new Error(`无效的发布日期：${releaseDate}`);
  }

  const headings = assertValidChangelog(contents);
  if (headings.some((heading) => heading.label === version)) {
    throw new Error(`CHANGELOG.md 已包含版本 ${version}`);
  }

  const unreleasedIndex = headings.findIndex((heading) => heading.label === "未发布");
  const unreleasedHeading = headings[unreleasedIndex];
  const unreleasedBody = getSectionBody(contents, headings, unreleasedIndex);
  if (!/^\s*-\s+\S/m.test(unreleasedBody)) {
    throw new Error("CHANGELOG.md 的“未发布”章节没有可发布的更新条目");
  }

  const nextHeading = headings[unreleasedIndex + 1];
  const history = nextHeading ? contents.slice(nextHeading.index).trimStart() : "";
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
    console.error(
      "Usage: node scripts/release-notes.mjs <version> [owner/repository]"
    );
    process.exit(1);
  }

  try {
    const contents = fs.readFileSync(changelogPath, "utf8");
    process.stdout.write(
      `${formatReleaseNotesForPublication(contents, version, process.argv[3])}\n`
    );
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
