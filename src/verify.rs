use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{
    graph::{EdgeKind, SemanticGraph},
    model::ModelFinding,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    ConfirmedGraphPath,
    Infeasible,
    Unknown,
    InvalidModelOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub status: VerificationStatus,
    pub path_exists: bool,
    pub feasibility: Feasibility,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Feasibility {
    Confirmed,
    Infeasible,
    Unknown,
}

pub trait FindingVerifier {
    fn verify(&self, graph: &SemanticGraph, finding: &ModelFinding) -> VerificationResult;
}

#[derive(Debug, Default)]
pub struct GraphPathVerifier;

impl FindingVerifier for GraphPathVerifier {
    fn verify(&self, graph: &SemanticGraph, finding: &ModelFinding) -> VerificationResult {
        if finding.node_path.is_empty() {
            return invalid("model returned an empty node path");
        }
        if let Some(missing) = finding.node_path.iter().find(|id| graph.node(id).is_none()) {
            return invalid(&format!("model referenced unknown node {missing}"));
        }
        let edges: HashSet<_> = graph
            .edges()
            .filter(|e| e.kind != EdgeKind::DataDependency)
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        for pair in finding.node_path.windows(2) {
            if !edges.contains(&(pair[0].as_str(), pair[1].as_str())) {
                return invalid(&format!("no control edge {} -> {}", pair[0], pair[1]));
            }
        }
        let feasibility = simple_feasibility(graph, &finding.node_path);
        let status = if feasibility == Feasibility::Infeasible {
            VerificationStatus::Infeasible
        } else {
            VerificationStatus::ConfirmedGraphPath
        };
        VerificationResult { status, path_exists: true, feasibility, explanation: "all nodes and consecutive control-flow edges exist; deeper path feasibility is not proven".into() }
    }
}

fn invalid(reason: &str) -> VerificationResult {
    VerificationResult {
        status: VerificationStatus::InvalidModelOutput,
        path_exists: false,
        feasibility: Feasibility::Unknown,
        explanation: reason.into(),
    }
}

fn simple_feasibility(graph: &SemanticGraph, path: &[String]) -> Feasibility {
    let mut branch_choices: HashMap<&str, &str> = HashMap::new();
    for pair in path.windows(2) {
        if let Some(edge) = graph.edges().find(|e| e.from == pair[0] && e.to == pair[1])
            && matches!(
                edge.kind,
                EdgeKind::TrueBranch | EdgeKind::FalseBranch | EdgeKind::SwitchCase
            )
            && let Some(previous) = branch_choices.insert(&pair[0], &pair[1])
            && previous != pair[1]
        {
            return Feasibility::Infeasible;
        }
    }
    Feasibility::Unknown
}

pub fn shortest_path(
    graph: &SemanticGraph,
    start: &str,
    goal: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    let mut queue = VecDeque::from([start.to_owned()]);
    let mut parent: HashMap<String, Option<String>> = HashMap::from([(start.to_owned(), None)]);
    while let Some(current) = queue.pop_front() {
        if current != start && goal(&current) {
            let mut path = vec![current.clone()];
            let mut cursor = current;
            while let Some(Some(p)) = parent.get(&cursor) {
                path.push(p.clone());
                cursor = p.clone();
            }
            path.reverse();
            return Some(path);
        }
        for edge in graph.edges().filter(|e| {
            e.from == current && e.kind != EdgeKind::DataDependency && e.kind != EdgeKind::Call
        }) {
            if !parent.contains_key(&edge.to) {
                parent.insert(edge.to.clone(), Some(current.clone()));
                queue.push_back(edge.to.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graph::{Edge, SemanticFunction},
        model::Detector,
    };
    use std::collections::BTreeMap;

    fn finding(path: Vec<&str>) -> ModelFinding {
        ModelFinding {
            id: "x".into(),
            title: "x".into(),
            confidence: 1.0,
            node_path: path.into_iter().map(Into::into).collect(),
            suspected_invariant: "x".into(),
            reason: "x".into(),
            category: None,
            detector: Detector::Ai,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_nonexistent_edge() {
        let graph = SemanticGraph {
            schema_version: 1,
            crate_name: "x".into(),
            functions: vec![SemanticFunction {
                name: "f".into(),
                nodes: vec![],
                edges: vec![Edge {
                    from: "a".into(),
                    to: "b".into(),
                    kind: EdgeKind::ControlFlow,
                    label: None,
                    metadata: BTreeMap::new(),
                }],
                context: BTreeMap::new(),
            }],
            interprocedural_edges: vec![],
        };
        assert_eq!(
            GraphPathVerifier
                .verify(&graph, &finding(vec!["a", "c"]))
                .status,
            VerificationStatus::InvalidModelOutput
        );
    }
}
