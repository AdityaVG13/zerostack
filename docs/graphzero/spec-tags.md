# SPEC-GZ Verifier Ledger

Authoritative machine-checkable ledger for GraphZero's executable oracle
verifiers. One requirement per row, one verifier per row. Every row is
`VERIFIED`; MISSING or prose-only rows are forbidden. The ledger is validated
by `scripts/check_spec_tags.py`, which also fails when any `#[test]` function
in the declared oracle harness is missing from this table.

Table schema (Markdown, exactly five cells per row):

| Column | Format |
|--------|--------|
| `ID` | `SPEC-GZ-NNN`, stable and unique |
| `Requirement` | one requirement, nonempty prose |
| `Source` | `<file path>::<symbol>`; the symbol text must exist in the source file |
| `Verifier` | `<oracle harness path>::<#[test] fn name>`; the test must exist in the harness |
| `Status` | exactly `VERIFIED` |

| ID | Requirement | Source | Verifier | Status |
|---|---|---|---|---|
| SPEC-GZ-001 | OracleMode serializes exactly to the five stable lowercase names gold, differential, metamorphic, property, and mutation. | crates/graphzero-query/src/oracle.rs::OracleMode | crates/graphzero-query/tests/oracle_harness.rs::all_five_modes_use_stable_names | VERIFIED |
| SPEC-GZ-002 | FailureBundle::from_report maps every GateFailure field losslessly and refuses a passed release-gate report as failure evidence. | crates/graphzero-query/src/oracle.rs::from_report | crates/graphzero-query/tests/oracle_harness.rs::report_mapping_is_lossless_and_failure_only | VERIFIED |
| SPEC-GZ-003 | Canonical bundle bytes are independent of input order and of JSON object-key order. | crates/graphzero-query/src/oracle.rs::canonical_bytes | crates/graphzero-query/tests/oracle_harness.rs::canonical_bytes_ignore_input_and_object_key_order | VERIFIED |
| SPEC-GZ-004 | Bundle round-trip preserves canonical bytes, and malformed JSON, unknown schema version, engine mismatch, and unknown fields fail closed. | crates/graphzero-query/src/oracle.rs::from_json | crates/graphzero-query/tests/oracle_harness.rs::round_trip_and_reject_malformed_contracts | VERIFIED |
| SPEC-GZ-005 | Validation rejects digest mismatch, empty failures, empty evidence, control characters, and unsorted evidence refs. | crates/graphzero-query/src/oracle.rs::validate | crates/graphzero-query/tests/oracle_harness.rs::reject_digest_empty_failure_evidence_and_unsorted_refs | VERIFIED |
| SPEC-GZ-006 | Normalization sorts failures by canonical diagnosis identity, retains duplicate failures, and sorts and deduplicates evidence refs. | crates/graphzero-query/src/oracle.rs::normalize | crates/graphzero-query/tests/oracle_harness.rs::stable_sort_retains_duplicate_failures | VERIFIED |
