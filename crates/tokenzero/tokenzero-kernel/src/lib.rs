#![forbid(unsafe_code)]

//! Typed TokenZero implementation consumed directly by ZeroKernel.
//!
//! This crate owns [`ZeroTokenEngine`] / `zero_abi::TokenEngine`. Engine and
//! CLI must depend inward; they must not path-include this source and
//! must not re-export these types as if they owned them.
//!
//! TokenZero owns measurement, projection, compression, and exact expansion.
//! It does not own shell process creation or expose model-facing commands.

/// Crate that owns [`ZeroTokenEngine`]. Product code that needs TokenEngine
/// imports `tokenzero_kernel`, not `tokenzero_engine`.
pub const TOKEN_ENGINE_OWNER_CRATE: &str = env!("CARGO_PKG_NAME");

pub use tokenzero_core::{
    EvidenceFreshness, LiveCandidate, LiveEntry, LiveParetoDecision, MetricOrder, ProtectedOutcome,
    VerifierIdentity, decide_live_pareto,
};

use std::path::PathBuf;
use std::str::FromStr;

use tiktoken_rs::tokenizer::{Tokenizer, get_tokenizer};
use tokenzero_core::{BYTES_ESTIMATOR_ID, LEXICAL_ESTIMATOR_ID};
use zero_abi::{
    CompressionRequest, CompressionResult, EngineError, EngineErrorKind, EngineInvocation,
    ExpandOptions, ProjectionRequest, ProjectionResult, TokenAccounting, TokenEngine, ZeroHandle,
};
use zero_store::{SelectionIndex, ZeroCas, ZeroObjectMetadata};

/// Output-contract 4-tuple for [`TokenAccounting`].
///
/// `raw` is billed source mass, `visible`/`spent` are presented mass,
/// `recovered` is cache/recovery mass (`cached`, 0 until a cache hit is
/// charged). This mapping is the account golden, not a second counter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountMass {
    pub raw: u64,
    pub visible: u64,
    pub recovered: u64,
    pub spent: u64,
}

/// Map ZeroKernel accounting onto Pulse/ledger raw/visible/recovered/spent.
///
/// `spent` is net presented mass: visible plus recovered/cached. Mapping
/// spent=visible alone hid expand charges and made Pulse look cheaper than
/// the task actually spent.
pub fn account_mass(accounting: &TokenAccounting) -> AccountMass {
    AccountMass {
        raw: accounting.billed,
        visible: accounting.visible,
        recovered: accounting.cached,
        spent: accounting.visible.saturating_add(accounting.cached),
    }
}

/// Pulse `estimator:<slug>` id for an approximate family, or the lexical gauge.
pub fn estimator_tokenizer_id(model_id: Option<&str>) -> String {
    let id = match model_id.and_then(tokenzero_core::tokenizer_metadata) {
        Some(meta) => format!("estimator:tokenzero-{}", meta.family.name()),
        None => LEXICAL_ESTIMATOR_ID.to_string(),
    };
    seal_tokenizer_id(id)
}

fn seal_tokenizer_id(id: String) -> String {
    tokenzero_core::preflight_tokenizer_id(&id).unwrap_or_else(|err| {
        panic!("kernel tokenizer id failed honesty preflight ({err}): {id}");
    });
    id
}

fn sealed_accounting(tokenizer: String, count: u64, certified: bool) -> TokenAccounting {
    TokenAccounting {
        tokenizer: seal_tokenizer_id(tokenizer),
        billed: count,
        visible: count,
        cached: 0,
        certified,
    }
}

/// Bundled tiktoken encoding name. Certified BPE, not Pulse `provider/model@hex`
/// (no revision digest is bound to the crate vocab).
pub fn tiktoken_tokenizer_id(model: &str) -> String {
    let encoding = match get_tokenizer(model) {
        Some(Tokenizer::Cl100kBase) => "cl100k_base",
        Some(Tokenizer::O200kBase) => "o200k_base",
        Some(Tokenizer::O200kHarmony) => "o200k_harmony",
        Some(Tokenizer::P50kBase) => "p50k_base",
        Some(Tokenizer::R50kBase) => "r50k_base",
        Some(Tokenizer::P50kEdit) => "p50k_edit",
        Some(Tokenizer::Gpt2) => "gpt2",
        None => "unknown",
    };
    seal_tokenizer_id(format!("tiktoken:{encoding}"))
}

