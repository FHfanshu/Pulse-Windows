// Run with: node --test scripts/check-rhythm.mjs
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../ui/src/recap/rhythm.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
const { rhythmBand, hourInBand } = await import('data:text/javascript;base64,' + Buffer.from(js).toString('base64'));
const highlighted = band => Array.from({ length: 24 }, (_, h) => h).filter(h => hourInBand(h, band));

test('afternoon work highlights 14 through 17 on the lower half', () => {
  const hours = Array(24).fill(1);
  for (const h of [14, 15, 16, 17]) hours[h] = 10;
  const band = rhythmBand({ hours });
  assert.deepEqual({ from: band.from, length: band.length }, { from: 14, length: 4 });
  assert.deepEqual(highlighted(band), [14, 15, 16, 17]);
  assert.equal(band.share, 40 / 60);
});

test('every reader gets their own busiest four hours, not a fixed 21:00 to 05:00', () => {
  const hours = Array(24).fill(0);
  for (const h of [10, 11, 12, 13]) hours[h] = 5;
  hours[22] = 1;
  const band = rhythmBand({ hours });
  assert.deepEqual(highlighted(band), [10, 11, 12, 13]);
  assert.equal(band.share, 20 / 21);
});

test('busiest four hours can cross midnight and ties keep the earliest start', () => {
  const hours = Array(24).fill(0);
  for (const h of [22, 23, 0, 1]) hours[h] = 10;
  assert.deepEqual(highlighted(rhythmBand({ hours })), [0, 1, 22, 23]);
  assert.equal(rhythmBand({ hours: Array(24).fill(1) }).from, 0);
});

test('missing, partial, and empty hourly data do not claim a highlight', () => {
  assert.equal(rhythmBand({}), null);
  assert.equal(rhythmBand({ hours: [1, 2] }), null);
  assert.equal(rhythmBand({ hours: Array(24).fill(0) }), null);
});
