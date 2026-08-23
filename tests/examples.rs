use std::path::PathBuf;

use mir_logic::{
    ExtractOptions, MirExtractor, heuristics,
    simplify::simplify,
    verify::{FindingVerifier, GraphPathVerifier, VerificationStatus},
};

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

#[test]
fn authentication_bad_is_found_and_good_is_clean() {
    let options = ExtractOptions {
        call_depth: 3,
        keep_raw_mir: false,
        target_dir: None,
    };
    let bad = simplify(
        &MirExtractor
            .extract(&example("auth_bad_session"), &options)
            .unwrap(),
        3,
    );
    let good = simplify(
        &MirExtractor
            .extract(&example("auth_good"), &options)
            .unwrap(),
        3,
    );
    let bad_findings = heuristics::analyze(&bad);
    assert!(bad_findings.iter().any(|finding| {
        finding.category.as_deref() == Some("authentication_bypass")
            && finding.node_path.iter().any(|id| {
                bad.node(id)
                    .and_then(|node| node.called_function.as_deref())
                    == Some("create_session")
            })
    }));
    assert!(heuristics::analyze(&good).is_empty());
    assert_eq!(
        GraphPathVerifier.verify(&bad, &bad_findings[0]).status,
        VerificationStatus::ConfirmedGraphPath
    );
}

#[test]
fn all_controlled_examples_match_metadata() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(mir_logic::eval::run(&root)).unwrap();
    assert_eq!(result.heuristic.false_positives, 0);
    assert_eq!(result.heuristic.false_negatives, 0);
    assert!(result.cases.len() >= 10);
}
