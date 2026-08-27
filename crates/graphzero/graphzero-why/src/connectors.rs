//! FR-010: honest connector state.

use serde::{Deserialize, Serialize};

use crate::schema::{ConnectorAvailability, ProvenanceSourceKind};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConnectorConfig {
    pub git_commit: bool,
    pub pr_thread: bool,
    pub issue: bool,
    pub agent_trace: bool,
}

impl ConnectorConfig {
    pub fn all_enabled() -> Self {
        Self {
            git_commit: true,
            pr_thread: true,
            issue: true,
            agent_trace: true,
        }
    }

    pub fn availability(&self, kind: ProvenanceSourceKind) -> ConnectorAvailability {
        let on = match kind {
            ProvenanceSourceKind::GitCommit => self.git_commit,
            ProvenanceSourceKind::PrThread => self.pr_thread,
            ProvenanceSourceKind::Issue => self.issue,
            ProvenanceSourceKind::AgentTrace => self.agent_trace,
        };
        if on {
            ConnectorAvailability::Available
        } else {
            ConnectorAvailability::Unknown
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectorStatus {
    pub git_commit: ConnectorAvailability,
    pub pr_thread: ConnectorAvailability,
    pub issue: ConnectorAvailability,
    pub agent_trace: ConnectorAvailability,
    pub absence_certificate: Option<String>,
}

impl ConnectorStatus {
    pub fn from_config(cfg: &ConnectorConfig) -> Self {
        Self {
            git_commit: cfg.availability(ProvenanceSourceKind::GitCommit),
            pr_thread: cfg.availability(ProvenanceSourceKind::PrThread),
            issue: cfg.availability(ProvenanceSourceKind::Issue),
            agent_trace: cfg.availability(ProvenanceSourceKind::AgentTrace),
            absence_certificate: None,
        }
    }
}
