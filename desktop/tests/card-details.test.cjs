const { test } = require("node:test");
const assert = require("node:assert/strict");
const dom = require("./fake-dom.cjs");

// What the details say is card-view.js's (card-view.test.cjs); here, how
// card-face.js's edit button shows them.
test("hover and focus update details locally and ignore another pool", () => {
  // Its usual interval, as the core gives the card.
  const refresh = { key: "a", seconds: 180 };
  let target = { account: { id: "a", renewal: "always", tokenExpiresAt: Date.now() / 1000 + 3600, refresh } };
  const tr = (key, ...args) => [key, ...args].join(" ");
  const face = dom.cardFace({
    pool: "claude",
    tr,
    controller: { flipTarget: () => target },
    cardContext: () => ({ pool: face.host.pool, locale: "en", tr, accountProxy: "" }),
  });
  const control = face.editButton("a", "Account");
  assert.match(control.title, /accounts.renewal.expires_format/);
  assert.equal(control.attributes["aria-label"], "accounts.card.edit Account");
  target.account.tokenExpiresAt = 1;
  control.onpointerenter();
  assert.match(control.title, /accounts.renewal.expired/);
  target = { account: { id: "a", proxyUrl: "direct://", refresh } };
  control.onfocus();
  assert.equal(control.title, "direct\naccounts.refresh.interval_format 3 minutes");
  face.host.pool = "codex";
  target = null;
  control.onpointerenter();
  assert.equal(control.title, "direct\naccounts.refresh.interval_format 3 minutes");
});
