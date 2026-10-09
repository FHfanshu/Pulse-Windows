// Writes the update feed the in-app updater reads (`latest.json`, attached to the GitHub release).
// Usage: node scripts/make-latest-json.mjs <installer.exe> <installer.exe.sig> <out latest.json>
// The version comes from src-tauri/tauri.conf.json, the notes from docs/releases/v<version>.md, and
// the download address is the same installer attached to release tag v<version>.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';

const [installer, signaturePath, out] = process.argv.slice(2);
assert.ok(installer && signaturePath && out, 'usage: make-latest-json.mjs <installer.exe> <installer.exe.sig> <out>');

const version = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8')).version;
const tag = `v${version}`;
const notesPath = `docs/releases/${tag}.md`;
assert.ok(fs.existsSync(notesPath), `Missing release notes: ${notesPath}`);
const signature = fs.readFileSync(signaturePath, 'utf8').trim();
assert.ok(signature.length > 0, 'The signature file is empty');
const name = path.basename(installer);
assert.match(name, /^[A-Za-z0-9._-]+$/, 'The installer name must be URL-safe');

const feed = {
  version,
  notes: fs.readFileSync(notesPath, 'utf8').trim(),
  pub_date: new Date().toISOString(),
  platforms: {
    'windows-x86_64': {
      signature,
      url: `https://github.com/FHfanshu/Pulse-Windows/releases/download/${tag}/${name}`,
    },
  },
};
fs.writeFileSync(out, JSON.stringify(feed, null, 2) + '\n');
console.log(`Wrote ${out} for ${tag} (${name})`);
