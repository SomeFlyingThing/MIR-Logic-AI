//! Nightly MIR extraction is deliberately isolated here.  The rest of the crate
//! only depends on the stable, serializable graph schema.
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use regex::Regex;
use walkdir::WalkDir;

use crate::graph::{
    Edge, EdgeKind, FunctionGraph, Local, Node, NodeKind, ProgramGraph, SourceLocation,
};

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub call_depth: usize,
    pub keep_raw_mir: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            call_depth: 3,
            keep_raw_mir: true,
        }
    }
}

pub trait GraphExtractor {
    fn extract(&self, project: &Path, options: &ExtractOptions) -> Result<ProgramGraph>;
}

#[derive(Debug, Default)]
pub struct MirExtractor;

impl GraphExtractor for MirExtractor {
    fn extract(&self, project: &Path, options: &ExtractOptions) -> Result<ProgramGraph> {
        self.extract(project, options)
    }
}

impl MirExtractor {
    pub fn extract(&self, project: &Path, options: &ExtractOptions) -> Result<ProgramGraph> {
        let manifest = if project.is_file() {
            project.to_path_buf()
        } else {
            project.join("Cargo.toml")
        };
        if !manifest.exists() {
            bail!("no Cargo.toml at {}", manifest.display());
        }
        let manifest_text = fs::read_to_string(&manifest)?;
        let value: toml::Value = toml::from_str(&manifest_text)?;
        let crate_name = value
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or("unknown-crate")
            .to_owned();
        let root = manifest.parent().unwrap_or(Path::new("."));
        let dump_dir = temp_dump_dir();
        fs::create_dir_all(&dump_dir)?;

        let dump_arg = format!("-Zdump-mir-dir={}", dump_dir.display());
        let output = Command::new("cargo")
            .args(["rustc", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(dump_dir.join("target"))
            .args(["--", "-Zdump-mir=built", "-Zdump-mir-exclude-pass-number"])
            .arg(dump_arg)
            .output()
            .context("failed to invoke cargo rustc; a nightly toolchain is required")?;
        if !output.status.success() {
            bail!(
                "MIR extraction failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let sources = SourceIndex::load(root);
        let mut functions = Vec::new();
        for entry in WalkDir::new(&dump_dir).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.ends_with(".built.after.mir") {
                let mir = fs::read_to_string(path)?;
                if let Some(graph) = parse_mir_function(&mir, &sources, options.keep_raw_mir)? {
                    functions.push(graph);
                }
            }
        }
        let _ = fs::remove_dir_all(&dump_dir);
        functions.sort_by(|a, b| a.name.cmp(&b.name));
        if functions.is_empty() {
            bail!("rustc produced no parseable built MIR functions");
        }
        let mut metadata = BTreeMap::new();
        metadata.insert("extractor".into(), "nightly -Zdump-mir=built".into());
        metadata.insert("call_depth".into(), options.call_depth.to_string());
        metadata.insert("manifest".into(), manifest.display().to_string());
        Ok(ProgramGraph {
            schema_version: 1,
            crate_name,
            functions,
            metadata,
        })
    }
}

fn temp_dump_dir() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("mir-logic-{}-{stamp}", std::process::id()))
}

#[derive(Default)]
pub(crate) struct SourceIndex {
    lines: Vec<(PathBuf, usize, String)>,
}

impl SourceIndex {
    fn load(root: &Path) -> Self {
        let mut lines = Vec::new();
        for entry in WalkDir::new(root.join("src"))
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.path().extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            if let Ok(text) = fs::read_to_string(entry.path()) {
                lines.extend(
                    text.lines().enumerate().map(|(i, line)| {
                        (entry.path().to_path_buf(), i + 1, line.trim().to_owned())
                    }),
                );
            }
        }
        Self { lines }
    }

