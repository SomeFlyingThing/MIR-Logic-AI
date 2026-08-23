use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{
    extractor::{ExtractOptions, MirExtractor},
    heuristics,
    model::{AnalysisContext, LogicModel, MockModel},
    simplify::simplify,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub true_positives: usize,
    pub false_positives: usize,
    pub true_negatives: usize,
    pub false_negatives: usize,
}

impl Metrics {
    pub fn precision(&self) -> f64 {
        ratio(
            self.true_positives,
            self.true_positives + self.false_positives,
        )
    }
    pub fn recall(&self) -> f64 {
        ratio(
            self.true_positives,
            self.true_positives + self.false_negatives,
        )
    }
    pub fn observe(&mut self, expected: bool, detected: bool) {
        match (expected, detected) {
            (true, true) => self.true_positives += 1,
            (false, true) => self.false_positives += 1,
            (false, false) => self.true_negatives += 1,
            (true, false) => self.false_negatives += 1,
        }
    }
}

fn ratio(a: usize, b: usize) -> f64 {
    if b == 0 { 0.0 } else { a as f64 / b as f64 }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub heuristic: Metrics,
    pub ai: Metrics,
    pub combined: Metrics,
    pub cases: Vec<EvalCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    pub project: String,
    pub expected_bug: bool,
    pub category: String,
    pub heuristic_findings: usize,
    pub ai_findings: usize,
}

pub async fn run(root: &Path) -> Result<EvalResult> {
    let mut result = EvalResult {
        heuristic: Metrics::default(),
        ai: Metrics::default(),
        combined: Metrics::default(),
        cases: vec![],
    };
    let model = MockModel;
    for entry in WalkDir::new(root)
        .min_depth(1)
        .max_depth(2)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_name() != "Cargo.toml" {
            continue;
        }
        let manifest = entry.path();
        let text = fs::read_to_string(manifest)?;
        let value: toml::Value = toml::from_str(&text)?;
        let Some(meta) = value
            .get("package")
            .and_then(|x| x.get("metadata"))
            .and_then(|x| x.get("mir_logic"))
        else {
            continue;
        };
        let expected = meta
            .get("expected_bug")
            .and_then(toml::Value::as_bool)
            .context("expected_bug must be boolean")?;
        let category = meta
            .get("category")
            .and_then(toml::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let project = manifest.parent().expect("manifest parent");
        let raw = MirExtractor.extract(project, &ExtractOptions::default())?;
        let graph = simplify(&raw, 3);
        let heuristic = heuristics::analyze(&graph);
        let ai = model
            .analyze(
                &graph,
                &AnalysisContext {
                    project: project.display().to_string(),
                    call_depth: 3,
                    instructions: None,
                },
            )
            .await?;
        result.heuristic.observe(expected, !heuristic.is_empty());
        result.ai.observe(expected, !ai.is_empty());
        result
            .combined
            .observe(expected, !heuristic.is_empty() || !ai.is_empty());
        result.cases.push(EvalCase {
            project: project.display().to_string(),
            expected_bug: expected,
            category,
            heuristic_findings: heuristic.len(),
            ai_findings: ai.len(),
        });
    }
    result.cases.sort_by(|a, b| a.project.cmp(&b.project));
    Ok(result)
}

pub fn terminal(result: &EvalResult) -> String {
    let mut out = String::from("detector    TP  FP  TN  FN  precision  recall\n");
    for (name, m) in [
        ("heuristic", &result.heuristic),
        ("ai(mock)", &result.ai),
        ("combined", &result.combined),
    ] {
        out.push_str(&format!(
            "{name:<11} {:>2}  {:>2}  {:>2}  {:>2}   {:>7.1}% {:>6.1}%\n",
            m.true_positives,
            m.false_positives,
            m.true_negatives,
            m.false_negatives,
            m.precision() * 100.0,
            m.recall() * 100.0
        ));
    }
    out.push('\n');
    for case in &result.cases {
        out.push_str(&format!(
            "{} expected={} heuristic={} ai={} [{}]\n",
            case.project,
            case.expected_bug,
            case.heuristic_findings,
            case.ai_findings,
            case.category
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn computes_metrics() {
        let mut m = Metrics::default();
        m.observe(true, true);
        m.observe(false, true);
        assert_eq!(m.precision(), 0.5);
        assert_eq!(m.recall(), 1.0);
    }
}
