import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  assertValidVersion,
  extractReleaseNotes,
  finalizeUnreleasedSection,
  formatLocalDate,
} from "./release-notes.mjs";

const root = process.cwd();
const VERSION_FILES = [
  "package.json",
  "src-tauri/tauri.conf.json",
  "src-tauri/Cargo.toml",
  "src-tauri/Cargo.lock",
];
const CHANGELOG_FILE = "CHANGELOG.md";
const RELEASE_FILES = [...VERSION_FILES, CHANGELOG_FILE];

const readFile = (relativePath) =>
  fs.readFileSync(path.join(root, relativePath), "utf8");

const run = (command, args, options = {}) => {
  execFileSync(command, args, {
    cwd: root,
    stdio: "inherit",
    ...options,
  });
};

const capture = (command, args) =>
  execFileSync(command, args, {
    cwd: root,
    encoding: "utf8",
  }).trim();

const parsePackageVersion = (contents) =>
  contents.match(/"version"\s*:\s*"(\d+\.\d+\.\d+)"/)?.[1] ?? null;

const parseCargoVersion = (contents) =>
  contents.match(/^version\s*=\s*"(\d+\.\d+\.\d+)"/m)?.[1] ?? null;

const parseCargoLockVersion = (contents) =>
  contents.match(
    /\[\[package\]\]\r?\nname = "codex-switcher"\r?\nversion = "(\d+\.\d+\.\d+)"/
  )?.[1] ?? null;

function parseVersionParts(versionInput) {
  const version = assertValidVersion(versionInput);
  return version.split(".").map((part) => BigInt(part));
}

export function compareVersions(leftInput, rightInput) {
  const left = parseVersionParts(leftInput);
  const right = parseVersionParts(rightInput);
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] < right[index]) return -1;
    if (left[index] > right[index]) return 1;
  }
  return 0;
}

export function resolveNextVersion(currentVersion, input) {
  const [major, minor, patch] = parseVersionParts(currentVersion);
  let nextVersion;

  if (input === "major") {
    nextVersion = `${major + 1n}.0.0`;
  } else if (input === "minor") {
    nextVersion = `${major}.${minor + 1n}.0`;
  } else if (input === "patch") {
    nextVersion = `${major}.${minor}.${patch + 1n}`;
  } else {
    nextVersion = assertValidVersion(input);
  }

  if (compareVersions(nextVersion, currentVersion) <= 0) {
    throw new Error(`新版本 ${nextVersion} 必须严格高于当前版本 ${currentVersion}`);
  }
  return nextVersion;
}

export function assertValidReleaseTag(tag) {
  if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tag)) {
    throw new Error(`无效的发布标签：${tag}（必须为 vX.Y.Z）`);
  }
  return tag.slice(1);
}

export function assertReleaseTagIsNewer(candidateTag, currentTag) {
  const candidateVersion = assertValidReleaseTag(candidateTag);
  const currentVersion = assertValidReleaseTag(currentTag);
  if (compareVersions(candidateVersion, currentVersion) <= 0) {
    throw new Error(
      `拒绝发布 ${candidateTag}：当前 latest 已是 ${currentTag}，版本必须严格递增`
    );
  }
  return candidateVersion;
}

function getVersionSnapshot(fileContents) {
  return {
    "package.json": parsePackageVersion(fileContents["package.json"]),
    "src-tauri/tauri.conf.json": parsePackageVersion(
      fileContents["src-tauri/tauri.conf.json"]
    ),
    "src-tauri/Cargo.toml": parseCargoVersion(
      fileContents["src-tauri/Cargo.toml"]
    ),
    "src-tauri/Cargo.lock": parseCargoLockVersion(
      fileContents["src-tauri/Cargo.lock"]
    ),
  };
}

function assertVersionsMatch(fileContents, expectedVersion) {
  const snapshot = getVersionSnapshot(fileContents);
  const mismatches = Object.entries(snapshot).filter(
    ([, version]) => version !== expectedVersion
  );
  if (mismatches.length > 0) {
    const details = mismatches
      .map(([file, version]) => `- ${file}: ${version ?? "missing"}`)
      .join("\n");
    throw new Error(`版本校验失败，预期 ${expectedVersion}：\n${details}`);
  }
  return snapshot;
}

export function createReleasePlan(fileContents, input, releaseDate = formatLocalDate()) {
  const currentVersion = parsePackageVersion(fileContents["package.json"]);
  if (!currentVersion) {
    throw new Error("无法从 package.json 获取当前版本");
  }

  assertVersionsMatch(fileContents, currentVersion);
  const nextVersion = resolveNextVersion(currentVersion, input);
  const updatedChangelog = finalizeUnreleasedSection(
    fileContents[CHANGELOG_FILE],
    nextVersion,
    releaseDate
  );
  const releaseNotes = extractReleaseNotes(updatedChangelog, nextVersion);
  return { currentVersion, nextVersion, releaseNotes, updatedChangelog };
}

function readReleaseFiles() {
  return Object.fromEntries(RELEASE_FILES.map((file) => [file, readFile(file)]));
}

function assertCleanTree() {
  const status = capture("git", ["status", "--short"]);
  if (status) {
    throw new Error(`Git 工作区必须保持干净：\n${status}`);
  }
}

function assertTagDoesNotExist(tag) {
  const localTag = capture("git", ["tag", "--list", tag]);
  if (localTag) {
    throw new Error(`本地 Git 标签 ${tag} 已存在`);
  }

  const remotes = capture("git", ["remote"]).split(/\r?\n/).filter(Boolean);
  if (remotes.includes("origin")) {
    const remoteTag = capture("git", [
      "ls-remote",
      "--tags",
      "origin",
      `refs/tags/${tag}`,
      `refs/tags/${tag}^{}`,
    ]);
    if (remoteTag) {
      throw new Error(`远程 Git 标签 ${tag} 已存在`);
    }
  }
}

