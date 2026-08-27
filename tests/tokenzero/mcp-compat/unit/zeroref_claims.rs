//! Gate public ZeroRef claims against retained conformance evidence (cqr.10).
//!
//! Local retained evidence may be `pending` when only the host OS ran. The
//! release aggregation gate requires fully green multi-OS evidence and does not
//! infer approval merely from the generic `CI` environment variable.

use std::{collections::BTreeMap, env, fs, path::PathBuf};

use serde_json::Value;

use crate::capability_descriptor::CapabilityDescriptor;
use tokenzero_core::McpToolSurface;

const REQUIRED_OS: [&str; 3] = ["macos", "linux", "windows"];
const REQUIRED_LIFECYCLE: [&str; 8] = [
    "fresh",
    "upgraded_legacy",
    "explicit_shared",
    "default_isolated",
    "incompatible_peer",
    "corruption",
    "disable",
    "rollback",
];

macro_rules! named_test {
    ($name:ident, $body:block) => {
        #[test]
        fn $name() $body
    };
}

macro_rules! require {
    ($($id:literal => $condition:expr),+ $(,)?) => {$ (
        assert!($condition, "requirement {} failed", $id);
    )+};
}

fn evidence_path() -> PathBuf {
    env::var_os("ZEROREF_EVIDENCE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/mcp-compat/fixtures/zeroref-conformance-evidence.json")
        })
}

fn load_evidence() -> Value {
    let text = fs::read_to_string(evidence_path()).expect("read zeroref conformance evidence");
    serde_json::from_str(&text).expect("parse zeroref conformance evidence")
}

fn require_all_os() -> bool {
    env::var_os("ZEROREF_REQUIRE_ALL_OS")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn os_row_status(row: &Value) -> &'static str {
    let cells = row["cells"].as_array().expect("cells");
    if cells.is_empty() {
        "empty"
    } else if cells.iter().any(|c| c["status"] == "fail") {
        "fail"
    } else if cells.iter().all(|c| c["status"] == "pass") {
        "pass"
    } else if cells.iter().any(|c| c["status"] == "skip") {
        "skip"
    } else {
        "mixed"
    }
}

named_test!(retained_evidence_is_green_or_pending_with_sibling_shas, {
    let evidence = load_evidence();
    let matrix = &evidence["matrix"];
    let status = matrix["status"].as_str().unwrap_or("");
    let fragments = matrix["fragment_rows"].as_array().expect("fragment_rows");
    require! {
        "evidence.schema" => evidence["schema"] == "zeroref-conformance-evidence/v1",
        "evidence.version" => evidence["zeroref_version"] == "v1",
        "evidence.docs_audit" => evidence["docs_audit"] == "ok",
        "matrix.status" => matches!(status, "green" | "pending"),
        "matrix.concurrent" => matrix["concurrent"]["status"] == "pass",
        "matrix.wrong_store" => matrix["wrong_store"]["status"] == "pass",
        "matrix.fragments.present" => !fragments.is_empty(),
        "matrix.fragments.pass" => fragments.iter().all(|row| row["status"] == "pass"),
    }

    let engines: Vec<_> = matrix["sibling_shas"]
        .as_array()
        .expect("sibling_shas")
        .iter()
        .filter_map(|row| row["engine"].as_str())
        .collect();
    for engine in ["fszero", "graphzero", "tokenzero"] {
        assert!(
            engines.contains(&engine),
            "missing sibling sha for {engine}"
        );
    }

    let mut by_os = BTreeMap::new();
    for row in matrix["rows"].as_array().expect("rows") {
        by_os.insert(
            row["os"].as_str().expect("os").to_string(),
            os_row_status(row),
        );
    }
    if require_all_os() {
        assert_eq!(
            status, "green",
            "CI/release requires green evidence, got {status}"
        );
        for os in REQUIRED_OS {
            let row_status = by_os.get(os).copied().unwrap_or("missing");
            assert_eq!(row_status, "pass", "required OS {os} got {row_status}");
        }
    } else {
        require! {
            "local.any_pass" => by_os.values().any(|s| *s == "pass"),
            "local.pass_or_skip" => by_os.values().all(|s| matches!(*s, "pass" | "skip")),
        }
        if status == "green" {
            for os in REQUIRED_OS {
                assert_eq!(by_os.get(os).copied(), Some("pass"), "green OS row {os}");
            }
        }
    }
});

named_test!(
    capability_descriptor_gates_cross_engine_until_multi_os_evidence,
    {
        let z = CapabilityDescriptor::for_surface(McpToolSurface::Classic).zeroref;
        require! {
            "descriptor.enabled" => z.enabled,
            "descriptor.shared_cas" => z.shared_cas,
            "descriptor.blob_ref_expand" => z.blob_ref_expand,
            // Option B: do not advertise proven multi-OS / cross-engine
            // portability until ZEROREF_REQUIRE_ALL_OS green evidence exists.
            "descriptor.cross_engine_gated" => !z.cross_engine,
            "descriptor.no_cross_engine_feature" =>
                z.features.iter().all(|f| f != "cross-engine-blob-expand"),
        }
        assert_eq!(z.portable_ref_kinds, ["blob"]);
        for kind in [
            "execution",
            "error",
            "session",
            "file",
            "graph",
            "index",
            "unit",
        ] {
            assert!(
                z.unsupported_portable_ref_kinds
                    .iter()
                    .any(|value| value == kind),
                "missing unsupported portable ref kind {kind}"
            );
        }
        assert!(
            z.limitations
                .iter()
                .any(|value| value.contains("performance claims"))
        );
        assert!(
            z.limitations.iter().any(|value| {
                value.contains("not multi-OS proof")
                    || value.contains("ZEROREF_REQUIRE_ALL_OS")
            }),
            "limitations must disclose host-local evidence is not multi-OS proof"
        );
        for scheme in ["tz://", "fz://", "gz://"] {
            assert!(
                z.ref_schemes.iter().any(|s| s == scheme),
                "missing scheme {scheme}"
            );
        }
    }
);

named_test!(lifecycle_smokes_required_cells_pass_when_present, {
    let path = env::var_os("ZEROREF_LIFECYCLE_EVIDENCE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.tmp/zeroref-lifecycle-evidence.json")
        });
    if !path.exists() {
        assert!(
            !require_all_os(),
            "CI/release requires lifecycle evidence at {}",
            path.display()
        );
        return;
    }
    let text = fs::read_to_string(&path).expect("read lifecycle evidence");
    let doc: Value = serde_json::from_str(&text).expect("parse lifecycle evidence");
    require! {
        "lifecycle.schema" => doc["schema"] == "zeroref-lifecycle-smokes/v1",
        "lifecycle.status" => doc["status"] == "pass",
    }
    let by_name: BTreeMap<_, _> = doc["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .map(|cell| {
            (
                cell["test"].as_str().expect("test"),
                cell["status"].as_str().expect("status"),
            )
        })
        .collect();
    for name in REQUIRED_LIFECYCLE {
        let status = by_name.get(name).copied().unwrap_or("missing");
        assert_eq!(status, "pass", "lifecycle cell {name} got {status}");
    }
});
