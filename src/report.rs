use std::{collections::HashMap, fmt::Write as _};

use serde::{Deserialize, Serialize};

use crate::{
    graph::{EdgeKind, SemanticGraph},
    model::ModelFinding,
    verify::{VerificationResult, VerificationStatus},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedFinding {
    pub finding: ModelFinding,
    pub verification: VerificationResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub schema_version: u32,
    pub project: String,
    pub graph: SemanticGraph,
    pub findings: Vec<VerifiedFinding>,
}

pub fn terminal(report: &AnalysisReport) -> String {
    if report.findings.is_empty() {
        return "No suspicious semantic paths found. This is not proof that the program is bug-free.\n".into();
    }
    let mut out = String::new();
    for verified in &report.findings {
        let f = &verified.finding;
        let severity = if f.confidence >= 0.85 {
            "HIGH"
        } else if f.confidence >= 0.65 {
            "MEDIUM"
        } else {
            "LOW"
        };
        let _ = writeln!(out, "{severity}  {}", f.title);
        if let Some(first_source) = f
            .node_path
            .iter()
            .filter_map(|id| report.graph.node(id))
            .find_map(|n| n.source.as_ref())
        {
            let _ = writeln!(out, "{}:{}", first_source.file, first_source.line);
        }
        let _ = writeln!(out, "Observed semantic path:");
        for (index, id) in f.node_path.iter().enumerate() {
            if let Some(node) = report.graph.node(id) {
                if index > 0 {
                    let edge_label = report
                        .graph
                        .edges()
                        .find(|e| e.from == f.node_path[index - 1] && e.to == *id)
                        .and_then(|e| e.label.as_deref());
                    if let Some(label) = edge_label {
                        let _ = writeln!(out, "    ↓ [{label}]");
                    } else {
                        let _ = writeln!(out, "    ↓");
                    }
                }
                let label = node
                    .called_function
                    .as_deref()
                    .or(node.variant.as_deref())
                    .or(node.branch_condition.as_deref())
                    .or(node.text.as_deref())
                    .unwrap_or(id);
                let _ = writeln!(out, "{label}");
            }
        }
        let _ = writeln!(out, "Hypothesis: {}", f.suspected_invariant);
        let _ = writeln!(
            out,
            "Graph verification: {}",
            status(&verified.verification.status)
        );
        let _ = writeln!(
            out,
            "Path feasibility: {:?}",
            verified.verification.feasibility
        );
        let _ = writeln!(out, "Confidence: {:.0}%", f.confidence * 100.0);
        let _ = writeln!(out, "Detector: {:?}\n", f.detector);
    }
    out
}

fn status(status: &VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::ConfirmedGraphPath => "CONFIRMED GRAPH PATH",
        VerificationStatus::Infeasible => "INFEASIBLE",
        VerificationStatus::Unknown => "UNKNOWN",
        VerificationStatus::InvalidModelOutput => "INVALID MODEL OUTPUT",
    }
}

pub fn dot(graph: &SemanticGraph, function_filter: Option<&str>) -> String {
    let mut out = String::from(
        "digraph mir_logic {\n  rankdir=TB;\n  node [shape=box,fontname=\"monospace\"];\n",
    );
    let mut ids = HashMap::new();
    let mut index = 0usize;
    for function in &graph.functions {
        if function_filter.is_some_and(|filter| function.name != filter) {
            continue;
        }
        let _ = writeln!(
            out,
            "  subgraph cluster_{index} {{ label=\"{}\";",
            escape(&function.name)
        );
        for node in &function.nodes {
            let dot_id = format!("n{index}");
            index += 1;
            ids.insert(node.id.clone(), dot_id.clone());
            let label = node
                .called_function
                .as_deref()
                .or(node.branch_condition.as_deref())
                .or(node.text.as_deref())
                .unwrap_or(&node.id);
            let _ = writeln!(
                out,
                "    {dot_id} [label=\"{}\\n{:?}\"];",
                escape(label),
                node.kind
            );
        }
        out.push_str("  }\n");
    }
    for edge in graph.edges() {
        let (Some(from), Some(to)) = (ids.get(&edge.from), ids.get(&edge.to)) else {
            continue;
        };
        let label = edge
            .label
            .clone()
            .unwrap_or_else(|| format!("{:?}", edge.kind));
        let style = if edge.kind == EdgeKind::DataDependency {
            "dashed"
        } else {
            "solid"
        };
        let _ = writeln!(
            out,
            "  {from} -> {to} [label=\"{}\",style={style}];",
            escape(&label)
        );
    }
    out.push_str("}\n");
    out
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
