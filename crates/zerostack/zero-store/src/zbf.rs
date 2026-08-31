//! Shared CAS persistence for canonical ZBF objects.

use zero_abi::{DurableProfile, Sha256Digest, ZbfError, ZbfObject};

use crate::{CasError, PutOutcome, SharedCas};

impl SharedCas {
    pub fn put_zbf(
        &self,
        object: &ZbfObject,
        profile: DurableProfile,
    ) -> Result<PutOutcome, ZbfError> {
        let bytes = object.to_bytes(profile)?;
        self.put_outcome(&bytes, profile.max_object_bytes())
            .map_err(cas_error)
    }

    pub fn get_zbf(
        &self,
        sha256: &str,
        expected_assembly_manifest_digest: Sha256Digest,
        profile: DurableProfile,
    ) -> Result<ZbfObject, ZbfError> {
        let bytes = self
            .get_verified_limited(sha256, profile.max_object_bytes())
            .map_err(cas_error)?;
        ZbfObject::from_bytes(&bytes, expected_assembly_manifest_digest, profile)
    }
}

fn cas_error(error: CasError) -> ZbfError {
    ZbfError::Cas {
        class: error.class(),
        message: error.to_string(),
    }
}
