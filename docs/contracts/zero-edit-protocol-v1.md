# zero-edit-protocol/v1

Normative contract for `crates/zero-codemode/src/edit_protocol.rs`.

One generic `EDIT` operation whose argument is a list of `EditOp` values.
Verbs live in the payload (`v` discriminant), not in the tool namespace.

Version string: `zep/1`.

Ref grammar is not redefined here:

- FSZero snap-to-file: `<path>#L<start>-L<end>` (1-based, inclusive)
- ZeroRef v1: `fz://blob/<sha256>[#L..|#B..]`
- GraphZero: `gz://node/<symbol>` and `gz://blob/...` evidence
