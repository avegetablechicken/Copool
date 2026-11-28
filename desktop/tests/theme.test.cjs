const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const P = require("../ui/presentation.js");

const root = path.resolve(__dirname, "../..");
/// A file in the given line endings, whatever the checkout's.
const read = (relativePath, newline) =>
  fs
    .readFileSync(path.join(root, relativePath), "utf8")
    .replace(/\r\n/g, "\n")
    .replace(/\n/g, newline);
// Each reading below takes either line ending, as a Windows checkout has.
const lf = (text) => text.replace(/\r\n/g, "\n");

for (const [format, newline] of [["LF", "\n"], ["CRLF", "\r\n"]]) {
  test(`Settings page exposes display theme options (${format})`, () => {
    const html = read("desktop/ui/index.html", newline);
    const selectMatch = lf(html).match(/<select id="theme" class="select">([\s\S]*?)<\/select>/);
    assert(selectMatch, "theme select is missing");
    const options = [...selectMatch[1].matchAll(/value="([^"]+)"[\s\S]*?data-l10n="([^"]+)"/g)].map(
      (match) => [match[1], match[2]],
    );
    assert.deepEqual(options, [
      ["system", "settings.theme.system"],
      ["light", "settings.theme.light"],
      ["dark", "settings.theme.dark"],
    ]);
  });

  test(`Stylesheet supports explicit light, dark, and system themes (${format})`, () => {
    const css = lf(read("desktop/ui/style.css", newline));
    assert.match(css, /:root\[data-theme="light"\]\s*\{\s*color-scheme:\s*light;\s*\}/);
    assert.match(css, /:root\[data-theme="dark"\]\s*\{\s*color-scheme:\s*dark;\s*\}/);
    assert.match(css, /:root:where\(\[data-theme="dark"\]\)\s*\{/);
    assert.match(css, /html\.frosted:where\(\[data-theme="dark"\]\)\s*\{/);
    const mediaBlocks = [...css.matchAll(/@media \(prefers-color-scheme: dark\)\s*\{([\s\S]*?)\n\}/g)];
    assert.equal(mediaBlocks.length, 6);
    // Every selector in them, not just one: the light theme overrides each.
    for (const [, body] of mediaBlocks) {
      const selectors = [...body.matchAll(/([^{}]+)\{/g)].flatMap(([, list]) =>
        list.split(",").map((selector) => selector.trim()),
      );
      assert(selectors.length > 0);
      for (const selector of selectors)
        assert.match(selector, /:not\(\[data-theme="light"\]\)/, selector);
    }
  });
}

test("A theme sets the root's data-theme, the color-scheme meta and the saved value", () => {
  assert.deepEqual(P.themeAttributes("dark"), { dataTheme: "dark", colorScheme: "dark", saved: "dark" });
  assert.deepEqual(P.themeAttributes("light"), { dataTheme: "light", colorScheme: "light", saved: "light" });
  assert.deepEqual(P.themeAttributes("system"), { dataTheme: null, colorScheme: "light dark", saved: "system" });
  assert.deepEqual(P.themeAttributes("unknown"), { dataTheme: null, colorScheme: "light dark", saved: "system" });
});
