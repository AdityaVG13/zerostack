# Real-task reading-set evaluation

This benchmark is a deterministic replay of real GraphZero changes, not a synthetic chain-call fixture. Each row in tasks.jsonl records one completed change task, the files needed for success, a broad unguided candidate set an agent would read from repo/module search (expanded from committed crate/module globs), and the graph-guided reading set produced from the target symbol.

The runner counts bytes from actual files in this checkout, estimates tokens as ceil(bytes / 4), and marks a policy successful only when its read set contains every success_files entry. All rows are counted; the script fails if a path is missing, a policy misses required files, or the published report drifts.

Reproduce:

    python3 benchmarks/real_task_eval/run.py --check

Scope limits: this is a replay benchmark over the GraphZero repo, not an LLM-in-the-loop success study across unrelated projects. It measures the byte/token advantage available to an agent that follows the committed reading-set closure for the same successful tasks.
