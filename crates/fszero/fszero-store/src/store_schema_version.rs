//! Store schema version stamps + skew rules (fszero-9smz).
//!
//! Three independently-released binaries share one `.zerostack` store.
//! Segments (journal, bookmarks, quarantine metadata) carry stamped versions.
//! Newer major => refuse loudly. Older minor (same major) => degrade gracefully.
//! Missing / older major => stamp current on open (upgrade path).

/// Current major. Newer major found on disk is a hard refuse.
pub const STORE_SCHEMA_MAJOR: u32 = 1;
/// Current minor. Older minor on same major is readable (degrade).
pub const STORE_SCHEMA_MINOR: u32 = 0;
/// Flat encoding written to meta: major * 1000 + minor.
/// Legacy stamps of `1` decode as major=1 minor=0 (see `decode_version`).
pub const STORE_SCHEMA_VERSION: u32 = STORE_SCHEMA_MAJOR * 1000 + STORE_SCHEMA_MINOR;

/// Meta key for the overall store stamp.
pub const META_STORE_SCHEMA_VERSION: &str = "store_schema_version";

/// Per-segment meta key prefix: `store_schema_segment_<name>`.
pub const SEGMENT_META_PREFIX: &str = "store_schema_segment_";

/// Segments that share the store and must be stamped independently.
pub const SCHEMA_SEGMENTS: &[&str] = &["journal", "bookmarks", "quarantine"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaSkew {
    Compatible,
    /// Same major, older minor: open ok, caller may degrade optional fields.
    OlderMinor {
        found_minor: u32,
        expected_minor: u32,
    },
    /// Missing stamp or older major: stamp/upgrade to current.
    UpgradeRequired {
        found: u32,
        expected: u32,
    },
    /// Newer major (or unreadable newer encoding): refuse, never guess.
    DowngradeRefused {
        found: u32,
        expected: u32,
    },
}

/// Decode a stored flat version into (major, minor).
/// Values `< 1000` are legacy flat majors (v1 => major 1 minor 0).
pub fn decode_version(v: u32) -> (u32, u32) {
    if v == 0 {
        (0, 0)
    } else if v < 1000 {
        (v, 0)
    } else {
        (v / 1000, v % 1000)
    }
}

pub fn encode_version(major: u32, minor: u32) -> u32 {
    if major == 0 { 0 } else { major * 1000 + minor }
}

pub fn segment_meta_key(segment: &str) -> String {
    format!("{SEGMENT_META_PREFIX}{segment}")
}

pub fn check_schema_skew(found: u32) -> SchemaSkew {
    let (found_major, found_minor) = decode_version(found);
    let (expected_major, expected_minor) = decode_version(STORE_SCHEMA_VERSION);
    if found_major > expected_major {
        SchemaSkew::DowngradeRefused {
            found,
            expected: STORE_SCHEMA_VERSION,
        }
    } else if found_major < expected_major {
        SchemaSkew::UpgradeRequired {
            found,
            expected: STORE_SCHEMA_VERSION,
        }
    } else if found_minor < expected_minor {
        SchemaSkew::OlderMinor {
            found_minor,
            expected_minor,
        }
    } else if found_minor > expected_minor {
        // Newer minor on same major: fields we may not understand — refuse.
        SchemaSkew::DowngradeRefused {
            found,
            expected: STORE_SCHEMA_VERSION,
        }
    } else {
        SchemaSkew::Compatible
    }
}
