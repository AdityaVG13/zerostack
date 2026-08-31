//! Reservation declare / check / acquire / release.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use graphzero_store::Snapshot;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::footprint::{FootprintSnapshot, footprint_from_intent_ops};
use crate::ledger::ReservationLedger;
use crate::schema::{
    ConflictGraphEdge, DeclareResponse, IntentOperation, IntentReservation,
    ReservationCheckResponse, ReservationQueryResponse, ReservationStatus, SCHEMA_VERSION,
};

#[derive(Debug)]
pub enum ReserveError {
    Validation(String),
    NotFound(String),
    Store(String),
}

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReserveError::Validation(s) => write!(f, "{s}"),
            ReserveError::NotFound(s) => write!(f, "{s}"),
            ReserveError::Store(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for ReserveError {}

pub fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn repo_id(repo: &Path) -> Result<String> {
    Ok(repo.canonicalize()?.display().to_string())
}

fn reservation_id(agent: &str, intent_hash: &[u8]) -> String {
    format!(
        "res_{}_{}",
        agent,
        graphzero_store::fast_hex(&intent_hash[..8])
    )
}

fn intent_digest(agent_id: &str, intent_ops: &[IntentOperation]) -> Vec<u8> {
    let payload = serde_json::to_vec(intent_ops).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(&payload);
    h.update(agent_id.as_bytes());
    h.finalize().to_vec()
}

fn store_err<E: std::fmt::Display>(err: E) -> ReserveError {
    ReserveError::Store(err.to_string())
}

fn open_ledger(store_root: &Path) -> Result<ReservationLedger, ReserveError> {
    ReservationLedger::open(store_root).map_err(store_err)
}

fn open_ledger_for_write(store_root: &Path) -> Result<ReservationLedger, ReserveError> {
    ReservationLedger::open_for_write(store_root).map_err(store_err)
}

static RESERVATION_LEDGER_MUTEX: Mutex<()> = Mutex::new(());

fn lock_reservation_ledger() -> Result<std::sync::MutexGuard<'static, ()>, ReserveError> {
    RESERVATION_LEDGER_MUTEX
        .lock()
        .map_err(|_| ReserveError::Store("reservation ledger mutex poisoned".into()))
}

struct ReservationLedgerFileLock {
    _file: File,
}

fn lock_reservation_ledger_file(
    store_root: &Path,
) -> Result<ReservationLedgerFileLock, ReserveError> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(store_root.join("reservation.lock"))
        .map_err(store_err)?;
    file.lock().map_err(store_err)?;
    Ok(ReservationLedgerFileLock { _file: file })
}

fn validate_declare_request(req: &DeclareRequest) -> Result<(), ReserveError> {
    if req.agent_id.trim().is_empty() {
        return Err(ReserveError::Validation("agent_id required".into()));
    }
    if req.intent_ops.is_empty() {
        return Err(ReserveError::Validation("intent_ops required".into()));
    }
    Ok(())
}

fn declare_response(
    reservation_id: String,
    fp: &FootprintSnapshot,
    ttl: u64,
    expires_at: u64,
) -> DeclareResponse {
    DeclareResponse {
        reservation_id,
        footprint_ref: fp.footprint_ref.clone(),
        status: ReservationStatus::Declared,
        ttl_seconds: ttl,
        expires_at,
        evidence_refs: fp.evidence_refs.clone(),
    }
}

fn try_refresh_existing_declare(
    ledger: &mut ReservationLedger,
    rid: &str,
    now: u64,
    ttl: u64,
    fp: &FootprintSnapshot,
) -> Result<Option<DeclareResponse>, ReserveError> {
    let existing = ledger.records().iter().find(|r| r.reservation_id == rid);
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing.status != ReservationStatus::Active
        && existing.status != ReservationStatus::Declared
    {
        return Ok(None);
    }
    let mut refreshed = existing.clone();
    refreshed.expires_at = now + ttl;
    refreshed.ttl_seconds = ttl;
    ledger.append(refreshed).map_err(store_err)?;
    Ok(Some(declare_response(rid.to_string(), fp, ttl, now + ttl)))
}

