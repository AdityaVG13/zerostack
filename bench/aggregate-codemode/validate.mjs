#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import pathModule from "node:path";
import { fileURLToPath } from "node:url";

const path = process.argv[2];
if (!path) throw new Error("usage: validate.mjs RECEIPT.json");
const receipt = JSON.parse(readFileSync(path, "utf8"));
const requiredWorkloads = ["mixed", "ref_heavy", "shell", "mutation", "cancellation", "failure"];
const requiredModes = ["aggregate_one_cell", "standalone_per_engine", "per_operation"];

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
function lowercaseDigest(value, label) {
  assert(typeof value === "string" && /^[0-9a-f]{64}$/.test(value), `${label} must be a lowercase SHA-256 digest`);
}
function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

assert(receipt.schema_version === 1, "schema_version must be 1");
assert(receipt.schema === "zerostack.aggregate_codemode_benchmark.v1", "unexpected schema");
assert(receipt.bead_key === "zerostack-codemode-first-execution-jgm.6", "unexpected bead key");
assert(receipt.result === "passed", "benchmark result did not pass");
assert(receipt.differential_correctness?.passed === true, "differential correctness failed");
assert(receipt.differential_correctness.failures.length === 0, "differential failures are nonempty");
assert(receipt.rerun_tolerance?.passed === true, "declared rerun tolerance failed");
assert(receipt.rerun_tolerance.checks.every((check) => check.passed === true), "one rerun tolerance check failed");
assert(JSON.stringify(receipt.corpus.workloads) === JSON.stringify(requiredWorkloads), "workload corpus is incomplete");
assert(JSON.stringify(receipt.corpus.modes) === JSON.stringify(requiredModes), "mode corpus is incomplete");
lowercaseDigest(receipt.corpus.sha256, "corpus.sha256");
assert(sha256(JSON.stringify(receipt.corpus.definition)) === receipt.corpus.sha256, "corpus.sha256 does not bind corpus.definition");
lowercaseDigest(receipt.provenance.addon.sha256, "addon.sha256");
assert(receipt.provenance.addon.source_binding.startsWith("digest_only"), "binary source limitation must remain explicit");
assert(receipt.provenance.source_repositories.length === 4, "all four source repositories must be recorded");
for (const repository of receipt.provenance.source_repositories) {
  assert(typeof repository.head === "string" && /^[0-9a-f]{40}$/.test(repository.head), `${repository.name} head is not exact`);
  assert(typeof repository.dirty === "boolean", `${repository.name} dirty state missing`);
  lowercaseDigest(repository.worktree_state_sha256, `${repository.name}.worktree_state_sha256`);
}
const expectedTrials = receipt.config.repetitions * requiredWorkloads.length * requiredModes.length
  * (receipt.config.coldRuns + receipt.config.warmRuns);
assert(receipt.trials.length === expectedTrials, `expected ${expectedTrials} trials, got ${receipt.trials.length}`);
for (const trial of receipt.trials) {
  assert(requiredWorkloads.includes(trial.workload), `unknown workload ${trial.workload}`);
  assert(requiredModes.includes(trial.mode), `unknown mode ${trial.mode}`);
  assert(trial.phase === "cold" || trial.phase === "warm", `unknown phase ${trial.phase}`);
  assert(trial.differential_correct === true, `trial ${trial.sequence} failed differential correctness`);
  assert(trial.token_accounting.kind === "conservative_utf8_bytes_div_4", "token estimator changed");
  assert(trial.token_accounting.exact_model_tokens === null, "provider-free run fabricated exact model tokens");
  lowercaseDigest(trial.envelope_digest, `trial ${trial.sequence} envelope_digest`);
  assert(Array.isArray(trial.envelope_statuses) && trial.envelope_statuses.length >= 1, `trial ${trial.sequence} envelope statuses missing`);
  lowercaseDigest(trial.result_digest, `trial ${trial.sequence} result_digest`);
  lowercaseDigest(trial.fixture_identity_sha256, `trial ${trial.sequence} fixture_identity_sha256`);
  lowercaseDigest(trial.fixture_content_sha256, `trial ${trial.sequence} fixture_content_sha256`);
}
const repoRoot = pathModule.resolve(pathModule.dirname(fileURLToPath(import.meta.url)), "../..");
const addonBytes = readFileSync(pathModule.resolve(repoRoot, receipt.provenance.addon.path));
assert(sha256(addonBytes) === receipt.provenance.addon.sha256, "addon digest does not match the measured file");
const checksum = readFileSync(`${path}.sha256`, "utf8").trim();
assert(checksum === `${sha256(readFileSync(path))}  ${pathModule.basename(path)}`, "receipt checksum sidecar mismatch");
assert(receipt.accepted_optimizations.length === 0, "measurement bead must not claim an optimization");
console.log(`validated ${receipt.trials.length} raw trials across ${requiredWorkloads.length} workloads and ${requiredModes.length} modes`);
