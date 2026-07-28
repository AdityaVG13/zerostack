# Post-RACC complexity hotspots -- 2026-07-28

Status: dirty working tree after the intentional wave edits. Measurements use lizard 1.23.0 with a cyclomatic-complexity threshold of 10 across the complete `crates`, `conformance`, and `scripts` roots, including new files and functions.

## Result

| Metric | Before | After |
| --- | ---: | ---: |
| Functions | 965 | 1,063 |
| Functions above CC 10 | 17 | 0 |
| Maximum CC | 31 | 10 |
| Total CC | 2,389 | 2,510 |
| NLOC | 12,564 | 13,209 |
| CC/function | 2.475648 | 2.361242 |
| CC/NLOC | 0.190146 | 0.190022 |

The hotspot result is **17 -> 0**, and maximum per-function CC is **31 -> 10**. No ΣCC reduction claim is made: the RACC expansion increased the measured inventory. The normalized metrics are reported to keep that inventory change visible.

## Root measurements

| Root | Functions | Above 10 | Max | Median | Total CC | Mean | NLOC |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| crates before | 650 | 12 | 31 | 1 | 1,427 | 2.20 | 8,145 |
| crates after | 718 | 0 | 10 | 1 | 1,517 | 2.11 | 8,649 |
| conformance before | 255 | 4 | 31 | 2 | 735 | 2.88 | 3,838 |
| conformance after | 272 | 0 | 10 | 2 | 753 | 2.77 | 3,945 |
| scripts before | 60 | 1 | 27 | 3 | 227 | 3.78 | 581 |
| scripts after | 73 | 0 | 10 | 3 | 240 | 3.29 | 615 |

## Before hotspots

- dispatch: `validate_schema_value` 31; `acquire_authority` 14
- zero-cert: `verify_completeness` 29; `verify` 17; `verify_search` 15
- zero-ledger: `charge` 15; zero-gate: `decide` 14
- zero-store cas: `publish_new_object_via_temp_with_sequence` 14
- machine-permit: `parse_identity` 13; `classify_waiter_entry` 12
- zero-store metadata: `observation_metadata` 12; `append_observation` 11
- conformance freshness: `validate` 31
- conformance racc: `check_task_transaction` 26; `check_release_aggregate` 18
- conformance schema_pairs: `resolve` 14
- scripts feature weights: `validate` 27

## Reproduce

Run once for each root (`crates`, `conformance`, and `scripts`):

```sh
uv run --python 3.12 python <cyclomatic-reduction-skill>/scripts/measure_complexity.py ROOT --threshold 10
```

The prior `thvg` value 1,071 is invalid because RACC expanded the inventory; these whole-root measurements supersede it.
