use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::{graph::SemanticGraph, model::ModelFinding, verify::VerificationResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetRecord {
    pub schema_version: u32,
    pub graph: SemanticGraph,
    pub finding: ModelFinding,
    pub label: Label,
    pub bug_type: Option<String>,
    pub source: String,
    pub verification: VerificationResult,
    pub human_feedback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Bug,
    NotBug,
    Uncertain,
    Unlabeled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedGraphRecord {
    pub schema_version: u32,
    pub good_graph: SemanticGraph,
    pub bad_graph: SemanticGraph,
    pub mutation: String,
    pub changed_nodes: Vec<String>,
    pub label: String,
}

pub fn export(runs: &Path, output: &Path) -> Result<usize> {
    let file = File::create(output)?;
    let mut writer = BufWriter::new(file);
    let mut count = 0;
    for entry in WalkDir::new(runs).into_iter().filter_map(Result::ok) {
        if entry.path().extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(entry.path())?;
        let value: Value = serde_json::from_str(&text)?;
        if value.get("good_graph").is_some() {
            serde_json::to_writer(&mut writer, &value)?;
            writer.write_all(b"\n")?;
            count += 1;
            continue;
        }
        let report: crate::report::AnalysisReport = serde_json::from_value(value)?;
        for verified in report.findings {
            let record = DatasetRecord {
                schema_version: 1,
                graph: report.graph.clone(),
                bug_type: verified.finding.category.clone(),
                finding: verified.finding,
                label: Label::Unlabeled,
                source: "analysis_run".into(),
                verification: verified.verification,
                human_feedback: None,
            };
            serde_json::to_writer(&mut writer, &record)?;
            writer.write_all(b"\n")?;
            count += 1;
        }
    }
    writer.flush()?;
    Ok(count)
}

pub fn label(labels_file: &Path, finding_id: &str, label: Label) -> Result<()> {
    let mut labels: std::collections::BTreeMap<String, Label> = if labels_file.exists() {
        serde_json::from_str(&fs::read_to_string(labels_file)?)?
    } else {
        Default::default()
    };
    labels.insert(finding_id.into(), label);
    if let Some(parent) = labels_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(labels_file, serde_json::to_string_pretty(&labels)?)?;
    Ok(())
}

pub fn write_pair(record: &PairedGraphRecord, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, serde_json::to_vec_pretty(record)?)?;
    Ok(())
}