fn new_declared_reservation(
    rid: String,
    repo: String,
    agent_id: String,
    intent_ops: Vec<IntentOperation>,
    fp: &FootprintSnapshot,
    now: u64,
    ttl: u64,
) -> IntentReservation {
    IntentReservation {
        schema_version: SCHEMA_VERSION,
        reservation_id: rid,
        repo_id: repo,
        agent_id,
        intent_ops,
        footprint_ref: fp.footprint_ref.clone(),
        evidence_refs: fp.evidence_refs.clone(),
        ttl_seconds: ttl,
        status: ReservationStatus::Declared,
        created_at: now,
        expires_at: now + ttl,
        contract_nodes: fp.contract_nodes.clone(),
    }
}

fn unknown_check_response(fp: &FootprintSnapshot) -> ReservationCheckResponse {
    let cert = serde_json::json!({
        "tier_a_percent": fp.tier_a_percent,
        "coverage": fp.tier_a_percent / 100.0,
    });
    ReservationCheckResponse {
        verdict: "unknown".into(),
        overlap_nodes: vec![],
        evidence_refs: fp.evidence_refs.clone(),
        conflict_edges: vec![],
        coverage: Some(fp.tier_a_percent / 100.0),
        certificate: Some(cert),
        blocking_reservation_ids: vec![],
    }
}

fn should_return_unknown(fp: &FootprintSnapshot) -> bool {
    fp.tier_a_percent < 100.0
}

fn evidence_ref_for_overlap(
    active: &IntentReservation,
    fp: &FootprintSnapshot,
    node: &str,
) -> String {
    active
        .evidence_refs
        .first()
        .cloned()
        .or_else(|| fp.evidence_refs.first().cloned())
        .unwrap_or_else(|| node.to_string())
}

fn conflict_overlap(
    ledger: &ReservationLedger,
    repo: &str,
    now: u64,
    agent_id: &str,
    fp: &FootprintSnapshot,
) -> (Vec<String>, Vec<ConflictGraphEdge>, Vec<String>) {
    let mut overlap = Vec::new();
    let mut edges = Vec::new();
    let mut blocking = Vec::new();
    let new_set: HashSet<_> = fp.contract_nodes.iter().cloned().collect();

    for active in ledger.blocking_for_conflict(repo, now) {
        if active.agent_id == agent_id {
            continue;
        }
        let active_set: HashSet<_> = active.contract_nodes.iter().cloned().collect();
        for node in new_set.intersection(&active_set) {
            overlap.push(node.clone());
            let evidence_ref = evidence_ref_for_overlap(&active, fp, node);
            edges.push(ConflictGraphEdge {
                from_reservation_id: active.reservation_id.clone(),
                to_agent_id: active.agent_id.clone(),
                node: node.clone(),
                evidence_ref: evidence_ref.clone(),
            });
            if !blocking.contains(&active.reservation_id) {
                blocking.push(active.reservation_id.clone());
            }
        }
    }
    (overlap, edges, blocking)
}

fn conflict_check_response(
    overlap: Vec<String>,
    edges: Vec<ConflictGraphEdge>,
    blocking: Vec<String>,
) -> ReservationCheckResponse {
    let evidence_refs: Vec<_> = edges.iter().map(|e| e.evidence_ref.clone()).collect();
    ReservationCheckResponse {
        verdict: "conflict".into(),
        overlap_nodes: overlap,
        evidence_refs,
        conflict_edges: edges,
        coverage: Some(1.0),
        certificate: None,
        blocking_reservation_ids: blocking,
    }
}

