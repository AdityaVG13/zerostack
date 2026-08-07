import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

type ActiveCall = { delegateValue: unknown; delegateCalls: number };

type ExecuteFrame = {
  type: "execute";
  id: number;
  cellId: string;
  source: string;
  timeoutMs: number;
  delegateValue: unknown;
  expectedDelegateCalls: number;
};

type ShutdownFrame = { type: "shutdown"; id: number };
type InputFrame = ExecuteFrame | ShutdownFrame;

function write(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

async function main(): Promise<void> {
  const contextManagerPath = process.argv[2];
  const cwd = process.argv[3];
  if (!contextManagerPath || !cwd) throw new Error("usage: senpi-driver.ts <context-manager.ts> <cwd>");
  const moduleUrl = pathToFileURL(contextManagerPath).href;
  const { JavaScriptKernel } = await import(moduleUrl);
  let active: ActiveCall | undefined;
  const kernel = new JavaScriptKernel({
    sessionId: "zerostack-ymp3",
    cwd,
    parallelPoolWidth: 16,
    onMessage: (message: Record<string, unknown>) => {
      if (message.type !== "tool-call") return;
      if (!active) throw new Error("unsolicited Senpi tool call");
      const callId = message.callId;
      if (typeof callId !== "string") throw new Error("Senpi tool call omitted callId");
      active.delegateCalls += 1;
      kernel.deliverToolReply({
        type: "tool-reply",
        callId,
        ok: true,
        value: active.delegateValue,
      });
    },
  });
  write({ type: "ready", protocol: "zerostack.senpi_driver.v1" });
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    let frame: InputFrame;
    try {
      frame = JSON.parse(line) as InputFrame;
    } catch (error) {
      write({ type: "error", error: `invalid JSON: ${String(error)}` });
      continue;
    }
    if (frame.type === "shutdown") {
      const started = process.hrtime.bigint();
      await kernel.close();
      write({ type: "shutdown_ack", id: frame.id, teardownNs: Number(process.hrtime.bigint() - started) });
      lines.close();
      process.stdin.pause();
      return;
    }
    if (frame.type !== "execute" || !Number.isSafeInteger(frame.id) || typeof frame.source !== "string") {
      write({ type: "error", id: (frame as { id?: unknown }).id, error: "invalid execute frame" });
      continue;
    }
    active = { delegateValue: frame.delegateValue, delegateCalls: 0 };
    const started = process.hrtime.bigint();
    try {
      const result = await kernel.run({
        cellId: frame.cellId,
        code: frame.source,
        timeoutMs: frame.timeoutMs,
      });
      const runNs = Number(process.hrtime.bigint() - started);
      if (active.delegateCalls !== frame.expectedDelegateCalls) {
        throw new Error(`delegate count ${active.delegateCalls} != ${frame.expectedDelegateCalls}`);
      }
      write({
        type: "response",
        id: frame.id,
        ok: result.ok,
        result,
        runNs,
        delegateCalls: active.delegateCalls,
        kernelMode: kernel.mode,
      });
    } catch (error) {
      write({
        type: "response",
        id: frame.id,
        ok: false,
        error: error instanceof Error ? error.message : String(error),
        runNs: Number(process.hrtime.bigint() - started),
        delegateCalls: active.delegateCalls,
        kernelMode: kernel.mode,
      });
    } finally {
      active = undefined;
    }
  }
  await kernel.close();
}

main().catch((error: unknown) => {
  console.error(error instanceof Error ? error.stack ?? error.message : String(error));
  process.exitCode = 1;
});
