# Performance evidence promotion

Profiling and benchmark runs begin under the ignored `tests/artifacts/perf/<run-id>/` scratch tree. Evidence becomes public only through an explicit promotion into `docs/evidence/perf/<run-id>/`.

## Minimum promoted package

| Artifact | Requirement |
| --- | --- |
| `fingerprint.json` | Commit, binary digest, profile, toolchain, host, and command |
| scenario description | Corpus, setup, operation, and measurement point |
| one profile artifact | Compact `cpu.json`, flamegraph, or cited text sample |
| hotspot table | Frames and attribution used by the interpretation |
| interpretation | What was observed, what changed, and what is not claimed |

Large raw sample sets and Instruments trace bundles stay local unless the owner approves their size and public value.

## Promotion rules

1. Finish the scenario and interpretation in the scratch run.
2. Select the smallest artifact that supports the cited hotspot.
3. Copy that evidence into a new tracked run directory.
4. Remove absolute host paths and secrets.
5. Verify every relative link from a fresh clone.
6. Cite the promoted package from the benchmark or performance document that uses it.

A directory containing only pointers to ignored local files is not promoted evidence and should not be committed.