#[derive(Clone, Debug)]
pub struct ZeroTokenEngine {
    cas: ZeroCas,
    model_id: Option<String>,
}

impl ZeroTokenEngine {
    pub fn open(store_root: impl Into<PathBuf>, model_id: Option<String>) -> Self {
        Self {
            cas: ZeroCas::open(store_root),
            model_id: model_id.or_else(active_model),
        }
    }

    /// Open without reading `TOKENZERO_MODEL` / `OMP_MODEL` / `OPENAI_MODEL`.
    /// `None` is the lexical estimator; goldens and hermetic tests use this.
    pub fn open_unbound(store_root: impl Into<PathBuf>, model_id: Option<String>) -> Self {
        Self {
            cas: ZeroCas::open(store_root),
            model_id,
        }
    }

    fn cancelled(invocation: &EngineInvocation) -> Result<(), EngineError> {
        if invocation.cancellation.is_cancelled() {
            return Err(EngineError::new(
                EngineErrorKind::Cancelled,
                "TokenZero operation cancelled",
                false,
            ));
        }
        Ok(())
    }

    fn count(&self, bytes: &[u8]) -> TokenAccounting {
        let text = std::str::from_utf8(bytes).ok();
        if let (Some(model), Some(text)) = (self.model_id.as_deref(), text)
            && let Ok(bpe) = tiktoken_rs::bpe_for_model(model)
        {
            let count = bpe.encode_with_special_tokens(text).len() as u64;
            return sealed_accounting(tiktoken_tokenizer_id(model), count, true);
        }
        let (count, tokenizer) = match text {
            Some(text) => (
                tokenzero_core::count_tokens_for_model(text, self.model_id.as_deref()) as u64,
                estimator_tokenizer_id(self.model_id.as_deref()),
            ),
            None => (bytes.len() as u64, BYTES_ESTIMATOR_ID.to_string()),
        };
        sealed_accounting(tokenizer, count, false)
    }

    fn clamp_visible_to_budget<'a>(
        &self,
        text: &'a str,
        max_tokens: u64,
    ) -> (&'a str, TokenAccounting, bool) {
        let accounting = self.count(text.as_bytes());
        if accounting.billed <= max_tokens {
            return (text, accounting, false);
        }

        // BPE token counts are not monotonic in prefix length, so binary
        // search can reject a long prefix that fits because a shorter
        // mid-token prefix counted higher. Longest-to-shortest char prefix
        // is the exact "largest prefix with billed <= max_tokens" scan.
        let mut ends: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
        ends.push(text.len());
        for &end in ends.iter().rev() {
            let prefix = &text[..end];
            let accounting = self.count(prefix.as_bytes());
            if accounting.billed <= max_tokens {
                return (prefix, accounting, end != text.len());
            }
        }
        (text.get(..0).unwrap_or(""), self.count(b""), true)
    }

    fn passthrough_compression(
        text: &str,
        handle: ZeroHandle,
        raw_accounting: &TokenAccounting,
    ) -> CompressionResult {
        CompressionResult {
            visible: text.to_owned(),
            exact: handle,
            truncated: false,
            omitted_tokens: 0,
            accounting: TokenAccounting {
                tokenizer: raw_accounting.tokenizer.clone(),
                billed: raw_accounting.billed,
                visible: raw_accounting.billed,
                cached: raw_accounting.cached,
                certified: raw_accounting.certified,
            },
        }
    }

    fn store_exact(&self, bytes: &[u8], media_type: &str) -> Result<ZeroHandle, EngineError> {
        let handle = self.cas.put(bytes).map_err(cas_error)?;
        let selection = std::str::from_utf8(bytes)
            .ok()
            .map(SelectionIndex::from_utf8);
        self.cas
            .publish_metadata(&ZeroObjectMetadata {
                handle: handle.clone(),
                byte_len: bytes.len() as u64,
                media_type: media_type.to_owned(),
                producer: "TokenZero".into(),
                contract_digest: "ZeroKernel.TokenEngine".into(),
                selection,
            })
            .map_err(cas_error)?;
        Ok(handle)
    }
}

