import assert from "node:assert/strict";
import test from "node:test";

import { extractDeactivationEmailDetails } from "../src/lib/deactivationEmail.ts";

const deactivationEmail = `**主题:**
OpenAI - Access Deactivated [C-DYEDw7i4Kzc1]
**发件人:**
OpenAI
**时间:**
2026-07-23T09:17:20Z
**内容:**

Hello,

We're writing with an important update about your ChatGPT account associated with laurettastoltenbergxe@outlook.com (User ID: user-RYM1RBgCzBmPF1ucHZ93BLOK).

Your account has been deactivated because recent activity violated our Terms and Usage Policies.
`;

test("extracts metadata from the provided markdown email format", () => {
  assert.deepEqual(extractDeactivationEmailDetails(deactivationEmail), {
    email: "laurettastoltenbergxe@outlook.com",
    rawDate: "2026-07-23T09:17:20Z",
    deactivatedAt: "2026-07-23T09:17:20.000Z",
    isDeactivationNotice: true,
  });
});

test("does not retain a date after the current text no longer contains one", () => {
  assert.deepEqual(extractDeactivationEmailDetails("Your account has been deactivated."), {
    email: null,
    rawDate: null,
    deactivatedAt: null,
    isDeactivationNotice: true,
  });
});

test("does not classify a generic authentication failure as deactivation", () => {
  assert.equal(
    extractDeactivationEmailDetails("Authentication failed. Please try again.")
      .isDeactivationNotice,
    false
  );
});
