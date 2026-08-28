export type FindMode =
  | "natural"
  | "pattern"
  | "word"
  | "literal"
  | "regex"
  | "imports"
  | "defs"
  | "symbols"
  | "definition"
  | "references"
  | "callers"
  | "callees"
  | "call_path"
  | "call-path"
  | "semantic";

export interface ReadOptions {
  range?: string;
  maxBytes?: number;
  recursive?: boolean;
  limit?: number;
  offset?: number;
}

export interface EditOptions {
  expectedPreimage?: string;
  startLine?: number;
  endLine?: number;
}

export type EditPatch =
  | { find: string; replacement: string }
  | { create: string }
  | { remove: true }
  | { kind: "replace_file"; content: string };

export type ApplyOperation =
  | { path: string; edit: { find: string; replacement: string } }
  | { path: string; create: string }
  | { path: string; replace: string }
  | { path: string; remove: true }
  | { path: string; before: string; content: string }
  | { path: string; after: string; content: string };

export interface ApplyResult {
  schema: "zerostack.effect";
  outcome: "staged";
  changedFiles: number;
  delta: string;
}

export interface FindOptions {
  mode?: FindMode;
  path?: string;
  language?: string;
  source?: string;
  sink?: string;
  limit?: number;
  budgetTokens?: number;
}

export interface LookupOptions {
  filter?: string;
  limit?: number;
  recursive?: boolean;
}

export interface ShellOptions {
  cwd?: string;
  timeoutMs?: number;
  stdin?: string;
  env?: Record<string, string>;
  maxVisibleBytes?: number;
}

export interface ExpandOptions {
  offset?: number;
  limit?: number;
  lineStart?: number;
  lineEnd?: number;
  symbol?: string;
}

export interface ReadSnapshotRequest {
  path?: string;
  target?: Record<string, unknown>;
  cardinality?: "exactly_one";
  selection?: Record<string, unknown>;
  view?: Record<string, unknown>;
}

export interface ReadSnapshot {
  schema: "zerostack.snap.workspace";
  path: string;
  source: { exact: string; [key: string]: unknown };
  recovery: { exact: string; [key: string]: unknown };
  [key: string]: unknown;
}

export interface ExpandResult {
  schema: "zerostack.expand";
  source: string;
  text?: string;
  bytes?: string;
  encoding: string;
  byteStart: number;
  byteEnd: number;
  byteLength: number;
  complete: boolean;
  next?: number;
  accounting: TokenAccounting;
}

export interface FileEffectReceipt {
  path: string;
  before?: string;
  after?: string;
  journal: string;
}

export interface FindHit {
  path: string;
  symbol?: string;
  lineStart?: number;
  lineEnd?: number;
  preview?: string;
  evidence?: string;
  score: number;
}

export interface StructuralCoverage {
  tierAPct: number;
  tierBPct: number;
  tierCPct: number;
  freshnessVerified: boolean;
  snapshotId: number;
}

export interface StructuralAbsence {
  class: "verified_empty" | "unknown" | "stale_index" | "low_coverage";
  reason: string;
  coverage?: StructuralCoverage;
  suggestion: string;
}

export interface StructuralBudget {
  requested: number;
  used: number;
  actualUsed: number;
  remaining: number;
  exceeded: boolean;
  truncated: boolean;
}

export interface FindResult {
  hits: FindHit[];
  indexDigest: string;
  complete: boolean;
  coverage?: StructuralCoverage;
  absence?: StructuralAbsence;
  budget?: StructuralBudget;
  diagnostic?: string;
  continuation?: string;
}

export interface TokenAccounting {
  tokenizer: string;
  sourceTokens: number;
  visibleTokens: number;
  recoveredTokens: number;
  certified: boolean;
}

export interface ShellResult {
  status: number;
  stdout: string;
  stderr: string;
  exact?: string;
  accounting: TokenAccounting;
}

export interface ZeroKernelState {
  get<T>(key: string): T | undefined;
  set<T>(key: string, value: T): void;
  has(key: string): boolean;
  delete(key: string): boolean;
  list(): string[];
}


export interface ZeroKernelSurface {
  read(
    target: string | ReadSnapshotRequest | ReadSnapshot,
    options?: ReadOptions | LookupOptions | ExpandOptions | Record<string, unknown>,
  ): Promise<string | string[] | ReadSnapshot | ExpandResult>;
  find(query: string | ({ query: string } & FindOptions), options?: FindOptions): Promise<FindResult>;
  edit(path: string | ReadSnapshot, patch: EditPatch, options?: EditOptions): Promise<FileEffectReceipt>;
  apply(operations: ApplyOperation[] | Record<string, unknown>): Promise<ApplyResult>;
  run(command: string | string[], options?: ShellOptions): Promise<ShellResult>;
  readonly state: ZeroKernelState;
}

export interface ZeroKernelOptions {
  root: string;
  sessionId?: string;
  stateRoot?: string;
  tokenizerModel?: string;
  wallMs?: number;
  cpuMs?: number;
  memoryBytes?: number;
  callLimit?: number;
  taskLimit?: number;
  outputByteLimit?: number;
}

export interface ZeroKernelResponse {
  protocol: "ZeroKernel";
  outcome: "Completed" | "Cancelled" | "Failed";
  value?: unknown;
  error?: { kind: string; detail: string; retryable: boolean };
  handles: string[];
  event: string;
  state: { before?: string; after?: string; unchanged: boolean };
  ledger: {
    wallNs: number;
    cpuNsUpperBound: number;
    calls: number;
    tasks: number;
    bytesRead: number;
    bytesWritten: number;
    bytesVisible: number;
  };
}

export interface ProviderUsagePublication {
  kernel_event: string;
  request_id: string;
  observation: string;
  observation_digest: string;
}

export interface ZeroKernelStatus {
  runtime: string;
  ready: boolean;
  terminated: boolean;
  inflight: number;
  completed: number;
  liveFrames: number;
  liveTasks: number;
  liveProcesses: number;
}

export declare class ZeroKernel {
  constructor(options: ZeroKernelOptions);
  initialize(): Promise<void>;
  executeCell(source: string, signal?: AbortSignal): Promise<ZeroKernelResponse>;
  recordProviderUsage(event: string, observationJson: string): Promise<ProviderUsagePublication>;
  status(): ZeroKernelStatus;
  shutdown(): Promise<void>;
}
