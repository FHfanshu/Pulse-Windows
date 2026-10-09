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
  // Keep keys that exist in the current locale file but not upstream (e.g. "Detected on this PC"),
  // so re-importing upstream does not drop them. They are appended after the upstream keys.
  const outPath = path.join(outDir, `${code}.json`);
  let kept = 0;
  if (fs.existsSync(outPath)) {
    const existing = JSON.parse(fs.readFileSync(outPath, "utf8"));
    for (const [k, v] of Object.entries(existing)) {
      if (!(k in out)) {
        out[k] = v;
        kept++;
      }
    }
  }
  fs.writeFileSync(outPath, JSON.stringify(out, null, 1) + "\n");
  console.log(code, Object.keys(out).length, kept ? `(kept ${kept} local-only keys)` : "");
}