    fn find_call(&self, function: &str) -> Option<(SourceLocation, String)> {
        let needle = format!("{}(", function.rsplit("::").next().unwrap_or(function));
        self.lines
            .iter()
            .find(|(_, _, line)| line.contains(&needle) && !line.trim_start().starts_with("fn "))
            .map(|(p, line, text)| {
                (
                    SourceLocation {
                        file: p.display().to_string(),
                        line: *line,
                        column: text.find(&needle).map(|x| x + 1),
                    },
                    text.clone(),
                )
            })
    }

    fn file_for_function(&self, function: &str) -> Option<String> {
        let needle = format!("fn {function}");
        self.lines
            .iter()
            .find(|(_, _, l)| l.contains(&needle))
            .map(|x| x.0.display().to_string())
    }
}

pub(crate) fn parse_mir_function(
    mir: &str,
    sources: &SourceIndex,
    keep_raw: bool,
) -> Result<Option<FunctionGraph>> {
    let fn_re = Regex::new(r"(?m)^fn\s+([^\s(]+)\(")?;
    let Some(caps) = fn_re.captures(mir) else {
        return Ok(None);
    };
    let name = caps[1].to_owned();
    if name.starts_with('<') || name.contains("::{closure#") {
        return Ok(None);
    }
    let local_re = Regex::new(r"^\s*let\s+(mut\s+)?(_\d+):\s*(.+);$")?;
    let debug_re = Regex::new(r"^\s*debug\s+([^\s]+)\s*=>\s*(_\d+)")?;
    let block_re = Regex::new(r"^\s*bb(\d+)(?:\s*\(cleanup\))?:\s*\{")?;
    let target_re = Regex::new(r"\bbb(\d+)\b")?;
    let local_ref_re = Regex::new(r"\b_\d+\b")?;
    let call_re = Regex::new(r"^(.*?)=\s*([^=]+?)\((.*)\)\s*->\s*\[")?;
    let assignment_re = Regex::new(r"^(_\d+(?:\.[^ ]+)?)\s*=\s*(.+);$")?;

    let mut locals: HashMap<String, Local> = HashMap::new();
    for line in mir.lines() {
        if let Some(c) = local_re.captures(line) {
            locals.insert(
                c[2].to_owned(),
                Local {
                    compiler_name: c[2].to_owned(),
                    semantic_name: None,
                    ty: Some(c[3].to_owned()),
                    mutable: c.get(1).is_some(),
                },
            );
        } else if let Some(c) = debug_re.captures(line) {
            if let Some(local) = locals.get_mut(&c[2]) {
                local.semantic_name = Some(c[1].to_owned());
            } else {
                locals.insert(
                    c[2].to_owned(),
                    Local {
                        compiler_name: c[2].to_owned(),
                        semantic_name: Some(c[1].to_owned()),
                        ty: None,
                        mutable: false,
                    },
                );
            }
        }
    }

    let entry_id = format!("{name}::entry");
    let exit_id = format!("{name}::exit");
    let mut nodes = vec![basic_node(
        &entry_id,
        &name,
        NodeKind::FunctionEntry,
        None,
        "function entry",
    )];
    let mut edges = Vec::new();
    let mut blocks: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut current_block = None;
    let mut discriminants: HashMap<String, String> = HashMap::new();
    let lines: Vec<&str> = mir.lines().collect();

    for raw_line in lines {
        if let Some(c) = block_re.captures(raw_line) {
            current_block = Some(c[1].parse::<usize>()?);
            continue;
        }
        if raw_line.trim() == "}" {
            current_block = None;
            continue;
        }
        let Some(bb) = current_block else { continue };
        let line = raw_line.trim();
        if line.is_empty() || is_noise(line) {
            continue;
        }
        let kind = classify(line);
        let seq = blocks.get(&bb).map_or(0, Vec::len);
        let id = format!("{name}::bb{bb}::n{seq}");
        let mut node = basic_node(&id, &name, kind, Some(bb), line);

        if let Some(c) = call_re.captures(line) {
            let destination = c[1].trim().to_owned();
            let called = clean_callable(c[2].trim());
            node.called_function = Some(called.clone());
            let raw_arguments = split_arguments(c[3].trim());
            node.operation = Some("call".into());
            node.writes.extend(
                local_ref_re
                    .find_iter(&destination)
                    .map(|x| semantic_local(x.as_str(), &locals)),
            );
            node.reads.extend(raw_arguments.iter().flat_map(|arg| {
                local_ref_re
                    .find_iter(arg)
                    .map(|x| semantic_local(x.as_str(), &locals))
            }));
            node.arguments = raw_arguments
                .iter()
                .map(|argument| rewrite_locals(argument, &locals))
                .collect();
            if let Some(local) = local_ref_re
                .find(&destination)
                .and_then(|x| locals.get(x.as_str()))
            {
                node.ty = local.ty.clone();
                node.variable = Some(
                    local
                        .semantic_name
                        .clone()
                        .unwrap_or_else(|| local.compiler_name.clone()),
                );
            }
            if let Some((source, text)) = sources.find_call(&called) {
                node.source = Some(source);
                node.source_text = Some(text);
            }
        } else if line.starts_with("switchInt(") {
            let condition = line
                .strip_prefix("switchInt(")
                .and_then(|s| s.split_once(')'))
                .map(|x| x.0)
                .unwrap_or(line);
            node.branch_condition = Some(rewrite_locals(condition, &locals));
            node.reads.extend(
                local_ref_re
                    .find_iter(condition)
                    .map(|x| semantic_local(x.as_str(), &locals)),
            );
            node.operation = Some("switch_int".into());
            if let Some(local) = local_ref_re.find(condition) {
                let local_name = local.as_str();
                if let Some(source_local) = discriminants.get(local_name) {
                    node.ty = locals.get(source_local).and_then(|l| l.ty.clone());
                    node.variable = Some(semantic_local(source_local, &locals));
                    node.metadata
                        .insert("discriminant_of".into(), source_local.clone());
                } else if let Some(local) = locals.get(local_name) {
                    node.ty = local.ty.clone();
                    node.variable = Some(
                        local
                            .semantic_name
                            .clone()
                            .unwrap_or_else(|| local.compiler_name.clone()),
                    );
                }
            }
        } else if line.starts_with("assert(") {
            node.branch_condition = Some(rewrite_locals(line, &locals));
            node.operation = Some("assert".into());
        } else if let Some(c) = assignment_re.captures(line) {
            let lhs = c[1].to_owned();
            let rhs = c[2].to_owned();
            if let Some(inner) = rhs
                .strip_prefix("discriminant(")
                .and_then(|x| x.strip_suffix(')'))
            {
                discriminants.insert(lhs.clone(), inner.to_owned());
                node.operation = Some("discriminant".into());
                node.ty = locals.get(inner).and_then(|l| l.ty.clone());
            } else if rhs.contains(" as ") || lhs.contains('.') || rhs.contains(".0:") {
                node.operation = Some("projection_or_variant".into());
                node.variant = extract_variant(&rhs);
            } else {
                node.operation = Some("assign".into());
            }
            node.writes.extend(
                local_ref_re
                    .find_iter(&lhs)
                    .map(|x| semantic_local(x.as_str(), &locals)),
            );
            node.reads.extend(
                local_ref_re
                    .find_iter(&rhs)
                    .map(|x| semantic_local(x.as_str(), &locals)),
            );
            node.variable = Some(rewrite_locals(&lhs, &locals));
            node.text = Some(rewrite_locals(line, &locals));
        }
        node.metadata
            .insert("compiler_text".into(), line.to_owned());
        blocks.entry(bb).or_default().push(id.clone());
        nodes.push(node);
    }

    nodes.push(basic_node(
        &exit_id,
        &name,
        NodeKind::FunctionExit,
        None,
        "function exit",
    ));
    if let Some(first) = blocks.get(&0).and_then(|x| x.first()) {
        edges.push(edge(&entry_id, first, EdgeKind::ControlFlow, None));
    }
    let by_id: HashMap<_, _> = nodes.iter().map(|n| (n.id.clone(), n)).collect();
    for (bb, ids) in &blocks {
        for pair in ids.windows(2) {
            edges.push(edge(&pair[0], &pair[1], EdgeKind::ControlFlow, None));
        }
        let Some(last_id) = ids.last() else { continue };
        let Some(last) = by_id.get(last_id) else {
            continue;
        };
        let compiler = last
            .metadata
            .get("compiler_text")
            .map(String::as_str)
            .unwrap_or_default();
        let targets: Vec<usize> = target_re
            .captures_iter(compiler)
            .filter_map(|c| c[1].parse().ok())
            .collect();
        if last.kind == NodeKind::Return {
            edges.push(edge(last_id, &exit_id, EdgeKind::Return, None));
        } else {
            for target in targets {
                let Some(to) = blocks.get(&target).and_then(|x| x.first()) else {
                    continue;
                };
                let (kind, label) =
                    terminator_edge(last, compiler, target, &locals, &discriminants);
                edges.push(edge(last_id, to, kind, label));
            }
            if targets_is_empty(compiler) && matches!(last.kind, NodeKind::Unknown) {
                let _ = bb;
            }
        }
    }
    add_def_use_edges(&nodes, &mut edges);
    let source_file = sources.file_for_function(&name);
    Ok(Some(FunctionGraph {
        name,
        nodes,
        edges,
        locals: locals.into_values().collect(),
        source_file,
        raw_mir: keep_raw.then(|| mir.to_owned()),
    }))
}

