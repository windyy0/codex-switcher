import assert from "node:assert/strict";
import test from "node:test";
import {
  formatClockDateTime,
  formatClockOffset,
  formatClockTime,
  formatClockUtcOffset,
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

test("clock details show each zone's date across midnight", () => {
  const now = new Date("2026-08-28T22:08:00Z");

  assert.equal(formatClockDateTime(now, "Asia/Shanghai"), "2026-08-29 06:08");
  assert.equal(formatClockDateTime(now, OPENAI_TIME_ZONE), "2026-08-28 15:08");
  assert.equal(formatClockDateTime(now, "UTC"), "2026-08-28 22:08");
  assert.equal(formatClockDateTime(new Date("2026-08-29T00:00:00Z"), "UTC"), "2026-08-29 00:00");
});

test("clock details use UTC offsets with daylight saving and fractional zones", () => {
  const summer = new Date("2026-08-28T22:08:00Z");
  const winter = new Date("2026-12-28T22:08:00Z");

  assert.equal(formatClockUtcOffset(summer, "Asia/Shanghai"), "UTC+8");
  assert.equal(formatClockUtcOffset(summer, OPENAI_TIME_ZONE), "UTC-7");
  assert.equal(formatClockUtcOffset(winter, OPENAI_TIME_ZONE), "UTC-8");
  assert.equal(formatClockUtcOffset(summer, "Asia/Kolkata"), "UTC+5:30");
  assert.equal(formatClockUtcOffset(summer, "UTC"), "UTC");
});
