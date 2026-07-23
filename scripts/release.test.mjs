import assert from "node:assert/strict";
import test from "node:test";
import {
  assertReleaseTagIsNewer,
  assertValidReleaseTag,
  compareVersions,
  createReleasePlan,
  resolveNextVersion,
} from "./release.mjs";
import {
  extractReleaseNotes,
  finalizeUnreleasedSection,
  formatLocalDate,
  formatReleaseNotesForPublication,
} from "./release-notes.mjs";
import {
  REQUIRED_UPDATER_PLATFORMS,
  verifyUpdaterManifest,
} from "./verify-updater-manifest.mjs";

function createFiles(overrides = {}) {
  return {
    "package.json": '{\n  "version": "1.2.3"\n}\n',
    "src-tauri/tauri.conf.json": '{\n  "version": "1.2.3"\n}\n',
    "src-tauri/Cargo.toml": '[package]\nversion = "1.2.3"\n',
    "src-tauri/Cargo.lock":
      '[[package]]\nname = "codex-switcher"\nversion = "1.2.3"\ndependencies = []\n',
    "CHANGELOG.md":
      "# Changelog\n\n" +
      "## [未发布]\n\n" +
      "- 新增发布校验\n\n" +
      "## [1.2.3] - 2026-07-22\n\n" +
      "- 上一个版本\n",
    "CHANGELOG.en.md":
      "# Changelog\n\n" +
      "## [Unreleased]\n\n" +
      "- Add release validation\n\n" +
      "## [1.2.3] - 2026-07-22\n\n" +
      "- Previous version\n",
    ...overrides,
  };
}

test("semantic versions are compared numerically and must strictly increase", () => {
  assert.equal(compareVersions("1.10.0", "1.9.9"), 1);
  assert.equal(resolveNextVersion("1.2.3", "patch"), "1.2.4");
  assert.equal(resolveNextVersion("1.2.3", "minor"), "1.3.0");
  assert.equal(resolveNextVersion("1.2.3", "major"), "2.0.0");
  assert.throws(() => resolveNextVersion("1.2.3", "1.2.3"), /严格高于/);
  assert.throws(() => resolveNextVersion("1.2.3", "1.2.2"), /严格高于/);
  assert.throws(() => resolveNextVersion("1.2.3", "01.3.0"), /无效的版本号/);
});

test("release tags use a strict vX.Y.Z form", () => {
  assert.equal(assertValidReleaseTag("v1.2.3"), "1.2.3");
  assert.throws(() => assertValidReleaseTag("1.2.3"), /无效的发布标签/);
  assert.throws(
    () => assertValidReleaseTag("v1.2.3\nmalicious=value"),
    /无效的发布标签/
  );
  assert.equal(assertReleaseTagIsNewer("v1.10.0", "v1.9.9"), "1.10.0");
  assert.throws(
    () => assertReleaseTagIsNewer("v1.9.9", "v1.10.0"),
    /版本必须严格递增/
  );
});

test("release dates use local calendar fields rather than UTC serialization", () => {
  const localBoundary = {
    getFullYear: () => 2026,
    getMonth: () => 6,
    getDate: () => 23,
    toISOString: () => "2026-07-22T16:30:00.000Z",
  };
  assert.equal(formatLocalDate(localBoundary), "2026-07-23");
});

test("changelog finalization rejects empty and duplicate sections", () => {
  const duplicateHistory =
    createFiles()["CHANGELOG.md"] + "\n## [1.2.3] - 2026-07-21\n\n- 重复\n";
  assert.throws(
    () => finalizeUnreleasedSection(duplicateHistory, "1.2.4", "2026-07-23"),
    /重复章节/
  );
  assert.throws(
    () => finalizeUnreleasedSection(createFiles()["CHANGELOG.md"], "1.2.3"),
    /已包含版本/
  );

  const emptyUnreleased = createFiles()["CHANGELOG.md"].replace(
    "- 新增发布校验",
    "仅供内部参考"
  );
  assert.throws(
    () => finalizeUnreleasedSection(emptyUnreleased, "1.2.4"),
    /没有可发布的更新条目/
  );
});

test("release planning validates everything before returning updated contents", () => {
  const files = createFiles();
  const originalSnapshot = structuredClone(files);
  const plan = createReleasePlan(files, "minor", "2026-07-23");

  assert.deepEqual(files, originalSnapshot);
  assert.equal(plan.currentVersion, "1.2.3");
  assert.equal(plan.nextVersion, "1.3.0");
  assert.match(plan.releaseNotes, /- 新增发布校验/);
  assert.match(plan.releaseNotes, /- Add release validation/);
  assert.match(
    plan.updatedChangelog,
    /## \[1\.3\.0\] - 2026-07-23/
  );
  assert.match(
    plan.updatedEnglishChangelog,
    /## \[1\.3\.0\] - 2026-07-23/
  );
});

