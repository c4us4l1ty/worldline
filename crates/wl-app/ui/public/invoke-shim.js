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

    check_in: function (args) {
      mockDb.checkin = args;
      return Promise.resolve(mockDb.velocity);
    },
    velocity: function () { return Promise.resolve(mockDb.velocity); },
    settings_get: function () {
      return Promise.resolve({
        theme: mockDb.theme || 'dark',
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
      //
      // `phase: [1, 3]`, not `null`. A directive with no phase is
      // indistinguishable from a finished one as far as
      // `complete_directive` is concerned — it advances `phase[0]` only
      // when there are phases left, so every seeded directive jumped
      // straight to the "All clear for today" stub on the first
      // completion, and the phase badge and progress bar could never be
      // seen under `dx serve`. Three phases means two completions are
      // visible before the queue drains, which is what the canvas is for.
      // The stub keeps `phase: null` — that is a correct "no active
      // phase", not a bug.
      mockDb.directive = {
        directive_id: 'dir-seeded-1',
        milestone_id: 'ms-seeded-1',
        title: args.title,
        instruction: 'First step: open your tools and start.',
        phase: [1, 3],
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
    // The task ledger, and the one command that resolves a row. Both are
    // FABRICATED — nothing is queried under `dx serve`. The control panel
    // renders `· MOCK` beside its System label whenever `transport()`
    // reports the mock, for the same reason the model catalog does
    // (PRD delta 148): a list of invented goals is indistinguishable from
    // a real one from the outside.
    task_ledger: function () {
      var rows = [
        {
          directive_id: 'dir-1', goal_id: 'goal-1',
          goal_title: (mockDb.goal && (mockDb.goal.title || mockDb.goal.intent)) || 'Ship Worldline v0.1',
          milestone_title: 'Wire the protocol',
          title: 'Draft the outline',
          instruction: 'On paper first, one page, no editing.',
          estimated_minutes: 20, state: 'active',
          complexity_label: 'standard', blocked_by: null, phase: [1, 2],
        },
        {
          directive_id: 'dir-2', goal_id: 'goal-1',
          goal_title: (mockDb.goal && (mockDb.goal.title || mockDb.goal.intent)) || 'Ship Worldline v0.1',
          milestone_title: 'Wire the protocol',
          title: 'Write the draft',
          instruction: null,
          estimated_minutes: 45, state: 'queued',
          complexity_label: 'standard',
          // The edge, which is the whole reason this page exists: a task
          // sitting in the queue with nothing visibly wrong with it.
          blocked_by: 'Draft the outline', phase: null,
        },
        {
          directive_id: 'dir-3', goal_id: 'goal-1',
          goal_title: (mockDb.goal && (mockDb.goal.title || mockDb.goal.intent)) || 'Ship Worldline v0.1',
          milestone_title: 'Ship the binary',
          title: 'Cut the release',
          instruction: null,
          estimated_minutes: 25, state: 'completed',
          complexity_label: 'heavy', blocked_by: null, phase: null,
        },
      ];
      return Promise.resolve(rows);
    },
    mark_task_done: function (args) {
      if (!args || !args.directive_id) { return Promise.reject('no directive id'); }
      // Echo the canvas shape back so the tick has something to re-read,
      // exactly as the real shell does: it returns the canvas AFTER the
      // tick, not the row.
      return Promise.resolve({
        directive_id: '', milestone_id: '',
        title: 'All clear for today',
        instruction: 'The queue is empty. Check in this evening or plan tomorrow\'s goal.',
        phase: null, estimated_minutes: 0, state: 'idle', milestone_title: null,
      });
    },
    // The difficulty estimator. Mirrors the shell's Beta mapping: the
    // rating IS the prior, and the counts travel with the percentage.
    calibration_view: function () {
      return Promise.resolve([
        { complexity: 3, label: 'standard', percent: 50, evidence: '0 of 0', observations: 0 },
        { complexity: 4, label: 'heavy', percent: 68, evidence: '4 of 6', observations: 6 },
      ]);
    },
    // Local, free, instant: the compose screen's Generate button is gated on
    // this rather than discovering a missing key from a provider error after
    // a billable request has gone out.
    ai_readiness: function () {
      var models = mockDb.keys && Object.keys(mockDb.keys);
      return Promise.resolve({
        provider: 'openrouter',
        model: (models && models.length) ? 'mockvendor/mock-model-000' : '',
        model_set: !!(models && models.length),
        key_set: !!(mockDb.keys && mockDb.keys.openrouter),
        missing: (models && models.length)
          ? null
          : 'Choose an architect model in Settings.',
      });
    },
    sync_now: function () { return Promise.resolve({ pushed: 0, pulled: 0, applied: 0, pending: 0, quarantined: 0, cursor: '' }); },
    relay_authenticate: function () {
      // Real shell returns RelayAuthView { account_id, expires_at }
      // (epoch seconds); the settings "Test connection" prints the
      // remaining session minutes from it.
      return Promise.resolve({ account_id: '9f2c'.repeat(8), expires_at: Math.floor(Date.now() / 1000) + 3600 });
    },
    // `master_plan_preview` — the BILLABLE half, and it writes nothing.
    // The mock deliberately does not touch `mockDb.goal`, because a
    // preview that created a goal under `dx serve` would make the staging
    // invisible: the user would see a plan appear and have no way to learn
    // that nothing was saved.
    master_plan_preview: function (args) {
      var text = (args && args.intent) || '';
      var complexity = (args && args.complexity) || 3;
      var words = ['light', 'light+', 'standard', 'heavy', 'deep'];
      return Promise.resolve({
        plan: {
          title: 'Ship the relay',
          description: 'A working 9:16 execution terminal.',
          milestones: [
            {
              title: 'Wire the protocol',
              description: null,
              rationale: 'Nothing downstream is testable until this lands.',
              directives: [
                {
                  title: 'Draft the outline', execution_context: 'On paper first.',
                  estimated_minutes: 20, after: null, phases: [],
                },
                {
                  title: 'Write the draft', execution_context: null,
                  estimated_minutes: 45, after: 0,
                  phases: [
                    { title: 'Section one', minutes: 20 },
                    { title: 'Section two', minutes: 25 },
                  ],
                },
              ],
            },
            {
              title: 'Ship the binary',
              description: null,
              rationale: 'Only worth doing once the relay answers.',
              directives: [
                {
                  title: 'Cut the release', execution_context: null,
                  estimated_minutes: 25, after: 2, phases: [],
                },
              ],
            },
          ],
        },
        repair: { notes: [] },
        complexity: complexity,
        fallback: null,
        _label: words[complexity - 1],
      });
    },
    // `commit_plan` — the local write, and the only half that persists.
    // It is a STRUCT parameter, so it arrives wrapped: the browser mock
    // ignores shapes, and the shell test in `ipc_tests.rs` is the gate.
    commit_plan: function (args) {
      var p = (args && args.preview) || {};
      if (!p.plan) { return Promise.reject('no plan in preview'); }
      mockDb.goal = { title: p.plan.title, complexity: p.complexity };
      return Promise.resolve({ goal_id: 'goal-previewed', warnings: [] });
    },
    // `set_always_on_top` used to be mocked here for the Settings pin
    // switch. The switch and the command are both gone (PRD delta 175), so
    // a call to it now falls through to the "unknown command" rejection —
    // which is the correct outcome, not a gap in the harness.
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
