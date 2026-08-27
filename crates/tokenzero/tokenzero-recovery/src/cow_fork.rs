//! Copy-on-write session forks over breakpoint-aligned cached prefixes.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokenzero_core::{ContentType, sha256_hex};

use crate::context_view::ContextProjection;
use crate::{RecoveryError, RecoveryStore};

#[derive(Debug, Error)]
pub enum CowForkError {
    #[error("session forks require a cache-breakpoint projection")]
    NotAtCacheBreakpoint,
    #[error("restore point belongs to branch {expected}, not {actual}")]
    WrongBranch { expected: String, actual: String },
    #[error("recovery ref {0} is unavailable")]
    MissingRecovery(String),
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchRecord {
    pub recovery_ref: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BranchLedgerAction {
    Fork,
    Append,
    Checkpoint,
    Discard,
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchLedgerEntry {
    pub branch_id: String,
    pub parent_branch_id: Option<String>,
    pub action: BranchLedgerAction,
    pub breakpoint_sha256: String,
    pub record_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRestorePoint {
    branch_id: String,
    records: Vec<BranchRecord>,
    pub discard_ledger_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForkCost {
    pub novelty_bytes: usize,
    pub shared_prefix_bytes: usize,
    pub full_replay_bytes: usize,
}

/// A session branch shares its sealed prefix by Arc and owns only novelty.
#[derive(Debug, Clone)]
pub struct CowSession {
    branch_id: String,
    parent_branch_id: Option<String>,
    shared_prefix: Arc<str>,
    breakpoint_sha256: String,
    novelty: Vec<(BranchRecord, String)>,
    ledger_refs: Vec<String>,
    at_breakpoint: bool,
}

impl CowSession {
    pub fn from_breakpoint(
        branch_id: impl Into<String>,
        projection: &ContextProjection,
    ) -> Result<Self, CowForkError> {
        if !projection.cache_breakpoint {
            return Err(CowForkError::NotAtCacheBreakpoint);
        }
        Ok(Self {
            branch_id: branch_id.into(),
            parent_branch_id: None,
            shared_prefix: Arc::from(projection.rendered.as_str()),
            breakpoint_sha256: sha256_hex(&projection.rendered),
            novelty: Vec::new(),
            ledger_refs: Vec::new(),
            at_breakpoint: true,
        })
    }

    pub fn fork(
        &self,
        store: &mut RecoveryStore,
        child_branch_id: impl Into<String>,
    ) -> Result<Self, CowForkError> {
        if !self.at_breakpoint {
            return Err(CowForkError::NotAtCacheBreakpoint);
        }
        let child_branch_id = child_branch_id.into();
        let mut child = Self {
            branch_id: child_branch_id,
            parent_branch_id: Some(self.branch_id.clone()),
            shared_prefix: Arc::clone(&self.shared_prefix),
            breakpoint_sha256: self.breakpoint_sha256.clone(),
            novelty: Vec::new(),
            ledger_refs: Vec::new(),
            at_breakpoint: true,
        };
        child.persist_ledger(store, BranchLedgerAction::Fork, Vec::new())?;
        Ok(child)
    }

    pub fn append(
        &mut self,
        store: &mut RecoveryStore,
        text: impl Into<String>,
    ) -> Result<String, CowForkError> {
        let text = text.into();
        let recovery_ref = store.store_blob_deferred(&text, ContentType::Unknown);
        let record = BranchRecord {
            recovery_ref: recovery_ref.clone(),
            bytes: text.len(),
        };
        self.novelty.push((record, text));
        self.at_breakpoint = false;
        self.persist_ledger(
            store,
            BranchLedgerAction::Append,
            vec![recovery_ref.clone()],
        )?;
        // Persist the novelty blob and its ledger entry before returning the
        // ref. Checkpoint/discard still seal the branch; append must not
        // advertise a handle that a later process cannot expand.
        store.persist_pending()?;
        Ok(recovery_ref)
    }

    /// Seal current novelty into a new shared cache breakpoint.
    pub fn checkpoint(&mut self, store: &mut RecoveryStore) -> Result<(), CowForkError> {
        let record_refs = self
            .novelty
            .iter()
            .map(|(record, _)| record.recovery_ref.clone())
            .collect();
        let rendered = self.rendered();
        self.shared_prefix = Arc::from(rendered);
        self.breakpoint_sha256 = sha256_hex(self.shared_prefix.as_ref());
        self.novelty.clear();
        self.at_breakpoint = true;
        self.persist_ledger(store, BranchLedgerAction::Checkpoint, record_refs)?;
        store.persist_pending()?;
        Ok(())
    }

    /// Discard branch novelty while retaining exact recovery refs for restore.
    pub fn discard(
        &mut self,
        store: &mut RecoveryStore,
    ) -> Result<BranchRestorePoint, CowForkError> {
        let records = self
            .novelty
            .iter()
            .map(|(record, _)| record.clone())
            .collect::<Vec<_>>();
        let refs = records
            .iter()
            .map(|record| record.recovery_ref.clone())
            .collect();
        let discard_ledger_ref = self.persist_ledger(store, BranchLedgerAction::Discard, refs)?;
        store.persist_pending()?;
        self.novelty.clear();
        self.at_breakpoint = true;
        Ok(BranchRestorePoint {
            branch_id: self.branch_id.clone(),
            records,
            discard_ledger_ref,
        })
    }

    /// Restore discarded novelty from the recovery store, never from caller bytes.
    pub fn restore(
        &mut self,
        store: &mut RecoveryStore,
        restore: &BranchRestorePoint,
    ) -> Result<(), CowForkError> {
        if restore.branch_id != self.branch_id {
            return Err(CowForkError::WrongBranch {
                expected: self.branch_id.clone(),
                actual: restore.branch_id.clone(),
            });
        }
        let mut recovered = Vec::with_capacity(restore.records.len());
        for record in &restore.records {
            let expanded = store.expand(&record.recovery_ref, Some("raw"), None, None, None, None);
            if !expanded.found {
                return Err(CowForkError::MissingRecovery(record.recovery_ref.clone()));
            }
            recovered.push((record.clone(), expanded.content));
        }
        let refs = recovered
            .iter()
            .map(|(record, _)| record.recovery_ref.clone())
            .collect();
        self.novelty = recovered;
        self.at_breakpoint = self.novelty.is_empty();
        self.persist_ledger(store, BranchLedgerAction::Restore, refs)?;
        store.persist_pending()?;
        Ok(())
    }

    pub fn rendered(&self) -> String {
        let novelty_bytes: usize = self.novelty.iter().map(|(_, text)| text.len()).sum();
        let mut rendered = String::with_capacity(self.shared_prefix.len() + novelty_bytes);
        rendered.push_str(&self.shared_prefix);
        for (_, text) in &self.novelty {
            rendered.push_str(text);
        }
        rendered
    }

    pub fn cost(&self) -> ForkCost {
        let novelty_bytes: usize = self.novelty.iter().map(|(_, text)| text.len()).sum();
        ForkCost {
            novelty_bytes,
            shared_prefix_bytes: self.shared_prefix.len(),
            full_replay_bytes: self.shared_prefix.len().saturating_add(novelty_bytes),
        }
    }

    pub fn breakpoint_sha256(&self) -> &str {
        &self.breakpoint_sha256
    }
    pub fn ledger_refs(&self) -> &[String] {
        &self.ledger_refs
    }
    pub fn shares_prefix_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared_prefix, &other.shared_prefix)
    }

    fn persist_ledger(
        &mut self,
        store: &mut RecoveryStore,
        action: BranchLedgerAction,
        record_refs: Vec<String>,
    ) -> Result<String, CowForkError> {
        let entry = BranchLedgerEntry {
            branch_id: self.branch_id.clone(),
            parent_branch_id: self.parent_branch_id.clone(),
            action,
            breakpoint_sha256: self.breakpoint_sha256.clone(),
            record_refs,
        };
        let json = serde_json::to_string(&entry)?;
        let ledger_ref = store.store_blob_deferred(&json, ContentType::JsonConfig);
        self.ledger_refs.push(ledger_ref.clone());
        Ok(ledger_ref)
    }
}
