import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDirectory, "..");
const changelogPath = path.join(root, "CHANGELOG.md");
const englishChangelogPath = path.join(root, "CHANGELOG.en.md");
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

export function assertValidChangelog(
  contents,
  { unreleasedLabel = "未发布", changelogName = "CHANGELOG.md" } = {}
) {
  const headings = getHeadings(contents);
  const counts = new Map();
  for (const heading of headings) {
    counts.set(heading.label, (counts.get(heading.label) ?? 0) + 1);
  }

  const duplicates = [...counts.entries()]
    .filter(([, count]) => count > 1)
    .map(([label]) => label);
  if (duplicates.length > 0) {
    throw new Error(`${changelogName} 包含重复章节：${duplicates.join("、")}`);
  }

  const unreleased = headings.filter((heading) => heading.label === unreleasedLabel);
  if (unreleased.length !== 1) {
    throw new Error(
      `${changelogName} 必须且只能包含一个“## [${unreleasedLabel}]”章节`
    );
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

export function extractReleaseNotes(
  contents,
  versionInput,
  { unreleasedLabel = "未发布", changelogName = "CHANGELOG.md" } = {}
) {
  const version = assertValidVersion(versionInput);
  const headings = assertValidChangelog(contents, { unreleasedLabel, changelogName });
  const headingIndex = headings.findIndex((heading) => heading.label === version);
  if (headingIndex === -1) {
    throw new Error(`${changelogName} 中找不到版本 ${version}`);
  }

  const body = getSectionBody(contents, headings, headingIndex);
  if (!body) {
    throw new Error(`${changelogName} 中的版本 ${version} 没有更新内容`);
  }

  return body;
}

export function formatReleaseNotesForPublication(
  contents,
  versionInput,
  repository = process.env.GITHUB_REPOSITORY ?? "windyy0/codex-switcher",
  englishContents = null
) {
  const version = assertValidVersion(versionInput);
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error(`无效的 GitHub 仓库：${repository}`);
  }

  const notes = extractReleaseNotes(contents, version);
  if (englishContents == null) {
    return `${notes}\n\n完整更新记录：https://github.com/${repository}/blob/v${version}/CHANGELOG.md`;
  }

  const englishNotes = extractReleaseNotes(englishContents, version, {
    unreleasedLabel: "Unreleased",
    changelogName: "CHANGELOG.en.md",
  });
  return [
    [
      "### 中文更新",
      "<!-- codex-switcher-release-notes:zh-CN -->",
      notes,
    ].join("\n\n"),
    [
      "### English updates",
      "<!-- codex-switcher-release-notes:en-US -->",
      englishNotes,
    ].join("\n\n"),
    [
      "<!-- codex-switcher-release-notes:links -->",
      `完整更新记录：https://github.com/${repository}/blob/v${version}/CHANGELOG.md`,
      `Full changelog: https://github.com/${repository}/blob/v${version}/CHANGELOG.en.md`,
    ].join("\n\n"),
  ].join("\n\n");
}

export function finalizeUnreleasedSection(
  contents,
  versionInput,
  releaseDate = formatLocalDate(),
  { unreleasedLabel = "未发布", changelogName = "CHANGELOG.md" } = {}
) {
  const version = assertValidVersion(versionInput);
  if (!/^\d{4}-\d{2}-\d{2}$/.test(releaseDate)) {
    throw new Error(`无效的发布日期：${releaseDate}`);
  }

  const headings = assertValidChangelog(contents, { unreleasedLabel, changelogName });
  if (headings.some((heading) => heading.label === version)) {
    throw new Error(`${changelogName} 已包含版本 ${version}`);
  }

  const unreleasedIndex = headings.findIndex(
    (heading) => heading.label === unreleasedLabel
  );
  const unreleasedHeading = headings[unreleasedIndex];
  const unreleasedBody = getSectionBody(contents, headings, unreleasedIndex);
  if (!/^\s*-\s+\S/m.test(unreleasedBody)) {
    throw new Error(
      `${changelogName} 的“${unreleasedLabel}”章节没有可发布的更新条目`
    );
  }

  const nextHeading = headings[unreleasedIndex + 1];
  const history = nextHeading ? contents.slice(nextHeading.index).trimStart() : "";
  const prefix = contents.slice(0, unreleasedHeading.index);
  const releasedSection = [
    `## [${unreleasedLabel}]`,
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
    const englishContents = fs.readFileSync(englishChangelogPath, "utf8");
    process.stdout.write(
      `${formatReleaseNotesForPublication(
        contents,
        version,
        process.argv[3],
        englishContents
      )}\n`
    );
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
