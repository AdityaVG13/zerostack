use graphzero_types::{ContentHash, fast_hex, fast_hex_32};

#[test]
fn content_hash_json_wire_shape_is_byte_array_newtype() {
    let hash = ContentHash::from_bytes([0xAB; 32]);
    let json = serde_json::to_string(&hash).expect("serialize content hash");

    assert_eq!(
        json,
        "[171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171]"
    );

    let roundtrip: ContentHash = serde_json::from_str(&json).expect("deserialize content hash");
    assert_eq!(roundtrip, hash);
}

#[test]
fn content_hash_default_is_zero_hash() {
    let hash = ContentHash::default();

    assert_eq!(hash, ContentHash::from_bytes([0; 32]));
    assert_eq!(hash.to_hex(), "0".repeat(64));
}

#[test]
fn content_hash_hex_roundtrip_rejects_non_wire_hex() {
    let hash = ContentHash::of(b"graphzero shared type wire contract");
    let hex = hash.to_hex();

    assert_eq!(ContentHash::from_hex(&hex), Some(hash));
    assert_eq!(ContentHash::from_hex(&hex[..63]), None);
    assert_eq!(ContentHash::from_hex(&(hex[..63].to_owned() + "g")), None);
}

#[test]
fn fast_hex_helpers_match_content_hash_hex_contract() {
    let bytes = [0x00, 0x0f, 0x10, 0xab, 0xff];
    let hash = ContentHash::from_bytes([0x5a; 32]);

    assert_eq!(fast_hex(&bytes), "000f10abff");
    assert_eq!(fast_hex_32(&[0x5a; 32]), hash.to_hex());
}
