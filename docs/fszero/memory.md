# FSZero durable memory

FSZero exposes path-shaped durable memory for standalone operator workflows. Entries use a stable `mem://` path and an exact `fz://blob/<digest>` body.

| Path | Role |
| --- | --- |
| `system/*` | Durable constraints |
| `facts/*` | Verified project facts |
| `episodic/*` | Session or incident summaries |
| `scratch/*` | Short-lived working material |

List paths before reading bodies. A ref expands only when the process can reach a compatible store and verify the digest.

## ZeroKernel boundary

`z.state` carries a small JSON fact between fresh frames. FSZero memory stores larger path-shaped content across sessions. Repository authority still belongs in versioned files. ZeroKernel does not expose an engine-local memory catalog.

## Privacy

Memory may contain sensitive project information. Protect the store with suitable filesystem permissions and retention policy. Shareable usage telemetry does not include memory bodies.
