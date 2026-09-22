import assert from "node:assert/strict";
import test from "node:test";

import {
  getSubscriptionMetadataFreshness,
  SUBSCRIPTION_METADATA_FRESH_MS,
} from "../src/lib/subscriptionMetadata.ts";

const NOW = Date.parse("2030-01-02T00:00:00Z");

test("subscription metadata stays fresh within the refresh grace window", () => {
  const refreshedAt = new Date(NOW - SUBSCRIPTION_METADATA_FRESH_MS).toISOString();
  assert.equal(getSubscriptionMetadataFreshness(refreshedAt, NOW), "fresh");
});

test("subscription metadata becomes cached after the refresh grace window", () => {
  const refreshedAt = new Date(NOW - SUBSCRIPTION_METADATA_FRESH_MS - 1).toISOString();
  assert.equal(getSubscriptionMetadataFreshness(refreshedAt, NOW), "cached");
});

test("missing or malformed subscription metadata timestamps are unknown", () => {
  assert.equal(getSubscriptionMetadataFreshness(null, NOW), "unknown");
  assert.equal(getSubscriptionMetadataFreshness("not-a-date", NOW), "unknown");
});
