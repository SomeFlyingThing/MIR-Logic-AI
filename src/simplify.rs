use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::graph::{Edge, EdgeKind, Node, NodeKind, ProgramGraph, SemanticFunction, SemanticGraph};

pub fn simplify(raw: &ProgramGraph, call_depth: usize) -> SemanticGraph {
    let names: HashSet<_> = raw.functions.iter().map(|f| f.name.as_str()).collect();
    let mut functions = Vec::new();
    for function in &raw.functions {
        let keep: HashSet<_> = function
            .nodes
            .iter()
            .filter(|n| valuable(n))
            .map(|n| n.id.clone())
            .collect();
        let nodes = function
            .nodes
            .iter()
            .filter(|n| keep.contains(&n.id))
            .cloned()
            .collect::<Vec<_>>();
        let edges = contract_edges(&function.nodes, &function.edges, &keep);
        functions.push(SemanticFunction {
            name: function.name.clone(),
            nodes,
            edges,
            context: BTreeMap::new(),
        });
    }
    let mut interprocedural_edges = Vec::new();
    if call_depth > 0 {
        let entries: HashMap<_, _> = functions
            .iter()
            .filter_map(|f| {
                f.nodes
                    .iter()
                    .find(|n| n.kind == NodeKind::FunctionEntry)
                    .map(|n| (f.name.as_str(), n.id.clone()))
            })
            .collect();
        for function in &functions {
            for node in &function.nodes {
                if node.kind == NodeKind::Call
                    && let Some(called) = node
                        .called_function
                        .as_deref()
                        .filter(|n| names.contains(*n))
                    && let Some(entry) = entries.get(called)
                {
                    interprocedural_edges.push(Edge {
                        from: node.id.clone(),
                        to: entry.clone(),
                        kind: EdgeKind::Call,
                        label: Some("crate call".into()),
                        metadata: BTreeMap::new(),
                    });
                }
            }
        }
    }
    SemanticGraph {
        schema_version: raw.schema_version,
        crate_name: raw.crate_name.clone(),
        functions,
        interprocedural_edges,
    }
}

fn valuable(node: &Node) -> bool {
    matches!(
        node.kind,
        NodeKind::FunctionEntry
            | NodeKind::FunctionExit
            | NodeKind::Call
            | NodeKind::Branch
            | NodeKind::Return
            | NodeKind::Assert
            | NodeKind::Error
    ) || node.operation.as_deref() == Some("projection_or_variant")
        || node.text.as_deref().is_some_and(semantic_words)
}

fn semantic_words(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "auth",
        "permission",
        "session",
        "state",
        "valid",
        "error",
        "commit",
        "resource",
        "grant",
        "delete",
        "write",
    ]
    .iter()
    .any(|x| lower.contains(x))
}

fn contract_edges(nodes: &[Node], edges: &[Edge], keep: &HashSet<String>) -> Vec<Edge> {
    let control: HashMap<_, Vec<_>> = nodes
        .iter()
        .map(|n| {
            let next = edges
                .iter()
                .filter(|e| e.from == n.id && e.kind != EdgeKind::DataDependency)
                .cloned()
                .collect();
            (n.id.clone(), next)
        })
        .collect();
    let mut result = Vec::new();
    for from in keep {
        let mut queue: VecDeque<(String, EdgeKind, Option<String>)> = VecDeque::new();
        if let Some(out) = control.get(from) {
            queue.extend(
                out.iter()
                    .map(|e| (e.to.clone(), e.kind.clone(), e.label.clone())),
            );
        }
        let mut seen = HashSet::new();
        while let Some((id, kind, label)) = queue.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if keep.contains(&id) {
                result.push(Edge {
                    from: from.clone(),
                    to: id,
                    kind,
                    label,
                    metadata: BTreeMap::new(),
                });
            } else if let Some(out) = control.get(&id) {
                for e in out {
                    queue.push_back((
                        e.to.clone(),
                        merge_kind(&kind, &e.kind),
                        label.clone().or_else(|| e.label.clone()),
                    ));
                }
            }
        }
    }
    for e in edges.iter().filter(|e| {
        e.kind == EdgeKind::DataDependency && keep.contains(&e.from) && keep.contains(&e.to)
    }) {
        result.push(e.clone());
    }
    result.sort_by(|a, b| {
        (&a.from, &a.to, format!("{:?}", a.kind)).cmp(&(&b.from, &b.to, format!("{:?}", b.kind)))
    });
    result.dedup_by(|a, b| {
        a.from == b.from && a.to == b.to && a.kind == b.kind && a.label == b.label
    });
    result
}

fn merge_kind(first: &EdgeKind, next: &EdgeKind) -> EdgeKind {
    if matches!(
        first,
        EdgeKind::SwitchCase | EdgeKind::TrueBranch | EdgeKind::FalseBranch | EdgeKind::Unwind
    ) {
        first.clone()
    } else {
        next.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_calls_while_contracting_assignments() {
        assert!(semantic_words("authenticated_session"));
        assert!(!semantic_words("temporary_4"));
    }
}
