//! Canonical strict bounded Zero Binary Format (ZBF-1) objects and profiles.
//!
//! ZBF uses a fixed 192-byte big-endian header. The header binds every object to
//! one assembly manifest, one durable profile, one producer contract, and one
//! payload digest. Container payloads are recursively length-delimited and are
//! decoded under fixed byte, child-count, and depth bounds before allocation.

use std::{error::Error, fmt};

use crate::{canonical_json, sha256, ArtifactOwnerV1, DigestV1};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const ZBF_MAGIC_V1: [u8; 8] = *b"ZEROZBF1";
pub const ZBF_SCHEMA_MAJOR_V1: u16 = 1;
pub const ZBF_SCHEMA_MINOR_V1: u16 = 0;
pub const ZBF_HEADER_LEN_V1: usize = 192;
pub const ZBF_CONTRACT_VERSION_V1: u16 = 1;
pub const ZBF_CONTAINER_FLAG_V1: u8 = 0x01;
pub const ZBF_MAX_OBJECT_BYTES_V1: u64 = 16 * 1024 * 1024;
pub const ZBF_MAX_CHILDREN_V1: u32 = 1024;
pub const ZBF_MAX_DEPTH_V1: u16 = 16;
const ZBF_PROFILE_DOMAIN_V1: &[u8] = b"zerostack.zbf_profile.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableProfileIdV1 {
    PortableStrict,
    ApfsStrict,
    Ext4XfsStrict,
    NtfsStrict,
}

impl DurableProfileIdV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PortableStrict => "portable_strict",
            Self::ApfsStrict => "apfs_strict",
            Self::Ext4XfsStrict => "ext4_xfs_strict",
            Self::NtfsStrict => "ntfs_strict",
        }
    }

    const fn filesystem(self) -> &'static str {
        match self {
            Self::PortableStrict => "portable",
            Self::ApfsStrict => "apfs",
            Self::Ext4XfsStrict => "ext4_or_xfs",
            Self::NtfsStrict => "ntfs",
        }
    }
}

/// Closed, non-weakenable publication and decode profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurableProfileV1 {
    id: DurableProfileIdV1,
}

impl DurableProfileV1 {
    pub const fn new(id: DurableProfileIdV1) -> Self {
        Self { id }
    }

    pub const fn portable_strict() -> Self {
        Self::new(DurableProfileIdV1::PortableStrict)
    }

    pub const fn id(self) -> DurableProfileIdV1 {
        self.id
    }

    pub const fn profile_version(self) -> u16 {
        1
    }

    pub const fn max_object_bytes(self) -> u64 {
        ZBF_MAX_OBJECT_BYTES_V1
    }

    pub const fn max_payload_bytes(self) -> u64 {
        ZBF_MAX_OBJECT_BYTES_V1 - ZBF_HEADER_LEN_V1 as u64
    }

    pub const fn max_children(self) -> u32 {
        ZBF_MAX_CHILDREN_V1
    }

    pub const fn max_depth(self) -> u16 {
        ZBF_MAX_DEPTH_V1
    }

    /// Canonical profile bytes. Profile names preregister platform identities;
    /// they do not claim native evidence for that filesystem.
    pub fn canonical_bytes(self) -> Vec<u8> {
        canonical_json(&json!({
            "profile_id": self.id.as_str(),
            "profile_version": self.profile_version(),
            "filesystem": self.id.filesystem(),
            "zbf_schema_major": ZBF_SCHEMA_MAJOR_V1,
            "zbf_schema_minor": ZBF_SCHEMA_MINOR_V1,
            "max_object_bytes": self.max_object_bytes(),
            "max_payload_bytes": self.max_payload_bytes(),
            "max_children": self.max_children(),
            "max_depth": self.max_depth(),
            "hash": "sha256",
            "atomic_publish": true,
            "sync_file_before_publish": true,
            "sync_parent_directory_after_publish": true,
            "unknown_required_versions": "reject"
        }))
        .into_bytes()
    }

