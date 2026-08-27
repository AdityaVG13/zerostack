//! ZeroRef v1 capability negotiation (fszero-c6q.5; annex §8, canonical ADR
//! §10 `zeroref-capability/v1`).
//!
//! ONE machine-readable descriptor tells the hub/peers what FSZero can emit
//! and read BEFORE any ref is routed. Every value is derived from the same
//! constants the parser (`core/zeroref.rs`), the store (`core/recovery.rs`),
//! and the CAS (`core/cas.rs`) use, so advertisement and behavior cannot
//! drift — pinned by `tests/capability.rs`.
//!
//! The descriptor carries NO secrets and NO absolute private paths: shared
//! roots and store health are reported as states and booleans only.

use serde_json::{Value, json};

use super::cas::{CAS_LAYOUT_VERSION, cas_layout};
use super::recovery::RecoveryStore;
use super::recovery::{ENGINE_OWNED_KINDS, NAMED_PAYLOAD_KEYS};
use super::session::FSZeroSession;
use super::zeroref::{
    BYTE_FRAGMENT_SEMANTICS, EMITTED_SCHEME, HASH_ALGORITHM, HASH_CASE, HASH_HEX_LEN,
    LEGACY_BYTE_FRAGMENT_ALIAS, LINE_FRAGMENT_SEMANTICS, PORTABLE_KINDS, ZEROREF_MAJOR,
    ZEROREF_MINOR, ZEROREF_VERSION, ZeroRefErrorClass, ZeroScheme,
};

/// Contract name peers negotiate on. A different name is a different
/// protocol, not a different version.
pub const CAPABILITY_CONTRACT_NAME: &str = "zeroref";

/// Recovery-store key the descriptor is published under at session init, so
/// peers can `expand("capabilities")` through any expansion surface
/// (CodeMode `fs.expand`, the `X` op, MCP `fszero.expand`).
pub const CAPABILITY_STORE_KEY: &str = "capabilities";

/// Annex "Same-store limitation" language: scheme SYNTAX acceptance is not
/// foreign-read capability.
const INTEROP_NOTE: &str = "gz://blob and tz://blob reads are same-store retag lookups of the local \
     fz://blob/<hash> key: the scheme is an identity claim, not shared storage, \
     and this store can only serve content it already holds. schemes.reads is \
     SYNTAX support; real foreign-read capability additionally requires \
     shared_cas.attached (and writable, for dual-write) on BOTH peers over the \
     same canonical CAS.";

impl FSZeroSession {
    /// The ZeroRef v1 capability descriptor for THIS session's effective
    /// configuration. Static fields come from parser/store constants; the
    /// `shared_cas` section is probed live (attachment, writability, store
    /// health) so a caller can distinguish local-only, shared, read-only,
    /// and degraded states before routing any payload.
    pub fn capability_descriptor(&self) -> Value {
        capability_descriptor_from_recovery(&self.recovery, self.durable_degraded)
    }

    /// Publish the descriptor under [`CAPABILITY_STORE_KEY`] so peers can
    /// expand it (codemode-reachable). Called at session init; best-effort —
    /// a store write failure never blocks the session, and the descriptor
    /// stays available via [`FSZeroSession::capability_descriptor`].
    pub fn publish_capabilities(&mut self) {
        publish_capability_store_keys(&mut self.recovery, self.durable_degraded);
        let operation_abi = super::operation_abi::operation_abi_descriptor().to_string();
        let _ = self.recovery.try_put_key(
            super::operation_abi::OPERATION_ABI_STORE_KEY,
            operation_abi.as_bytes(),
        );
    }
}

/// Publish capability + filesystem-contract descriptors into a recovery store
/// (session and embedded paths share this body).
pub fn publish_capability_store_keys(recovery: &mut RecoveryStore, durable_degraded: bool) {
    let descriptor = capability_descriptor_from_recovery(recovery, durable_degraded).to_string();
    let _ = recovery.try_put_key(CAPABILITY_STORE_KEY, descriptor.as_bytes());
    let filesystem_contract =
        super::filesystem_contract::filesystem_contract_descriptor().to_string();
    let _ = recovery.try_put_key(
        super::filesystem_contract::FILESYSTEM_CONTRACT_STORE_KEY,
        filesystem_contract.as_bytes(),
    );
}

