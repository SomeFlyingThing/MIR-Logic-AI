use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    RemoveFailureReturn,
    ContinueAfterError,
    RemoveAuthFailureReturn,
    InvertBooleanCondition,
    AndToOr,
    OrToAnd,
    RemovePermissionCall,
    BypassValidation,
    MoveSensitiveOperationBeforeCheck,
    IgnoreErrorResult,
    SwapMatchArms,
    RemoveStateValidation,
    SkipRequiredState,
    UseAfterClose,
    UseBeforeOpen,
    CommitAfterRollback,
    CommitOnFailure,
    RemoveInitialization,
    BypassGuard,
    IncorrectSuccessTransition,
    IncorrectFailureTransition,
}

impl MutationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RemoveFailureReturn => "remove_failure_return",
            Self::ContinueAfterError => "continue_after_error",
            Self::RemoveAuthFailureReturn => "remove_auth_failure_return",
            Self::InvertBooleanCondition => "invert_boolean_condition",
            Self::AndToOr => "and_to_or",
            Self::OrToAnd => "or_to_and",
            Self::RemovePermissionCall => "remove_permission_call",
            Self::BypassValidation => "bypass_validation",
            Self::MoveSensitiveOperationBeforeCheck => "move_sensitive_operation_before_check",
            Self::IgnoreErrorResult => "ignore_error_result",
            Self::SwapMatchArms => "swap_match_arms",
            Self::RemoveStateValidation => "remove_state_validation",
            Self::SkipRequiredState => "skip_required_state",
            Self::UseAfterClose => "use_after_close",
            Self::UseBeforeOpen => "use_before_open",
            Self::CommitAfterRollback => "commit_after_rollback",
            Self::CommitOnFailure => "commit_on_failure",
            Self::RemoveInitialization => "remove_initialization",
            Self::BypassGuard => "bypass_guard",
            Self::IncorrectSuccessTransition => "incorrect_success_transition",
            Self::IncorrectFailureTransition => "incorrect_failure_transition",
        }
    }
}

impl std::str::FromStr for MutationKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "remove_failure_return" => Ok(Self::RemoveFailureReturn),
            "continue_after_error" => Ok(Self::ContinueAfterError),
            "remove_auth_failure_return" => Ok(Self::RemoveAuthFailureReturn),
            "invert_boolean_condition" => Ok(Self::InvertBooleanCondition),
            "and_to_or" => Ok(Self::AndToOr),
            "or_to_and" => Ok(Self::OrToAnd),
            "remove_permission_call" => Ok(Self::RemovePermissionCall),
            "ignore_error_result" => Ok(Self::IgnoreErrorResult),
            "swap_match_arms" => Ok(Self::SwapMatchArms),
            "remove_state_validation" => Ok(Self::RemoveStateValidation),
            "bypass_validation" => Ok(Self::BypassValidation),
            "move_sensitive_operation_before_check" => Ok(Self::MoveSensitiveOperationBeforeCheck),
            "skip_required_state" => Ok(Self::SkipRequiredState),
            "use_after_close" => Ok(Self::UseAfterClose),
            "use_before_open" => Ok(Self::UseBeforeOpen),
            "commit_after_rollback" => Ok(Self::CommitAfterRollback),
            "commit_on_failure" => Ok(Self::CommitOnFailure),
            "remove_initialization" => Ok(Self::RemoveInitialization),
            "bypass_guard" => Ok(Self::BypassGuard),
            "incorrect_success_transition" => Ok(Self::IncorrectSuccessTransition),
            "incorrect_failure_transition" => Ok(Self::IncorrectFailureTransition),
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
        MutationKind::RemoveFailureReturn | MutationKind::ContinueAfterError => {
            replace_once(source, "return handle_failure();", "handle_failure();")
        }
        MutationKind::RemoveAuthFailureReturn => replace_once(
            source,
            "return Err(Error::Unauthorized);",
            "eprintln!(\"authentication failed\");",
        ),
        MutationKind::InvertBooleanCondition => replace_once(source, "if is_valid", "if !is_valid"),
        MutationKind::AndToOr => replace_once(source, " && ", " || "),
        MutationKind::OrToAnd => replace_once(source, " || ", " && "),
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
        MutationKind::BypassValidation => replace_once(
            source,
            "if !validate_payload(input) { return false; }",
            "let _ = validate_payload(input);",
        ),
        MutationKind::MoveSensitiveOperationBeforeCheck => replace_once(
            source,
            "check_permission();\nsensitive_operation();",
            "sensitive_operation();\ncheck_permission();",
        ),
        MutationKind::SkipRequiredState => replace_once(
            source,
            "transition(State::Ready);",
            "transition(State::Active);",
        ),
        MutationKind::UseAfterClose => replace_once(
            source,
            "close(resource);",
            "close(resource);\nuse_resource(resource);",
        ),
        MutationKind::UseBeforeOpen => replace_once(
            source,
            "let resource = open_resource();",
            "use_resource(default_resource());\nlet resource = open_resource();",
        ),
        MutationKind::CommitAfterRollback => replace_once(
            source,
            "rollback(transaction);",
            "rollback(transaction);\ncommit(transaction);",
        ),
        MutationKind::CommitOnFailure => replace_once(
            source,
            "return rollback(transaction);",
            "rollback(transaction);\ncommit(transaction);",
        ),
        MutationKind::RemoveInitialization => replace_once(
            source,
            "initialize(component);",
            "// initialization removed",
        ),
        MutationKind::BypassGuard => {
            replace_once(source, "let _guard = lock();", "// guard bypassed")
        }
        MutationKind::IncorrectSuccessTransition => {
            replace_once(source, "State::Ready", "State::Failed")
        }
        MutationKind::IncorrectFailureTransition => {
            replace_once(source, "State::Failed", "State::Ready")
        }
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

    #[test]
    fn mutation_precondition_rejects_unrelated_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.rs");
        let output = directory.path().join("output.rs");
        fs::write(&source, "fn harmless() {}").unwrap();
        assert!(mutate_file(&source, &output, MutationKind::UseAfterClose).is_err());
        assert!(!output.exists());
    }
}
