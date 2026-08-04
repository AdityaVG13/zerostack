# ZeroStack V3 canonical claim authority

This directory is the checked-in authority for 138 V3 claims and 14 executable freezes.

## Generate and verify

```text
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack_z0 cargo run -p zero-testkit --example generate_authority > conformance/authority/canonical-authority-v1.json
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack_z0 cargo test -p zero-testkit authority_ledger -- --test-threads=1
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack_z0 cargo test -p zero-testkit provenance_round_trip -- --test-threads=1
```

`canonical-authority-v1.json` must regenerate byte-for-byte. Corrections require a successor version. Do not rewrite a published fixture.

## Authority inputs

`provenance-v1.json` names exactly five authority inputs. The source fixtures preserve the Round5 claim ledger, Round5 source archive audit, and Round8 freeze bytes. Duplicate observed digests remain separate provenance rows with distinct record IDs and source coordinates.

All claims start `NOT_YET_PROVEN`. All freezes start `UNIMPLEMENTED`. `VERIFIED_AT_FREEZE` is non-public. Only Z8 can promote public wording.

Target and distribution profiles are preregistrations, not support claims. RCH, cross-compilation, Docker, Wine, and emulation do not count as native evidence.
