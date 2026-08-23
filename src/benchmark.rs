use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    generator::{BinaryLabel, GeneratedRecord, load_records},
    graph::{EdgeKind, NodeKind, SemanticGraph},
    heuristics,
};

pub const BENCHMARK_PROMPT_VERSION: &str = "logic-binary-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkModel {
    Heuristic,
    Mock,
    OpenaiCompatible,
}

impl std::str::FromStr for BenchmarkModel {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "heuristic" => Ok(Self::Heuristic),
            "mock" => Ok(Self::Mock),
            "openai-compatible" => Ok(Self::OpenaiCompatible),
            _ => bail!("unknown benchmark model {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    SourceOnly,
    SemanticGraphOnly,
    SourcePlusGraph,
    RawMirOnly,
}

impl std::str::FromStr for InputMode {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "source-only" => Ok(Self::SourceOnly),
            "semantic-graph-only" => Ok(Self::SemanticGraphOnly),
            "source-plus-graph" => Ok(Self::SourcePlusGraph),
            "raw-mir-only" => Ok(Self::RawMirOnly),
            _ => bail!("unknown input mode {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Ablation {
    Names,
    Types,
    DataFlow,
    SourceSnippets,
    VariantNames,
    CfgOnly,
}

impl std::str::FromStr for Ablation {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "names" => Ok(Self::Names),
            "types" => Ok(Self::Types),
            "data-flow" => Ok(Self::DataFlow),
            "source-snippets" => Ok(Self::SourceSnippets),
            "variant-names" => Ok(Self::VariantNames),
            "cfg-only" => Ok(Self::CfgOnly),
            _ => bail!("unknown ablation {value}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    pub dataset: PathBuf,
    pub model: BenchmarkModel,
    pub input_mode: InputMode,
    pub ablations: Vec<Ablation>,
    pub cache_dir: PathBuf,
    pub limit: Option<usize>,
    pub temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prediction {
    pub record_id: String,
    pub predicted_bug: bool,
    pub expected_bug: bool,
    pub confidence: f64,
    pub model: String,
    pub input_mode: InputMode,
    pub ablations: Vec<Ablation>,
    pub cached: bool,
    pub token_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BenchmarkMetrics {
    pub total: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub true_negatives: usize,
    pub false_negatives: usize,
    pub accuracy: f64,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub false_positive_rate: f64,
    pub false_negative_rate: f64,
}

impl BenchmarkMetrics {
    fn observe(&mut self, expected: bool, predicted: bool) {
        self.total += 1;
        match (expected, predicted) {
            (true, true) => self.true_positives += 1,
            (false, true) => self.false_positives += 1,
            (false, false) => self.true_negatives += 1,
            (true, false) => self.false_negatives += 1,
        }
    }
    fn finish(&mut self) {
        self.accuracy = ratio(self.true_positives + self.true_negatives, self.total);
        self.precision = ratio(
            self.true_positives,
            self.true_positives + self.false_positives,
        );
        self.recall = ratio(
            self.true_positives,
            self.true_positives + self.false_negatives,
        );
        self.f1 = if self.precision + self.recall == 0.0 {
            0.0
        } else {
            2.0 * self.precision * self.recall / (self.precision + self.recall)
        };
        self.false_positive_rate = ratio(
            self.false_positives,
            self.false_positives + self.true_negatives,
        );
        self.false_negative_rate = ratio(
            self.false_negatives,
            self.false_negatives + self.true_positives,
        );
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub dataset: String,
    pub model: BenchmarkModel,
    pub model_name: String,
    pub prompt_version: String,
    pub input_mode: InputMode,
    pub ablations: Vec<Ablation>,
    pub metrics: BenchmarkMetrics,
    pub by_domain: BTreeMap<String, BenchmarkMetrics>,
    pub by_mutation: BTreeMap<String, BenchmarkMetrics>,
    pub by_template: BTreeMap<String, BenchmarkMetrics>,
    pub by_call_depth: BTreeMap<String, BenchmarkMetrics>,
    pub by_identifier_mode: BTreeMap<String, BenchmarkMetrics>,
    pub by_challenge_set: BTreeMap<String, BenchmarkMetrics>,
    pub predictions: Vec<Prediction>,
}

pub async fn run(config: &BenchmarkConfig) -> Result<BenchmarkReport> {
    let mut records = load_records(&config.dataset)?;
    if let Some(limit) = config.limit {
        records.truncate(limit);
    }
    fs::create_dir_all(&config.cache_dir)?;
    let model_name = match config.model {
        BenchmarkModel::Heuristic => "heuristic".into(),
        BenchmarkModel::Mock => "mock-semantic-detector".into(),
        BenchmarkModel::OpenaiCompatible => {
            env::var("MIR_LOGIC_MODEL").unwrap_or_else(|_| "gpt-5-mini".into())
        }
    };
    let mut report = BenchmarkReport {
        schema_version: 1,
        dataset: config.dataset.display().to_string(),
        model: config.model,
        model_name: model_name.clone(),
        prompt_version: BENCHMARK_PROMPT_VERSION.into(),
        input_mode: config.input_mode,
        ablations: config.ablations.clone(),
        metrics: BenchmarkMetrics::default(),
        by_domain: BTreeMap::new(),
        by_mutation: BTreeMap::new(),
        by_template: BTreeMap::new(),
        by_call_depth: BTreeMap::new(),
        by_identifier_mode: BTreeMap::new(),
        by_challenge_set: BTreeMap::new(),
        predictions: vec![],
    };
    for record in records {
        let graph = ablate_graph(&record.graph, &config.ablations);
        let mut prediction = match config.model {
            BenchmarkModel::Heuristic | BenchmarkModel::Mock => {
                local_prediction(&record, &graph, config)
            }
            BenchmarkModel::OpenaiCompatible => {
                llm_prediction(&record, &graph, config, &model_name).await?
            }
        };
        prediction.expected_bug = record.label == BinaryLabel::Bug;
        observe_report(&mut report, &record, &prediction);
        report.predictions.push(prediction);
    }
    finish_report(&mut report);
    Ok(report)
}

fn local_prediction(
    record: &GeneratedRecord,
    graph: &SemanticGraph,
    config: &BenchmarkConfig,
) -> Prediction {
    let predicted_bug = match config.input_mode {
        InputMode::SemanticGraphOnly | InputMode::SourcePlusGraph => {
            !heuristics::analyze(graph).is_empty()
        }
        InputMode::SourceOnly => source_baseline(&record.source),
        InputMode::RawMirOnly => record.raw_mir.as_deref().is_some_and(source_baseline),
    };
    Prediction {
        record_id: record.id.clone(),
        predicted_bug,
        expected_bug: false,
        confidence: if predicted_bug { 0.75 } else { 0.55 },
        model: format!("{:?}", config.model).to_ascii_lowercase(),
        input_mode: config.input_mode,
        ablations: config.ablations.clone(),
        cached: false,
        token_usage: None,
    }
}

// Deliberately weak textual baseline. It does not inspect the label or mutation
// metadata and exists only to make source-vs-graph comparisons explicit.
fn source_baseline(source: &str) -> bool {
    let source = strip_generation_markers(source);
    let controller = source
        .rsplit_once("_controller(accepted: bool) -> bool")
        .map(|(_, controller)| controller)
        .unwrap_or(&source);
    !controller.contains("return false") && controller.matches("false").count() == 0
}

async fn llm_prediction(
    record: &GeneratedRecord,
    graph: &SemanticGraph,
    config: &BenchmarkConfig,
    model_name: &str,
) -> Result<Prediction> {
    let payload = benchmark_payload(record, graph, config.input_mode)?;
    let cache_key = cache_key(record, config, model_name, &payload);
    let cache_path = config.cache_dir.join(format!("{cache_key:016x}.json"));
    if cache_path.exists() {
        let mut prediction: Prediction = serde_json::from_slice(&fs::read(cache_path)?)?;
        prediction.cached = true;
        return Ok(prediction);
    }
    let api_key = env::var("MIR_LOGIC_API_KEY")
        .or_else(|_| env::var("OPENAI_API_KEY"))
        .context("set MIR_LOGIC_API_KEY or OPENAI_API_KEY for LLM benchmark")?;
    let base =
        env::var("MIR_LOGIC_API_BASE").unwrap_or_else(|_| "https://api.openai.com/v1".into());
    let prompt = format!(
        "Classify whether this Rust logic-flow example violates its stated invariant. Do not infer the answer from formatting markers. Return only JSON {{\"bug\":boolean,\"confidence\":0..1,\"reason\":string}}. Invariant: {}\nInput mode: {:?}\nPayload:\n{}",
        record.invariant, config.input_mode, payload
    );
    let body = json!({"model": model_name, "temperature": config.temperature, "messages": [{"role":"system","content":"You are a conservative semantic logic-bug classifier."},{"role":"user","content":prompt}], "response_format":{"type":"json_object"}});
    let response = reqwest::Client::new()
        .post(format!("{}/chat/completions", base.trim_end_matches('/')))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        bail!("model API returned {status}: {value}");
    }
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .context("missing model content")?;
    let parsed: Value = serde_json::from_str(
        content
            .trim()
            .trim_start_matches("```json")
            .trim_end_matches("```")
            .trim(),
    )?;
    let usage = value.get("usage").map(|usage| TokenUsage {
        prompt_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
        completion_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
    });
    let prediction = Prediction {
        record_id: record.id.clone(),
        predicted_bug: parsed
            .get("bug")
            .and_then(Value::as_bool)
            .context("model response missing bug boolean")?,
        expected_bug: record.label == BinaryLabel::Bug,
        confidence: parsed
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.5),
        model: model_name.into(),
        input_mode: config.input_mode,
        ablations: config.ablations.clone(),
        cached: false,
        token_usage: usage,
    };
    fs::write(&cache_path, serde_json::to_vec_pretty(&prediction)?)?;
    let metadata = json!({"model":model_name,"prompt_version":BENCHMARK_PROMPT_VERSION,"dataset_version":record.dataset_version,"temperature":config.temperature,"timestamp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),"prediction_file":cache_path.file_name()});
    fs::write(
        cache_path.with_extension("meta.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;
    Ok(prediction)
}

fn benchmark_payload(
    record: &GeneratedRecord,
    graph: &SemanticGraph,
    mode: InputMode,
) -> Result<String> {
    match mode {
        InputMode::SourceOnly => Ok(strip_generation_markers(&record.source)),
        InputMode::SemanticGraphOnly => Ok(serde_json::to_string(graph)?),
        InputMode::SourcePlusGraph => Ok(format!(
            "SOURCE:\n{}\nSEMANTIC_GRAPH:\n{}",
            strip_generation_markers(&record.source),
            serde_json::to_string(graph)?
        )),
        InputMode::RawMirOnly => Ok(record
            .raw_mir
            .clone()
            .context("record does not contain raw MIR")?),
    }
}

fn strip_generation_markers(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            line.replace("/* MIR_LOGIC_MUTATION_POINT */", "")
                .replace("// MIR_LOGIC_MUTATION_POINT", "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cache_key(
    record: &GeneratedRecord,
    config: &BenchmarkConfig,
    model: &str,
    payload: &str,
) -> u64 {
    let value = format!(
        "{}|{}|{:?}|{:?}|{:?}|{}|{}",
        record.dataset_version,
        record.id,
        config.input_mode,
        config.ablations,
        config.temperature,
        model,
        payload
    );
    value
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        })
}

pub fn ablate_graph(graph: &SemanticGraph, ablations: &[Ablation]) -> SemanticGraph {
    let mut graph = graph.clone();
    if ablations
        .iter()
        .any(|ablation| matches!(ablation, Ablation::Names | Ablation::CfgOnly))
    {
        neutralize_identifiers(&mut graph);
    }
    for ablation in ablations {
        for function in &mut graph.functions {
            for node in &mut function.nodes {
                match ablation {
                    Ablation::Names => {
                        node.called_function = None;
                        node.variable = None;
                        node.branch_condition = None;
                        node.text = None;
                        node.source_text = None;
                        node.arguments.clear();
                        node.reads.clear();
                        node.writes.clear();
                        node.metadata.clear();
                    }
                    Ablation::Types => node.ty = None,
                    Ablation::SourceSnippets => {
                        node.source = None;
                        node.source_text = None;
                    }
                    Ablation::VariantNames => node.variant = None,
                    Ablation::CfgOnly => {
                        node.source = None;
                        node.source_text = None;
                        node.text = None;
                        node.ty = None;
                        node.variable = None;
                        node.operation = None;
                        node.branch_condition = None;
                        node.called_function = None;
                        node.arguments.clear();
                        node.variant = None;
                        node.reads.clear();
                        node.writes.clear();
                        node.metadata.clear();
                    }
                    Ablation::DataFlow => {}
                }
            }
            if matches!(ablation, Ablation::DataFlow | Ablation::CfgOnly) {
                function
                    .edges
                    .retain(|edge| edge.kind != EdgeKind::DataDependency);
            }
            if *ablation == Ablation::VariantNames {
                for edge in &mut function.edges {
                    if edge.kind == EdgeKind::SwitchCase {
                        edge.label = None;
                    }
                }
            }
            if *ablation == Ablation::CfgOnly {
                function.nodes.retain(|node| {
                    matches!(
                        node.kind,
                        NodeKind::FunctionEntry
                            | NodeKind::FunctionExit
                            | NodeKind::Branch
                            | NodeKind::Call
                            | NodeKind::Return
                            | NodeKind::Assert
                            | NodeKind::Error
                    )
                });
            }
        }
    }
    graph
}

fn neutralize_identifiers(graph: &mut SemanticGraph) {
    let mut ids = BTreeMap::new();
    for (function_index, function) in graph.functions.iter().enumerate() {
        for (node_index, node) in function.nodes.iter().enumerate() {
            ids.insert(
                node.id.clone(),
                format!("function_{function_index}::node_{node_index}"),
            );
        }
    }
    for (function_index, function) in graph.functions.iter_mut().enumerate() {
        function.name = format!("function_{function_index}");
        for node in &mut function.nodes {
            if let Some(id) = ids.get(&node.id) {
                node.id.clone_from(id);
            }
            node.function.clone_from(&function.name);
        }
        for edge in &mut function.edges {
            if let Some(id) = ids.get(&edge.from) {
                edge.from.clone_from(id);
            }
            if let Some(id) = ids.get(&edge.to) {
                edge.to.clone_from(id);
            }
        }
    }
    for edge in &mut graph.interprocedural_edges {
        if let Some(id) = ids.get(&edge.from) {
            edge.from.clone_from(id);
        }
        if let Some(id) = ids.get(&edge.to) {
            edge.to.clone_from(id);
        }
    }
    graph.crate_name = "crate".into();
}

fn observe_report(report: &mut BenchmarkReport, record: &GeneratedRecord, prediction: &Prediction) {
    let expected = record.label == BinaryLabel::Bug;
    report.metrics.observe(expected, prediction.predicted_bug);
    observe_group(
        &mut report.by_domain,
        record.domain.as_str(),
        expected,
        prediction.predicted_bug,
    );
    observe_group(
        &mut report.by_template,
        record.generation.template.as_str(),
        expected,
        prediction.predicted_bug,
    );
    observe_group(
        &mut report.by_call_depth,
        &record.generation.call_depth.to_string(),
        expected,
        prediction.predicted_bug,
    );
    observe_group(
        &mut report.by_identifier_mode,
        record.generation.identifier_mode.as_str(),
        expected,
        prediction.predicted_bug,
    );
    observe_group(
        &mut report.by_mutation,
        record.generation.paired_mutation.as_str(),
        expected,
        prediction.predicted_bug,
    );
    if let Some(challenge) = record.generation.challenge_set {
        observe_group(
            &mut report.by_challenge_set,
            challenge.as_str(),
            expected,
            prediction.predicted_bug,
        );
    }
}

fn observe_group(
    map: &mut BTreeMap<String, BenchmarkMetrics>,
    key: &str,
    expected: bool,
    predicted: bool,
) {
    map.entry(key.into())
        .or_default()
        .observe(expected, predicted);
}

fn finish_report(report: &mut BenchmarkReport) {
    report.metrics.finish();
    for map in [
        &mut report.by_domain,
        &mut report.by_mutation,
        &mut report.by_template,
        &mut report.by_call_depth,
        &mut report.by_identifier_mode,
        &mut report.by_challenge_set,
    ] {
        for metrics in map.values_mut() {
            metrics.finish();
        }
    }
}

pub fn terminal(report: &BenchmarkReport) -> String {
    format!(
        "model={} input={:?} total={} accuracy={:.1}% precision={:.1}% recall={:.1}% F1={:.1}% FPR={:.1}% FNR={:.1}%\n",
        report.model_name,
        report.input_mode,
        report.metrics.total,
        report.metrics.accuracy * 100.0,
        report.metrics.precision * 100.0,
        report.metrics.recall * 100.0,
        report.metrics.f1 * 100.0,
        report.metrics.false_positive_rate * 100.0,
        report.metrics.false_negative_rate * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::SemanticGraph;

    #[test]
    fn benchmark_metrics_are_correct() {
        let mut metrics = BenchmarkMetrics::default();
        metrics.observe(true, true);
        metrics.observe(false, true);
        metrics.observe(false, false);
        metrics.observe(true, false);
        metrics.finish();
        assert_eq!(metrics.accuracy, 0.5);
        assert_eq!(metrics.precision, 0.5);
        assert_eq!(metrics.recall, 0.5);
        assert_eq!(metrics.f1, 0.5);
    }

    #[test]
    fn ablations_remove_requested_features() {
        let graph = SemanticGraph {
            schema_version: 1,
            crate_name: "x".into(),
            functions: vec![],
            interprocedural_edges: vec![],
        };
        assert_eq!(
            ablate_graph(&graph, &[Ablation::CfgOnly]).functions.len(),
            0
        );
    }
}
