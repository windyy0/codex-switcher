import assert from "node:assert/strict";
import test from "node:test";

import { createSingleFlight, withTimeout } from "../src/lib/async.ts";

test("single-flight shares one in-progress operation", async () => {
  const singleFlight = createSingleFlight<number>();
  let calls = 0;
  let release: ((value: number) => void) | undefined;
  const pending = new Promise<number>((resolve) => {
    release = resolve;
  });

  const first = singleFlight.run(() => {
    calls += 1;
    return pending;
  });
  const second = singleFlight.run(() => {
    calls += 1;
    return Promise.resolve(2);
  });

  assert.equal(first, second);
  assert.equal(singleFlight.isRunning(), true);
  release?.(1);
  assert.equal(await second, 1);
  assert.equal(singleFlight.isRunning(), false);
  assert.equal(calls, 1);
});

test("single-flight accepts a new operation after settlement", async () => {
  const singleFlight = createSingleFlight<number>();
  assert.equal(await singleFlight.run(async () => 1), 1);
  assert.equal(await singleFlight.run(async () => 2), 2);
});

test("single-flight releases a failed operation", async () => {
  const singleFlight = createSingleFlight<number>();
  await assert.rejects(singleFlight.run(async () => {
    throw new Error("offline");
  }), /offline/);

  assert.equal(await singleFlight.run(async () => 2), 2);
});

test("timeout rejects a stalled operation", async () => {
  await assert.rejects(
    withTimeout(new Promise<never>(() => {}), 5, "refresh timed out"),
    /refresh timed out/
  );
});

test("timeout preserves a result that arrives in time", async () => {
  assert.equal(await withTimeout(Promise.resolve(42), 100, "too slow"), 42);
});
