use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::{
    graph::{Edge, EdgeKind, Node, NodeKind, SemanticGraph},
    model::{Detector, ModelFinding},
};

pub fn analyze(graph: &SemanticGraph) -> Vec<ModelFinding> {
    let mut findings = semantic_path_candidates(graph);
    findings.extend(permission_dominance(graph));
    deduplicate(findings)
}

pub(crate) fn semantic_path_candidates(graph: &SemanticGraph) -> Vec<ModelFinding> {
    let mut findings = Vec::new();
    for function in &graph.functions {
        for branch in function.nodes.iter().filter(|n| n.kind == NodeKind::Branch) {
            let prefix =
                nearest_preceding_call(function.nodes.as_slice(), &function.edges, &branch.id);
            let prefix_node = prefix
                .as_deref()
                .and_then(|id| function.nodes.iter().find(|node| node.id == id));
            for failure_edge in function
                .edges
                .iter()
                .filter(|e| e.from == branch.id && is_failure_edge(e, branch, prefix_node))
            {
                if let Some(path) = path_to_suspicious(graph, &failure_edge.to) {
                    let last = graph
                        .node(path.last().expect("nonempty path"))
                        .expect("known node");
                    let mut full_path = Vec::new();
                    if let Some(ref call) = prefix {
                        full_path = path_between(&function.edges, call, &branch.id)
                            .unwrap_or_else(|| vec![branch.id.clone()]);
                    } else {
                        full_path.push(branch.id.clone());
                    }
                    if full_path.last() != path.first() {
                        full_path.extend(path);
                    }
                    let called = last
                        .called_function
                        .as_deref()
                        .unwrap_or("success operation");
                    let category = classify_category(branch, last);
                    findings.push(make_finding(
                        format!("Failure path reaches {called}"),
                        0.84,
                        full_path,
                        invariant_for(&category, called),
                        format!("A compiler-confirmed failure/negative branch can reach `{called}` without a recognizable recovery check."),
                        category,
                    ));
                }
            }
        }
    }
    findings
}

fn path_to_suspicious(graph: &SemanticGraph, start: &str) -> Option<Vec<String>> {
    let mut queue = VecDeque::from([(start.to_owned(), vec![start.to_owned()], false)]);
    let mut seen = HashSet::new();
    while let Some((current, path, recovered)) = queue.pop_front() {
        if !seen.insert((current.clone(), recovered)) {
            continue;
        }
        let node = graph.node(&current)?;
        let called = node.called_function.as_deref().unwrap_or_default();
        let now_recovered = recovered || is_recovery(called);
        if !now_recovered && is_suspicious_success(called) {
            return Some(path);
        }
        if path.len() >= 12 || node.kind == NodeKind::Return || node.kind == NodeKind::FunctionExit
        {
            continue;
        }
        for edge in graph.edges().filter(|e| {
            e.from == current
                && e.kind != EdgeKind::DataDependency
                && e.kind != EdgeKind::Call
                && e.kind != EdgeKind::Unwind
        }) {
            let mut next_path = path.clone();
            next_path.push(edge.to.clone());
            queue.push_back((edge.to.clone(), next_path, now_recovered));
        }
    }
    None
}

fn is_failure_edge(edge: &Edge, branch: &Node, preceding_call: Option<&Node>) -> bool {
    let label = edge
        .label
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let condition = branch
        .branch_condition
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let preceding_semantics = preceding_call
        .and_then(|node| node.called_function.as_deref())
        .is_some_and(semantic_check);
    label.contains("err")
        || label.contains("none")
        || (label == "false" && (semantic_check(&condition) || preceding_semantics))
}

fn semantic_check(text: &str) -> bool {
    ["auth", "permission", "valid", "allowed", "open", "ready"]
        .iter()
        .any(|x| text.contains(x))
}
fn is_recovery(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "authenticate",
        "check_permission",
        "validate",
        "reopen",
        "rollback",
    ]
    .iter()
    .any(|x| n.contains(x))
}
fn is_suspicious_success(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "create_session",
        "grant",
        "commit",
        "sensitive",
        "delete",
        "write_protected",
        "use_resource",
        "transition_to_active",
        "publish",
    ]
    .iter()
    .any(|x| n.contains(x))
}

