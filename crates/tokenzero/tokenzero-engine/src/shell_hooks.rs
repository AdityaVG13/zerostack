//! Transport-neutral process-observation hooks for the domain shell engine.
//!
//! Raw-worker lifecycle code installs process observation at process start.
//! The domain engine never imports planners or transport adapters directly.

use std::sync::RwLock;

pub type NoteChildFn = fn(Option<u32>, Option<u32>, &'static str);
pub type ReserveBackgroundFn = fn(&str);
pub type NoteBackgroundChildFn = fn(&str, Option<u32>, Option<u32>);
pub type FinishBackgroundFn = fn(&str);
pub type SnapshotFn = fn() -> serde_json::Value;

#[derive(Clone, Copy)]
struct Hooks {
    note_child: NoteChildFn,
    reserve_background_job: ReserveBackgroundFn,
    note_background_child: NoteBackgroundChildFn,
    finish_background_job: FinishBackgroundFn,
    process_observation_snapshot: SnapshotFn,
}

fn noop_note_child(_pid: Option<u32>, _pgid: Option<u32>, _state: &'static str) {}
fn noop_reserve(_id: &str) {}
fn noop_note_background(_id: &str, _pid: Option<u32>, _pgid: Option<u32>) {}
fn noop_finish(_id: &str) {}
fn empty_snapshot() -> serde_json::Value {
    serde_json::json!({})
}

fn default_hooks() -> Hooks {
    Hooks {
        note_child: noop_note_child,
        reserve_background_job: noop_reserve,
        note_background_child: noop_note_background,
        finish_background_job: noop_finish,
        process_observation_snapshot: empty_snapshot,
    }
}

static HOOKS: RwLock<Hooks> = RwLock::new(Hooks {
    note_child: noop_note_child,
    reserve_background_job: noop_reserve,
    note_background_child: noop_note_background,
    finish_background_job: noop_finish,
    process_observation_snapshot: empty_snapshot,
});

/// Install transport-neutral worker process-observation hooks.
pub fn install(hooks: ProcessHooks) {
    if let Ok(mut guard) = HOOKS.write() {
        *guard = Hooks {
            note_child: hooks.note_child,
            reserve_background_job: hooks.reserve_background_job,
            note_background_child: hooks.note_background_child,
            finish_background_job: hooks.finish_background_job,
            process_observation_snapshot: hooks.process_observation_snapshot,
        };
    }
}

/// Reset hooks to no-ops (tests).
#[allow(dead_code)]
pub fn reset() {
    if let Ok(mut guard) = HOOKS.write() {
        *guard = default_hooks();
    }
}

impl ProcessHooks {
    /// Hooks that only track the foreground child process for raw-worker v2
    /// cancellation; every other slot stays a no-op.
    pub fn with_note_child(note_child: NoteChildFn) -> Self {
        Self {
            note_child,
            reserve_background_job: noop_reserve,
            note_background_child: noop_note_background,
            finish_background_job: noop_finish,
            process_observation_snapshot: empty_snapshot,
        }
    }
}

/// Domain-facing worker process-observation bundle.
#[derive(Clone, Copy)]
pub struct ProcessHooks {
    pub note_child: NoteChildFn,
    pub reserve_background_job: ReserveBackgroundFn,
    pub note_background_child: NoteBackgroundChildFn,
    pub finish_background_job: FinishBackgroundFn,
    pub process_observation_snapshot: SnapshotFn,
}

fn current() -> Hooks {
    HOOKS.read().map(|g| *g).unwrap_or_else(|_| default_hooks())
}

pub fn note_child(pid: Option<u32>, pgid: Option<u32>, cancellation_state: &'static str) {
    (current().note_child)(pid, pgid, cancellation_state);
}

pub fn reserve_background_job(id: &str) {
    (current().reserve_background_job)(id);
}

pub fn note_background_child(id: &str, pid: Option<u32>, pgid: Option<u32>) {
    (current().note_background_child)(id, pid, pgid);
}

pub fn finish_background_job(id: &str) {
    (current().finish_background_job)(id);
}

pub fn process_observation_snapshot() -> serde_json::Value {
    (current().process_observation_snapshot)()
}
