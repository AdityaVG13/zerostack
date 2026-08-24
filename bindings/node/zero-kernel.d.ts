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
}

export interface WriteOptions {
  expectedPreimage?: string;
}

export interface EditOptions extends WriteOptions {
  startLine?: number;
  endLine?: number;
}

export interface RemoveOptions extends WriteOptions {}

export interface AstPatch {
  language: string;
  pattern: string;
  replacement: string;
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

export interface ProjectOptions {
  visibleBytes?: number;
  mediaType?: string;
}

export interface CompressionOptions {
  maxTokens?: number;
  mode?: "auto" | "passthrough" | "diagnostic" | "structured" | "dedupe" | "diff-aware" | "exact" | "lossy";
  label?: string;
  mediaType?: string;
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
  billed: number;
  visible: number;
  cached: number;
  certified: boolean;
}

export interface ProjectionResult {
  visible: string;
  exact?: string;
  accounting: TokenAccounting;
}

export interface CompressionResult {
  visible: string;
  exact: string;
  truncated: boolean;
  omittedTokens: number;
  accounting: TokenAccounting;
}

export interface ShellResult {
  status: number;
  stdout: string;
  stderr: string;
  exact?: string;
  accounting: TokenAccounting;
}

export interface ZeroKernelState {
  get<T>(key: string): Promise<T | null>;
  set<T>(key: string, value: T): Promise<void>;
  has(key: string): Promise<boolean>;
  delete(key: string): Promise<boolean>;
  list(): Promise<string[]>;
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

export interface ZeroKernelStatus {
  runtime: "ZeroKernel";
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
  status(): ZeroKernelStatus;
  shutdown(): Promise<void>;
}