fn clear_check_response(fp: &FootprintSnapshot) -> ReservationCheckResponse {
    ReservationCheckResponse {
        verdict: "clear".into(),
        overlap_nodes: vec![],
        evidence_refs: fp.evidence_refs.clone(),
        conflict_edges: vec![],
        coverage: Some(1.0),
        certificate: None,
        blocking_reservation_ids: vec![],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclareRequest {
    pub agent_id: String,
    pub intent_ops: Vec<IntentOperation>,
    pub ttl_seconds: u64,
}

pub struct ReserveService {
    pub store_root: std::path::PathBuf,
    pub repo_root: std::path::PathBuf,
}

impl ReserveService {
    pub fn new(store_root: &Path, repo_root: &Path) -> Self {
        Self {
            store_root: store_root.to_path_buf(),
            repo_root: repo_root.to_path_buf(),
        }
    }

    fn current_snapshot(&self) -> Result<Snapshot, ReserveError> {
        Snapshot::open(&self.store_root, Some(&self.repo_root)).map_err(store_err)
    }

    fn load_footprint_for_ops(
        &self,
        intent_ops: &[IntentOperation],
    ) -> Result<FootprintSnapshot, ReserveError> {
        let snapshot = self.current_snapshot()?;
        footprint_from_intent_ops(&snapshot, intent_ops).map_err(store_err)
    }

    pub fn declare(&self, req: DeclareRequest) -> Result<DeclareResponse, ReserveError> {
        validate_declare_request(&req)?;
        let fp = self.load_footprint_for_ops(&req.intent_ops)?;
        let now = now_ts();
        let ttl = req.ttl_seconds.max(60);
        let digest = intent_digest(&req.agent_id, &req.intent_ops);
        let rid = reservation_id(&req.agent_id, &digest);
        let _thread_guard = lock_reservation_ledger()?;
        let _file_guard = lock_reservation_ledger_file(&self.store_root)?;
        let mut ledger = open_ledger_for_write(&self.store_root)?;
        let repo = repo_id(&self.repo_root).map_err(store_err)?;
        if let Some(resp) = try_refresh_existing_declare(&mut ledger, &rid, now, ttl, &fp)? {
            return Ok(resp);
        }
        let rec = new_declared_reservation(
            rid.clone(),
            repo,
            req.agent_id,
            req.intent_ops,
            &fp,
            now,
            ttl,
        );
        ledger.append(rec).map_err(store_err)?;
        Ok(declare_response(rid, &fp, ttl, now + ttl))
    }

    pub fn check(
        &self,
        agent_id: &str,
        intent_ops: &[IntentOperation],
        acquire: bool,
    ) -> Result<ReservationCheckResponse, ReserveError> {
        self.check_with_ttl(agent_id, intent_ops, acquire, None)
    }

    pub fn check_with_ttl(
        &self,
        agent_id: &str,
        intent_ops: &[IntentOperation],
        acquire: bool,
        ttl_seconds: Option<u64>,
    ) -> Result<ReservationCheckResponse, ReserveError> {
        let fp = self.load_footprint_for_ops(intent_ops)?;
        let now = now_ts();
        let repo = repo_id(&self.repo_root).map_err(store_err)?;
        let _thread_guard = lock_reservation_ledger()?;
        let _file_guard = lock_reservation_ledger_file(&self.store_root)?;
        let mut ledger = open_ledger_for_write(&self.store_root)?;
        let (overlap, edges, blocking) = conflict_overlap(&ledger, &repo, now, agent_id, &fp);
        if !overlap.is_empty() {
            notify_conflict_if_configured(agent_id, &overlap);
            return Ok(conflict_check_response(overlap, edges, blocking));
        }
        if should_return_unknown(&fp) {
            return Ok(unknown_check_response(&fp));
        }
        if acquire {
            self.acquire_internal_locked(
                &mut ledger,
                agent_id,
                intent_ops,
                &fp,
                &repo,
                now,
                ttl_seconds.unwrap_or(3600).max(60),
            )?;
        }
        Ok(clear_check_response(&fp))
    }

    fn acquire_internal_locked(
        &self,
        ledger: &mut ReservationLedger,
        agent_id: &str,
        intent_ops: &[IntentOperation],
        fp: &FootprintSnapshot,
        repo: &str,
        now: u64,
        ttl: u64,
    ) -> Result<(), ReserveError> {
        let (overlap, _, _) = conflict_overlap(ledger, repo, now, agent_id, fp);
        if !overlap.is_empty() {
            return Err(ReserveError::Validation(format!(
                "reservation conflict while acquiring: {}",
                overlap.join(",")
            )));
        }
        let digest = intent_digest(agent_id, intent_ops);
        let rid = reservation_id(agent_id, &digest);
        let rec = IntentReservation {
            schema_version: SCHEMA_VERSION,
            reservation_id: rid,
            repo_id: repo.to_string(),
            agent_id: agent_id.to_string(),
            intent_ops: intent_ops.to_vec(),
            footprint_ref: fp.footprint_ref.clone(),
            evidence_refs: fp.evidence_refs.clone(),
            ttl_seconds: ttl,
            status: ReservationStatus::Active,
            created_at: now,
            expires_at: now + ttl,
            contract_nodes: fp.contract_nodes.clone(),
        };
        ledger.append(rec).map_err(store_err)
    }

    pub fn release(&self, agent_id: &str, reservation_id: &str) -> Result<(), ReserveError> {
        let now = now_ts();
        let _thread_guard = lock_reservation_ledger()?;
        let _file_guard = lock_reservation_ledger_file(&self.store_root)?;
        let mut ledger = open_ledger_for_write(&self.store_root)?;
        let rec = ledger
            .materialized(now)
            .into_iter()
            .find(|r| r.reservation_id == reservation_id && r.agent_id == agent_id)
            .ok_or_else(|| ReserveError::NotFound(reservation_id.into()))?;
        let mut released = rec;
        released.status = ReservationStatus::Released;
        ledger.append(released).map_err(store_err)
    }

    pub fn query_active(&self) -> Result<ReservationQueryResponse, ReserveError> {
        let now = now_ts();
        let repo = repo_id(&self.repo_root).map_err(store_err)?;
        let ledger = open_ledger(&self.store_root)?;
        let active = ledger.active(&repo, now);
        Ok(ReservationQueryResponse {
            active_count: active.len(),
            reservations: active,
        })
    }
}

pub fn declare_reservation(
    store_root: &Path,
    repo_root: &Path,
    req: DeclareRequest,
) -> Result<DeclareResponse, ReserveError> {
    ReserveService::new(store_root, repo_root).declare(req)
}

pub fn check_reservation(
    store_root: &Path,
    repo_root: &Path,
    agent_id: &str,
    intent_ops: &[IntentOperation],
    acquire: bool,
) -> Result<ReservationCheckResponse, ReserveError> {
    ReserveService::new(store_root, repo_root).check(agent_id, intent_ops, acquire)
}

pub fn check_reservation_with_ttl(
    store_root: &Path,
    repo_root: &Path,
    agent_id: &str,
    intent_ops: &[IntentOperation],
    acquire: bool,
    ttl_seconds: Option<u64>,
) -> Result<ReservationCheckResponse, ReserveError> {
    ReserveService::new(store_root, repo_root).check_with_ttl(
        agent_id,
        intent_ops,
        acquire,
        ttl_seconds,
    )
}

pub fn acquire_reservation(
    store_root: &Path,
    repo_root: &Path,
    agent_id: &str,
    intent_ops: &[IntentOperation],
    ttl_seconds: Option<u64>,
) -> Result<ReservationCheckResponse, ReserveError> {
    ReserveService::new(store_root, repo_root).check_with_ttl(
        agent_id,
        intent_ops,
        true,
        ttl_seconds,
    )
}

pub fn release_reservation(
    store_root: &Path,
    repo_root: &Path,
    agent_id: &str,
    reservation_id: &str,
) -> Result<(), ReserveError> {
    ReserveService::new(store_root, repo_root).release(agent_id, reservation_id)
}

pub fn list_active_reservations(
    store_root: &Path,
    repo_root: &Path,
) -> Result<ReservationQueryResponse, ReserveError> {
    ReserveService::new(store_root, repo_root).query_active()
}

static NOTIFY_HOOK_FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn notify_conflict(agent_id: &str, overlap: &[String], configured: bool) {
    if configured && !overlap.is_empty() {
        NOTIFY_HOOK_FIRED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = (agent_id, overlap);
    }
}

pub fn notify_conflict_if_configured(agent_id: &str, overlap: &[String]) {
    notify_conflict(agent_id, overlap, std::env::var("GRAPHZERO_AGENT").is_ok());
}

#[doc(hidden)]
pub fn test_notify_conflict(agent_id: &str, overlap: &[String]) {
    notify_conflict(agent_id, overlap, true);
}

#[doc(hidden)]
pub fn test_notify_hook_count() -> usize {
    NOTIFY_HOOK_FIRED.load(std::sync::atomic::Ordering::SeqCst)
}

#[doc(hidden)]
pub fn test_reset_notify_hook() {
    NOTIFY_HOOK_FIRED.store(0, std::sync::atomic::Ordering::SeqCst);
}
