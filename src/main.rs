use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use mir_logic::{
    ExtractOptions, MirExtractor,
    dataset::{self, Label},
    eval, heuristics,
    model::{AnalysisContext, LogicModel, MockModel, OpenAICompatibleModel},
    mutation::{MutationKind, mutate_file},
    report::{self, AnalysisReport, VerifiedFinding},
    simplify::simplify,
    verify::{FindingVerifier, GraphPathVerifier},
};

#[derive(Parser)]
#[command(
    name = "mir-logic",
    version,
    about = "Semantic logic-flow analysis over Rust MIR"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Extract {
        project: PathBuf,
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
        #[arg(long, default_value_t = 3)]
        call_depth: usize,
    },
    Graph {
        project: PathBuf,
        #[arg(long)]
        function: Option<String>,
        #[arg(long, value_enum, default_value = "dot")]
        format: Format,
        #[arg(long, default_value_t = 3)]
        call_depth: usize,
    },
    Analyze {
        project: PathBuf,
        #[arg(long, value_enum, default_value = "mock")]
        model: Model,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
        #[arg(long, default_value_t = 3)]
        call_depth: usize,
        #[arg(long)]
        no_heuristics: bool,
    },
    Eval {
        #[arg(default_value = "examples")]
        examples: PathBuf,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Dataset {
        #[command(subcommand)]
        command: DatasetCommand,
    },
    Label {
        finding_id: String,
        #[arg(value_enum)]
        label: LabelArg,
        #[arg(long, default_value = ".mir-logic/labels.json")]
        file: PathBuf,
    },
    Mutate {
        source: PathBuf,
        output: PathBuf,
        #[arg(long)]
        mutation: String,
    },
}

#[derive(Subcommand)]
enum DatasetCommand {
    Export {
        runs: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Pair {
        good_project: PathBuf,
        bad_project: PathBuf,
        #[arg(long)]
        mutation: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 3)]
        call_depth: usize,
    },
}

#[derive(Clone, ValueEnum)]
enum Format {
    Json,
    Dot,
    Text,
}

#[derive(Clone, ValueEnum)]
enum Model {
    Mock,
    OpenaiCompatible,
    None,
}

#[derive(Clone, ValueEnum)]
enum LabelArg {
    Bug,
    NotBug,
    Uncertain,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Extract {
            project,
            format,
            call_depth,
        } => {
            let raw = MirExtractor.extract(
                &project,
                &ExtractOptions {
                    call_depth,
                    keep_raw_mir: true,
                },
            )?;
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&raw)?),
                Format::Dot => println!("{}", report::dot(&simplify(&raw, call_depth), None)),
                Format::Text => println!("extracted {} functions", raw.functions.len()),
            }
        }
        Command::Graph {
            project,
            function,
            format,
            call_depth,
        } => {
            let graph = extract_semantic(&project, call_depth)?;
            match format {
                Format::Dot => print!("{}", report::dot(&graph, function.as_deref())),
                Format::Json => println!("{}", serde_json::to_string_pretty(&graph)?),
                Format::Text => println!(
                    "{} functions, {} semantic nodes",
                    graph.functions.len(),
                    graph.functions.iter().map(|f| f.nodes.len()).sum::<usize>()
                ),
            }
        }
        Command::Analyze {
            project,
            model,
            format,
            call_depth,
            no_heuristics,
        } => {
            let graph = extract_semantic(&project, call_depth)?;
            let mut findings = if no_heuristics {
                vec![]
            } else {
                heuristics::analyze(&graph)
            };
            let context = AnalysisContext {
                project: project.display().to_string(),
                call_depth,
                instructions: None,
            };
            match model {
                Model::Mock => findings.extend(MockModel.analyze(&graph, &context).await?),
                Model::OpenaiCompatible => findings.extend(
                    OpenAICompatibleModel::from_env()?
                        .analyze(&graph, &context)
                        .await?,
                ),
                Model::None => {}
            }
            findings.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
            findings.dedup_by(|a, b| {
                a.category == b.category
                    && a.node_path.last() == b.node_path.last()
                    && a.detector == b.detector
            });
            let verifier = GraphPathVerifier;
            let verified = findings
                .into_iter()
                .map(|finding| {
                    let verification = verifier.verify(&graph, &finding);
                    VerifiedFinding {
                        finding,
                        verification,
                    }
                })
                .collect();
            let report = AnalysisReport {
                schema_version: 1,
                project: project.display().to_string(),
                graph,
                findings: verified,
            };
            save_run(&report)?;
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&report)?),
                _ => print!("{}", report::terminal(&report)),
            }
        }
        Command::Eval { examples, format } => {
            let result = eval::run(&examples).await?;
            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&result)?),
                _ => print!("{}", eval::terminal(&result)),
            }
        }
        Command::Dataset { command } => match command {
            DatasetCommand::Export { runs, output } => println!(
                "exported {} records to {}",
                dataset::export(&runs, &output)?,
                output.display()
            ),
            DatasetCommand::Pair {
                good_project,
                bad_project,
                mutation,
                output,
                call_depth,
            } => {
                let good_graph = extract_semantic(&good_project, call_depth)?;
                let bad_graph = extract_semantic(&bad_project, call_depth)?;
                let changed_nodes = changed_node_ids(&good_graph, &bad_graph);
                let record = dataset::PairedGraphRecord {
                    schema_version: 1,
                    good_graph,
                    bad_graph,
                    mutation,
                    changed_nodes,
                    label: "logic_bug".into(),
                };
                dataset::write_pair(&record, &output)?;
                println!("wrote paired graph to {}", output.display());
            }
        },
        Command::Label {
            finding_id,
            label,
            file,
        } => {
            let label = match label {
                LabelArg::Bug => Label::Bug,
                LabelArg::NotBug => Label::NotBug,
                LabelArg::Uncertain => Label::Uncertain,
            };
            dataset::label(&file, &finding_id, label)?;
            println!("labeled {finding_id}");
        }
        Command::Mutate {
            source,
            output,
            mutation,
        } => println!(
            "{}",
            serde_json::to_string_pretty(&mutate_file(
                &source,
                &output,
                mutation.parse::<MutationKind>()?
            )?)?
        ),
    }
    Ok(())
}

fn extract_semantic(project: &Path, call_depth: usize) -> Result<mir_logic::SemanticGraph> {
    Ok(simplify(
        &MirExtractor.extract(
            project,
            &ExtractOptions {
                call_depth,
                keep_raw_mir: false,
            },
        )?,
        call_depth,
    ))
}

fn save_run(report: &AnalysisReport) -> Result<()> {
    let directory = Path::new(".mir-logic/runs");
    fs::create_dir_all(directory)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    fs::write(
        directory.join(format!("run-{stamp}.json")),
        serde_json::to_vec_pretty(report)?,
    )?;
    Ok(())
}

fn changed_node_ids(
    good: &mir_logic::SemanticGraph,
    bad: &mir_logic::SemanticGraph,
) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let feature = |graph: &mir_logic::SemanticGraph| -> BTreeMap<String, String> {
        graph
            .functions
            .iter()
            .flat_map(|function| &function.nodes)
            .map(|node| {
                (
                    node.id.clone(),
                    format!(
                        "{:?}|{:?}|{:?}|{:?}",
                        node.kind, node.called_function, node.text, node.ty
                    ),
                )
            })
            .collect()
    };
    let (good, bad) = (feature(good), feature(bad));
    good.keys()
        .chain(bad.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|id| good.get(id) != bad.get(id))
        .collect()
}