fn classify(line: &str) -> NodeKind {
    if line.starts_with("switchInt(") || line.starts_with("falseEdge") {
        NodeKind::Branch
    } else if line.starts_with("assert(") {
        NodeKind::Assert
    } else if line == "return;" {
        NodeKind::Return
    } else if line == "unreachable;" || line == "resume;" {
        NodeKind::Error
    } else if line.contains(" -> [") && line.contains('(') {
        NodeKind::Call
    } else if line.contains(" = ") {
        NodeKind::Assignment
    } else {
        NodeKind::Unknown
    }
}

fn is_noise(line: &str) -> bool {
    line.starts_with("StorageLive(")
        || line.starts_with("StorageDead(")
        || line.starts_with("FakeRead(")
        || line.starts_with("PlaceMention(")
        || line == "nop;"
}

fn basic_node(id: &str, function: &str, kind: NodeKind, block: Option<usize>, text: &str) -> Node {
    Node {
        id: id.into(),
        function: function.into(),
        block,
        kind,
        source: None,
        source_text: None,
        text: Some(text.into()),
        ty: None,
        variable: None,
        operation: None,
        branch_condition: None,
        called_function: None,
        arguments: vec![],
        variant: None,
        reads: vec![],
        writes: vec![],
        metadata: BTreeMap::new(),
    }
}

