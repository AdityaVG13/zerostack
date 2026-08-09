#!/usr/bin/env python3
"""Focused tests for the canonical-JSON implementation guard.

Proves the guard stays loud about real independent encoders while accepting
thin zero-abi delegating wrappers and #[test]/#[cfg(test)] source-local
helpers, and that the tracked known-exception path still reports excused.
"""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import check_canonical_json_impls as cj  # noqa: E402


class CanonicalJsonGuardTests(unittest.TestCase):
    def _scan(self, files):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, content in files.items():
                path = root / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
            violations, excused, _checked = cj.scan_roots([root])
        return violations, excused

    def test_delegating_wrapper_is_allowed(self):
        violations, _ = self._scan(
            {
                "crates/graphzero-query/src/witness_cache.rs": (
                    "    pub fn canonical_key_json(&self) -> String {\n"
                    "        let value = serde_json::to_value(self)\n"
                    "            .expect(\"cache key is JSON-serializable\");\n"
                    "        zero_abi::canonical_json(&value)\n"
                    "    }\n"
                ),
            }
        )
        self.assertEqual(violations, [])

    def test_source_local_test_helper_is_allowed(self):
        violations, _ = self._scan(
            {
                "crates/graphzero-query/src/witness_cache.rs": (
                    "    #[test]\n"
                    "    fn scope_digest_streaming_matches_canonical_json_value_tree() {\n"
                    "        let streaming = roots.scope_digest(\"src/\");\n"
                    "        let classic = zero_abi::sha256_hex(\n"
                    "            zero_abi::canonical_json(&listing).as_bytes());\n"
                    "        assert_eq!(streaming, classic);\n"
                    "    }\n"
                ),
            }
        )
        self.assertEqual(violations, [])

    def test_cfg_test_module_helper_is_allowed(self):
        violations, _ = self._scan(
            {
                "crates/example/src/lib.rs": (
                    "    #[cfg(test)]\n"
                    "    mod tests {\n"
                    "        fn canonical_json_helper(value: &Value) -> String {\n"
                    "            let mut out = String::new();\n"
                    "            write_canonical(value, &mut out);\n"
                    "            out\n"
                    "        }\n"
                    "    }\n"
                ),
            }
        )
        self.assertEqual(violations, [])

    def test_qualified_test_attr_is_allowed(self):
        violations, _ = self._scan(
            {
                "crates/example/src/lib.rs": (
                    "    #[tokio::test]\n"
                    "    fn canonical_json_helper(value: &Value) -> String {\n"
                    "        write_canonical(value, &mut String::new());\n"
                    "        String::new()\n"
                    "    }\n"
                ),
            }
        )
        self.assertEqual(violations, [])

    def test_cfg_feature_attr_does_not_exempt_production_encoder(self):
        violations, _ = self._scan(
            {
                "crates/example/src/lib.rs": (
                    "    #[cfg(feature = \"latest\")]\n"
                    "    fn canonical_json(value: &Value) -> String {\n"
                    "        let mut out = String::new();\n"
                    "        write_canonical(value, &mut out);\n"
                    "        out\n"
                    "    }\n"
                ),
            }
        )
        self.assertEqual(len(violations), 1, violations)
        self.assertIn("canonical_json", violations[0])

    def test_independent_encoders_still_fail(self):
        violations, _ = self._scan(
            {
                "crates/graphzero-query/src/deterministic_facts.rs": (
                    "    pub fn canonical_json(value: &Value) -> String {\n"
                    "        let mut out = String::new();\n"
                    "        write_canonical(value, &mut out);\n"
                    "        out\n"
                    "    }\n"
                ),
                "src/core/canonicalize.rs": (
                    "    fn canonicalize_json_value(v: &serde_json::Value)\n"
                    "        -> serde_json::Value {\n"
                    "        match v {\n"
                    "            serde_json::Value::Object(map) => v.clone(),\n"
                    "            other => other.clone(),\n"
                    "        }\n"
                    "    }\n"
                ),
            }
        )
        self.assertEqual(len(violations), 2)
        self.assertTrue(
            any("deterministic_facts.rs" in v for v in violations), violations
        )
        self.assertTrue(any("canonicalize.rs" in v for v in violations), violations)

    def test_known_exception_remains_excused(self):
        violations, excused = self._scan(
            {
                "crates/graphzero-pack/src/manifest.rs": (
                    "    pub fn unsigned_canonical_json(&self) -> String {\n"
                    "        let mut out = String::new();\n"
                    "        emit_struct_order(self, &mut out);\n"
                    "        out\n"
                    "    }\n"
                ),
            }
        )
        self.assertEqual(violations, [])
        self.assertEqual(len(excused), 1)
        self.assertIn("zerostack-t76", excused[0])


if __name__ == "__main__":
    unittest.main()
