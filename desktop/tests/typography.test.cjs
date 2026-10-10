const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const root = path.resolve(__dirname, "../..");
const css = fs.readFileSync(path.join(root, "desktop/ui/style.css"), "utf8");
const html = fs.readFileSync(path.join(root, "desktop/ui/index.html"), "utf8");
test("System typography retains automatic optical sizing without forced variation coordinates", () => {
  const rule = css.match(/\.macos body,\s*\.macos body \*\s*\{([^}]+)\}/)?.[1];
  assert(rule, "macOS typography rule is missing");
  assert.match(rule, /font-optical-sizing:\s*auto/);
  assert.match(rule, /font-variation-settings:\s*normal/);
});
test("Body text uses Coport's small system type without synthesized faces", () => {
  const rule = css.match(/\n\nbody\s*\{([^}]+)\}/)?.[1];
  assert(rule, "body rule is missing");
  assert.match(rule, /font:\s*11px\/1\.35\s+-apple-system/);
  assert.match(rule, /font-synthesis:\s*none/);
});
test("A single Coport stylesheet is loaded", () => {
  const sheets = [...html.matchAll(/<link rel="stylesheet" href="([^"]+)"/g)];
  assert.deepEqual(
    sheets.map((m) => m[1]),
    ["style.css"],
  );
});
