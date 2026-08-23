use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    RemoveAuthFailureReturn,
    InvertBooleanCondition,
    AndToOr,
    RemovePermissionCall,
    IgnoreErrorResult,
    SwapMatchArms,
    RemoveStateValidation,
}

impl std::str::FromStr for MutationKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "remove_auth_failure_return" => Ok(Self::RemoveAuthFailureReturn),
            "invert_boolean_condition" => Ok(Self::InvertBooleanCondition),
            "and_to_or" => Ok(Self::AndToOr),
            "remove_permission_call" => Ok(Self::RemovePermissionCall),
            "ignore_error_result" => Ok(Self::IgnoreErrorResult),
            "swap_match_arms" => Ok(Self::SwapMatchArms),
            "remove_state_validation" => Ok(Self::RemoveStateValidation),
            _ => bail!("unknown mutation {s}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationMetadata {
    pub mutation: MutationKind,
    pub source: PathBuf,
    pub output: PathBuf,
    pub replacements: usize,
    pub label: String,
}

pub fn mutate_file(source: &Path, output: &Path, kind: MutationKind) -> Result<MutationMetadata> {
    let original =
        fs::read_to_string(source).with_context(|| format!("read {}", source.display()))?;
    let (mutated, replacements) = apply(&original, &kind);
    if replacements == 0 {
        bail!("mutation pattern was not found in {}", source.display());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, mutated)?;
    Ok(MutationMetadata {
        mutation: kind,
        source: source.into(),
        output: output.into(),
        replacements,
        label: "logic_bug".into(),
    })
}

fn apply(source: &str, kind: &MutationKind) -> (String, usize) {
    match kind {
        MutationKind::RemoveAuthFailureReturn => replace_once(
            source,
            "return Err(Error::Unauthorized);",
            "eprintln!(\"authentication failed\");",
        ),
        MutationKind::InvertBooleanCondition => replace_once(source, "if is_valid", "if !is_valid"),
        MutationKind::AndToOr => replace_once(source, " && ", " || "),
        MutationKind::RemovePermissionCall => replace_once(
            source,
            "check_permission(user)?;",
            "// MIR-LOGIC mutation: permission check removed",
        ),
        MutationKind::IgnoreErrorResult => {
            replace_once(source, "operation()?;", "let _ = operation();")
        }
        MutationKind::SwapMatchArms => replace_once(
            source,
            "Ok(value) => use_value(value),\n        Err(error) => handle_error(error),",
            "Ok(value) => handle_error(Default::default()),\n        Err(_) => use_value(Default::default()),",
        ),
        MutationKind::RemoveStateValidation => replace_once(
            source,
            "validate_state(state)?;",
            "// MIR-LOGIC mutation: validation removed",
        ),
    }
}

fn replace_once(source: &str, from: &str, to: &str) -> (String, usize) {
    if source.contains(from) {
        (source.replacen(from, to, 1), 1)
    } else {
        (source.to_owned(), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_early_return() {
        let (out, count) = apply(
            "return Err(Error::Unauthorized);\ncreate_session();",
            &MutationKind::RemoveAuthFailureReturn,
        );
        assert_eq!(count, 1);
        assert!(out.contains("authentication failed"));
    }
}
