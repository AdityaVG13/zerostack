export type AsgrepMode =
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

export interface AsgrepOptions {
  mode?: AsgrepMode;
  path?: string;
  language?: string;
  source?: string;
  sink?: string;
  limit?: number;
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

export interface AsgrepHit {
  path: string;
  symbol?: string;
  lineStart?: number;
  lineEnd?: number;
  preview?: string;
  evidence?: string;
  score: number;
}

export interface AsgrepResult {
  hits: AsgrepHit[];
  indexDigest: string;
  complete: boolean;
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

export interface ZeroKernelHelp {
  methods: readonly string[];
}

export interface ZeroKernelInspection {
  sessionId: string;
  stateRoot?: string;
  liveFrames: number;
  liveTasks: number;
  liveProcesses: number;
  recoveryRequired: boolean;
}

export interface ZeroKernelSurface {
  read(target: string, options?: ReadOptions): Promise<string | string[]>;
  find(query: string | ({ query: string } & AsgrepOptions), options?: AsgrepOptions): Promise<AsgrepResult>;
  edit(path: string, patch: EditPatch, options?: EditOptions): Promise<FileEffectReceipt>;
  apply(operations: ApplyOperation[]): Promise<ApplyResult>;
  run(command: string | string[], options?: ShellOptions): Promise<ShellResult>;
  readonly state: ZeroKernelState;

  /** Compatibility aliases. New plans use the six operations above. */
  write(path: string, content: string, options?: WriteOptions): Promise<FileEffectReceipt>;
  remove(path: string, options?: RemoveOptions): Promise<FileEffectReceipt>;
  transact<T>(operation: () => Promise<T>): Promise<T>;
  asgrep(query: string, options?: AsgrepOptions): Promise<AsgrepResult>;
  lookup(path?: string, options?: LookupOptions): Promise<string[]>;
  parallel<T>(operations: Array<() => Promise<T>>): Promise<T[]>;
  pipeline<T>(items: T[], ...stages: Array<(item: unknown) => Promise<unknown>>): Promise<unknown[]>;
  shell(command: string | string[], options?: ShellOptions): Promise<ShellResult>;
  measure(value: unknown): Promise<TokenAccounting>;
  project(value: unknown, options?: ProjectOptions): Promise<ProjectionResult>;
  compress(value: unknown, options?: CompressionOptions): Promise<CompressionResult>;
  expand(handle: string, options?: ExpandOptions): Promise<string>;
  help(query?: string): Promise<ZeroKernelHelp>;
  inspect(): Promise<ZeroKernelInspection>;
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
