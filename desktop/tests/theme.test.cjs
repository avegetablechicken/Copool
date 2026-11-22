const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
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

function extractFunction(text, name) {
  const source = lf(text);
  const start = source.indexOf(`function ${name}(`);
  assert.notEqual(start, -1, `missing function ${name}`);
  const end = source.indexOf("\n}\n", start);
  assert.notEqual(end, -1, `unterminated function ${name}`);
  return source.slice(start, end + 2);
}

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

  test(`applyTheme updates root dataset, color-scheme meta, and storage (${format})`, () => {
    const fnSource = extractFunction(read("desktop/ui/app.js", newline), "applyTheme");
    const storage = new Map();
    const meta = { content: "light dark" };
    const rootElement = { dataset: {} };
    const scope = {
      P,
      document: {
        documentElement: rootElement,
        querySelector: (selector) => (selector === 'meta[name="color-scheme"]' ? meta : null),
      },
      localStorage: {
        setItem: (key, value) => storage.set(key, value),
      },
    };
    vm.runInNewContext(`${fnSource}\nthis.applyTheme = applyTheme;`, scope);

    scope.applyTheme("dark");
    assert.equal(rootElement.dataset.theme, "dark");
    assert.equal(meta.content, "dark");
    assert.equal(storage.get("copool.theme"), "dark");

    scope.applyTheme("light");
    assert.equal(rootElement.dataset.theme, "light");
    assert.equal(meta.content, "light");
    assert.equal(storage.get("copool.theme"), "light");

    scope.applyTheme("system");
    assert.equal("theme" in rootElement.dataset, false);
    assert.equal(meta.content, "light dark");
    assert.equal(storage.get("copool.theme"), "system");

    scope.applyTheme("unknown");
    assert.equal("theme" in rootElement.dataset, false);
    assert.equal(meta.content, "light dark");
    assert.equal(storage.get("copool.theme"), "system");
  });
}
