from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scripts.rotation_snapshot_sha import (
    SnapshotDigestError,
    canonical_snapshot_bytes,
    load_snapshot,
    seal_snapshot,
    snapshot_sha256,
    verify_snapshot,
)


class RotationSnapshotDigestTest(unittest.TestCase):
    def fixture(self) -> dict[str, object]:
        return {
            "schema_version": "1.0",
            "created_at": "2026-07-17T05:40:14Z",
            "root": "/workspace/GraphZero",
            "summary": {"files": 2, "clean": True},
            "files": [
                {"path": "src/lib.rs", "sha256": "a" * 64},
                {"path": "docs/café.md", "sha256": "b" * 64},
            ],
        }

    def test_writer_verifier_round_trip_matches_stored_artifact_digest(self) -> None:
        sealed = seal_snapshot(self.fixture())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "snapshot.json"
            path.write_text(json.dumps(sealed, indent=2) + "\n", encoding="utf-8")
            loaded = load_snapshot(path)
        self.assertEqual(verify_snapshot(loaded), loaded["snapshot_sha256"])
        self.assertEqual(snapshot_sha256(loaded), loaded["snapshot_sha256"])

    def test_canonical_bytes_pin_field_set_order_timestamp_and_utf8(self) -> None:
        fixture = self.fixture()
        expected = (
            b'{"created_at":"2026-07-17T05:40:14Z","files":'
            b'[{"path":"src/lib.rs","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},'
            b'{"path":"docs/caf\xc3\xa9.md","sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],'
            b'"root":"/workspace/GraphZero","schema_version":"1.0","summary":{"clean":true,"files":2}}'
        )
        self.assertEqual(canonical_snapshot_bytes(fixture), expected)
        self.assertEqual(
            snapshot_sha256(fixture),
            "e2383a54c32d61a3e3a3a641005f751da6bb775040ee397b2467f122f1e05a1e",
        )
        reversed_fixture = dict(reversed(list(fixture.items())))
        self.assertEqual(snapshot_sha256(reversed_fixture), snapshot_sha256(fixture))
        fixture["created_at"] = "2026-07-17T05:40:15Z"
        self.assertNotEqual(snapshot_sha256(fixture), snapshot_sha256(self.fixture()))

    def test_digest_field_is_excluded_but_payload_changes_fail(self) -> None:
        sealed = seal_snapshot(self.fixture())
        sealed["snapshot_sha256"] = "0" * 64
        with self.assertRaisesRegex(SnapshotDigestError, "digest mismatch"):
            verify_snapshot(sealed)
        resealed = seal_snapshot(sealed)
        resealed["summary"] = {"files": 3, "clean": True}
        with self.assertRaisesRegex(SnapshotDigestError, "digest mismatch"):
            verify_snapshot(resealed)

    def test_duplicate_keys_floats_and_noncanonical_digest_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "duplicate.json"
            path.write_text('{"created_at":"a","created_at":"b"}', encoding="utf-8")
            with self.assertRaisesRegex(SnapshotDigestError, "duplicate JSON object key"):
                load_snapshot(path)
        with self.assertRaisesRegex(SnapshotDigestError, "floating-point"):
            snapshot_sha256({"ratio": 1.5})
        with self.assertRaisesRegex(SnapshotDigestError, "64 lowercase hex"):
            verify_snapshot({"snapshot_sha256": "ABC"})


if __name__ == "__main__":
    unittest.main()
