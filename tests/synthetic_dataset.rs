use std::collections::HashMap;

use mir_logic::{
    benchmark::{self, BenchmarkConfig, BenchmarkModel, InputMode},
    generator::{GenerateConfig, generate_dataset, load_records},
};

#[test]
fn generated_dataset_compiles_pairs_splits_and_benchmarks() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("dataset");
    let manifest = generate_dataset(&GenerateConfig {
        count: 12,
        seed: 12_345,
        output: output.clone(),
        split: true,
        batch_size: 12,
        call_depth: 5,
        run_invariant_tests: true,
        keep_workspaces: false,
        overwrite: false,
    })
    .unwrap();
    assert_eq!(manifest.statistics.pair_count, 12);
    assert_eq!(manifest.statistics.record_count, 24);
    assert_eq!(manifest.statistics.compilation_rejections, 0);
    assert_eq!(manifest.statistics.generation_failures, 0);
    assert!(manifest.statistics.by_domain.len() >= 5);

    let mut records = Vec::new();
    for name in ["train.jsonl", "validation.jsonl", "test.jsonl"] {
        records.extend(load_records(&output.join(name)).unwrap());
    }
    assert_eq!(records.len(), 24);
    let mut pair_splits = HashMap::new();
    for record in &records {
        if let Some(previous) = pair_splits.insert(record.pair_id.clone(), record.split) {
            assert_eq!(previous, record.split, "a pair leaked across splits");
        }
        assert!(record.generation.compiled);
        assert!(record.generation.invariant_tested);
        assert!(!record.graph.functions.is_empty());
    }
    assert!(records.iter().any(|record| {
        !record.graph_delta.added_nodes.is_empty()
            || !record.graph_delta.removed_nodes.is_empty()
            || !record.graph_delta.added_edges.is_empty()
            || !record.graph_delta.removed_edges.is_empty()
            || !record.graph_delta.changed_features.is_empty()
    }));
    assert!(
        records
            .iter()
            .filter_map(|record| record.mutation.as_ref())
            .all(|mutation| {
                !mutation.expected_bad_path.is_empty()
                    && !mutation.resolved_consequence_nodes.is_empty()
            })
    );

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let report = runtime
        .block_on(benchmark::run(&BenchmarkConfig {
            dataset: output.join("test.jsonl"),
            model: BenchmarkModel::Heuristic,
            input_mode: InputMode::SemanticGraphOnly,
            ablations: vec![],
            cache_dir: output.join("cache"),
            limit: None,
            temperature: 0.0,
        }))
        .unwrap();
    assert_eq!(
        report.metrics.total,
        load_records(&output.join("test.jsonl")).unwrap().len()
    );
    assert!(output.join("manifest.json").exists());
    assert!(output.join("challenges/identifier_blind.jsonl").exists());
}
