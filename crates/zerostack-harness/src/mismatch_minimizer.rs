//! Delta-debug a failing statement list. Schema statements are never removed.

use serde::{Deserialize, Serialize};

use crate::mismatch::MismatchClassification;
use crate::repo::sha256_hex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Subsystem {
    Parser,
    Selector,
    Schema,
    Storage,
    Journal,
    Cas,
    Abi,
    Spec,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadStmt {
    pub schema: bool,
    pub text: String,
}

impl WorkloadStmt {
    pub fn schema(text: impl Into<String>) -> Self {
        Self {
            schema: true,
            text: text.into(),
        }
    }

    pub fn work(text: impl Into<String>) -> Self {
        Self {
            schema: false,
            text: text.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MismatchSignature {
    /// First 16 hex chars of SHA-256 of the canonical minimal repro.
    pub hash: String,
    pub classification: MismatchClassification,
    pub subsystem: Subsystem,
    pub minimal_statement_count: usize,
    pub first_diverging_sql: String,
}

impl MismatchSignature {
    pub fn from_minimal(
        stmts: &[WorkloadStmt],
        classification: MismatchClassification,
        subsystem: Subsystem,
    ) -> Self {
        let canonical = canonical_repro(stmts);
        let digest = sha256_hex(canonical.as_bytes());
        let first = stmts
            .iter()
            .find(|stmt| !stmt.schema)
            .or_else(|| stmts.first())
            .map(|stmt| stmt.text.clone())
            .unwrap_or_default();
        Self {
            hash: digest.chars().take(16).collect(),
            classification,
            subsystem,
            minimal_statement_count: stmts.len(),
            first_diverging_sql: first,
        }
    }

    pub fn dedup_key(&self) -> String {
        format!(
            "{}-{:?}-{}",
            self.hash,
            self.subsystem,
            self.classification.discriminant_name()
        )
    }
}

pub fn canonical_repro(stmts: &[WorkloadStmt]) -> String {
    stmts
        .iter()
        .map(|stmt| {
            if stmt.schema {
                format!("SCHEMA\t{}", stmt.text.trim())
            } else {
                format!("WORK\t{}", stmt.text.trim())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Binary partition, then single-statement deletion. Schema rows stay.
pub fn minimize<F>(stmts: &[WorkloadStmt], mut reproduces: F) -> Vec<WorkloadStmt>
where
    F: FnMut(&[WorkloadStmt]) -> bool,
{
    let schema: Vec<WorkloadStmt> = stmts.iter().filter(|s| s.schema).cloned().collect();
    let mut workload: Vec<WorkloadStmt> = stmts.iter().filter(|s| !s.schema).cloned().collect();

    loop {
        let n = workload.len();
        if n <= 1 {
            break;
        }
        let mid = n / 2;
        let first = compose(&schema, &workload[..mid]);
        let second = compose(&schema, &workload[mid..]);
        if reproduces(&first) {
            workload = workload[..mid].to_vec();
            continue;
        }
        if reproduces(&second) {
            workload = workload[mid..].to_vec();
            continue;
        }
        let mut shrunk = false;
        for i in (0..workload.len()).rev() {
            let mut without = workload.clone();
            without.remove(i);
            let candidate = compose(&schema, &without);
            if reproduces(&candidate) {
                workload = without;
                shrunk = true;
                break;
            }
        }
        if !shrunk {
            break;
        }
    }
    compose(&schema, &workload)
}

fn compose(schema: &[WorkloadStmt], workload: &[WorkloadStmt]) -> Vec<WorkloadStmt> {
    let mut out = schema.to_vec();
    out.extend(workload.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<WorkloadStmt> {
        vec![
            WorkloadStmt::schema("CREATE TABLE t"),
            WorkloadStmt::schema("CREATE INDEX i"),
            WorkloadStmt::work("INSERT 1"),
            WorkloadStmt::work("INSERT 2"),
            WorkloadStmt::work("BUG"),
            WorkloadStmt::work("INSERT 3"),
            WorkloadStmt::work("INSERT 4"),
            WorkloadStmt::work("INSERT 5"),
            WorkloadStmt::work("INSERT 6"),
            WorkloadStmt::work("INSERT 7"),
        ]
    }

    fn has_bug(stmts: &[WorkloadStmt]) -> bool {
        stmts.iter().any(|s| s.text == "BUG")
    }

    #[test]
    fn reduces_to_schema_plus_one() {
        let minimal = minimize(&fixture(), has_bug);
        assert!(
            minimal
                .iter()
                .any(|s| s.schema && s.text == "CREATE TABLE t")
        );
        assert!(
            minimal
                .iter()
                .any(|s| s.schema && s.text == "CREATE INDEX i")
        );
        assert_eq!(minimal.iter().filter(|s| !s.schema).count(), 1);
        assert_eq!(minimal.iter().find(|s| !s.schema).unwrap().text, "BUG");
    }

    #[test]
    fn schema_is_never_removed() {
        let stmts = vec![
            WorkloadStmt::schema("CREATE TABLE t"),
            WorkloadStmt::work("ok"),
        ];
        let minimal = minimize(&stmts, |_| true);
        assert!(minimal.iter().any(|s| s.schema));
    }

    #[test]
    fn same_root_cause_shares_signature() {
        let class = MismatchClassification::TrueDivergence {
            description: "bug".into(),
        };
        let a = minimize(&fixture(), has_bug);
        let b = minimize(&fixture(), has_bug);
        let sa = MismatchSignature::from_minimal(&a, class.clone(), Subsystem::Parser);
        let sb = MismatchSignature::from_minimal(&b, class, Subsystem::Parser);
        assert_eq!(sa.hash, sb.hash);
        assert_eq!(sa.dedup_key(), sb.dedup_key());
        assert_eq!(sa.hash.len(), 16);
    }
}
