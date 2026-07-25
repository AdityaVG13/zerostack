//! G1–G10 check ids map to stable harness names (audit gap coverage index).

use zerostack_codemode_conformance::checks::CheckId;

#[test]
fn all_ten_g_checks_have_distinct_stable_names() {
    let names: Vec<_> = CheckId::ALL.iter().map(|id| id.as_str()).collect();
    assert_eq!(names.len(), 10);
    let unique: std::collections::HashSet<_> = names.iter().copied().collect();
    assert_eq!(unique.len(), 10);
    assert!(names.contains(&"G2_refs"));
    assert!(names.contains(&"G4_leak_proof"));
    assert!(names.contains(&"G10_sandbox"));
}
