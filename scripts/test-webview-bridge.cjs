// Tests for the small JS boundary shared by Android and the browser harness.
// Run with `node --test scripts/test-webview-bridge.cjs`; no npm dependencies.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const shim = fs.readFileSync(path.join(__dirname, '../crates/wl-app/ui/public/invoke-shim.js'), 'utf8');

function load({ url = 'http://localhost:1420', targets = {}, invoke, protectedScreen = false } = {}) {
  const clicks = [];
  const window = { location: new URL(url) };
  if (invoke) window.__TAURI_INTERNALS__ = { invoke };
  const document = {
    querySelector(selector) {
      if (selector === '[data-back-protected]') return protectedScreen ? {} : null;
      const [, layer] = /^button\[data-back-layer="(\w+)"\]$/.exec(selector);
      if (!Object.hasOwn(targets, layer)) return null;
      return {
        disabled: targets[layer] === 'disabled',
        click() { clicks.push(layer); delete targets[layer]; },
      };
    },
  };
  vm.runInNewContext(shim, { window, document });
  return { ...window, clicks };
}

test('Android Back closes one layer at a time before leaving the page', () => {
  const app = load({ targets: { page: true, telemetry: true, nav: true, picker: true } });
  for (const layer of ['picker', 'nav', 'telemetry', 'page']) {
    assert.equal(app.wlHandleBack(), true);
    assert.equal(app.clicks.at(-1), layer);
  }
  assert.equal(app.clicks.length, 4);
  assert.equal(app.wlHandleBack(), false);
});

test('Back cannot bypass a disabled action or abandon backup onboarding', () => {
  const busy = load({ targets: { page: 'disabled' } });
  assert.equal(busy.wlHandleBack(), true);
  assert.deepEqual(busy.clicks, []);
  assert.equal(load({ protectedScreen: true }).wlHandleBack(), true);
});

test('bundled native pages never silently fall back to mock data', async () => {
  const url = 'https://tauri.localhost/';
  const missing = load({ url });
  assert.equal(missing.wlTransport(), 'none');
  await assert.rejects(missing.wlInvoke('identity_status'), /native command bridge/);
  const native = load({ url, invoke: async (command, args) => ({ command, args }) });
  assert.equal(native.wlTransport(), 'native');
  const args = { directive_id: 'abc' };
  assert.deepEqual(await native.wlInvoke('mark_task_done', args), { command: 'mark_task_done', args });
  const browser = load();
  assert.equal(browser.wlTransport(), 'mock');
  assert.equal((await browser.wlInvoke('identity_status')).has, false);
});
