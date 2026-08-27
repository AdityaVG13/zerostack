//! Verify claim command core.

use std::path::Path;

use anyhow::{Context, Result};
use graphzero_store::{ClaimKind, supported_claim_kinds_csv};
use serde::Serialize;
use serde_json::json;

use super::paths::{canonical_repo, store_root};

pub fn verify_json(repo: &Path, target: &str, claim: &str) -> Result<String> {
    let repo = canonical_repo(repo)?;
    let root = store_root(&repo);
    // Domain dispatcher owns verify + verification_graph (single execution).
    let _ = ClaimKind::parse_claim_kind(claim).with_context(|| {
        format!(
            "unknown claim {claim:?}; valid: {}",
            supported_claim_kinds_csv()
        )
    })?;
    let ctx =
        graphzero_engine::EngineContext::for_paths(repo, root, graphzero_engine::AdapterKind::Cli);
    let args = json!({ "target": target, "claim": claim });
    let domain = graphzero_engine::dispatch(&ctx, "verify", &args)
        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
    serde_json::to_string(&domain.value).context("serialize verify json")
}

#[derive(Debug, Clone, Serialize)]
pub struct ParsedPrClaim {
    pub target: String,
    pub claim: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrClaimResult {
    pub target: String,
    pub claim: String,
    pub verified: bool,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrClaimGateReport {
    pub schema: &'static str,
    pub verified: bool,
    pub claims: Vec<PrClaimResult>,
}

pub fn parse_pr_claims(body: &str) -> Vec<ParsedPrClaim> {
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed
                .strip_prefix("GraphZero-Claim:")
                .or_else(|| trimmed.strip_prefix("graphzero-claim:"))?;
            let mut parts = rest.split_whitespace();
            let claim = parts.next()?.to_string();
            let target = parts.next()?.to_string();
            Some(ParsedPrClaim { target, claim })
        })
        .collect()
}

pub fn verify_pr_claims_json(repo: &Path, claims_file: &Path) -> Result<String> {
    let body = std::fs::read_to_string(claims_file)
        .with_context(|| format!("read claims file {}", claims_file.display()))?;
    let claims = parse_pr_claims(&body);
    let mut results = Vec::with_capacity(claims.len());
    for parsed in claims {
        let raw = verify_json(repo, &parsed.target, &parsed.claim)?;
        let value: serde_json::Value = serde_json::from_str(&raw).context("verify json parse")?;
        let verified = value
            .get("verified")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        results.push(PrClaimResult {
            target: parsed.target,
            claim: parsed.claim,
            verified,
            result: value,
        });
    }
    let report = PrClaimGateReport {
        schema: "graphzero-pr-claim-gate/v1",
        verified: !results.is_empty() && results.iter().all(|r| r.verified),
        claims: results,
    };
    serde_json::to_string(&report).context("serialize claim gate report")
}