fn nearest_preceding_call(nodes: &[Node], edges: &[Edge], branch: &str) -> Option<String> {
    let reverse: HashMap<_, Vec<_>> = nodes
        .iter()
        .map(|n| {
            (
                n.id.as_str(),
                edges
                    .iter()
                    .filter(|e| e.to == n.id && e.kind != EdgeKind::DataDependency)
                    .map(|e| e.from.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut queue = VecDeque::from([branch]);
    let mut seen = HashSet::new();
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        if id != branch
            && let Some(node) = nodes
                .iter()
                .find(|n| n.id == id && n.kind == NodeKind::Call)
        {
            return Some(node.id.clone());
        }
        if let Some(previous) = reverse.get(id) {
            queue.extend(previous.iter().copied());
        }
    }
    None
}

fn permission_dominance(graph: &SemanticGraph) -> Vec<ModelFinding> {
    let mut out = Vec::new();
    for function in &graph.functions {
        let Some(entry) = function
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::FunctionEntry)
        else {
            continue;
        };
        let checks: HashSet<_> = function
            .nodes
            .iter()
            .filter(|n| {
                n.called_function
                    .as_deref()
                    .is_some_and(|x| x.to_ascii_lowercase().contains("permission"))
            })
            .map(|n| n.id.as_str())
            .collect();
        for sensitive in function.nodes.iter().filter(|n| {
            n.called_function
                .as_deref()
                .is_some_and(is_suspicious_success)
        }) {
            if let Some(path) = path_avoiding(&function.edges, &entry.id, &sensitive.id, &checks) {
                let has_permission_semantics = function.nodes.iter().any(|n| {
                    n.text
                        .as_deref()
                        .is_some_and(|t| t.to_ascii_lowercase().contains("permission"))
                });
                if has_permission_semantics
                    || sensitive
                        .called_function
                        .as_deref()
                        .is_some_and(|n| n.contains("sensitive"))
                {
                    out.push(make_finding("Sensitive operation is not dominated by permission check".into(), 0.76, path, "Every path to a sensitive operation should pass a successful permission check.".into(), "An alternate CFG path reaches the operation while avoiding the recognizable permission check.".into(), "permission_bypass".into()));
                }
            }
        }
    }
    out
}

fn path_avoiding(
    edges: &[Edge],
    start: &str,
    goal: &str,
    avoid: &HashSet<&str>,
) -> Option<Vec<String>> {
    let mut queue = VecDeque::from([(start.to_owned(), vec![start.to_owned()])]);
    let mut seen = HashSet::new();
    while let Some((id, path)) = queue.pop_front() {
        if id == goal {
            return Some(path);
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        for edge in edges.iter().filter(|e| {
            e.from == id && e.kind != EdgeKind::DataDependency && e.kind != EdgeKind::Unwind
        }) {
            if avoid.contains(edge.to.as_str()) {
                continue;
            }
            let mut p = path.clone();
            p.push(edge.to.clone());
            queue.push_back((edge.to.clone(), p));
        }
    }
    None
}

fn path_between(edges: &[Edge], start: &str, goal: &str) -> Option<Vec<String>> {
    let mut queue = VecDeque::from([(start.to_owned(), vec![start.to_owned()])]);
    let mut seen = HashSet::new();
    while let Some((id, path)) = queue.pop_front() {
        if id == goal {
            return Some(path);
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        for edge in edges.iter().filter(|edge| {
            edge.from == id
                && edge.kind != EdgeKind::DataDependency
                && edge.kind != EdgeKind::Unwind
        }) {
            let mut next = path.clone();
            next.push(edge.to.clone());
            queue.push_back((edge.to.clone(), next));
        }
    }
    None
}

fn classify_category(branch: &Node, last: &Node) -> String {
    let all = format!(
        "{} {} {}",
        branch.ty.as_deref().unwrap_or_default(),
        branch.variable.as_deref().unwrap_or_default(),
        last.called_function.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    if all.contains("auth") || all.contains("session") {
        "authentication_bypass".into()
    } else if all.contains("permission") || all.contains("grant") || all.contains("sensitive") {
        "permission_bypass".into()
    } else if all.contains("resource") {
        "resource_lifecycle".into()
    } else if all.contains("state") || all.contains("transition") {
        "invalid_state_transition".into()
    } else {
        "result_misuse".into()
    }
}

fn invariant_for(category: &str, called: &str) -> String {
    match category {
        "authentication_bypass" => {
            "Session creation or privileged work should require successful authentication.".into()
        }
        "permission_bypass" => {
            "Sensitive operations should require an affirmative permission check.".into()
        }
        "resource_lifecycle" => {
            "A resource should only be used after successful acquisition/opening.".into()
        }
        "invalid_state_transition" => {
            "The state transition should require a valid source state.".into()
        }
        _ => format!("`{called}` should not be reached from the error variant without recovery."),
    }
}

fn make_finding(
    title: String,
    confidence: f64,
    node_path: Vec<String>,
    suspected_invariant: String,
    reason: String,
    category: String,
) -> ModelFinding {
    let id = format!(
        "heuristic-{}-{}",
        category,
        node_path
            .last()
            .map(String::as_str)
            .unwrap_or("unknown")
            .replace(':', "-")
    );
    ModelFinding {
        id,
        title,
        confidence,
        node_path,
        suspected_invariant,
        reason,
        category: Some(category),
        detector: Detector::Heuristic,
        metadata: BTreeMap::from([(
            "heuristic".into(),
            "semantic naming + graph reachability".into(),
        )]),
    }
}

fn deduplicate(findings: Vec<ModelFinding>) -> Vec<ModelFinding> {
    let mut seen = HashSet::new();
    findings
        .into_iter()
        .filter(|f| seen.insert((f.category.clone(), f.node_path.last().cloned())))
        .collect()
}
