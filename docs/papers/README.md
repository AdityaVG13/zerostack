# ZeroStack paper series

These six LaTeX files are publication scaffolds. They contain no measured
result. Theory statements remain conditional until their Lean declarations,
source digest, build log, and axiom report exist. Systems and empirical claims
must cite code commits, test artifacts, and raw benchmark receipts.

Public claim rules:

- Use only labeled Q99-State, Q99-Input, or Q99-Total claims.
- Report provider cache hits and exact reasoning continuation separately.
- Keep implemented, formally attested, and measured statements separate.
- Do not state a measured improvement before its result table and receipt exist.
- Keep conjectures outside release and runtime authorization.

Run the gate from the repository root (through RCH on this project):

```bash
rch exec -- make -C docs/papers verify
```
