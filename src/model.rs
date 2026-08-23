use std::{collections::BTreeMap, env};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{graph::SemanticGraph, heuristics};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelFinding {
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub confidence: f64,
    pub node_path: Vec<String>,
    pub suspected_invariant: String,
    pub reason: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub detector: Detector,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Detector {
    Ai,
    #[default]
    Heuristic,
    Combined,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalysisContext {
    pub project: String,
    pub call_depth: usize,
    pub instructions: Option<String>,
}

#[async_trait]
pub trait LogicModel: Send + Sync {
    async fn analyze(
        &self,
        graph: &SemanticGraph,
        context: &AnalysisContext,
    ) -> Result<Vec<ModelFinding>>;
    fn name(&self) -> &'static str;
}

/// Deterministic stand-in that exercises exactly the same structured model and
/// verification pipeline without network access.
#[derive(Debug, Default)]
pub struct MockModel;

#[async_trait]
impl LogicModel for MockModel {
    async fn analyze(
        &self,
        graph: &SemanticGraph,
        _context: &AnalysisContext,
    ) -> Result<Vec<ModelFinding>> {
        let mut findings = heuristics::semantic_path_candidates(graph);
        for finding in &mut findings {
            finding.detector = Detector::Ai;
            finding.confidence = (finding.confidence + 0.08).min(0.97);
            finding
                .metadata
                .insert("backend".into(), "mock semantic reasoner".into());
        }
        Ok(findings)
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}

#[derive(Debug, Clone)]
pub struct OpenAICompatibleModel {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    client: reqwest::Client,
}

impl OpenAICompatibleModel {
    pub fn from_env() -> Result<Self> {
        let base_url =
            env::var("MIR_LOGIC_API_BASE").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let api_key = env::var("MIR_LOGIC_API_KEY")
            .or_else(|_| env::var("OPENAI_API_KEY"))
            .context("set MIR_LOGIC_API_KEY or OPENAI_API_KEY")?;
        let model = env::var("MIR_LOGIC_MODEL").unwrap_or_else(|_| "gpt-5-mini".into());
        Ok(Self {
            base_url,
            api_key,
            model,
            client: reqwest::Client::new(),
        })
    }

    fn prompt(&self, graph: &SemanticGraph, context: &AnalysisContext) -> Result<String> {
        let compact = serde_json::to_string(graph)?;
        Ok(format!(
            r#"You are analyzing a semantic graph extracted from Rust MIR. Find semantic logic-flow inconsistencies, not memory-safety bugs. The compiler graph is authoritative about edge existence; you are not. Return only JSON matching {{"findings":[{{"title":string,"confidence":0..1,"node_path":[node IDs],"suspected_invariant":string,"reason":string,"category":string}}]}}. Every path must list consecutive existing graph edges. Consider failed authentication reaching session creation, denied permissions reaching sensitive operations, Err reaching success continuations, invalid state transitions, failed resource acquisition followed by use, or transaction failure followed by commit. These are examples, not universal rules. Avoid findings without semantic evidence.
Context: project={}, call_depth={}
Graph: {}"#,
            context.project, context.call_depth, compact
        ))
    }
}

#[async_trait]
impl LogicModel for OpenAICompatibleModel {
    async fn analyze(
        &self,
        graph: &SemanticGraph,
        context: &AnalysisContext,
    ) -> Result<Vec<ModelFinding>> {
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": "Return strictly valid JSON. Treat MIR reachability as data, not as proof of a bug."},
                {"role": "user", "content": self.prompt(graph, context)?}
            ],
            "response_format": {"type": "json_object"}
        });
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .context("model returned non-JSON HTTP response")?;
        if !status.is_success() {
            bail!("model API returned {status}: {value}");
        }
        let content = value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .context("missing choices[0].message.content")?;
        parse_model_output(content)
    }

    fn name(&self) -> &'static str {
        "openai-compatible"
    }
}

#[derive(Deserialize)]
struct FindingEnvelope {
    findings: Vec<ModelFinding>,
}

pub fn parse_model_output(content: &str) -> Result<Vec<ModelFinding>> {
    let clean = content
        .trim()
        .strip_prefix("```json")
        .unwrap_or(content.trim())
        .strip_suffix("```")
        .unwrap_or(content.trim())
        .trim();
    let mut findings = serde_json::from_str::<FindingEnvelope>(clean)?.findings;
    for (index, finding) in findings.iter_mut().enumerate() {
        if finding.id.is_empty() {
            finding.id = format!("ai-{index}");
        }
        finding.detector = Detector::Ai;
    }
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_model_output() {
        let output = r#"{"findings":[{"title":"bad","confidence":0.9,"node_path":["a","b"],"suspected_invariant":"x","reason":"y"}]}"#;
        let findings = parse_model_output(output).unwrap();
        assert_eq!(findings[0].id, "ai-0");
        assert_eq!(findings[0].detector, Detector::Ai);
    }
}