impl TokenEngine for ZeroTokenEngine {
    fn measure(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
    ) -> Result<TokenAccounting, EngineError> {
        Self::cancelled(invocation)?;
        Ok(self.count(bytes))
    }

    fn certify(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
        claimed: &TokenAccounting,
    ) -> Result<zero_abi::CertifyResult, EngineError> {
        Self::cancelled(invocation)?;
        let recomputed = self.count(bytes);
        Ok(zero_abi::CertifyResult {
            matches: &recomputed == claimed,
            recomputed,
        })
    }

    fn project(
        &self,
        invocation: &EngineInvocation,
        request: ProjectionRequest,
    ) -> Result<ProjectionResult, EngineError> {
        Self::cancelled(invocation)?;
        let limit = request.visible_byte_limit as usize;
        let raw_accounting = self.count(&request.bytes);
        if request.bytes.len() <= limit
            && let Ok(text) = std::str::from_utf8(&request.bytes)
        {
            return Ok(ProjectionResult {
                visible: text.to_owned(),
                visible_source_bytes: request.bytes.len() as u64,
                exact: None,
                accounting: raw_accounting,
            });
        }
        if limit < 80 {
            return Err(EngineError::new(
                EngineErrorKind::Budget,
                "visible output budget is too small for an exact ZeroHandle",
                false,
            ));
        }
        let handle = self.store_exact(&request.bytes, &request.media_type)?;
        let source = String::from_utf8_lossy(&request.bytes);
        let marker = format!("\nexact: {handle}");
        let visible = bounded_utf8(&source, &marker, limit);
        let visible_source_bytes = visible.strip_suffix(&marker).map_or(0, str::len) as u64;
        let visible_count = self.count(visible.as_bytes());
        // Never-worse vs the raw payload: a handle capsule that costs more
        // tokens than sending the source is not a projection. Pass through
        // even when the byte budget is tight — TokenZero's authority is
        // tokens, not framing bytes.
        if visible_count.visible > raw_accounting.billed
            && let Ok(text) = std::str::from_utf8(&request.bytes)
        {
            return Ok(ProjectionResult {
                visible: text.to_owned(),
                visible_source_bytes: request.bytes.len() as u64,
                exact: None,
                accounting: raw_accounting,
            });
        }
        Ok(ProjectionResult {
            visible,
            visible_source_bytes,
            exact: Some(handle),
            accounting: TokenAccounting {
                tokenizer: raw_accounting.tokenizer,
                billed: raw_accounting.billed,
                visible: visible_count.visible,
                cached: raw_accounting.cached,
                certified: raw_accounting.certified && visible_count.certified,
            },
        })
    }

