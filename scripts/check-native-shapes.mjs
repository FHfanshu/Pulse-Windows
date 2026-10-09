// Exercises nativeShapes producer ownership and frame coalescing without a WebView or Tauri.
// Run: node scripts/check-native-shapes.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

const fixture = "M0 0L1 0L1 1Z";
const sourcePath = new URL("../ui/src/panel/nativeShapes.tsx", import.meta.url);
const source = readFileSync(sourcePath, "utf8").replaceAll("import.meta", "globalThis.__meta");
const { outputText, diagnostics = [] } = ts.transpileModule(source, {
  reportDiagnostics: true,
  compilerOptions: {
    jsx: ts.JsxEmit.ReactJSX,
    module: ts.ModuleKind.CommonJS,
    target: ts.ScriptTarget.ES2022,
  },
});
const errors = diagnostics.filter((diagnostic) => diagnostic.category === ts.DiagnosticCategory.Error);
assert.deepEqual(errors, [], "nativeShapes.tsx should transpile without errors");

function createTauriMock() {
  let nextEpoch = 0;
  const begins = [];
  const snapshots = [];
  const invoke = (command, args) => {
    if (command === "begin_backdrop_shapes") {
      const epoch = ++nextEpoch;
      return new Promise((resolve) => begins.push({ epoch, resolve }));
    }
    if (command === "set_backdrop_shapes") {
      snapshots.push(args.shapes);
      return Promise.resolve();
    }
    throw new Error(`Unexpected Tauri command: ${command}`);
  };
  return {
    begins,
    snapshots,
    invoke,
    resolveBegin(index) {
      assert.ok(begins[index], `begin request ${index} should exist`);
      begins[index].resolve(begins[index].epoch);
    },
  };
}

function loadHarness(tauri) {
  let nextFrame = 1;
  let nextTimer = 1;
  const frames = new Map();
  const timers = new Map();
  const components = new Map();
  let rendering = null;

  const react = {
    useRef(initial) {
      assert.ok(rendering, "useRef must run during a component render");
      const index = rendering.refIndex++;
      if (!(index in rendering.instance.refs)) rendering.instance.refs[index] = { current: initial };
      return rendering.instance.refs[index];
    },
    useLayoutEffect(effect, deps) {
      assert.ok(rendering, "useLayoutEffect must run during a component render");
      rendering.effects.push({ index: rendering.effectIndex++, effect, deps });
    },
  };

  const requestAnimationFrame = (callback) => {
    const id = nextFrame++;
    frames.set(id, callback);
    return id;
  };
  const cancelAnimationFrame = (id) => frames.delete(id);
  const setTimeout = (callback, delay) => {
    const id = nextTimer++;
    timers.set(id, { callback, delay });
    return id;
  };
  const clearTimeout = (id) => timers.delete(id);
  const hotDisposers = [];
  const moduleObject = { exports: {} };
  const context = vm.createContext({
    module: moduleObject,
    exports: moduleObject.exports,
    require(name) {
      if (name === "react") return react;
      if (name === "@tauri-apps/api/core") return { invoke: tauri.invoke };
      if (name === "react/jsx-runtime") return { jsx: () => null, jsxs: () => null };
      throw new Error(`Unexpected module: ${name}`);
    },
    requestAnimationFrame,
    cancelAnimationFrame,
    setTimeout,
    clearTimeout,
    window: { setTimeout, clearTimeout },
    getComputedStyle: () => ({ opacity: "1" }),
    __meta: { env: { DEV: false }, hot: { dispose: (callback) => hotDisposers.push(callback) } },
  });
  vm.runInContext(outputText, context, { filename: "nativeShapes.cjs" });

  function render(key, call) {
    let instance = components.get(key);
    if (!instance) {
      instance = { refs: [], effects: [] };
      components.set(key, instance);
    }
    rendering = { instance, refIndex: 0, effectIndex: 0, effects: [] };
    call();
    const pending = rendering.effects;
    rendering = null;
    for (const { index, effect, deps } of pending) {
      const previous = instance.effects[index];
      const sameDeps = deps && previous?.deps && deps.length === previous.deps.length && deps.every((dep, i) => Object.is(dep, previous.deps[i]));
      if (deps && previous && sameDeps) continue;
      previous?.cleanup?.();
      const cleanup = effect();
      instance.effects[index] = { deps, cleanup: typeof cleanup === "function" ? cleanup : undefined };
    }
    return instance;
  }

  function unmount(key) {
    const instance = components.get(key);
    if (!instance) return;
    for (let index = instance.effects.length - 1; index >= 0; index--) instance.effects[index]?.cleanup?.();
    components.delete(key);
  }

  return {
    api: moduleObject.exports,
    render,
    unmount,
    pendingFrames: () => frames.size,
    advanceFrame() {
      const callbacks = [...frames.values()];
      frames.clear();
      for (const callback of callbacks) callback(16);
      return callbacks.length;
    },
    pendingTimers: timers,
    hotDisposers,
  };
}

