export type FindMode =
  | "natural"
  | "pattern"
  | "word"
  | "literal"
  | "regex"
  | "imports"
  | "defs"
  | "symbols"
  | "references"
  | "callers"
  | "callees"
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
}

export type EditPatch =
  | { find: string; replacement: string }
  | { create: string }
  | { remove: true }
  | { kind: "replace_exact"; old: string; replacement: string; expectedCount: 1 }
  | { kind: "replace_lines" | "insert_before" | "insert_after" | "replace_file"; content: string };

export type ApplyOperation =
  | { path: string; edit: { find: string; replacement: string } }
  | { path: string; create: string }
  | { path: string; replace: string }
  | { path: string; remove: true }
  | { path: string; before: string; content: string }
  | { path: string; after: string; content: string };

export interface EffectTargetRequest {
  path: string;
  expect?: "exists" | "absent";
}

export type EffectChangeRequest =
  | { target: string; kind: "replace_exact"; old: string; replacement: string; expectedCount: 1 }
  | { target: string; kind: "replace_file" | "create_file"; content: string }
  | { target: string; kind: "insert_before" | "insert_after"; content: string; anchor: { exactText: string } }
  | { target: string; kind: "remove_file" };

export interface EffectVerificationRequest {
  parse?: boolean;
  changedTargetsOnly?: boolean;
  command?: { argv: string[]; timeoutMs: number };
}

export interface EffectRequest {
  targets: Record<string, EffectTargetRequest>;
  changes: EffectChangeRequest[];
  verify?: EffectVerificationRequest;
}

export interface EffectTargetResult {
  name: string;
  path: string;
  kind: "edit" | "create" | "remove";
  before?: string;
  after?: string;
  journal: string;
}

export interface EffectVerificationResult {
  parse: string;
  command: string;
  changedTargetsOnly: boolean;
}

