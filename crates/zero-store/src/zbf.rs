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

