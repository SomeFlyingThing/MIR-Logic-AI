use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type NodeId = String;
pub type RawMirGraph = ProgramGraph;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgramGraph {
    pub schema_version: u32,
    pub crate_name: String,
    pub functions: Vec<FunctionGraph>,
    pub metadata: BTreeMap<String, String>,
}

impl ProgramGraph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.functions
            .iter()
            .flat_map(|f| &f.nodes)
            .find(|n| n.id == id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionGraph {
    pub name: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub locals: Vec<Local>,
    pub source_file: Option<String>,
    pub raw_mir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Local {
    pub compiler_name: String,
    pub semantic_name: Option<String>,
    pub ty: Option<String>,
    pub mutable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub function: String,
    pub block: Option<usize>,
    pub kind: NodeKind,
    pub source: Option<SourceLocation>,
    pub source_text: Option<String>,
    pub text: Option<String>,
    pub ty: Option<String>,
    pub variable: Option<String>,
    pub operation: Option<String>,
    pub branch_condition: Option<String>,
    pub called_function: Option<String>,
    pub arguments: Vec<String>,
    pub variant: Option<String>,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    FunctionEntry,
    FunctionExit,
    BasicBlock,
    Assignment,
    Branch,
    Call,
    Return,
    Assert,
    Variant,
    Error,
    StateChange,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub label: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    ControlFlow,
    TrueBranch,
    FalseBranch,
    SwitchCase,
    Call,
    Return,
    DataDependency,
    Unwind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: String,
    pub line: usize,
    pub column: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticGraph {
    pub schema_version: u32,
    pub crate_name: String,
    pub functions: Vec<SemanticFunction>,
    pub interprocedural_edges: Vec<Edge>,
}

impl SemanticGraph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.functions
            .iter()
            .flat_map(|f| &f.nodes)
            .find(|n| n.id == id)
    }

    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.functions
            .iter()
            .flat_map(|f| &f.edges)
            .chain(&self.interprocedural_edges)
    }

    pub fn function(&self, name: &str) -> Option<&SemanticFunction> {
        self.functions.iter().find(|f| f.name == name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticFunction {
    pub name: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub context: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_serialization_is_stable() {
        let graph = ProgramGraph {
            schema_version: 1,
            crate_name: "demo".into(),
            functions: vec![],
            metadata: BTreeMap::new(),
        };
        let json = serde_json::to_string(&graph).unwrap();
        assert!(json.contains("\"schema_version\":1"));
        assert_eq!(serde_json::from_str::<ProgramGraph>(&json).unwrap(), graph);
    }
}
