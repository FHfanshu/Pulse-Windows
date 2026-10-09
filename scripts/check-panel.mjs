// Browser regression check. Pass a Playwright Page connected to the Vite dev server.
// No native window or user data is touched: all Tauri IPC uses the fixture below.
import assert from 'node:assert/strict';

export async function checkPanel(page, url = 'http://localhost:1420/panel.html') {
  await page.addInitScript(() => {
    const callbacks = new Map();
    const listeners = new Map();
    let next = 0;
    const usage = (provider, slot = '') => ({
      account: { provider, slot },
      windows: [{ id: 'five', kind: { type: 'fiveHour' }, usedFraction: 0.25,
        windowSeconds: 18000, resetsAt: new Date(Date.now() + 3600000).toISOString(),
        reportsLength: true, estimate: null, isExhausted: false, scope: null, nextExpiry: null, label: null }],
      observedAt: new Date().toISOString(), state: { kind: 'live' }, plan: 'Pro',
      creditBalance: null, creditRemaining: null, origin: null, isCached: false,
    });
    window.__panelTest = {
      calls: [], layout: null,
      settings: { language: 'en', detailedCards: ['claudeCode'], readsTokenSpend: true,
        splitAccounts: [], panelSize: 'standard', railSpacing: 'standard' },
      usages: [usage('claudeCode')], usage,
      emit: (event, payload) => {
        for (const id of listeners.get(event) || []) callbacks.get(id)?.({ event, payload });
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      transformCallback: (fn) => { callbacks.set(++next, fn); return next; },
      invoke: async (cmd, args) => {
        const test = window.__panelTest;
        test.calls.push({ cmd, args });
        switch (cmd) {
          case 'plugin:event|listen':
            listeners.set(args.event, [...(listeners.get(args.event) || []), args.handler]);
            return args.handler;
          case 'get_settings': return test.settings;
          case 'get_snapshot': return { usages: test.usages, refreshing: [] };
          case 'spend_card_providers': return ['claudeCode', 'codex'];
          case 'set_geometry': {
            const s = args.geometry.verticalDocked;
            test.layout = { frame: { x: 0, y: 0, ...s.panel },
              rail: { x: s.panel.w - s.rail.w, y: 100, ...s.rail }, edge: 'right', docked: true };
            test.emit('panel-layout', test.layout);
            return;
          }
          case 'card_spend': return {
            days: [{ date: '2026-10-09', tokens: 12345, cost: 0 }],
            today: { tokens: 12345, cost: 0 }, week: { tokens: 12345, cost: 0 },
            month: { tokens: 12345, cost: 0 }, currency: null, topModel: null, cacheHitRate: null,
          };
          case 'estimated_value': return [];
          case 'prompt_cache': return null;
        }
      },
    };
  });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url);
  await page.waitForSelector('.rail');
  const hover = () => page.evaluate(() => {
    const t = window.__panelTest;
    t.emit('pointer', { point: [t.layout.rail.x + 32, t.layout.rail.y + 64], pressed: false, dragging: false });
  });
  const configure = async (provider, slot, detailed, spend) => {
    await page.evaluate(({ provider, slot, detailed, spend }) => {
      const t = window.__panelTest;
      t.emit('pointer', { point: null, pressed: false, dragging: false });
      const account = slot ? provider + '#' + slot : provider;
      t.settings = { ...t.settings, detailedCards: detailed ? [account] : [], readsTokenSpend: spend };
      t.usages = [t.usage(provider, slot)];
      t.emit('settings-changed', t.settings);
      t.emit('usage-changed', { usages: t.usages, refreshing: [] });
      t.calls = [];
    }, { provider, slot, detailed, spend });
    await page.waitForFunction(() => !document.querySelector('.card'));
    await hover();
    await page.waitForSelector('.card');
  };
  await hover();
  await page.waitForSelector('.activity');
  await page.waitForFunction(() => document.querySelector('.activity')?.textContent.includes('Today'));
  assert.match(await page.locator('.card').innerText(), /Pro.*Updated just now/s);
  assert.match(await page.locator('.activity').innerText(), /all accounts/);
  assert.ok(await page.evaluate(() => window.__panelTest.calls.some(c => c.cmd === 'estimated_value')));

  // A fresh supported provider's extra account must initiate a read, not rely on a primary card having opened.
  await configure('codex', 'extra', true, true);
  await page.waitForFunction(() => document.querySelector('.activity')?.textContent.includes('Today'));
  const extraCalls = await page.evaluate(() => window.__panelTest.calls);
  assert.ok(extraCalls.some(c => c.cmd === 'card_spend' && c.args.provider === 'codex'));
  assert.ok(!extraCalls.some(c => c.cmd === 'estimated_value'));
  assert.match(await page.locator('.card').innerText(), /Pro.*Updated just now/s);
  assert.match(await page.locator('.activity').innerText(), /all accounts/);

  await configure('codex', 'extra', true, false);
  assert.equal(await page.locator('.activity').count(), 0);
  assert.ok(!await page.evaluate(() => window.__panelTest.calls.some(c => c.cmd === 'card_spend')));

  await configure('codex', 'extra', false, true);
  assert.equal(await page.locator('.activity').count(), 0);
  assert.ok(!await page.evaluate(() => window.__panelTest.calls.some(c => c.cmd === 'card_spend')));

  await configure('deepSeek', '', true, true);
  assert.equal(await page.locator('.activity').count(), 0);
  assert.ok(!await page.evaluate(() => window.__panelTest.calls.some(c => c.cmd === 'card_spend')));
  assert.deepEqual(errors, []);
  return 'Passed: primary and extra-account detail cards, local activity, quota estimate isolation, both settings off, unsupported provider.';
}
