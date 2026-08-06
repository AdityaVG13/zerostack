//! ZeroStore integration for the canonical `zero-abi` ZBF-1 contract.
//!
//! Format types remain re-exported here for source compatibility. `zero-abi`
//! is the only format authority; this module owns only CAS persistence glue.

use zero_abi::DigestV1;
pub use zero_abi::zbf::*;

use crate::{CasError, PutOutcome, SharedCas};

impl SharedCas {
    pub fn put_zbf(
        &self,
        object: &ZbfObjectV1,
        profile: DurableProfileV1,
    ) -> Result<PutOutcome, ZbfErrorV1> {
        let bytes = object.to_bytes(profile)?;
        self.put_outcome(&bytes, profile.max_object_bytes())
            .map_err(cas_error)
    }

    pub fn get_zbf(
        &self,
        sha256: &str,
        expected_assembly_manifest_digest: DigestV1,
        profile: DurableProfileV1,
    ) -> Result<ZbfObjectV1, ZbfErrorV1> {
        let bytes = self
            .get_verified_limited(sha256, profile.max_object_bytes())
            .map_err(cas_error)?;
        ZbfObjectV1::from_bytes(&bytes, expected_assembly_manifest_digest, profile)
    }
}

fn cas_error(error: CasError) -> ZbfErrorV1 {
    ZbfErrorV1::Cas {
        class: error.class(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zero_abi::ArtifactOwnerV1;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    #[test]
    fn durable_reopen_zbf_round_trip_across_handles() {
        let dir = tempdir().unwrap();
        let profile = DurableProfileV1::portable_strict();
        let object = ZbfObjectV1::new_leaf(
            ZbfArtifactKindV1::Plan,
            ArtifactOwnerV1::ZeroStack,
            digest(1),
            profile,
            digest(2),
            digest(3),
            b"canonical payload".to_vec(),
        )
        .unwrap();
        let first = SharedCas::open(dir.path());
        let outcome = first.put_zbf(&object, profile).unwrap();
        drop(first);

        let reopened = SharedCas::open(dir.path());
        assert_eq!(
            reopened.get_zbf(&outcome.hash, digest(1), profile).unwrap(),
            object
        );
        let independent_session = SharedCas::open(dir.path());
        assert_eq!(
            independent_session
                .get_zbf(&outcome.hash, digest(1), profile)
                .unwrap(),
            object
        );
    }
}
