// Worldline invoke shim — bridges Dioxus (wasm) to Tauri host commands.
// In the browser (dx serve hot reload), falls back to a mock harness so
// the UI is fully developable without the native shell.
(function () {
  // Resolve lazily: Tauri injects __TAURI_INTERNALS__ after <head> parse,
  // so an eager capture would pin nativeInvoke=null forever (native runs
  // would silently fall back to mocks).
  function native() {
    try {
      var t = window.__TAURI_INTERNALS__;
      if (t && typeof t.invoke === 'function') { return t.invoke; }
    } catch (e) { /* web dev mode */ }
    return null;
  }

  var mockDb = {
    identity: null,
    directive: null,
    goal: null,
    velocity: { milestones_remaining: 3, days_remaining: 21, target_per_day: 0.14, completion_ratio: 0.8, estimate_adjustment: 0.92 },
    checkin: null,
  };

  var MOCKS = {
    identity_status: function () {
      return Promise.resolve({
        has: !!mockDb.identity,
        unlocked: !!mockDb.identity,
        vault_has_mnemonic: !!mockDb.identity,
      });
    },
    identity_unlock: function () {
      if (mockDb.identity) { return Promise.resolve('mock-account'); }
      return Promise.reject('no mnemonic in vault');
    },
    set_api_key: function (args) {
      if (!args.provider || !args.key) { return Promise.reject('bad key args'); }
      mockDb.keys = mockDb.keys || {};
      mockDb.keys[args.provider] = args.key;
      return Promise.resolve(true);
    },
    has_api_key: function (args) {
      return Promise.resolve(!!(mockDb.keys && mockDb.keys[args.provider]));
    },
    delete_api_key: function (args) {
      var had = !!(mockDb.keys && mockDb.keys[args.provider]);
      if (mockDb.keys) { delete mockDb.keys[args.provider]; }
      return Promise.resolve(had);
    },
    identity_generate: function () {
      mockDb.identity = {
        phrase: 'beacon orbit silence dynamic marble drift lattice kinetic harbor canyon velvet anchor',
        account_id: '9f2c'.repeat(8),
        verify_indices: [2, 6, 10],
      };
      return Promise.resolve(mockDb.identity);
    },
    identity_verify_backup: function (args) {
      // Positional like the shell (identity_verify_backup compares each
      // word against its challenge index — membership is not enough).
      var words = args.words || [];
      var indices = args.indices || [];
      var phrase = ((mockDb.identity && mockDb.identity.phrase) || '').split(' ');
      if (phrase.length !== 12 || indices.length !== words.length || indices.length === 0) {
        return Promise.resolve(false);
      }
      var ok = indices.every(function (idx, i) {
        var w = words[i] || '';
        return phrase[idx] !== undefined && phrase[idx].toLowerCase() === w.trim().toLowerCase();
      });
      return Promise.resolve(ok);
    },
    // Fresh-install fidelity: no directive until a goal or briefing
    // creates one, so `dx serve` exercises the "No active directive."
    // empty-state and the drawer → create-goal path (MVP-1).
    current_directive: function () {
      return Promise.resolve(mockDb.directive || {
        directive_id: '', milestone_id: '',
        title: 'All clear for today',
        instruction: 'The queue is empty. Check in this evening or plan tomorrow\'s goal.',
        phase: null, estimated_minutes: 0, state: 'idle', milestone_title: null,
      });
    },
    complete_directive: function () {
      var d = mockDb.directive;
      if (d && d.phase && d.phase[0] < d.phase[1]) {
        d.phase = [d.phase[0] + 1, d.phase[1]];
        d.estimated_minutes = 25;
      } else {
        mockDb.directive = {
          directive_id: '', milestone_id: '',
          title: 'All clear for today',
          instruction: 'The queue is empty. Check in this evening.',
          phase: null, estimated_minutes: 0, state: 'idle', milestone_title: null,
        };
      }
      return Promise.resolve(mockDb.directive);
    },
    bail_out: function (args) {
      mockDb.directive = null;
      return Promise.resolve({
        directive_id: 'dir-demo-1',
        reason: args.reason,
        recovery: 'downsized:15',
      });
    },
    check_in: function (args) {
      mockDb.checkin = args;
      return Promise.resolve(mockDb.velocity);
    },
    velocity: function () { return Promise.resolve(mockDb.velocity); },
    settings_get: function () {
      return Promise.resolve({
        theme: 'dark', hotkey: 'alt+space', always_on_top: false,
        ai_provider: null, tier1_model: null, tier2_model: null, relay_url: null,
      });
    },
    settings_save: function (args) { return Promise.resolve(null); },
    create_goal: function (args) {
      mockDb.goal = args;
      // Mirror the shell's B-002 auto-seed: a manual goal arrives with
      // one starter directive so the MVP path (create → active
      // directive on canvas) works with no API key.
      mockDb.directive = {
        directive_id: 'dir-seeded-1',
        milestone_id: 'ms-seeded-1',
        title: args.title,
        instruction: 'First step: open your tools and start.',
        phase: null,
        estimated_minutes: 25,
        state: 'active',
        milestone_title: 'First steps',
      };
      return Promise.resolve({
        id: 'goal-1', title: args.title, target_date: args.target_date || null,
        milestone_done: 0, milestone_total: 1,
      });
    },
    create_manual_milestone: function (args) { return Promise.resolve('ms-' + Date.now()); },
    create_manual_directive: function (args) { return Promise.resolve('dir-' + Date.now()); },
    sync_now: function () { return Promise.resolve({ pushed: 0, pulled: 0, applied: 0, pending: 0, quarantined: 0, cursor: '' }); },
    relay_authenticate: function () {
      // Real shell returns RelayAuthView { account_id, expires_at }
      // (epoch seconds); the settings "Test connection" prints the
      // remaining session minutes from it.
      return Promise.resolve({ account_id: '9f2c'.repeat(8), expires_at: Math.floor(Date.now() / 1000) + 3600 });
    },
    master_plan: function (args) { return Promise.resolve('goal-ai-1'); },
    morning_briefing: function (args) {
      var titles = ['Write 300 words on Section 2.1', "Review yesterday's test failures"];
      mockDb.directive = {
        directive_id: 'dir-brief-1', milestone_id: 'ms-mock-1',
        title: titles[0], instruction: 'Draft the merge-semantics prose. Momentum only.',
        phase: [1, 2], estimated_minutes: 25, state: 'active', milestone_title: 'Persistence Layer',
      };
      return Promise.resolve({ titles: titles, created_ids: ['dir-brief-1', 'dir-brief-2'] });
    },
    set_always_on_top: function (args) { return Promise.resolve(null); },
    toggle_window_visibility: function () { return Promise.resolve(null); },
    identity_restore: function (args) { return Promise.resolve('restored-' + (args.phrase || '').length); },
  };

  window.wlInvoke = function (cmd, args) {
    var n = native();
    if (n) {
      return n(cmd, args);
    }
    if (MOCKS[cmd]) {
      return MOCKS[cmd](args || {});
    }
    return Promise.reject('unknown command: ' + cmd);
  };
})();
