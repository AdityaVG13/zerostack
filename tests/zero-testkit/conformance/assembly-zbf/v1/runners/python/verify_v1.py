#!/usr/bin/env python3
import hashlib
import json
import struct
import sys
from pathlib import Path

HEADER = 192
MAX_FILE = 16 * 1024 * 1024
DOMAIN = b"zerostack.assembly_manifest.v1\0"


def fail(code: str) -> None:
    print(code, file=sys.stderr)
    raise SystemExit(2)


def read_checked(root: Path, vector: dict) -> bytes:
    data = (root / vector["file"]).read_bytes()
    if len(data) != vector["byte_len"] or len(data) > MAX_FILE:
        fail("fixture_length_mismatch")
    if hashlib.sha256(data).hexdigest() != vector["sha256"]:
        fail("fixture_digest_mismatch")
    return data


def verify_zbf(data: bytes) -> None:
    if len(data) < HEADER or data[:8] != b"ZEROZBF1":
        fail("zbf_bad_magic")
    major, minor, kind = struct.unpack(">HHH", data[8:14])
    owner, flags = data[14], data[15]
    payload_len = struct.unpack(">Q", data[16:24])[0]
    if (major, minor) != (1, 0) or not 1 <= kind <= 9 or owner > 4 or flags & ~1:
        fail("zbf_header_mismatch")
    if payload_len > MAX_FILE - HEADER or HEADER + payload_len != len(data):
        fail("zbf_length_mismatch")
    if any(data[184:192]):
        fail("zbf_reserved_nonzero")
    if hashlib.sha256(data[HEADER:]).digest() != data[152:184]:
        fail("zbf_payload_digest_mismatch")


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage")
    root = Path(sys.argv[1])
    index_bytes = (root / "index.json").read_bytes()
    index = json.loads(index_bytes)
    canonical = json.dumps(index, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    if canonical != index_bytes or index["schema_version"] != 1:
        fail("index_noncanonical")
    manifest = read_checked(root, index["assembly_manifest"])
    if json.dumps(json.loads(manifest), sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode() != manifest:
        fail("manifest_noncanonical")
    if hashlib.sha256(DOMAIN + manifest).hexdigest() != index["assembly_manifest"]["semantic_digest"]:
        fail("manifest_digest_mismatch")
    leaf = read_checked(root, index["zbf_leaf"])
    container = read_checked(root, index["zbf_container"])
    verify_zbf(leaf)
    verify_zbf(container)
    print("assembly_zbf_kat:python:v1:passed")


if __name__ == "__main__":
    main()
