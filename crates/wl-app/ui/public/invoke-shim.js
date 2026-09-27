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
        theme: mockDb.theme || 'dark', always_on_top: false,
        ai_provider: null, tier1_model: null, tier2_model: null, relay_url: null,
      });
    },
    // The theme switch now applies AND persists on click, so the mock has
    // to remember it — otherwise flipping the switch in `dx serve` reverts
    // on the next remount and the control looks broken.
    settings_save: function (args) {
      var s = (args && args.settings) || args;
      if (s && s.theme) { mockDb.theme = s.theme; }
      return Promise.resolve(null);
    },
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
    // Control panel fixtures. Both are FABRICATED — nothing is queried
    // under `dx serve`. The drawer renders `· MOCK` beside the section
    // label whenever `transport()` reports the mock, for the same reason
    // the model catalog does (PRD delta 148): a list of invented goals
    // looks exactly like a real one from the outside.
    list_goals: function () {
      return Promise.resolve(mockDb.goal ? [
        {
          id: 'goal-1',
          title: mockDb.goal.title || 'Untitled goal',
          target_date: mockDb.goal.target_date || null,
          milestone_done: 1,
          milestone_total: 3,
        },
      ] : []);
    },
    entropy_log: function () {
      return Promise.resolve([
        {
          id: 'bail-1',
          goal_id: 'goal-1',
          goal_title: mockDb.goal ? (mockDb.goal.title || 'Untitled goal') : 'Ship Worldline v0.1',
          directive_id: 'dir-9',
          directive_title: 'Draft Section 2.1',
          reason: 'miscalculated_scope',
          note: 'kept re-reading, no forward motion',
          date: '2026-09-21',
          still_blocked: false,
        },
        {
          id: 'bail-2',
          goal_id: 'goal-1',
          goal_title: 'Ship Worldline v0.1',
          directive_id: 'dir-4',
          directive_title: 'Send the contract to legal',
          reason: 'external_dependency',
          note: null,
          date: '2026-09-19',
          still_blocked: true,
        },
      ]);
    },
    sync_now: function () { return Promise.resolve({ pushed: 0, pulled: 0, applied: 0, pending: 0, quarantined: 0, cursor: '' }); },
    relay_authenticate: function () {
      // Real shell returns RelayAuthView { account_id, expires_at }
      // (epoch seconds); the settings "Test connection" prints the
      // remaining session minutes from it.
      return Promise.resolve({ account_id: '9f2c'.repeat(8), expires_at: Math.floor(Date.now() / 1000) + 3600 });
    },
    // `intent` is raw user text, not a title — the architect names the
    // goal. This mock ignores arg shapes entirely, so a rename of the
    // shell's params (goal_title → intent) will NOT be caught here; it has
    // to be verified against the real shell.
    master_plan: function (args) {
      var text = (args && args.intent) || '';
      mockDb.goal = { intent: text, target_date: (args && args.target_date) || null };
      mockDb.directive = {
        directive_id: 'dir-plan-1',
        milestone_id: 'ms-plan-1',
        title: 'Scope the first sitting',
        instruction: 'Momentum only — do not plan the whole thing.',
        phase: null,
        estimated_minutes: 25,
        state: 'active',
        milestone_title: 'First steps',
      };
      return Promise.resolve('goal-ai-1');
    },
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
    identity_restore: function (args) { return Promise.resolve('restored-' + (args.phrase || '').length); },

    // Provider model catalog. Enough rows to exercise the search ranking
    // and the row cap (MAX_ROWS is 60, so this list is deliberately
    // larger), and two of the ids are in the curated "Recommended" set so
    // that section renders in the browser harness too.
    list_models: function (args) {
      var families = [
        ['anthropic', 'claude-sonnet-5', 'Anthropic: Claude Sonnet 5', 1000000],
        ['anthropic', 'claude-sonnet-4.6', 'Anthropic: Claude Sonnet 4.6', 1000000],
        ['anthropic', 'claude-opus-4.1', 'Anthropic: Claude Opus 4.1', 200000],
        ['openai', 'gpt-5.4', 'OpenAI: GPT-5.4', 1050000],
        ['openai', 'gpt-5.4-mini', 'OpenAI: GPT-5.4 Mini', 400000],
        ['openai', 'gpt-5-mini', 'OpenAI: GPT-5 Mini', 400000],
        ['google', 'gemini-3.1-pro-preview', 'Google: Gemini 3.1 Pro Preview', 1048576],
        ['google', 'gemini-3.1-flash-lite', 'Google: Gemini 3.1 Flash Lite', 1048576],
        ['mistralai', 'mistral-large-2512', 'Mistral: Large', 131000],
        ['meta-llama', 'llama-4-maverick', 'Meta: Llama 4 Maverick', 1048576],
        ['qwen', 'qwen3.5-plus-20260420', 'Qwen: Qwen3.5 Plus', 1000000],
        ['deepseek', 'deepseek-v3.2', 'DeepSeek: V3.2', 163840],
      ];
      var models = [];
      for (var i = 0; i < families.length; i++) {
        var f = families[i];
        models.push({
          id: f[0] + '/' + f[1],
          name: f[2],
          context_length: f[3],
          supports_response_format: true,
        });
      }
      // Pad past the 60-row render cap so the "showing N of M" line and
      // the ranking behaviour are visible in the harness.
      for (var j = 0; j < 80; j++) {
        models.push({
          id: 'mockvendor/mock-model-' + String(j).padStart(3, '0'),
          name: 'Mock Vendor: Model ' + j,
          context_length: 128000,
          supports_response_format: j % 7 === 0 ? false : true,
        });
      }
      return Promise.resolve({
        provider: (args && args.provider) || 'openrouter',
        models: models,
        recommended: [
          'anthropic/claude-sonnet-5',
          'openai/gpt-5.4',
          'google/gemini-3.1-pro-preview',
        ],
        cached: false,
      });
    },
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
