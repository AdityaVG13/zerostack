# CodeMode and MCP mode

ZeroStack engines can run in one of two integration modes. These are alternatives, not layers.

## Standard MCP adapter

The harness registers engine tools and invokes each operation through MCP. This mode favors compatibility with clients that expect ordinary tool calls.

## CodeMode

The harness exposes a constrained JavaScript execution surface. An agent submits a plan that can call typed ZeroStack APIs sequentially or in parallel, reuse intermediate refs, and return a small final result.

~~~js
const [files, graph] = await Promise.all([
  zero.fs.compound("search", { query: "recoverable ref" }),
  zero.graph.orient("architecture", "ref flow"),
]);

return { files: files.ref, graph: graph.ref };
~~~

This Cloudflare-style execution model reduces protocol round trips and keeps intermediate values inside the sandbox rather than exposing each one to model context.

## Exclusive deployment rule

Choose exactly one:

| Deployment | Register standard engine MCP tools | Register CodeMode |
| --- | ---: | ---: |
| Standard MCP mode | Yes | No |
| CodeMode | No | Yes |

Never register both for the same deployment. Duplicate surfaces waste context, create ambiguous routing, and can split state. The active ZeroStack deployment uses CodeMode only.

CodeMode is not a fourth engine. It is an execution mode over TokenZero, FSZero, and GraphZero.