test("release planning fails before mutation when source versions disagree", () => {
  const files = createFiles({
    "src-tauri/Cargo.toml": '[package]\nversion = "1.2.2"\n',
  });
  const originalSnapshot = structuredClone(files);
  assert.throws(() => createReleasePlan(files, "patch"), /版本校验失败/);
  assert.deepEqual(files, originalSnapshot);
});

test("release note extraction selects the exact version", () => {
  const changelog =
    "# Changelog\n\n" +
    "## [未发布]\n\n- 下一版\n\n" +
    "## [1.10.0] - 2026-07-23\n\n- 新版\n\n" +
    "## [1.1.0] - 2026-07-22\n\n- 旧版\n";
  assert.equal(extractReleaseNotes(changelog, "v1.1.0"), "- 旧版");
  assert.throws(() => extractReleaseNotes(changelog, "1.0.0"), /找不到版本/);
});

test("release notes remain valid when CHANGELOG uses CRLF", () => {
  const changelog = createFiles()["CHANGELOG.md"].replace(/\n/g, "\r\n");
  assert.equal(extractReleaseNotes(changelog, "v1.2.3"), "- 上一个版本");
});

test("published release notes include an exact-version changelog link", () => {
  const notes = formatReleaseNotesForPublication(
    createFiles()["CHANGELOG.md"],
    "v1.2.3",
    "example/codex-switcher",
    createFiles()["CHANGELOG.en.md"]
  );
  assert.match(
    notes,
    /完整更新记录：https:\/\/github\.com\/example\/codex-switcher\/blob\/v1\.2\.3\/CHANGELOG\.md/
  );
  assert.match(
    notes,
    /Full changelog: https:\/\/github\.com\/example\/codex-switcher\/blob\/v1\.2\.3\/CHANGELOG\.en\.md/
  );
  assert.match(notes, /codex-switcher-release-notes:en-US/);
});

test("updater manifest requires every signed platform for the exact tag", () => {
  const platforms = Object.fromEntries(
    REQUIRED_UPDATER_PLATFORMS.map((platform) => [
      platform,
      {
        signature: `signature-${platform}`,
        url: `https://github.com/example/project/releases/download/v1.2.3/${platform}.zip`,
      },
    ])
  );
  const manifest = JSON.stringify({
    version: "1.2.3",
    notes:
      "<!-- codex-switcher-release-notes:zh-CN -->\n- 修复切换问题\n\n<!-- codex-switcher-release-notes:en-US -->\n- Fix switching\n\n完整更新记录：https://github.com/example/project/blob/v1.2.3/CHANGELOG.md\nFull changelog: https://github.com/example/project/blob/v1.2.3/CHANGELOG.en.md",
    pub_date: "2026-07-23T00:00:00.000Z",
    platforms,
  });

  assert.equal(
    verifyUpdaterManifest(manifest, "v1.2.3", "example/project").version,
    "1.2.3"
  );

  const missingWindows = JSON.stringify({
    version: "1.2.3",
    notes:
      "<!-- codex-switcher-release-notes:zh-CN -->\n- 修复切换问题\n\n<!-- codex-switcher-release-notes:en-US -->\n- Fix switching\n\n完整更新记录：https://github.com/example/project/blob/v1.2.3/CHANGELOG.md\nFull changelog: https://github.com/example/project/blob/v1.2.3/CHANGELOG.en.md",
    platforms: Object.fromEntries(
      Object.entries(platforms).filter(([platform]) => platform !== "windows-x86_64")
    ),
  });
  assert.throws(
    () => verifyUpdaterManifest(missingWindows, "v1.2.3", "example/project"),
    /缺少平台 windows-x86_64/
  );

  const wrongTagUrl = manifest.replace(
    "/releases/download/v1.2.3/",
    "/releases/download/v1.2.2/"
  );
  assert.throws(
    () => verifyUpdaterManifest(wrongTagUrl, "v1.2.3", "example/project"),
    /下载地址无效/
  );
  const wrongRepositoryUrl = manifest.replace(
    "https://github.com/example/project/releases/download/",
    "https://github.com/attacker/project/releases/download/"
  );
  assert.throws(
    () => verifyUpdaterManifest(wrongRepositoryUrl, "v1.2.3", "example/project"),
    /下载地址无效/
  );
});