/// Build the ZeroRef v1 capability descriptor from the durable store and
/// degraded flag. Used by both `FSZeroSession` and the embeddable `FsZeroStore`
/// so the same recovery state produces the same advertisement.
pub fn capability_descriptor_from_recovery(
    recovery: &RecoveryStore,
    durable_degraded: bool,
) -> Value {
    let attached = recovery.cas_attached();
    let writable = recovery.cas_writable().unwrap_or(false);
    let durable = recovery.store_db_path().is_some() && !durable_degraded;
    let store_state = if durable { "durable" } else { "degraded" };
    let shared_interop = if attached && writable {
        "enabled"
    } else if attached {
        "read_only"
    } else {
        "disabled"
    };
    let mut remediation: Vec<String> = Vec::new();
    if !durable {
        remediation.push( "durable store unavailable: refs are session-scoped and will not survive this              process; check permissions on the project store directory (.fszero or              .zerostack/fszero) and reopen".to_string(), );
    }
    if attached && !writable {
        remediation.push( "shared CAS attached but not writable: mints will not dual-write into the canonical store; fix permissions on the blobs/ directory under the store root".to_string(),);
    }
    json!({
        "contract": { "name": CAPABILITY_CONTRACT_NAME, "major": ZEROREF_MAJOR, "minor": ZEROREF_MINOR, "version": ZEROREF_VERSION, },
        "hash": { "algo": HASH_ALGORITHM, "hex_len": HASH_HEX_LEN as u64, "case": HASH_CASE },
        "schemes": { "emits": [EMITTED_SCHEME.as_str()], "reads": ZeroScheme::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(), },
        "ref_kinds": { "portable": PORTABLE_KINDS, "engine_owned": ENGINE_OWNED_KINDS },
        "fragments": {
            "byte": BYTE_FRAGMENT_SEMANTICS, "line": LINE_FRAGMENT_SEMANTICS,
                // fszero-00vq: engine expand surfaces clamp line-span ENDS past
                // EOF; byte spans and line starts stay strict. The portable
                // ZeroRef::select contract path never clamps.
                "clamps": { "byte": false, "line_start": false, "line_end": true },
            "legacy_input_aliases": [LEGACY_BYTE_FRAGMENT_ALIAS], },
        "legacy": {
            "mode": "migration_window", "named_keys": NAMED_PAYLOAD_KEYS,
            "view_aliases": ["view_<id>/{path,ref,bytes}", "r<id>/{path,ref,bytes}"],
            "seq_refs": "fz://seq/* are execution-scoped: expansion returns corrective \
                 guidance naming the durable fz://blob ref to expand instead, never bytes",
        },
        "shared_cas": { "layout": cas_layout(), "version": CAS_LAYOUT_VERSION, "attached": attached, "writable": attached && writable, "store_state": store_state, },
        "interop": { "shared_interop": shared_interop, "foreign_blob_reads": "same_store_retag", "note": INTEROP_NOTE, },
        // No size limits currently — emitted as explicit null (never
        // omitted) so peers can distinguish "unlimited" from "unknown".
        "limits": { "max_object_bytes": Value::Null },
        "error_classes": ZeroRefErrorClass::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(), "remediation": remediation,
    })
}

/// Fields every zeroref capability descriptor must carry, extracted with
/// tolerance for sibling-engine key aliases (GraphZero publishes
/// `hash.algorithm`/`hash.hex_length`/`shared_cas.layout_version` and names
/// the contract via `schema: "zeroref-capability/v1"`). Unknown ADDITIVE
/// fields are ignored (forward-compatible); missing REQUIRED fields are not.
struct DescriptorFields {
    name: String,
    major: u64,
    algo: String,
    hex_len: u64,
    layout: Option<String>,
    layout_version: u64,
}

#[inline]
fn malformed_cap(side: &str, detail: impl std::fmt::Display) -> String {
    format!("malformed_capability: {side} {detail}")
}
#[inline]
fn incompatible_cap(detail: impl std::fmt::Display) -> String {
    format!("incompatible_capability: {detail}")
}

fn str_at(v: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|p| v.pointer(p).and_then(|f| f.as_str()))
        .map(str::to_string)
}

fn u64_at(v: &Value, pointers: &[&str]) -> Option<u64> {
    pointers
        .iter()
        .find_map(|p| v.pointer(p).and_then(|f| f.as_u64()))
}

