use fszero_store::recovery::seq_ref_scoped_err;

#[test]
fn seq_ref_scoped_err_points_at_z_blob() {
    let msg = seq_ref_scoped_err("seq/read/1");
    assert!(msg.contains("z://blob/"), "{msg}");
    assert!(!msg.contains("fz://"), "{msg}");
    assert!(msg.contains("seq/"), "{msg}");
}