    pub fn digest(self) -> DigestV1 {
        let bytes = self.canonical_bytes();
        let mut bound = Vec::with_capacity(ZBF_PROFILE_DOMAIN_V1.len() + bytes.len());
        bound.extend_from_slice(ZBF_PROFILE_DOMAIN_V1);
        bound.extend_from_slice(&bytes);
        DigestV1::from_bytes(sha256(&bound))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u16)]
#[serde(rename_all = "snake_case")]
pub enum ZbfArtifactKindV1 {
    AssemblyManifest = 1,
    FsPack = 2,
    GraphPack = 3,
    TokenPack = 4,
    Plan = 5,
    Receipt = 6,
    Witness = 7,
    Effect = 8,
    Snapshot = 9,
}

impl ZbfArtifactKindV1 {
    fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::AssemblyManifest,
            2 => Self::FsPack,
            3 => Self::GraphPack,
            4 => Self::TokenPack,
            5 => Self::Plan,
            6 => Self::Receipt,
            7 => Self::Witness,
            8 => Self::Effect,
            9 => Self::Snapshot,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZbfHeaderV1 {
    pub schema_major: u16,
    pub schema_minor: u16,
    pub kind: ZbfArtifactKindV1,
    pub owner: ArtifactOwnerV1,
    pub flags: u8,
    pub payload_len: u64,
    pub assembly_manifest_digest: DigestV1,
    pub durable_profile_digest: DigestV1,
    pub source_root_digest: DigestV1,
    pub producer_contract_digest: DigestV1,
    pub payload_digest: DigestV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZbfPayloadV1 {
    Bytes(Vec<u8>),
    Children(Vec<ZbfObjectV1>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZbfObjectV1 {
    pub header: ZbfHeaderV1,
    pub payload: ZbfPayloadV1,
}

impl ZbfObjectV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new_leaf(
        kind: ZbfArtifactKindV1,
        owner: ArtifactOwnerV1,
        assembly_manifest_digest: DigestV1,
        profile: DurableProfileV1,
        source_root_digest: DigestV1,
        producer_contract_digest: DigestV1,
        payload: Vec<u8>,
    ) -> Result<Self, ZbfErrorV1> {
        let payload_len = bounded_payload_len(payload.len(), profile)?;
        Ok(Self {
            header: ZbfHeaderV1 {
                schema_major: ZBF_SCHEMA_MAJOR_V1,
                schema_minor: ZBF_SCHEMA_MINOR_V1,
                kind,
                owner,
                flags: 0,
                payload_len,
                assembly_manifest_digest,
                durable_profile_digest: profile.digest(),
                source_root_digest,
                producer_contract_digest,
                payload_digest: DigestV1::from_bytes(sha256(&payload)),
            },
            payload: ZbfPayloadV1::Bytes(payload),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_container(
        kind: ZbfArtifactKindV1,
        owner: ArtifactOwnerV1,
        assembly_manifest_digest: DigestV1,
        profile: DurableProfileV1,
        source_root_digest: DigestV1,
        producer_contract_digest: DigestV1,
        children: Vec<ZbfObjectV1>,
    ) -> Result<Self, ZbfErrorV1> {
        let payload = encode_children(&children, profile, 0)?;
        let payload_len = bounded_payload_len(payload.len(), profile)?;
        Ok(Self {
            header: ZbfHeaderV1 {
                schema_major: ZBF_SCHEMA_MAJOR_V1,
                schema_minor: ZBF_SCHEMA_MINOR_V1,
                kind,
                owner,
                flags: ZBF_CONTAINER_FLAG_V1,
                payload_len,
                assembly_manifest_digest,
                durable_profile_digest: profile.digest(),
                source_root_digest,
                producer_contract_digest,
                payload_digest: DigestV1::from_bytes(sha256(&payload)),
            },
            payload: ZbfPayloadV1::Children(children),
        })
    }

    pub fn to_bytes(&self, profile: DurableProfileV1) -> Result<Vec<u8>, ZbfErrorV1> {
        self.to_bytes_at(profile, 0)
    }

    fn to_bytes_at(&self, profile: DurableProfileV1, depth: u16) -> Result<Vec<u8>, ZbfErrorV1> {
        require_depth(depth, profile)?;
        validate_header_contract(&self.header, profile)?;
        let payload = match &self.payload {
            ZbfPayloadV1::Bytes(bytes) => {
                if self.header.flags != 0 {
                    return Err(ZbfErrorV1::HeaderPayloadMismatch);
                }
                bounded_payload_len(bytes.len(), profile)?;
                bytes.clone()
            }
            ZbfPayloadV1::Children(children) => {
                if self.header.flags != ZBF_CONTAINER_FLAG_V1 {
                    return Err(ZbfErrorV1::HeaderPayloadMismatch);
                }
                encode_children(children, profile, depth)?
            }
        };
        let payload_len = bounded_payload_len(payload.len(), profile)?;
        let payload_digest = DigestV1::from_bytes(sha256(&payload));
        if self.header.payload_len != payload_len || self.header.payload_digest != payload_digest {
            return Err(ZbfErrorV1::HeaderPayloadMismatch);
        }
        let total_len = ZBF_HEADER_LEN_V1
            .checked_add(payload.len())
            .ok_or(ZbfErrorV1::LengthOverflow)?;
        if u64::try_from(total_len).map_err(|_| ZbfErrorV1::LengthOverflow)?
            > profile.max_object_bytes()
        {
            return Err(ZbfErrorV1::ObjectTooLarge {
                actual: total_len as u64,
                maximum: profile.max_object_bytes(),
            });
        }
        let mut out = Vec::with_capacity(total_len);
        encode_header(&self.header, &mut out);
        out.extend_from_slice(&payload);
        Ok(out)
    }

    pub fn from_bytes(
        bytes: &[u8],
        expected_assembly_manifest_digest: DigestV1,
        profile: DurableProfileV1,
    ) -> Result<Self, ZbfErrorV1> {
        decode_object(bytes, expected_assembly_manifest_digest, profile, 0)
    }

    pub fn identity(&self, profile: DurableProfileV1) -> Result<DigestV1, ZbfErrorV1> {
        Ok(DigestV1::from_bytes(sha256(&self.to_bytes(profile)?)))
    }
}

fn bounded_payload_len(actual: usize, profile: DurableProfileV1) -> Result<u64, ZbfErrorV1> {
    let actual = u64::try_from(actual).map_err(|_| ZbfErrorV1::LengthOverflow)?;
    if actual > profile.max_payload_bytes() {
        return Err(ZbfErrorV1::PayloadTooLarge {
            actual,
            maximum: profile.max_payload_bytes(),
        });
    }
    Ok(actual)
}

fn require_depth(depth: u16, profile: DurableProfileV1) -> Result<(), ZbfErrorV1> {
    if depth > profile.max_depth() {
        Err(ZbfErrorV1::DepthExceeded {
            actual: depth,
            maximum: profile.max_depth(),
        })
    } else {
        Ok(())
    }
}

fn encode_children(
    children: &[ZbfObjectV1],
    profile: DurableProfileV1,
    parent_depth: u16,
) -> Result<Vec<u8>, ZbfErrorV1> {
    let count = u32::try_from(children.len()).map_err(|_| ZbfErrorV1::TooManyChildren {
        actual: u32::MAX,
        maximum: profile.max_children(),
    })?;
    if count > profile.max_children() {
        return Err(ZbfErrorV1::TooManyChildren {
            actual: count,
            maximum: profile.max_children(),
        });
    }
    let child_depth = parent_depth
        .checked_add(1)
        .ok_or(ZbfErrorV1::DepthExceeded {
            actual: u16::MAX,
            maximum: profile.max_depth(),
        })?;
    require_depth(child_depth, profile)?;
    let mut payload = Vec::new();
    payload.extend_from_slice(&count.to_be_bytes());
    for child in children {
        let bytes = child.to_bytes_at(profile, child_depth)?;
        let len = u64::try_from(bytes.len()).map_err(|_| ZbfErrorV1::LengthOverflow)?;
        let next_len = payload
            .len()
            .checked_add(8)
            .and_then(|value| value.checked_add(bytes.len()))
            .ok_or(ZbfErrorV1::LengthOverflow)?;
        bounded_payload_len(next_len, profile)?;
        payload.extend_from_slice(&len.to_be_bytes());
        payload.extend_from_slice(&bytes);
    }
    Ok(payload)
}

fn decode_object(
    bytes: &[u8],
    expected_assembly_manifest_digest: DigestV1,
    profile: DurableProfileV1,
    depth: u16,
) -> Result<ZbfObjectV1, ZbfErrorV1> {
    require_depth(depth, profile)?;
    let actual_len = u64::try_from(bytes.len()).map_err(|_| ZbfErrorV1::LengthOverflow)?;
    if actual_len > profile.max_object_bytes() {
        return Err(ZbfErrorV1::ObjectTooLarge {
            actual: actual_len,
            maximum: profile.max_object_bytes(),
        });
    }
    if bytes.len() < ZBF_HEADER_LEN_V1 {
        return Err(ZbfErrorV1::UnexpectedEof);
    }
    let header = decode_header(&bytes[..ZBF_HEADER_LEN_V1])?;
    validate_header_contract(&header, profile)?;
    if header.assembly_manifest_digest != expected_assembly_manifest_digest {
        return Err(ZbfErrorV1::AssemblyMismatch {
            expected: expected_assembly_manifest_digest,
            actual: header.assembly_manifest_digest,
        });
    }
    let payload_len =
        usize::try_from(header.payload_len).map_err(|_| ZbfErrorV1::LengthOverflow)?;
    if header.payload_len > profile.max_payload_bytes() {
        return Err(ZbfErrorV1::PayloadTooLarge {
            actual: header.payload_len,
            maximum: profile.max_payload_bytes(),
        });
    }
    let expected_len = ZBF_HEADER_LEN_V1
        .checked_add(payload_len)
        .ok_or(ZbfErrorV1::LengthOverflow)?;
    if bytes.len() < expected_len {
        return Err(ZbfErrorV1::UnexpectedEof);
    }
    if bytes.len() > expected_len {
        return Err(ZbfErrorV1::TrailingBytes);
    }
    let payload = &bytes[ZBF_HEADER_LEN_V1..expected_len];
    let actual_digest = DigestV1::from_bytes(sha256(payload));
    if actual_digest != header.payload_digest {
        return Err(ZbfErrorV1::DigestMismatch {
            expected: header.payload_digest,
            actual: actual_digest,
        });
    }
    let payload = if header.flags == ZBF_CONTAINER_FLAG_V1 {
        ZbfPayloadV1::Children(decode_children(
            payload,
            expected_assembly_manifest_digest,
            profile,
            depth,
        )?)
    } else {
        ZbfPayloadV1::Bytes(payload.to_vec())
    };
    Ok(ZbfObjectV1 { header, payload })
}

fn decode_children(
    payload: &[u8],
    expected_assembly_manifest_digest: DigestV1,
    profile: DurableProfileV1,
    parent_depth: u16,
) -> Result<Vec<ZbfObjectV1>, ZbfErrorV1> {
    let mut cursor = Cursor::new(payload);
    let count = cursor.u32()?;
    if count > profile.max_children() {
        return Err(ZbfErrorV1::TooManyChildren {
            actual: count,
            maximum: profile.max_children(),
        });
    }
    let child_depth = parent_depth
        .checked_add(1)
        .ok_or(ZbfErrorV1::DepthExceeded {
            actual: u16::MAX,
            maximum: profile.max_depth(),
        })?;
    require_depth(child_depth, profile)?;
    let capacity = usize::try_from(count).map_err(|_| ZbfErrorV1::LengthOverflow)?;
    let mut children = Vec::with_capacity(capacity);
    for _ in 0..count {
        let child_len = usize::try_from(cursor.u64()?).map_err(|_| ZbfErrorV1::LengthOverflow)?;
        if child_len < ZBF_HEADER_LEN_V1 {
            return Err(ZbfErrorV1::ContainerMalformed);
        }
        let child_bytes = cursor.take(child_len)?;
        children.push(decode_object(
            child_bytes,
            expected_assembly_manifest_digest,
            profile,
            child_depth,
        )?);
    }
    if cursor.remaining() != 0 {
        return Err(ZbfErrorV1::TrailingBytes);
    }
    Ok(children)
}

fn validate_header_contract(
    header: &ZbfHeaderV1,
    profile: DurableProfileV1,
) -> Result<(), ZbfErrorV1> {
    if header.schema_major != ZBF_SCHEMA_MAJOR_V1 || header.schema_minor != ZBF_SCHEMA_MINOR_V1 {
        return Err(ZbfErrorV1::UnsupportedVersion {
            major: header.schema_major,
            minor: header.schema_minor,
        });
    }
    if header.flags & !ZBF_CONTAINER_FLAG_V1 != 0 {
        return Err(ZbfErrorV1::UnknownFlags(header.flags));
    }
    let expected = profile.digest();
    if header.durable_profile_digest != expected {
        return Err(ZbfErrorV1::ProfileMismatch {
            expected,
            actual: header.durable_profile_digest,
        });
    }
    Ok(())
}

fn encode_header(header: &ZbfHeaderV1, out: &mut Vec<u8>) {
    out.extend_from_slice(&ZBF_MAGIC_V1);
    out.extend_from_slice(&header.schema_major.to_be_bytes());
    out.extend_from_slice(&header.schema_minor.to_be_bytes());
    out.extend_from_slice(&(header.kind as u16).to_be_bytes());
    out.push(owner_to_u8(header.owner));
    out.push(header.flags);
    out.extend_from_slice(&header.payload_len.to_be_bytes());
    for digest in [
        header.assembly_manifest_digest,
        header.durable_profile_digest,
        header.source_root_digest,
        header.producer_contract_digest,
        header.payload_digest,
    ] {
        out.extend_from_slice(digest.as_bytes());
    }
    out.extend_from_slice(&[0_u8; 8]);
    debug_assert_eq!(out.len(), ZBF_HEADER_LEN_V1);
}

fn decode_header(bytes: &[u8]) -> Result<ZbfHeaderV1, ZbfErrorV1> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != ZBF_MAGIC_V1 {
        return Err(ZbfErrorV1::BadMagic);
    }
    let schema_major = cursor.u16()?;
    let schema_minor = cursor.u16()?;
    let raw_kind = cursor.u16()?;
    let kind = ZbfArtifactKindV1::from_u16(raw_kind).ok_or(ZbfErrorV1::InvalidKind(raw_kind))?;
    let raw_owner = cursor.u8()?;
    let owner = owner_from_u8(raw_owner).ok_or(ZbfErrorV1::InvalidOwner(raw_owner))?;
    let flags = cursor.u8()?;
    let payload_len = cursor.u64()?;
    let assembly_manifest_digest = cursor.digest()?;
    let durable_profile_digest = cursor.digest()?;
    let source_root_digest = cursor.digest()?;
    let producer_contract_digest = cursor.digest()?;
    let payload_digest = cursor.digest()?;
    if cursor.take(8)?.iter().any(|byte| *byte != 0) {
        return Err(ZbfErrorV1::ReservedNonZero);
    }
    if cursor.remaining() != 0 {
        return Err(ZbfErrorV1::TrailingBytes);
    }
    Ok(ZbfHeaderV1 {
        schema_major,
        schema_minor,
        kind,
        owner,
        flags,
        payload_len,
        assembly_manifest_digest,
        durable_profile_digest,
        source_root_digest,
        producer_contract_digest,
        payload_digest,
    })
}

fn owner_to_u8(owner: ArtifactOwnerV1) -> u8 {
    match owner {
        ArtifactOwnerV1::ZeroStack => 0,
        ArtifactOwnerV1::FsZero => 1,
        ArtifactOwnerV1::GraphZero => 2,
        ArtifactOwnerV1::TokenZero => 3,
        ArtifactOwnerV1::PiZeroStack => 4,
    }
}

fn owner_from_u8(value: u8) -> Option<ArtifactOwnerV1> {
    Some(match value {
        0 => ArtifactOwnerV1::ZeroStack,
        1 => ArtifactOwnerV1::FsZero,
        2 => ArtifactOwnerV1::GraphZero,
        3 => ArtifactOwnerV1::TokenZero,
        4 => ArtifactOwnerV1::PiZeroStack,
        _ => return None,
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ZbfErrorV1> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ZbfErrorV1::LengthOverflow)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(ZbfErrorV1::UnexpectedEof)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, ZbfErrorV1> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ZbfErrorV1> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ZbfErrorV1> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, ZbfErrorV1> {
        let bytes = self.take(8)?;
        Ok(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn digest(&mut self) -> Result<DigestV1, ZbfErrorV1> {
        let bytes = self.take(32)?;
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(bytes);
        Ok(DigestV1::from_bytes(digest))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZbfFailureCodeV1 {
    UnexpectedEof,
    BadMagic,
    UnsupportedVersion,
    InvalidKind,
    InvalidOwner,
    UnknownFlags,
    ReservedNonZero,
    LengthOverflow,
    ObjectTooLarge,
    PayloadTooLarge,
    TooManyChildren,
    DepthExceeded,
    TrailingBytes,
    DigestMismatch,
    AssemblyMismatch,
    ProfileMismatch,
    HeaderPayloadMismatch,
    ContainerMalformed,
    StoreFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZbfErrorV1 {
    UnexpectedEof,
    BadMagic,
    UnsupportedVersion {
        major: u16,
        minor: u16,
    },
    InvalidKind(u16),
    InvalidOwner(u8),
    UnknownFlags(u8),
    ReservedNonZero,
    LengthOverflow,
    ObjectTooLarge {
        actual: u64,
        maximum: u64,
    },
    PayloadTooLarge {
        actual: u64,
        maximum: u64,
    },
    TooManyChildren {
        actual: u32,
        maximum: u32,
    },
    DepthExceeded {
        actual: u16,
        maximum: u16,
    },
    TrailingBytes,
    DigestMismatch {
        expected: DigestV1,
        actual: DigestV1,
    },
    AssemblyMismatch {
        expected: DigestV1,
        actual: DigestV1,
    },
    ProfileMismatch {
        expected: DigestV1,
        actual: DigestV1,
    },
    HeaderPayloadMismatch,
    ContainerMalformed,
    Cas {
        class: &'static str,
        message: String,
    },
}

impl ZbfErrorV1 {
    pub const fn code(&self) -> ZbfFailureCodeV1 {
        match self {
            Self::UnexpectedEof => ZbfFailureCodeV1::UnexpectedEof,
            Self::BadMagic => ZbfFailureCodeV1::BadMagic,
            Self::UnsupportedVersion { .. } => ZbfFailureCodeV1::UnsupportedVersion,
            Self::InvalidKind(_) => ZbfFailureCodeV1::InvalidKind,
            Self::InvalidOwner(_) => ZbfFailureCodeV1::InvalidOwner,
            Self::UnknownFlags(_) => ZbfFailureCodeV1::UnknownFlags,
            Self::ReservedNonZero => ZbfFailureCodeV1::ReservedNonZero,
            Self::LengthOverflow => ZbfFailureCodeV1::LengthOverflow,
            Self::ObjectTooLarge { .. } => ZbfFailureCodeV1::ObjectTooLarge,
            Self::PayloadTooLarge { .. } => ZbfFailureCodeV1::PayloadTooLarge,
            Self::TooManyChildren { .. } => ZbfFailureCodeV1::TooManyChildren,
            Self::DepthExceeded { .. } => ZbfFailureCodeV1::DepthExceeded,
            Self::TrailingBytes => ZbfFailureCodeV1::TrailingBytes,
            Self::DigestMismatch { .. } => ZbfFailureCodeV1::DigestMismatch,
            Self::AssemblyMismatch { .. } => ZbfFailureCodeV1::AssemblyMismatch,
            Self::ProfileMismatch { .. } => ZbfFailureCodeV1::ProfileMismatch,
            Self::HeaderPayloadMismatch => ZbfFailureCodeV1::HeaderPayloadMismatch,
            Self::ContainerMalformed => ZbfFailureCodeV1::ContainerMalformed,
            Self::Cas { .. } => ZbfFailureCodeV1::StoreFailure,
        }
    }
}

impl fmt::Display for ZbfErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ZBF failure: {:?}", self.code())
    }
}

impl Error for ZbfErrorV1 {}

pub fn zbf_contract_manifest_v1() -> serde_json::Value {
    json!({
        "contract": "zerostack.zbf",
        "contract_version": ZBF_CONTRACT_VERSION_V1,
        "schema_major": ZBF_SCHEMA_MAJOR_V1,
        "schema_minor": ZBF_SCHEMA_MINOR_V1,
        "byte_order": "big_endian",
        "header_bytes": ZBF_HEADER_LEN_V1,
        "header_fields": [
            "magic", "schema_major", "schema_minor", "artifact_kind", "owner", "flags",
            "payload_len", "assembly_manifest_digest", "durable_profile_digest",
            "source_root_digest", "producer_contract_digest", "payload_digest", "reserved"
        ],
        "container_payload": "u32 child_count; repeated u64 child_len + complete_zbf_object",
        "profile_domain": "zerostack.zbf_profile.v1\u{0}",
        "required_profiles": ["portable_strict", "apfs_strict", "ext4_xfs_strict", "ntfs_strict"],
        "bounds": {
            "max_object_bytes": ZBF_MAX_OBJECT_BYTES_V1,
            "max_children": ZBF_MAX_CHILDREN_V1,
            "max_depth": ZBF_MAX_DEPTH_V1
        },
        "unknown_required_versions": "reject",
        "unknown_flags": "reject",
        "reserved_bytes": "must_be_zero"
    })
}

pub fn zbf_contract_digest_v1() -> DigestV1 {
    DigestV1::from_bytes(sha256(
        canonical_json(&zbf_contract_manifest_v1()).as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn leaf(profile: DurableProfileV1) -> ZbfObjectV1 {
        ZbfObjectV1::new_leaf(
            ZbfArtifactKindV1::Plan,
            ArtifactOwnerV1::ZeroStack,
            digest(1),
            profile,
            digest(2),
            digest(3),
            b"canonical payload".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn zbf_canonical_round_trip_and_digest_are_stable() {
        let profile = DurableProfileV1::portable_strict();
        let child = leaf(profile);
        let object = ZbfObjectV1::new_container(
            ZbfArtifactKindV1::Snapshot,
            ArtifactOwnerV1::ZeroStack,
            digest(1),
            profile,
            digest(4),
            digest(5),
            vec![child],
        )
        .unwrap();
        let bytes = object.to_bytes(profile).unwrap();
        assert_eq!(
            ZbfObjectV1::from_bytes(&bytes, digest(1), profile).unwrap(),
            object
        );
        assert_eq!(bytes.len(), 413);
        assert_eq!(
            object.identity(profile).unwrap().to_hex(),
            "025ca5465d0ebf7bb086f896775880b746c0197a5ce428d7036a27d5341fd559"
        );
        assert_eq!(
            zbf_contract_digest_v1().to_hex(),
            "c33216eac0bb9e45b5a8d6337c71df2d4a439582a8a4e03d66b0a6b9e9a16670"
        );
    }

    #[test]
    fn zbf_profile_digests_are_stable_and_distinct() {
        let profiles = [
            DurableProfileIdV1::PortableStrict,
            DurableProfileIdV1::ApfsStrict,
            DurableProfileIdV1::Ext4XfsStrict,
            DurableProfileIdV1::NtfsStrict,
        ]
        .map(|id| DurableProfileV1::new(id).digest());
        assert_eq!(
            profiles[0].to_hex(),
            "c8bf0ccc2c25dcd2f222a137c612e6daae00c2f4509c75eedc3b87592d0c7c9c"
        );
        for pair in profiles.iter().enumerate() {
            for other in profiles.iter().skip(pair.0 + 1) {
                assert_ne!(pair.1, other);
            }
        }
    }

    #[test]
    fn zbf_oversize_and_torn_inputs_fail_closed() {
        let profile = DurableProfileV1::portable_strict();
        let mut bytes = leaf(profile).to_bytes(profile).unwrap();
        bytes[16..24].copy_from_slice(&(profile.max_payload_bytes() + 1).to_be_bytes());
        assert_eq!(
            ZbfObjectV1::from_bytes(&bytes, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::PayloadTooLarge
        );

        let mut torn = leaf(profile).to_bytes(profile).unwrap();
        torn.pop();
        assert_eq!(
            ZbfObjectV1::from_bytes(&torn, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::UnexpectedEof
        );
    }

    #[test]
    fn zbf_trailing_reserved_and_payload_mutants_fail_closed() {
        let profile = DurableProfileV1::portable_strict();
        let bytes = leaf(profile).to_bytes(profile).unwrap();

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            ZbfObjectV1::from_bytes(&trailing, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::TrailingBytes
        );

        let mut reserved = bytes.clone();
        reserved[191] = 1;
        assert_eq!(
            ZbfObjectV1::from_bytes(&reserved, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::ReservedNonZero
        );

        let mut payload = bytes;
        payload[ZBF_HEADER_LEN_V1] ^= 1;
        assert_eq!(
            ZbfObjectV1::from_bytes(&payload, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::DigestMismatch
        );
    }

    #[test]
    fn zbf_assembly_and_profile_swaps_fail_before_payload() {
        let profile = DurableProfileV1::portable_strict();
        let bytes = leaf(profile).to_bytes(profile).unwrap();
        assert_eq!(
            ZbfObjectV1::from_bytes(&bytes, digest(9), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::AssemblyMismatch
        );
        assert_eq!(
            ZbfObjectV1::from_bytes(
                &bytes,
                digest(1),
                DurableProfileV1::new(DurableProfileIdV1::ApfsStrict),
            )
            .unwrap_err()
            .code(),
            ZbfFailureCodeV1::ProfileMismatch
        );
    }

    #[test]
    fn zbf_deep_nesting_is_bounded() {
        let profile = DurableProfileV1::portable_strict();
        let mut object = leaf(profile);
        let mut rejection = None;
        for _ in 0..=profile.max_depth() {
            match ZbfObjectV1::new_container(
                ZbfArtifactKindV1::Snapshot,
                ArtifactOwnerV1::ZeroStack,
                digest(1),
                profile,
                digest(2),
                digest(3),
                vec![object.clone()],
            ) {
                Ok(next) => object = next,
                Err(error) => {
                    rejection = Some(error);
                    break;
                }
            }
        }
        assert_eq!(
            rejection.expect("depth limit must reject").code(),
            ZbfFailureCodeV1::DepthExceeded
        );
    }
}
