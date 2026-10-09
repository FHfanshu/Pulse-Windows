// Keep the desktop version, Cargo workspace and frontend package in sync before shipping.
import fs from 'node:fs';
import assert from 'node:assert/strict';

const readJson = file => JSON.parse(fs.readFileSync(file, 'utf8'));
const version = readJson('src-tauri/tauri.conf.json').version;
assert.match(version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/, 'App version must be a semver string');
assert.equal(readJson('package.json').version, version, 'package.json version differs');
const lock = readJson('package-lock.json');
assert.equal(lock.version, version, 'package-lock.json version differs');
assert.equal(lock.packages[''].version, version, 'package-lock.json root package version differs');
const cargo = fs.readFileSync('Cargo.toml', 'utf8');
const workspace = cargo.match(/\[workspace\.package\]([\s\S]*?)(?=\n\[|$)/)?.[1];
assert.equal(workspace?.match(/^version\s*=\s*"([^"]+)"/m)?.[1], version, 'Cargo workspace version differs');
const cargoLock = fs.readFileSync('Cargo.lock', 'utf8');
for (const name of ['pulse', 'pulse-core']) {
  const entry = cargoLock.split('[[package]]').find(block => block.match(/^name\s*=\s*"([^"]+)"/m)?.[1] === name);
  assert.equal(entry?.match(/^version\s*=\s*"([^"]+)"/m)?.[1], version, `Cargo.lock version differs for ${name}`);
}
const expectedTag = process.env.PULSE_RELEASE_TAG;
if (expectedTag) {
  assert.equal(expectedTag, `v${version}`, 'Release tag must match the application version');
  assert.ok(fs.existsSync(`docs/releases/${expectedTag}.md`), 'Versioned release notes are required');
}
console.log(`Pulse ${version}: all version files agree${expectedTag ? ` with ${expectedTag}` : ''}`);
if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\n`);
