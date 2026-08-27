#!/usr/bin/env python3
"""Regenerate the honest head-to-head bake-off report from committed artifacts."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORPUS_INPUT_PATHS = (
    'benchmarks/latency/results.json',
    'benchmarks/impact_bakeoff/report.json',
    'benchmarks/gold/edges.jsonl',
)


def _hash_field(digest, label, data):
    label_bytes = label.encode('utf-8')
    digest.update(len(label_bytes).to_bytes(8, 'big'))
    digest.update(label_bytes)
    digest.update(len(data).to_bytes(8, 'big'))
    digest.update(data)


def workspace_version(root=ROOT):
    lines = (root / 'Cargo.toml').read_text().splitlines()
    section_start = lines.index('[workspace.package]')
    version_line = next(
        line for line in lines[section_start + 1:] if line.strip().startswith('version =')
    )
    return version_line.split('=', 1)[1].strip().strip('"')


def input_fingerprint(report, root=ROOT):
    """Return the SHA-256 binding a report to every declared bake-off input."""
    binding = report['input_binding']
    digest = hashlib.sha256()
    generator_path = report['generated_by']
    _hash_field(digest, 'generator_path', generator_path.encode('utf-8'))
    _hash_field(digest, 'generator_bytes', (root / generator_path).read_bytes())
    for relative in binding['corpus_paths']:
        _hash_field(digest, 'corpus_path', relative.encode('utf-8'))
        _hash_field(digest, 'corpus_bytes', (root / relative).read_bytes())
    versions = binding['competitor_version_identifiers']
    recorded_versions = [
        {'name': competitor['name'], 'version_identifier': competitor['version_identifier']}
        for competitor in report['competitors']
    ]
    if versions != recorded_versions:
        raise ValueError('input binding competitor versions do not match report competitors')
    _hash_field(
        digest,
        'competitor_version_identifiers',
        json.dumps(versions, sort_keys=True, separators=(',', ':')).encode('utf-8'),
    )
    return digest.hexdigest()
lat = json.loads((ROOT / 'benchmarks/latency/results.json').read_text())
impact = json.loads((ROOT / 'benchmarks/impact_bakeoff/report.json').read_text())
structural = next(c for c in impact['competitors'] if c['name'] == 'graphzero_structural')
tasks = ['fresh_index_cost', 'warm_index_cost', 'call_graph_correctness', 'tokens_per_navigation_task', 'staleness_time_to_correct_answer']
report = {
    'schema_version': 1,
    'generated_by': 'benchmarks/head_to_head_bakeoff/run.py',
    'corpus': {'name': 'graphzero', 'rust_files': lat['corpus']['rust_files'], 'ground_truth': 'benchmarks/gold/edges.jsonl'},
    'integrity': {'no_rows_dropped': True, 'baseline_conditions_identical': True, 'unavailable_competitors_are_not_scored_as_measured': True, 'byte_exact_recovery_scored': True},
    'tasks': tasks,
    'sample_accounting': {
        'total_samples': len(tasks),
        'dropped_count': 0,
        'losses': structural['losses'],
    },
    'competitors': [
        {'name': 'graphzero', 'class': 'ref_first_graph', 'version_identifier': f'workspace:{workspace_version()}', 'availability': 'measured', 'fresh_index_ms': lat['cold_index']['cold_index_s'] * 1000.0, 'warm_index_ms': lat['warm_reindex']['warm_reindex_s'] * 1000.0, 'orient_p50_ms': lat['orient']['orient_symbol_p50_ms'], 'blast_p50_ms': lat['blast']['blast_p50_ms'], 'correctness': {'true_edges': structural['true_edges'], 'confirmed_non_edges': structural['confirmed_non_edges'], 'true_positives': structural['true_positives'], 'false_negatives': structural['false_negatives'], 'false_positives': structural['false_positives']}, 'tokens_per_navigation_task': {'body': 'g:/q: compact refs', 'lossless_recovery': True}, 'staleness': {'status': 'measured_elsewhere', 'artifact': 'benchmarks/rebaseline/latest.json'}, 'losses': structural['losses']},
        {'name': 'stack_graphs_class', 'class': 'stack-graphs', 'version_identifier': 'not-run:no-local-cli', 'availability': 'not_run_no_local_cli', 'fresh_index_ms': None, 'warm_index_ms': None, 'orient_p50_ms': None, 'blast_p50_ms': None, 'correctness': None, 'tokens_per_navigation_task': None, 'staleness': None, 'losses': ['not scored as measured; no local executable/corpus adapter was invoked']},
        {'name': 'scip_class', 'class': 'SCIP index consumers', 'version_identifier': 'not-run:adapter-only', 'availability': 'adapter_present_not_full_competitor', 'fresh_index_ms': None, 'warm_index_ms': None, 'orient_p50_ms': None, 'blast_p50_ms': None, 'correctness': None, 'tokens_per_navigation_task': None, 'staleness': None, 'losses': ['GraphZero can ingest SCIP, but this bake-off did not run a separate SCIP competitor under identical conditions']},
        {'name': 'repomap_class', 'class': 'repo map / summary graph tools', 'version_identifier': 'not-run:no-byte-exact-contract', 'availability': 'not_run_no_byte_exact_recovery_contract', 'fresh_index_ms': None, 'warm_index_ms': None, 'orient_p50_ms': None, 'blast_p50_ms': None, 'correctness': None, 'tokens_per_navigation_task': None, 'staleness': None, 'losses': ['not scored as measured; byte-exact recovery equivalence was unavailable']},
    ],
    'conclusion': 'Only GraphZero is measured in this repo artifact. Competitor classes are named and held out until runnable local adapters exist; this prevents fabricated head-to-head wins.',
}
report['input_binding'] = {
    'corpus_paths': list(CORPUS_INPUT_PATHS),
    'competitor_version_identifiers': [
        {
            'name': competitor['name'],
            'version_identifier': competitor['version_identifier'],
        }
        for competitor in report['competitors']
    ],
}
report['input_fingerprint'] = {
    'algorithm': 'sha256',
    'value': input_fingerprint(report),
}

if __name__ == '__main__':
    path = ROOT / 'benchmarks/head_to_head_bakeoff/report.json'
    path.write_text(json.dumps(report, indent=2) + '\n')
    print(path)