function verifyExistingRelease(tag) {
  const version = assertValidReleaseTag(tag);
  let taggedCommit;
  try {
    taggedCommit = capture("git", ["rev-parse", `${tag}^{commit}`]);
  } catch {
    throw new Error(`Git 标签 ${tag} 不存在或无法解析`);
  }
  const headCommit = capture("git", ["rev-parse", "HEAD"]);
  if (headCommit !== taggedCommit) {
    throw new Error(
      `当前源码不是 ${tag} 对应的提交：HEAD=${headCommit}，tag=${taggedCommit}`
    );
  }

  const files = readReleaseFiles();
  assertVersionsMatch(files, version);
  const releaseNotes = extractReleaseNotes(files[CHANGELOG_FILE], version);
  console.log(`已验证发布 ${tag}：版本文件一致，更新日志存在。`);
  console.log("发布重点：");
  console.log(releaseNotes);
}

function restoreReleaseFiles(originalFiles) {
  for (const file of RELEASE_FILES) {
    fs.writeFileSync(path.join(root, file), originalFiles[file], "utf8");
  }
  execFileSync("git", ["reset", "--", ...RELEASE_FILES], {
    cwd: root,
    stdio: "ignore",
  });
}

function rollbackLocalRelease(originalHead, tag, originalFiles) {
  const currentHead = capture("git", ["rev-parse", "HEAD"]);
  const createdTag = capture("git", ["tag", "--list", tag]);
  if (createdTag) {
    const tagCommit = capture("git", ["rev-parse", `${tag}^{commit}`]);
    if (currentHead === originalHead || tagCommit !== currentHead) {
      throw new Error(`拒绝删除无法确认由本次发布创建的标签 ${tag}`);
    }
    execFileSync("git", ["tag", "-d", tag], { cwd: root, stdio: "ignore" });
  }

  if (currentHead !== originalHead) {
    execFileSync("git", ["reset", "--mixed", originalHead], {
      cwd: root,
      stdio: "ignore",
    });
  }
  restoreReleaseFiles(originalFiles);
}

function createRelease(input, shouldPush) {
  assertCleanTree();
  const originalFiles = readReleaseFiles();
  const originalHead = capture("git", ["rev-parse", "HEAD"]);
  const plan = createReleasePlan(originalFiles, input);
  const tag = `v${plan.nextVersion}`;

  // All checks that can reasonably fail are completed before any file is changed.
  assertTagDoesNotExist(tag);
  const branch = capture("git", ["branch", "--show-current"]);
  if (!branch) {
    throw new Error("无法在 detached HEAD 状态创建发布");
  }

  console.log(`发布版本：${plan.currentVersion} -> ${plan.nextVersion}`);
  console.log("发布重点：");
  console.log(plan.releaseNotes);

  try {
    run("node", ["scripts/bump-version.mjs", plan.nextVersion]);
    fs.writeFileSync(
      path.join(root, CHANGELOG_FILE),
      plan.updatedChangelog,
      "utf8"
    );
    assertVersionsMatch(readReleaseFiles(), plan.nextVersion);
    run("git", ["add", ...RELEASE_FILES]);
    run("git", ["commit", "-m", `chore: release ${plan.nextVersion}`]);
    run("git", ["tag", "-a", tag, "-m", tag]);
  } catch (error) {
    try {
      rollbackLocalRelease(originalHead, tag, originalFiles);
    } catch (rollbackError) {
      const originalMessage = error instanceof Error ? error.message : String(error);
      const rollbackMessage =
        rollbackError instanceof Error ? rollbackError.message : String(rollbackError);
      throw new Error(
        `${originalMessage}\n发布回滚失败，请手动检查工作区：${rollbackMessage}`
      );
    }
    throw error;
  }

  console.log(`已创建 ${tag} 的提交和标签。`);
  if (shouldPush) {
    try {
      run("git", ["push", "--atomic", "origin", branch, tag]);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(
        `${message}\n本地发布 ${tag} 已创建但尚未推送，请重试：git push --atomic origin ${branch} ${tag}`
      );
    }
    console.log(`已推送 ${branch} 和 ${tag}。`);
  } else {
    console.log("暂未推送，确认后运行：");
    console.log(`git push --atomic origin ${branch} ${tag}`);
  }
}

function printUsage() {
  console.error(
    "Usage:\n" +
      "  node scripts/release.mjs <version|major|minor|patch> [--push]\n" +
      "  node scripts/release.mjs --verify <vX.Y.Z>\n" +
      "  node scripts/release.mjs --assert-newer-tag <candidate> <current>"
  );
}

function main(args) {
  if (args[0] === "--assert-newer-tag") {
    if (args.length !== 3) {
      printUsage();
      process.exitCode = 1;
      return;
    }
    assertReleaseTagIsNewer(args[1], args[2]);
    console.log(`已验证 ${args[1]} 高于当前 latest ${args[2]}。`);
    return;
  }

  if (args[0] === "--verify") {
    if (args.length !== 2) {
      printUsage();
      process.exitCode = 1;
      return;
    }
    verifyExistingRelease(args[1]);
    return;
  }

  const positionalArgs = args.filter((arg) => arg !== "--push");
  const pushOptionCount = args.filter((arg) => arg === "--push").length;
  const input = positionalArgs[0];
  const unknownOptions = args.filter(
    (arg) => arg.startsWith("--") && arg !== "--push"
  );
  if (
    positionalArgs.length !== 1 ||
    pushOptionCount > 1 ||
    unknownOptions.length > 0
  ) {
    printUsage();
    process.exitCode = 1;
    return;
  }
  createRelease(input, args.includes("--push"));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
