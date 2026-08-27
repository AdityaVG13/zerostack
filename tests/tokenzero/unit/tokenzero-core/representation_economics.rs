use tokenzero_core::representation_economics::*;

fn resources(value: u64) -> RepresentationResources {
    RepresentationResources {
        stored_bytes: value,
        wire_bytes: value,
        source_tokens: value,
        visible_tokens: value,
        expansion_work: value,
        verification_work: value,
        latency_micros: value,
        metadata_bytes: value,
    }
}

#[test]
fn dominated_representation_is_removed() {
    let records = vec![
        RepresentationRecord {
            representation_root: "a".into(),
            semantic_root: "s".into(),
            adapter_root: "m".into(),
            kind: RepresentationKind::RawBytes,
            exact: true,
            resources: resources(1),
        },
        RepresentationRecord {
            representation_root: "b".into(),
            semantic_root: "s".into(),
            adapter_root: "m".into(),
            kind: RepresentationKind::CompressedBytes,
            exact: true,
            resources: resources(2),
        },
    ];
    assert_eq!(pareto_frontier(&records), vec![records[0].clone()]);
}

#[test]
fn segmentation_uses_minimum_additive_path() {
    let candidates = vec![
        SegmentCandidate {
            start: 0,
            end: 4,
            additive_cost: 10,
            boundary_kind: "whole".into(),
        },
        SegmentCandidate {
            start: 0,
            end: 2,
            additive_cost: 2,
            boundary_kind: "token".into(),
        },
        SegmentCandidate {
            start: 2,
            end: 4,
            additive_cost: 3,
            boundary_kind: "token".into(),
        },
    ];
    let plan = optimal_segmentation(4, &candidates).unwrap();
    assert_eq!(plan.total_cost, 5);
    assert_eq!(plan.segments.len(), 2);
}

#[test]
fn multi_tokenizer_plan_prices_every_adapter() {
    let candidate = |start, end, base, a, b| MultiTokenizerSegmentCandidate {
        segment: SegmentCandidate {
            start,
            end,
            additive_cost: base,
            boundary_kind: "token".into(),
        },
        tokenizer_costs: vec![
            TokenizerCost {
                tokenizer_root: "a".into(),
                tokens: a,
            },
            TokenizerCost {
                tokenizer_root: "b".into(),
                tokens: b,
            },
        ],
    };
    let candidates = vec![
        candidate(0, 2, 0, 9, 9),
        candidate(0, 1, 0, 2, 2),
        candidate(1, 2, 0, 2, 2),
    ];
    let plan = optimal_multi_tokenizer_segmentation(
        2,
        &candidates,
        &[
            TokenizerWeight {
                tokenizer_root: "a".into(),
                weight_ppm: 500_000,
            },
            TokenizerWeight {
                tokenizer_root: "b".into(),
                weight_ppm: 500_000,
            },
        ],
    )
    .unwrap();
    assert_eq!(plan.total_cost, 4);
    assert_eq!(plan.segments.len(), 2);
}

#[test]
fn decision_surface_is_canonical_and_frontier_only() {
    let records = vec![
        RepresentationRecord {
            representation_root: "a".into(),
            semantic_root: "s".into(),
            adapter_root: "m".into(),
            kind: RepresentationKind::RawBytes,
            exact: true,
            resources: resources(1),
        },
        RepresentationRecord {
            representation_root: "b".into(),
            semantic_root: "s".into(),
            adapter_root: "m".into(),
            kind: RepresentationKind::CompressedBytes,
            exact: true,
            resources: resources(2),
        },
    ];
    let rendered = render_capsule_decision_surface("capsule", &records, None).unwrap();
    assert!(rendered.json.contains("\"representationRoot\":\"a\""));
    assert!(!rendered.json.contains("\"representationRoot\":\"b\""));
    assert_eq!(
        rendered.root,
        zero_abi::sha256_hex(rendered.json.as_bytes())
    );
}
