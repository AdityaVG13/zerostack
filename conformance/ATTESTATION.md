# Conformance runtime evidence

**Option B:** engine conformance reports are private, local-only runtime evidence. They are not durable repository attestations and are not verified by CI or a clean clone. Reports remain ignored because runtime output can contain host-specific details.

Validate an existing local report set explicitly:

    python3 conformance/scripts/check_freshness.py conformance/reports/attestation.json

The validator never creates or discovers evidence. Each index entry names one report by basename and repeats its engine, surface, semantic contract digest, operation registry digest, engine Git revision, timestamp, and basename-only binary identity. Validation cross-checks those fields, requires fresh complete passing reports, and rejects absolute binary paths, missing reports, and unindexed JSON reports.

A future durable publication flow must be separate and explicit: sign reports and their index; scrub and verify host/private data; pin immutable engine revisions and provenance; review the artifact; commit it outside the ignored runtime directory; and verify signatures and freshness in CI. Until that exists, local reports and validator results are not release or CI attestations.
