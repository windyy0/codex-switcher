import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { assertValidReleaseTag } from "./release.mjs";

export const REQUIRED_UPDATER_PLATFORMS = [
  "darwin-aarch64",
  "darwin-x86_64",
  "linux-x86_64",
  "windows-x86_64",
];

export function verifyUpdaterManifest(
  contents,
  tagInput,
  repository = process.env.GITHUB_REPOSITORY ?? "windyy0/codex-switcher"
) {
  const version = assertValidReleaseTag(tagInput);
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error(`无效的 GitHub 仓库：${repository}`);
  }
  let manifest;
  try {
    manifest = JSON.parse(contents);
  } catch (error) {
    throw new Error(
      `latest.json 不是有效的 JSON：${error instanceof Error ? error.message : String(error)}`
    );
  }

  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    throw new Error("latest.json 的顶层必须是对象");
  }
  if (manifest.version !== version) {
    throw new Error(
      `latest.json 版本不一致：预期 ${version}，实际 ${String(manifest.version)}`
    );
  }
  if (typeof manifest.notes !== "string" || !manifest.notes.trim()) {
    throw new Error("latest.json 缺少本次更新重点");
  }
  const expectedChangelogUrl =
    `https://github.com/${repository}/blob/${tagInput}/CHANGELOG.md`;
  if (!manifest.notes.includes(expectedChangelogUrl)) {
    throw new Error("latest.json 缺少当前版本的完整更新记录链接");
  }
  if (!manifest.platforms || typeof manifest.platforms !== "object") {
    throw new Error("latest.json 缺少 platforms 对象");
  }

  const expectedDownloadPrefix =
    `https://github.com/${repository}/releases/download/${tagInput}/`;
  for (const platform of REQUIRED_UPDATER_PLATFORMS) {
    const entry = manifest.platforms[platform];
    if (!entry || typeof entry !== "object") {
      throw new Error(`latest.json 缺少平台 ${platform}`);
    }
    if (typeof entry.signature !== "string" || !entry.signature.trim()) {
      throw new Error(`latest.json 的 ${platform} 缺少更新签名`);
    }
    if (
      typeof entry.url !== "string" ||
      !entry.url.startsWith(expectedDownloadPrefix)
    ) {
      throw new Error(`latest.json 的 ${platform} 下载地址无效或不属于 ${tagInput}`);
    }
  }

  return manifest;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [manifestPath, tag, repository] = process.argv.slice(2);
  if (!manifestPath || !tag) {
    console.error(
      "Usage: node scripts/verify-updater-manifest.mjs <latest.json> <vX.Y.Z>"
    );
    process.exit(1);
  }

  try {
    const contents = fs.readFileSync(path.resolve(manifestPath), "utf8");
    verifyUpdaterManifest(contents, tag, repository);
    console.log(`已验证 ${tag} 的 latest.json：四个平台、下载地址和签名均完整。`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
