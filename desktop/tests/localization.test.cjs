const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const legacy = (suite, name, fn) => test(`${suite}.${name}`, fn);
const root = path.resolve(__dirname, "../.."),
  resource = path.join(root, "resources/locales");
const strings = (locale) =>
  Object.fromEntries(
    [
      ...fs
        .readFileSync(
          path.join(resource, `${locale}.lproj/Localizable.strings`),
          "utf8",
        )
        .matchAll(/"((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)"\s*;/g),
    ].map((m) => [JSON.parse(`"${m[1]}"`), JSON.parse(`"${m[2]}"`)]),
  );
const english = strings("en"),
  locales = fs
    .readdirSync(resource)
    .filter((p) => p.endsWith(".lproj"))
    .map((p) => p.slice(0, -6));
legacy("LocalizationConsistencyTests", "testLocaleKeySetsMatchEnglish", () => {
  assert(Object.keys(english).length > 0);
  for (const locale of ["en", "zh-Hans", "ja", "ko"])
    assert.deepEqual(
      Object.keys(strings(locale)).sort(),
      Object.keys(english).sort(),
      locale,
    );
});
legacy(
  "LocalizationConsistencyTests",
  "testSourceLocalizationKeysExistInAllLocales",
  () => {
    const source =
      fs.readFileSync(path.join(root, "desktop/ui/index.html"), "utf8") +
      fs.readFileSync(path.join(root, "desktop/ui/app.js"), "utf8");
    const used = new Set(
      [...source.matchAll(/data-(?:l10n|title)="([^"]+)"/g)].map((m) => m[1]),
    );
    for (const m of source.matchAll(/tr\(\s*["']([^"']+)["']/g)) used.add(m[1]);
    assert(used.size > 0);
    for (const locale of ["en", "zh-Hans", "ja", "ko"]) {
      const map = strings(locale);
      for (const key of used)
        assert(Object.hasOwn(map, key), `${locale} missing ${key}`);
    }
  },
);
legacy(
  "LocalizationConsistencyTests",
  "testErrorStringsAreTranslatedInNonEnglishLocales",
  () => {
    for (const locale of ["zh-Hans", "ja", "ko"]) {
      const map = strings(locale);
      for (const key of Object.keys(map).filter((k) => k.startsWith("error.")))
        assert.notEqual(map[key], english[key], `${locale}: ${key}`);
    }
  },
);
test("Generated localizations match all original language resources", () => {
  const scope = { window: {} };
  vm.runInNewContext(
    fs.readFileSync(path.join(root, "desktop/ui/locales.js"), "utf8"),
    scope,
  );
  const generated = JSON.parse(JSON.stringify(scope.window.COPOOL_LOCALES));
  for (const locale of locales)
    assert.deepEqual(generated[locale], strings(locale));
});

test("Positional localized messages preserve literal percent signs in account labels", () => {
  const { formatLocalized } = require("../ui/presentation.js");
  assert.equal(
    formatLocalized("Remote deployment completed: %1$@", ["Tokyo"]),
    "Remote deployment completed: Tokyo",
  );
  assert.equal(
    formatLocalized("%2$@ / %1$@", ["literal%@", "second"]),
    "second / literal%@",
  );
  assert.equal(
    formatLocalized("%@ · %@ · 100%%", ["first", "second"]),
    "first · second · 100%",
  );
});
