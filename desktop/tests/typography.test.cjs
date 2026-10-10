const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const root = path.resolve(__dirname, "../..");
const baseline = JSON.parse(
  fs.readFileSync(path.join(root, "Tests/fixtures/swift-font-metrics.json")),
);
const css = fs.readFileSync(
  path.join(root, "desktop/ui/typography.css"),
  "utf8",
);
test("System typography retains automatic optical sizing without forced variation coordinates", () => {
  const rule = css.match(/\.macos body,\s*\.macos body \*\s*\{([^}]+)\}/)?.[1];
  assert(rule, "macOS typography rule is missing");
  assert.match(rule, /font-optical-sizing:\s*auto\s*!important/);
  assert.match(rule, /font-variation-settings:\s*normal\s*!important/);
  assert.match(rule, /font-synthesis:\s*none/);
});
test("Typography role sizes retain the original AppKit measurements", () => {
  for (const [role, { size }] of Object.entries(baseline.roles)) {
    const value = css.match(new RegExp(`--font-${role}:\\s*(\\d+)px`));
    assert.equal(Number(value?.[1]), size, role);
  }
});
test("Material styling is loaded before the protected native typography rules", () => {
  const html = fs.readFileSync(
    path.join(root, "desktop/ui/index.html"),
    "utf8",
  );
  assert(html.indexOf("typography.css") > html.indexOf("theme.css"));
});
