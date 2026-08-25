import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  CLOSE_MAIN_WINDOW_COMMAND,
  requestMainWindowClose,
} from "../src/lib/windowLifecycle.ts";

test("main-window close uses the backend lifecycle command", async () => {
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

  await requestMainWindowClose(async (command, args) => {
    calls.push({ command, args });
    return null;
  });

  assert.deepEqual(calls, [{ command: CLOSE_MAIN_WINDOW_COMMAND, args: undefined }]);
});

test("main-window close does not swallow backend failures", async () => {
  const failure = new Error("hide rejected");
  await assert.rejects(
    requestMainWindowClose(async () => {
      throw failure;
    }),
    failure
  );
});

test("the close button cannot bypass the backend lifecycle command", () => {
  const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

  assert.match(appSource, /requestMainWindowClose\(invokeBackend\)/);
  assert.doesNotMatch(appSource, /appWindow\.(?:close|hide)\(\)/);
});