function svgAt(x, y, width = 100) {
  const host = {};
  return {
    isConnected: true,
    getBoundingClientRect: () => ({ left: x, top: y, width, height: width }),
    closest: (selector) => selector === ".card-host" ? host : null,
  };
}

async function check(name, fn) {
  await fn();
  console.log(`ok   ${name}`);
}

const tauri = createTauriMock();
const first = loadHarness(tauri);

await check("inactive native mode schedules the initial rail snapshot after begin resolves", async () => {
  first.render("rail", () => first.api.RailOutline({ d: fixture, x: 10, y: 20 }));
  assert.equal(first.advanceFrame(), 1);
  assert.equal(tauri.begins.length, 1);
  assert.equal(tauri.snapshots.length, 0);
  tauri.resolveBegin(0);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(first.advanceFrame(), 1);
  assert.equal(tauri.snapshots.length, 1);
  assert.equal(tauri.snapshots[0].epoch, 1);
  assert.equal(tauri.snapshots[0].seq, 1);
  assert.equal(tauri.snapshots[0].rail.d, fixture);
  assert.equal(tauri.snapshots[0].rail.x, 10);
  assert.equal(tauri.snapshots[0].card, null);
});

await check("cleanup and new card registration in one commit send only the final card", async () => {
  first.render("old-card", () => first.api.useCardOutline({ current: svgAt(30, 40) }, fixture, 100));
  assert.equal(first.advanceFrame(), 1);
  assert.equal(tauri.snapshots.at(-1).card.x, 30);

  const before = tauri.snapshots.length;
  first.unmount("old-card");
  first.render("new-card", () => first.api.useCardOutline({ current: svgAt(70, 80) }, fixture, 100));
  assert.equal(first.pendingFrames(), 1);
  assert.equal(first.advanceFrame(), 1);
  assert.equal(tauri.snapshots.length, before + 1);
  assert.equal(tauri.snapshots.at(-1).card.d, fixture);
  assert.equal(tauri.snapshots.at(-1).card.x, 70);
  assert.equal(tauri.snapshots.at(-1).card.y, 80);
});

await check("a late old cleanup preserves the new owner and one frame emits at most one snapshot", () => {
  first.render("stale-card", () => first.api.useCardOutline({ current: svgAt(1, 2) }, fixture, 100));
  first.render("latest-card", () => first.api.useCardOutline({ current: svgAt(90, 100) }, fixture, 100));
  first.unmount("stale-card");
  first.api.setNativeActive(true);
  const before = tauri.snapshots.length;
  assert.equal(first.pendingFrames(), 1);
  assert.equal(first.advanceFrame(), 1);
  assert.equal(tauri.snapshots.length, before + 1);
  assert.equal(tauri.snapshots.at(-1).card.x, 90);
  assert.equal(tauri.snapshots.at(-1).card.y, 100);
});

const restarted = loadHarness(tauri);
await check("a new producer starts a fresh epoch at sequence one", async () => {
  restarted.render("rail", () => restarted.api.RailOutline({ d: fixture, x: 3, y: 4 }));
  assert.equal(restarted.advanceFrame(), 1);
  assert.equal(tauri.begins.length, 2);
  tauri.resolveBegin(1);
  await new Promise((resolve) => setImmediate(resolve));
  const before = tauri.snapshots.length;
  assert.equal(restarted.advanceFrame(), 1);
  assert.equal(tauri.snapshots.length, before + 1);
  assert.equal(tauri.snapshots.at(-1).epoch, 2);
  assert.equal(tauri.snapshots.at(-1).seq, 1);
  assert.equal(tauri.snapshots.at(-1).rail.d, fixture);
});
