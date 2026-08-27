# GraphZero metamorphic corpus

Each JSONL row defines one source input, one transformation, and a canonical result envelope. The corpus covers extraction, query, and store invariants without measuring performance.

Required envelope fields:
- `status`: `ok` or `err`.
- `checks`: named assertions that the executable tests must enforce.
