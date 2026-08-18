//! K0 guest `z` surface catalog (`zerostack-fhcj`).
//!
//! The one shared table of what the K0 guest surface is and which V6
//! capabilities the read-only `z.invoke` / `z.parallel` seam may reach.
//! The K0 capability broker (`zsx-core` preflight) and the confined
//! interpreter's guest surface (`zero-codemode`) read exactly this table,
//! so no second catalog can drift and the surface adds no capability: every
//! `z.*` name is this table, and every invoked capability must also exist in
//! the V6 registration.
//!
//! K0 has no effect or commit surface: `z.transaction` and every other
//! absent name fail typed, and the read-only table below is the entire
//! capability reach of `z.invoke` / `z.parallel` (no write, edit, transact,
//! reserve, index, remember, shell, or grant-mint method is listed).

/// The guest surface root name (a `z` global inside a fresh K0 runtime).
pub const K0_GUEST_SURFACE_ROOT: &str = "z";

/// Direct callable `z.<member>(...)` names of the stable core surface.
pub const K0_GUEST_MEMBERS: &[&str] = &[
    "help",
    "inspect",
    "invoke",
    "parallel",
    "resolve",
    "expand",
    "snap",
    "view",
    "return",
    "persistHandle",
];

/// `z.state.<member>(...)` names (small serializable per-call state).
pub const K0_GUEST_STATE_MEMBERS: &[&str] = &["get", "has", "set", "delete", "list"];

/// `z.capabilities.<member>(...)` names (capability search).
pub const K0_GUEST_CAPABILITIES_MEMBERS: &[&str] = &["search"];

/// Names that are deliberately absent from K0 and fail typed wherever they
/// appear (broker preflight and runtime).
pub const K0_GUEST_ABSENT_MEMBERS: &[&str] = &["transaction"];

/// Property namespaces of the guest surface (`z.context`, `z.state`,
/// `z.capabilities`). `z.context` is a plain data object; the other two are
/// method groups.
pub const K0_GUEST_PROPERTIES: &[&str] = &["context", "state", "capabilities"];

/// Maximum call specs in one `z.parallel` batch (bounded by construction;
/// the interpreter's per-call connector in-flight bound stays authoritative
/// for simultaneous dispatches).
pub const K0_PARALLEL_LIMIT: usize = 16;

/// Maximum keys in the per-call guest state map.
pub const K0_STATE_MAX_KEYS: usize = 64;
/// Maximum bytes of one guest state key.
pub const K0_STATE_MAX_KEY_BYTES: usize = 128;
/// Maximum serialized bytes of one guest state value.
pub const K0_STATE_MAX_VALUE_BYTES: usize = 4 * 1024;
/// Maximum total serialized bytes of the whole guest state map.
pub const K0_STATE_MAX_TOTAL_BYTES: usize = 16 * 1024;

/// Maximum results of one `z.capabilities.search(query)`.
pub const K0_CAPABILITIES_SEARCH_MAX_RESULTS: usize = 32;

/// Capability surfaces that name authority classes K0 deliberately does not
/// grant: GPU, process/spawn, shell-as-a-surface, operating system,
/// environment, network, database, daemon, FFI, and dynamic codegen. A
/// mention of one fails typed with its authority class instead of the
/// generic unknown-surface text, so the denial is the law, not a catalog
/// gap. Because no grant exists, the corresponding live-resource counts
/// (GPU contexts, guest processes, network transfers) are structurally
/// zero. `token.shell` is NOT listed: it is a registered V6 capability and
/// keeps the canonical adapter policy on the direct `zero.*` surface; it
/// stays outside the read-only reach of `z.invoke`/`z.parallel`.
pub const K0_DENIED_SURFACES: &[(&str, &str)] = &[
    ("gpu", "GPU"),
    ("cuda", "GPU"),
    ("vulkan", "GPU"),
    ("metal", "GPU"),
    ("process", "process"),
    ("spawn", "process"),
    ("shell", "shell"),
    ("os", "operating-system"),
    ("env", "environment"),
    ("net", "network"),
    ("network", "network"),
    ("http", "network"),
    ("fetch", "network"),
    ("socket", "network"),
    ("db", "database"),
    ("daemon", "daemon"),
    ("pool", "daemon"),
    ("ffi", "FFI"),
    ("codegen", "dynamic-codegen"),
];

/// The authority class denied for `surface`, if any (`gpu`, `process`,
/// `network`, `shell`, `daemon`, `database`, `operating-system`,
/// `environment`, `FFI`, `dynamic-codegen`).
pub fn denied_authority(surface: &str) -> Option<&'static str> {
    K0_DENIED_SURFACES
        .iter()
        .find(|(candidate, _)| *candidate == surface)
        .map(|(_, class)| *class)
}

/// Every V6 capability the read-only `z.invoke` / `z.parallel` seam may
/// reach, in stable order. This is the complete K0 capability reach: no
/// write/edit/transact/multi_edit, no reserve/index/remember, no shell/job
/// effects, and no grant minting is listed here.
pub const K0_READ_ONLY_CAPABILITIES: &[(&str, &str)] = &[
    ("fs", "plan"),
    ("fs", "structural"),
    ("fs", "compound"),
    ("fs", "read"),
    ("fs", "world"),
    ("fs", "multi_read"),
    ("fs", "multi_list"),
    ("fs", "multi_search"),
    ("fs", "multi_ast_search"),
    ("fs", "lookup"),
    ("graph", "blast"),
    ("graph", "query"),
    ("graph", "multi_query"),
    ("graph", "orient"),
    ("graph", "recall"),
    ("graph", "verify"),
    ("graph", "snap"),
    ("token", "compact"),
    ("token", "expand"),
    ("token", "find"),
    ("token", "read"),
    ("token", "job"),
    ("help", "search"),
    ("help", "catalog"),
];

/// `fs.compound` operation names that lower to read-only capabilities
/// (approved by the single lowering authority; the broker and the runtime
/// check the op, never the method name alone).
pub const K0_READ_ONLY_COMPOUND_OPS: &[&str] = &[
    "read",
    "search",
    "find",
    "grep",
    "list",
    "tree",
    "inventory",
    "resolve",
    "lookup",
];

/// Whether `surface.method` is in the read-only K0 capability reach.
pub fn is_k0_read_only_capability(surface: &str, method: &str) -> bool {
    K0_READ_ONLY_CAPABILITIES
        .iter()
        .any(|(s, m)| *s == surface && *m == method)
}

/// Whether one `fs.compound` operation name lowers read-only.
pub fn is_k0_read_only_compound_op(op: &str) -> bool {
    K0_READ_ONLY_COMPOUND_OPS.contains(&op)
}

/// Whether `member` is a direct callable `z.<member>` name.
pub fn is_k0_guest_member(member: &str) -> bool {
    K0_GUEST_MEMBERS.contains(&member)
}

/// Whether `member` is a `z.state.<member>` name.
pub fn is_k0_guest_state_member(member: &str) -> bool {
    K0_GUEST_STATE_MEMBERS.contains(&member)
}

/// Whether `member` is a `z.capabilities.<member>` name.
pub fn is_k0_guest_capabilities_member(member: &str) -> bool {
    K0_GUEST_CAPABILITIES_MEMBERS.contains(&member)
}
