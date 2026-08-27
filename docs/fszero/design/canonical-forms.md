# Canonical forms (fszero-i5px)

Artifact classes:

| Class | Rules |
|---|---|
| repo-map | `files[]` sorted by path; each `{path,digest}` |
| orient-pack | JSON object keys sorted; string whitespace normalized |
| search-results | hits sorted by (path,start,end); query trimmed |

Metamorphic: order/whitespace variants share one content digest.
No false collapse: different digests/paths stay distinct.
GTNH 23k host measurement is out of scope for this unit; synthetic dedup mass is unit-tested.
