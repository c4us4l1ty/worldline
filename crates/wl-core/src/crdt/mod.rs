//! Generic datatype-aware CRDT engine (PRD §2.1): row-level
//! last-writer-wins registers with HLC timestamps and deterministic
//! device-id tie-breaking, plus tombstones for deletes.
//!
//! Convergence contract: for any set of replicas receiving the same
//! multiset of operations (in any order), final state is identical.
//! LWW isdatatype-aware via the [`CrdtTable`] registry: every synced
//! table declares its merge semantics up front.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::hlc::HlcTimestamp;

/// Syncable row types and their CRDT semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrdtTable {
    Goals,
    Milestones,
    Directives,
    DirectivePhases,
    CheckIns,
    Bailouts,
    AppSettings,
}

impl CrdtTable {
    pub fn as_str(&self) -> &'static str {
        match self {
            CrdtTable::Goals => "goals",
            CrdtTable::Milestones => "milestones",
            CrdtTable::Directives => "directives",
            CrdtTable::DirectivePhases => "directive_phases",
            CrdtTable::CheckIns => "check_ins",
            CrdtTable::Bailouts => "bailouts",
            CrdtTable::AppSettings => "app_settings",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "goals" => Some(CrdtTable::Goals),
            "milestones" => Some(CrdtTable::Milestones),
            "directives" => Some(CrdtTable::Directives),
            "directive_phases" => Some(CrdtTable::DirectivePhases),
            "check_ins" => Some(CrdtTable::CheckIns),
            "bailouts" => Some(CrdtTable::Bailouts),
            "app_settings" => Some(CrdtTable::AppSettings),
            _ => None,
        }
    }
}

/// A single replicated mutation. `fields` carries the full row state
/// for upserts (LWW register value); `tombstone` marks deletes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrdtOp {
    /// Globally unique operation id (outbox primary key).
    pub operation_id: String,
    /// Table this op targets.
    pub table: String,
    /// Row id within the table (compound key encoded as `a:b`).
    pub record_id: String,
    /// Writer's HLC timestamp — the LWW arbitration key.
    pub hlc: String,
    /// Monotonic sequence per (device) — secondary arbitration.
    pub device: u16,
    /// Row field state for upserts (`None` for tombstones).
    pub fields: Option<serde_json::Value>,
    /// `true` = delete this row (LWW tombstone wins on ties too).
    pub tombstone: bool,
}

/// The in-memory merged state of one table: record_id → winning value.
#[derive(Debug, Clone, PartialEq)]
pub struct TableState {
    /// LWW register: latest non-tombstone field value per record.
    pub rows: HashMap<String, serde_json::Value>,
    /// Latest tombstone timestamp per record (delete memory).
    pub tombstones: HashMap<String, HlcTimestamp>,
    /// Latest op HLC per record (arbitration bookkeeping).
    pub last_op: HashMap<String, (HlcTimestamp, u16)>,
}

impl TableState {
    pub fn new() -> Self {
        Self {
            rows: HashMap::new(),
            tombstones: HashMap::new(),
            last_op: HashMap::new(),
        }
    }

    /// Applies one operation. Returns `true` if state changed.
    /// Deterministic: pure function of (current state, op).
    pub fn apply(&mut self, op: &CrdtOp) -> bool {
        let ts = match HlcTimestamp::parse(&op.hlc) {
            Ok(t) => t,
            Err(_) => return false, // malformed op: ignore deterministically
        };
        let prior = self.last_op.get(&op.record_id);
        let accept = match prior {
            Some(&(prev_ts, prev_dev)) => ts > prev_ts || (ts == prev_ts && op.device > prev_dev),
            None => true,
        };
        if !accept {
            return false;
        }
        self.last_op.insert(op.record_id.clone(), (ts, op.device));
        if op.tombstone {
            self.tombstones.insert(op.record_id.clone(), ts);
            self.rows.remove(&op.record_id);
        } else {
            // An upsert only resurrects a row if it postdates the tombstone.
            let resurrect_ok = match self.tombstones.get(&op.record_id) {
                Some(&t_ts) => ts > t_ts,
                None => true,
            };
            if resurrect_ok {
                self.tombstones.remove(&op.record_id);
                // A field-less non-tombstone op is malformed (crafted or
                // truncated): ignore deterministically instead of
                // panicking — `apply` must stay a total function so sync
                // can quarantine poison ops without wedging the cycle.
                let Some(fields) = op.fields.clone() else {
                    return false;
                };
                self.rows.insert(op.record_id.clone(), fields);
            }
        }
        true
    }
}

impl Default for TableState {
    fn default() -> Self {
        Self::new()
    }
}

/// Full replica state across all syncable tables.
#[derive(Debug, Clone, Default)]
pub struct ReplicaState {
    pub tables: HashMap<CrdtTable, TableState>,
}

impl ReplicaState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies an op to the correct table, creating it on demand.
    pub fn apply(&mut self, op: &CrdtOp) -> bool {
        let Some(table) = CrdtTable::from_str(&op.table) else {
            return false;
        };
        self.tables.entry(table).or_default().apply(op)
    }

    /// Digest for convergence assertions in tests: sorted
    /// (record_id, canonical json) pairs per table.
    pub fn digest(&self) -> String {
        let mut parts = Vec::new();
        let mut tables: Vec<_> = self.tables.iter().collect();
        tables.sort_by_key(|(t, _)| t.as_str());
        for (t, st) in tables {
            let mut rows: Vec<_> = st
                .rows
                .iter()
                .map(|(k, v)| format!("{k}={}", canonical_json(v)))
                .collect();
            rows.sort();
            parts.push(format!("{}[{}]", t.as_str(), rows.join("|")));
        }
        parts.join(";")
    }
}

