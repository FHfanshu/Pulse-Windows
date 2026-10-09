// Run with: node --test scripts/check-rhythm.mjs
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../ui/src/recap/rhythm.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
const { rhythmBand, hourInBand } = await import('data:text/javascript;base64,' + Buffer.from(js).toString('base64'));
const highlighted = band => Array.from({ length: 24 }, (_, h) => h).filter(h => hourInBand(h, band));

test('afternoon summary highlights 14 through 17 on the lower half', () => {
  const hours = Array(24).fill(1);
  for (const h of [14, 15, 16, 17]) hours[h] = 10;
  const band = rhythmBand({ hours, persona: 'allDay' });
  assert.deepEqual({ from: band.from, length: band.length }, { from: 14, length: 4 });
  assert.deepEqual(highlighted(band), [14, 15, 16, 17]);
  assert.equal(band.share, 40 / 60);
});

test('night owl wraps across midnight and excludes 05:00', () => {
  const band = rhythmBand({ hours: Array(24).fill(1), persona: 'nightOwl' });
  assert.deepEqual(highlighted(band), [0, 1, 2, 3, 4, 21, 22, 23]);
  assert.equal(band.share, 8 / 24);
});

test('early bird and day shift use the same bands as their captions', () => {
  const hours = Array(24).fill(1);
  assert.deepEqual(highlighted(rhythmBand({ hours, persona: 'earlyBird' })), [5, 6, 7, 8, 9]);
  assert.deepEqual(highlighted(rhythmBand({ hours, persona: 'dayShift' })), [10, 11, 12, 13, 14, 15, 16, 17]);
});

test('busiest four hours can cross midnight and ties keep the first band', () => {
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
