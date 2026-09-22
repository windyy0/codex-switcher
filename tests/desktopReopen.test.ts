import assert from "node:assert/strict";
import test from "node:test";

import { finishDesktopTransition } from "../src/lib/desktopReopen.ts";

test("account changes finish before the captured desktop reopens", async () => {
  const calls: string[] = [];
  let finishSwitch!: () => void;
  const pendingSwitch = new Promise<void>((resolve) => {
    finishSwitch = resolve;
  });

  const result = finishDesktopTransition(
    { canSwitch: true, reopenToken: "captured" },
    async () => {
      calls.push("switch");
      await pendingSwitch;
      calls.push("switched");
      return true;
    },
    async (token) => {
      calls.push(`reopen:${token}`);
    },
  );

  assert.deepEqual(calls, ["switch"]);
  finishSwitch();
  assert.equal(await result, true);
  assert.deepEqual(calls, ["switch", "switched", "reopen:captured"]);
});

test("failed account changes and remaining processes prevent reopening", async () => {
  for (const [canSwitch, accountChanged] of [[false, true], [true, false]] as const) {
    const calls: string[] = [];
    const completed = await finishDesktopTransition(
      { canSwitch, reopenToken: "captured" },
      async () => {
        calls.push("switch");
        return accountChanged;
      },
      async () => {
        calls.push("reopen");
      },
    );
    assert.equal(completed, false);
    assert.equal(calls.includes("reopen"), false);
  }
});

test("standalone close can reopen and missing targets are passed to the UI", async () => {
  const tokens: Array<string | null> = [];
  assert.equal(
    await finishDesktopTransition(
      { canSwitch: true, reopenToken: null },
      null,
      async (token) => {
        tokens.push(token);
      },
    ),
    true,
  );
  assert.deepEqual(tokens, [null]);
});