    fn compress(
        &self,
        invocation: &EngineInvocation,
        request: CompressionRequest,
    ) -> Result<CompressionResult, EngineError> {
        Self::cancelled(invocation)?;
        if request.max_tokens == 0 {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "compression max_tokens must be positive",
                false,
            ));
        }
        let text = std::str::from_utf8(&request.bytes).map_err(|_| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                "compression input must be UTF-8",
                false,
            )
        })?;
        let mode = if request.mode.is_empty() {
            tokenzero_core::Mode::Auto
        } else {
            tokenzero_core::Mode::from_str(&request.mode)
                .map_err(|error| EngineError::new(EngineErrorKind::InvalidInput, error, false))?
        };
        let raw_accounting = self.count(&request.bytes);
        let handle = self.store_exact(&request.bytes, &request.media_type)?;
        let mut capsule = tokenzero_core::make_capsule_with_recovery_ref(
            text,
            raw_accounting.billed as usize,
            mode,
            request.max_tokens as usize,
            request.label.as_deref(),
            Some(&handle.to_string()),
        )
        .map_err(|error| EngineError::new(EngineErrorKind::Budget, error, false))?;
        // Auto+oversized must pick the same honest digest family at every
        // budget, then clamp. Restricting Dedupe/Structured to the
        // passthrough case (max_tokens >= billed) made a looser budget
        // collapse a repetitive payload while a tighter budget kept a
        // longer summarize_lines view — visible(tight) > visible(loose).
        let oversized = request.bytes.len() > 512 || text.lines().count() > 20;
        if oversized && mode == tokenzero_core::Mode::Auto {
            let mut best = capsule.clone();
            let mut best_cost = self.count(best.text.as_bytes()).billed;
            for alt_mode in [
                tokenzero_core::Mode::Dedupe,
                tokenzero_core::Mode::Structured,
            ] {
                if let Ok(alt) = tokenzero_core::make_capsule_with_recovery_ref(
                    text,
                    raw_accounting.billed as usize,
                    alt_mode,
                    request.max_tokens as usize,
                    request.label.as_deref(),
                    Some(&handle.to_string()),
                ) {
                    // Honesty is the kernel tokenizer, not core's lexical
                    // `visible_tokens` mixed with tiktoken `billed`.
                    let alt_cost = self.count(alt.text.as_bytes()).billed;
                    if alt_cost < raw_accounting.billed && alt_cost < best_cost {
                        best_cost = alt_cost;
                        best = alt;
                    }
                }
            }
            capsule = best;
        }
        let capsule_accounting = self.count(capsule.text.as_bytes());
        // Never-worse vs raw, before clamp. Clamping a worse wrapper to
        // max_tokens used to report omitted_tokens as a save.
        if capsule_accounting.billed > raw_accounting.billed {
            return Ok(Self::passthrough_compression(text, handle, &raw_accounting));
        }
        let (visible, visible_accounting, truncated) =
            self.clamp_visible_to_budget(&capsule.text, u64::from(request.max_tokens));
        let visible_tokens = visible_accounting.billed.min(u64::from(request.max_tokens));
        if visible_tokens > raw_accounting.billed {
            return Ok(Self::passthrough_compression(text, handle, &raw_accounting));
        }
        Ok(CompressionResult {
            visible: visible.to_owned(),
            exact: handle,
            truncated,
            omitted_tokens: raw_accounting.billed.saturating_sub(visible_tokens),
            accounting: TokenAccounting {
                tokenizer: raw_accounting.tokenizer,
                billed: raw_accounting.billed,
                visible: visible_tokens,
                cached: raw_accounting.cached,
                certified: raw_accounting.certified && visible_accounting.certified,
            },
        })
    }

    fn expand(
        &self,
        invocation: &EngineInvocation,
        handle: &ZeroHandle,
        options: ExpandOptions,
    ) -> Result<Vec<u8>, EngineError> {
        Self::cancelled(invocation)?;
        self.cas.expand(handle, &options).map_err(cas_error)
    }
}

fn active_model() -> Option<String> {
    ["TOKENZERO_MODEL", "OMP_MODEL", "OPENAI_MODEL"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn bounded_utf8(source: &str, marker: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    if marker.len() >= limit {
        let mut end = limit;
        while end > 0 && !marker.is_char_boundary(end) {
            end -= 1;
        }
        return marker[..end].to_owned();
    }
    let head_limit = limit - marker.len();
    let mut end = source.len().min(head_limit);
    while end > 0 && !source.is_char_boundary(end) {
        end -= 1;
    }
    let mut visible = String::with_capacity(end + marker.len());
    visible.push_str(&source[..end]);
    visible.push_str(marker);
    visible
}

fn cas_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        EngineErrorKind::Corrupt,
        format!("ZeroHandle CAS: {error}"),
        false,
    )
}

#[cfg(test)]
mod crate_boundary {
    #[test]
    fn kernel_owns_token_engine() {
        assert_eq!(super::TOKEN_ENGINE_OWNER_CRATE, "tokenzero-kernel");
    }
}
