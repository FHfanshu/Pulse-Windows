// Converts the upstream Pulse Localizable.strings files into flat JSON locale files.
// Usage: node scripts/strings-to-json.mjs <upstream Sources/Pulse/Resources> locales
import fs from "node:fs";
import path from "node:path";

const [, , srcDir, outDir] = process.argv;
const langs = {
  "en.lproj": "en",
  "zh-Hans.lproj": "zh-Hans",
  "zh-Hant.lproj": "zh-Hant",
  "ja.lproj": "ja",
  "ko.lproj": "ko",
};

const ESCAPES = { n: "\n", t: "\t", '"': '"', "\\": "\\" };

function unescape(s) {
  return s.replace(/\\(u[0-9a-fA-F]{4}|.)/g, (_, c) =>
    c.length > 1 ? String.fromCharCode(parseInt(c.slice(1), 16)) : (ESCAPES[c] ?? c),
  );
}

for (const [dir, code] of Object.entries(langs)) {
  const text = fs
    .readFileSync(path.join(srcDir, dir, "Localizable.strings"), "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "");
  const re = /"((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)"\s*;/g;
  const out = {};
  let m;
  while ((m = re.exec(text))) out[unescape(m[1])] = unescape(m[2]);
  fs.writeFileSync(path.join(outDir, `${code}.json`), JSON.stringify(out, null, 1) + "\n");
  console.log(code, Object.keys(out).length);
}