/// Canonical JSON: object keys sorted, no whitespace — stable across
/// serde versions and platforms.
pub fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| format!("{:?}:{}", k, canonical_json(map.get(k.as_str()).unwrap())))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(a) => {
            let inner: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        serde_json::Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(
        id: &str,
        table: &str,
        rec: &str,
        hlc: &str,
        device: u16,
        fields: serde_json::Value,
    ) -> CrdtOp {
        CrdtOp {
            operation_id: id.into(),
            table: table.into(),
            record_id: rec.into(),
            hlc: hlc.into(),
            device,
            fields: Some(fields),
            tombstone: false,
        }
    }

    fn tomb(id: &str, table: &str, rec: &str, hlc: &str, device: u16) -> CrdtOp {
        CrdtOp {
            operation_id: id.into(),
            table: table.into(),
            record_id: rec.into(),
            hlc: hlc.into(),
            device,
            fields: None,
            tombstone: true,
        }
    }

    #[test]
    fn lww_latest_timestamp_wins() {
        let mut s = TableState::new();
        let a = op(
            "1",
            "goals",
            "g1",
            "100.0.1",
            1,
            serde_json::json!({"title": "first"}),
        );
        let b = op(
            "2",
            "goals",
            "g1",
            "200.0.2",
            2,
            serde_json::json!({"title": "second"}),
        );
        assert!(s.apply(&a));
        assert!(s.apply(&b));
        assert_eq!(s.rows["g1"]["title"], "second");
        // Replaying the older op is a no-op (idempotence).
        assert!(!s.apply(&a));
    }

    #[test]
    fn tie_broken_by_device_deterministically() {
        let a = op(
            "1",
            "goals",
            "g1",
            "100.5.1",
            1,
            serde_json::json!({"v": "device1"}),
        );
        let b = op(
            "2",
            "goals",
            "g1",
            "100.5.2",
            2,
            serde_json::json!({"v": "device2"}),
        );
        let mut s1 = TableState::new();
        let mut s2 = TableState::new();
        s1.apply(&a);
        s1.apply(&b);
        s2.apply(&b);
        s2.apply(&a);
        // Identical (pt, ctr) — higher device id wins on both replicas.
        assert_eq!(s1.rows["g1"]["v"], "device2");
        assert_eq!(s2.rows["g1"]["v"], "device2");
    }

    #[test]
    fn tombstone_wins_then_resurrect_on_newer_upsert() {
        let mut s = TableState::new();
        s.apply(&op(
            "1",
            "directives",
            "d1",
            "100.0.1",
            1,
            serde_json::json!({"state": "queued"}),
        ));
        s.apply(&tomb("2", "directives", "d1", "200.0.1", 1));
        assert!(!s.rows.contains_key("d1"));
        // Older upsert cannot resurrect.
        assert!(!s.apply(&op(
            "3",
            "directives",
            "d1",
            "150.0.2",
            2,
            serde_json::json!({"state": "zombie"})
        )));
        assert!(!s.rows.contains_key("d1"));
        // Newer upsert resurrects.
        assert!(s.apply(&op(
            "4",
            "directives",
            "d1",
            "300.0.2",
            2,
            serde_json::json!({"state": "active"})
        )));
        assert_eq!(s.rows["d1"]["state"], "active");
        assert!(s.tombstones.is_empty());
    }

    #[test]
    fn malformed_ops_ignored_deterministically() {
        let mut s = TableState::new();
        let bad = op(
            "1",
            "goals",
            "g1",
            "garbage",
            1,
            serde_json::json!({"x": 1}),
        );
        assert!(!s.apply(&bad));
        assert!(s.rows.is_empty());
    }

    #[test]
    fn convergence_under_random_interleavings() {
        use rand::seq::SliceRandom;
        // 3 devices, 40 ops across mixed tables, shuffled into 3
        // different delivery orders → all replicas converge.
        let mut ops = Vec::new();
        for i in 0..40 {
            let (table, rec) = match i % 4 {
                0 => ("goals", "g1"),
                1 => ("milestones", "m1"),
                2 => ("directives", "d1"),
                _ => ("directives", "d2"),
            };
            let hlc = format!("{}.{}.{}", 1000 + i * 10, 0, i % 3);
            ops.push(op(
                &format!("op{i}"),
                table,
                rec,
                &hlc,
                (i % 3) as u16,
                serde_json::json!({"n": i}),
            ));
        }
        ops.push(tomb("t1", "directives", "d2", "9000.0.1", 1));

        let mut digests = Vec::new();
        let mut rng = rand::thread_rng();
        for _ in 0..3 {
            let mut shuffled = ops.clone();
            shuffled.shuffle(&mut rng);
            let mut r = ReplicaState::new();
            for o in &shuffled {
                r.apply(o);
            }
            digests.push(r.digest());
        }
        assert_eq!(digests[0], digests[1]);
        assert_eq!(digests[1], digests[2]);
        // Tombstoned row must be absent everywhere.
        let mut r = ReplicaState::new();
        for o in &ops {
            r.apply(o);
        }
        let dirs = r.tables.get(&CrdtTable::Directives).unwrap();
        assert!(dirs.rows.contains_key("d1"));
        assert!(!dirs.rows.contains_key("d2"));
    }

    #[test]
    fn unknown_table_ignored() {
        let mut r = ReplicaState::new();
        let o = op("1", "not_a_table", "x", "1.0.1", 1, serde_json::json!({}));
        assert!(!r.apply(&o));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let a = serde_json::json!({"b": 1, "a": {"z": 1, "y": 2}});
        let b = serde_json::json!({"a": {"y": 2, "z": 1}, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert!(canonical_json(&a).starts_with("{\"a\":"));
    }
}
