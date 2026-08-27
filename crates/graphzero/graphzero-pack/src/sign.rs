//! ed25519 manifest signing (fail-closed verify on install).

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};

use crate::manifest::{PackManifest, PackSignKey};

pub fn sign_manifest(manifest: &mut PackManifest, key: &PackSignKey) -> Result<()> {
    let payload = manifest.canonical_unsigned_bytes()?;
    let sig = key.signing_key().sign(&payload);
    manifest.signature_hex = hex::encode(sig.to_bytes());
    Ok(())
}

pub fn verify_manifest_signature(manifest: &PackManifest, key: &VerifyingKey) -> Result<()> {
    let payload = manifest.canonical_unsigned_bytes()?;
    let sig_bytes = hex::decode(&manifest.signature_hex).context("decode signature hex")?;
    if sig_bytes.len() != 64 {
        bail!("invalid signature length {}", sig_bytes.len());
    }
    let mut arr = [0u8; 64];
    arr.copy_from_slice(&sig_bytes);
    let sig = Signature::from_bytes(&arr);
    key.verify(&payload, &sig)
        .map_err(|_| anyhow::anyhow!("pack manifest signature invalid"))
}
