# Real-task evaluation methodology

This directory is the committed measuring stick for graph-guided reading-set usefulness on completed GraphZero change tasks.

Integrity rules:
- Every task row in tasks.jsonl is counted; the runner has no skip list.
- Input paths and globs are committed before report generation.
- Byte counts come from actual files in the checkout at runtime.
- Success means the policy read set contains every success_files path for that task.
- Losses are published: per-row savings can be below the Northstar 5-10x target.

Policies:
- unguided: broad repo/module candidate files expanded from committed globs, representing a search-first agent reading the implementation/test/docs neighborhood.
- graph_guided: the committed reading-set closure for the same target symbol and task.

Reproduce with:

    python3 benchmarks/real_task_eval/run.py --check

The current aggregate report is in report.json and is intentionally reproducible byte-for-byte by the runner.
