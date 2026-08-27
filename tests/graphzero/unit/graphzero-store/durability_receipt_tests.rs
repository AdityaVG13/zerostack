
use super::*;

#[test]
fn canonical_manifest_digest_is_stable() {
    let manifest = Manifest::default();
    assert_eq!(manifest_digest(&manifest), manifest_digest(&manifest));
}