fn extract_fields(side: &str, v: &Value) -> Result<DescriptorFields, String> {
    if v.is_null() {
        return Err(format!(
            "missing_capability: {side} sent no ZeroRef capability descriptor; treat it as legacy local-only and do not route refs to it"
        ));
    }

    if !v.is_object() {
        return Err(malformed_cap(side, "descriptor must be a JSON object"));
    }
    let name = str_at(v, &["/contract/name"])
        .or_else(|| {
            // GraphZero shape: schema "zeroref-capability/v1" names the contract.
            str_at(v, &["/schema"])
                .filter(|s| s.starts_with("zeroref-capability/"))
                .map(|_| CAPABILITY_CONTRACT_NAME.to_string())
        })
        .ok_or_else(|| {
            malformed_cap(
                side,
                "descriptor missing contract name (contract.name or schema)",
            )
        })?;
    let major = u64_at(v, &["/contract/major"])
        .ok_or_else(|| malformed_cap(side, "descriptor missing numeric contract.major"))?;
    let algo = str_at(v, &["/hash/algo", "/hash/algorithm"])
        .ok_or_else(|| malformed_cap(side, "descriptor missing hash.algo"))?;
    let hex_len = u64_at(v, &["/hash/hex_len", "/hash/hex_length"])
        .ok_or_else(|| malformed_cap(side, "descriptor missing hash.hex_len"))?;
    let layout_version = u64_at(v, &["/shared_cas/version", "/shared_cas/layout_version"])
        .ok_or_else(|| malformed_cap(side, "descriptor missing shared_cas layout version"))?;
    let layout = str_at(v, &["/shared_cas/layout"]);
    Ok(DescriptorFields {
        name,
        major,
        algo,
        hex_len,
        layout,
        layout_version,
    })
}

/// Sibling engines spell the layout placeholders differently
/// (`<hh>`/`<first-two-hex>` vs `<xx>`, `<64-hex>` vs `<hash>`); the paths
/// they produce are identical. Normalize before comparing so only REAL
/// layout differences (different directories/sharding) refuse interop.
fn normalize_layout(layout: &str) -> String {
    layout
        .replace("<hh>", "<xx>")
        .replace("<first-two-hex>", "<xx>")
        .replace("<64-hex>", "<hash>")
        .replace("<sha256>", "<hash>")
}

/// Strictly validate a peer's capability descriptor against ours BEFORE any
/// payload work (fszero-c6q.5).
///
/// - Different contract name/major, hash algorithm, hash hex length, or
///   shared-CAS layout semantics → `Err("incompatible_capability: …")`.
/// - Missing descriptor (`null`) → `Err("missing_capability: …")`;
///   type-broken/absent required fields → `Err("malformed_capability: …")`.
/// - Unknown ADDITIVE fields and newer minors are ignored
///   (forward-compatible): a valid newer-minor peer validates Ok.
pub fn validate_peer_descriptor(ours: &Value, theirs: &Value) -> Result<(), String> {
    let local = extract_fields("local", ours)?;
    let peer = extract_fields("peer", theirs)?;
    if peer.name != local.name {
        return Err(incompatible_cap(format!(
            "peer negotiates contract '{}' but this build speaks '{}' — these are different protocols, not versions",
            peer.name, local.name
        )));
    }
    if peer.major != local.major {
        return Err(incompatible_cap(format!(
            "peer speaks {} contract major {} but this build supports major {} — upgrade the older side before passing refs",
            local.name, peer.major, local.major
        )));
    }
    if peer.algo != local.algo || peer.hex_len != local.hex_len {
        return Err(incompatible_cap(format!(
            "peer identity is {}/{} but this build requires {}/{} — ref identities would not interoperate",
            peer.algo, peer.hex_len, local.algo, local.hex_len
        )));
    }
    if peer.layout_version != local.layout_version {
        return Err(incompatible_cap(format!(
            "peer shared-CAS layout version {} differs from ours {} — objects would land at different paths",
            peer.layout_version, local.layout_version
        )));
    }
    if let (Some(peer_layout), Some(local_layout)) = (&peer.layout, &local.layout) {
        if normalize_layout(peer_layout) != normalize_layout(local_layout) {
            return Err(incompatible_cap(format!(
                "peer shared-CAS layout '{peer_layout}' differs from ours '{local_layout}' — objects would land at different paths"
            )));
        }
    }
    Ok(())
}

/// Whether both descriptors advertise live shared-CAS dual-write interop.
///
/// **Validation success does not imply this is true** (fszero-w2g.27 / .48):
/// identity match only means schemes/hash/layout are compatible. Foreign blob
/// dual-write requires both sides attached+writable (`interop.shared_interop`
/// string `"enabled"`, or boolean `true` for compact test fixtures).
pub fn negotiate_shared_interop(ours: &Value, theirs: &Value) -> Result<bool, String> {
    validate_peer_descriptor(ours, theirs)?;
    Ok(interop_dual_write_live(ours) && interop_dual_write_live(theirs))
}

/// True when a descriptor claims live dual-write shared CAS.
fn interop_dual_write_live(desc: &Value) -> bool {
    let attached = desc
        .pointer("/shared_cas/attached")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let writable = desc
        .pointer("/shared_cas/writable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !attached || !writable {
        return false;
    }
    match desc.pointer("/interop/shared_interop") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "enabled",
        _ => false,
    }
}