export interface ApplyResult {
  schema: "zerostack.effect";
  outcome: "staged";
  delta: string;
  targets: EffectTargetResult[];
  changedFiles: number;
  verification: EffectVerificationResult;
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
  bytes?: { start: number; end: number };
  lines?: { start: number; end: number };
  next?: number | string;
  all?: true;
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

export interface SnapToFileManifest {
  schema_version: string;
  root: string;
  exact_objects: Record<string, unknown>[];
  causal_graph: Record<string, unknown>;
  proof_graph: Record<string, unknown>;
  representations: Record<string, unknown>[];
  per_object_layers: Record<string, unknown>[];
  demand_scenarios: Record<string, unknown>[];
  resource_ledger: Record<string, unknown>;
  shadow_note: string;
}

export interface SnapToFileDemand {
  scenario_id: string;
  projection_atoms: string[];
  request_root: string;
}

export interface SnapToFileProtectedScope {
  scope_id: string;
  protected_atoms: string[];
  scope_root: string;
}

export interface SnapToFileNativeBaseline {
  discovery_bytes: number;
  probe_count: number;
}

export interface SnapToFileReadRequest {
  manifest: SnapToFileManifest;
  demand: SnapToFileDemand;
  scope: SnapToFileProtectedScope;
  completeness: string;
  nativeBaseline: SnapToFileNativeBaseline;
}

export interface SnapToFileReadTarget {
  snapToFile: SnapToFileReadRequest;
}

export interface SnapDecisionView {
  task_contract_root: string;
  project_root: string;
  causal_lens_root: string;
  supported_decisions: string[];
  evidence_refs: string[];
  omitted_classes: string[];
  expansion_handles: string[];
  completeness_grade: "Proved" | "BoundedComplete" | "Observed" | "Unknown";
  unresolved_question: string | null;
  baseline_escape: boolean;
  canonical_render_root: string | null;
}

export interface SnapPacketMetrics {
  visible_bytes: number;
  backend_work: number;
  retry_count: number;
  first_try_sufficiency: boolean;
  false_complete: boolean;
  certified_atoms: number;
  expanded_atoms: number;
  native_baseline_bytes: number;
  native_baseline_probes: number;
  native_savings_bytes: number;
}

export interface SnapPacket {
  schema_version: string;
  packet_version: number;
  outcome: "snapped" | "escaped" | "refused";
  family: string;
  request_root: string;
  project_root: string;
  scope_root: string;
  index_root: string;
  index_version: string;
  plan_root: string | null;
  projection_root: string | null;
  certificate_root: string | null;
  checker_identity: string | null;
  checker_version: string | null;
  handle_id: string | null;
  proved_levels: string[];
  unproved_levels: { level: string; reason: string }[];
  evidence_refs: string[];
  obligations: string[];
  atoms: { atom_root: string; byte_len: number }[];
  metrics: SnapPacketMetrics | null;
  baseline_escape: boolean;
  primary_file_orientation: boolean;
  reasons: string[];
  decision_view_root: string;
}

export interface SnapExpandPermit {
  handle_id: string;
  project_root: string;
  request_root: string;
  protected_scope_root: string;
  demand_plan_root: string;
  index_root: string;
  index_version: string;
  renderer_contract: string;
  tenant: string;
  epoch: number;
  projection_root: string;
}

export interface SnapDemandPlan {
  scenario_id: string;
  demanded_atoms: string[];
  demand_weight: number;
  projection_atoms: string[];
  plan_root: string;
  projection_root: string;
}

export interface SnapExpandLedger {
  rows: {
    class: string;
    amount: number;
    unit: string;
    measurement_source: "exact" | "estimate" | "unknown";
  }[];
}

export interface SnapFirstExpansion {
  handle_id: string;
  permit: SnapExpandPermit;
  plan: SnapDemandPlan;
  atoms: { atom_root: string; byte_len: number }[];
  projection_root: string;
  visible_bytes: number;
  certified_atoms: number;
  first_try_sufficiency: boolean;
  ledger: SnapExpandLedger;
  native_baseline: SnapToFileNativeBaseline;
  session: { handle_id: string; delta_seq: number; terminal: boolean };
}

export interface SnapSafeExpandHandle {
  abi_version: string;
  handle_version: number;
  project_root: string;
  request_root: string;
  protected_scope_root: string;
  demand_plan_root: string;
  index_root: string;
  index_version: string;
  renderer_contract: string;
  tenant: string;
  epoch: number;
  projection_root: string;
  completeness: {
    certificate_root: string;
    verdict: SafetyVerdict;
    checker_identity: string;
    checker_version: string;
    first_attempt: boolean;
  };
  issue_nonce: string;
  handle_id: string;
  issuance_mac: string;
}

export interface SnapToFileReadResult {
  schema: "zerostack.zero_kernel.snap_to_file";
  packet: SnapPacket;
  view: SnapDecisionView;
  expansion?: SnapFirstExpansion;
  handle?: SnapSafeExpandHandle;
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
  exactDigest: string;
  recoveredTokens: number;
  complete: boolean;
  next?: number;
  accounting: TokenAccounting;
}

export interface DirectoryPage {
  entries: string[];
  next: number | null;
  complete: boolean;
}

export interface FileMetadata {
  mode: number;
  modifiedUnixNs: string;
  symlinkTarget?: string;
  symlinkTargetIsDir?: boolean;
}

export interface FileEffectReceipt {
  kind: "write" | "edit" | "remove" | "restore";
  path: string;
  before?: string;
  after?: string;
  beforeMetadata?: FileMetadata;
  journal: string;
}

export interface FindHit {
  path: string;
  symbol?: string;
  lineStart?: number;
  lineEnd?: number;
  preview?: string;
  evidence?: string;
  source?: string;
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

export type SafetyVerdict =
  | "safe"
  | { unsafe: { reasons: string[] } }
  | { unknown: { reasons: string[] } };

export interface TaskLensSelector {
  capsuleRoot?: string;
  requiredSnapshot?: string;
}

export type TaskLensFindRequest = { query: string; taskLens: TaskLensSelector } & FindOptions;

export interface TaskLensCompilerImpact {
  complete: boolean;
  edgeRoots: string[];
  reverseRoots: string[];
}

export interface TaskLensResult {
  verdict: SafetyVerdict;
  locus?: FindHit;
  impact: TaskLensCompilerImpact;
  proofSupport: string[];
  evidenceRoots: string[];
  coverage?: StructuralCoverage;
  indexDigest: string;
  reasons: string[];
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
  read(target: SnapToFileReadTarget): Promise<SnapToFileReadResult>;
  read(
    target: string | ReadSnapshotRequest | ReadSnapshot,
    options?: ReadOptions | LookupOptions | ExpandOptions,
  ): Promise<string | string[] | DirectoryPage | ReadSnapshot | ExpandResult>;
  find(request: TaskLensFindRequest): Promise<TaskLensResult>;
  find(query: string | ({ query: string } & FindOptions), options?: FindOptions): Promise<FindResult>;
  edit(path: string | ReadSnapshot, patch: EditPatch, options?: EditOptions): Promise<FileEffectReceipt>;
  apply(operations: ApplyOperation[] | EffectRequest): Promise<ApplyResult>;
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

export type EngineErrorKind =
  | "invalid_input"
  | "outside_workspace"
  | "not_found"
  | "conflict"
  | "cancelled"
  | "deadline"
  | "budget"
  | "unsupported"
  | "corrupt"
  | "io"
  | "internal";

export interface ZeroOperationTrace {
  sequence: number;
  method: string;
  status: "completed" | "failed";
  capsuleRoot: string;
  occurrence: number;
  parallelGroup?: number;
  target?: string;
  detail?: string;
  resultCount?: number;
  changedFiles?: number;
  durationNs: number;
}

export interface TurnRecord {
  sequence: number;
  class: "semantic_decision" | "mechanical" | "retry_repair" | "verification" | "user_preference" | "unknown";
  operationCount: number;
  retryCount?: number;
  resourceLedgerRoot: string;
  traceRoot: string;
}

export interface ZeroKernelResponse {
  protocol: "ZeroKernel";
  outcome: "Completed" | "Cancelled" | "Failed";
  value?: string;
  error?: { kind: EngineErrorKind; detail: string; retryable: boolean };
  operations?: ZeroOperationTrace[];
  operationsTruncated?: boolean;
  handles?: string[];
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
  turn?: TurnRecord;
  effects?: FileEffectReceipt[];
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
  registerSnapToFileCompleteness(completenessJson: string): Promise<string>;
  status(): ZeroKernelStatus;
  shutdown(): Promise<void>;
}
