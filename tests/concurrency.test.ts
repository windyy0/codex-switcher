import assert from "node:assert/strict";
import test from "node:test";

import { runWithConcurrency } from "../src/lib/concurrency.ts";

test("concurrent work reports a worker failure after the batch settles", async () => {
  const attempted: number[] = [];
  const failure = new Error("metadata refresh failed");

  await assert.rejects(
    runWithConcurrency(
      [1, 2, 3],
      async (item) => {
        attempted.push(item);
        if (item === 2) throw failure;
      },
      1
    ),
    failure
  );

  assert.deepEqual(attempted.sort(), [1, 2, 3]);
});

test("concurrent work resolves after every successful item", async () => {
  const completed: number[] = [];

  await runWithConcurrency(
    [1, 2, 3, 4],
    async (item) => {
      completed.push(item);
    },
    2
  );

  assert.deepEqual(completed.sort(), [1, 2, 3, 4]);
});