fn edge(from: &str, to: &str, kind: EdgeKind, label: Option<String>) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        kind,
        label,
        metadata: BTreeMap::new(),
    }
}

fn semantic_local(name: &str, locals: &HashMap<String, Local>) -> String {
    locals
        .get(name)
        .and_then(|l| l.semantic_name.clone())
        .unwrap_or_else(|| name.to_owned())
}

fn rewrite_locals(text: &str, locals: &HashMap<String, Local>) -> String {
    let re = Regex::new(r"\b_\d+\b").expect("static regex");
    re.replace_all(text, |c: &regex::Captures<'_>| {
        semantic_local(&c[0], locals)
    })
    .into_owned()
}

fn clean_callable(text: &str) -> String {
    text.trim_start_matches("move ")
        .trim_start_matches("copy ")
        .trim_matches('<')
        .trim_matches('>')
        .to_owned()
}

fn split_arguments(args: &str) -> Vec<String> {
    if args.is_empty() {
        return vec![];
    }
    args.split(',').map(|x| x.trim().to_owned()).collect()
}

fn extract_variant(rhs: &str) -> Option<String> {
    let start = rhs.find(" as ")? + 4;
    let end = rhs[start..].find(')')? + start;
    Some(rhs[start..end].to_owned())
}

fn terminator_edge(
    last: &Node,
    text: &str,
    target: usize,
    locals: &HashMap<String, Local>,
    discriminants: &HashMap<String, String>,
) -> (EdgeKind, Option<String>) {
    let unwind_re = Regex::new(&format!(r"unwind:\s*bb{target}\b")).expect("generated regex");
    if unwind_re.is_match(text) {
        return (EdgeKind::Unwind, Some("unwind".into()));
    }
    if text.starts_with("switchInt") {
        let case_re = Regex::new(&format!(r"([^,\[]+):\s*bb{target}\b")).expect("generated regex");
        let raw = case_re.captures(text).map(|c| c[1].trim().to_owned());
        let ty = last.ty.as_deref().unwrap_or_default();
        let label = raw
            .map(|case| variant_label(ty, &case))
            .or_else(|| Some("otherwise".into()));
        let kind = match label.as_deref() {
            Some("true") => EdgeKind::TrueBranch,
            Some("false") => EdgeKind::FalseBranch,
            _ => EdgeKind::SwitchCase,
        };
        return (kind, label);
    }
    let _ = (locals, discriminants);
    (EdgeKind::ControlFlow, None)
}

