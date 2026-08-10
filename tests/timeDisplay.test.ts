import assert from "node:assert/strict";
import test from "node:test";
import {
  formatClockOffset,
  formatClockTime,
  OPENAI_TIME_ZONE,
} from "../src/lib/timeDisplay.ts";

test("OpenAI clock uses San Francisco daylight time in summer", () => {
  const summer = new Date("2026-08-10T06:03:00Z");

  assert.equal(formatClockTime(summer, OPENAI_TIME_ZONE, "en-GB"), "23:03");
  assert.equal(formatClockOffset(summer, OPENAI_TIME_ZONE, "en-US"), "GMT-7");
});

test("OpenAI clock switches to San Francisco standard time in winter", () => {
  const winter = new Date("2026-12-10T06:03:00Z");

  assert.equal(formatClockTime(winter, OPENAI_TIME_ZONE, "en-GB"), "22:03");
  assert.equal(formatClockOffset(winter, OPENAI_TIME_ZONE, "en-US"), "GMT-8");
});