fn variant_label(ty: &str, case: &str) -> String {
    if ty.contains("Result<") || ty.contains("result::Result<") {
        match case {
            "0" => "Result::Ok".into(),
            "1" => "Result::Err".into(),
            _ => case.into(),
        }
    } else if ty.contains("Option<") || ty.contains("option::Option<") {
        match case {
            "0" => "Option::None".into(),
            "1" => "Option::Some".into(),
            _ => case.into(),
        }
    } else if ty == "bool" {
        match case {
            "0" => "false".into(),
            "1" => "true".into(),
            _ => case.into(),
        }
    } else {
        case.into()
    }
}

fn targets_is_empty(text: &str) -> bool {
    !text.contains("bb")
}

fn add_def_use_edges(nodes: &[Node], edges: &mut Vec<Edge>) {
    let mut defs: HashMap<&str, &str> = HashMap::new();
    for node in nodes {
        for read in &node.reads {
            if let Some(from) = defs.get(read.as_str())
                && *from != node.id
            {
                edges.push(edge(
                    from,
                    &node.id,
                    EdgeKind::DataDependency,
                    Some(read.clone()),
                ));
            }
        }
        for write in &node.writes {
            defs.insert(write, &node.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_calls_switches_and_variants() {
        let mir = r#"// MIR for `login` after built
fn login(_1: &str) -> () {
 debug password => _1;
 let mut _2: std::result::Result<User, AuthError>;
 let mut _3: isize;
 bb0: {
  _2 = authenticate(copy _1) -> [return: bb1, unwind: bb3];
 }
 bb1: {
  _3 = discriminant(_2);
  switchInt(move _3) -> [0: bb2, 1: bb3, otherwise: bb4];
 }
 bb2: {
  return;
 }
 bb3: {
  _0 = create_session() -> [return: bb2, unwind continue];
 }
 bb4: {
  unreachable;
 }
}"#;
        let graph = parse_mir_function(mir, &SourceIndex::default(), true)
            .unwrap()
            .unwrap();
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.called_function.as_deref() == Some("authenticate"))
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|e| e.label.as_deref() == Some("Result::Err"))
        );
    }
}
