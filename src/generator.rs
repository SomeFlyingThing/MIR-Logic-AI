//! Reproducible synthetic logic-bug corpus generation.
//!
//! A scenario is semantic metadata. Rendering is a separate step, so the same
//! invariant can be expressed using different Rust types, vocabularies, CFG
//! templates, call depths, and semantics-preserving noise transformations.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    extractor::{ExtractOptions, MirExtractor},
    graph::{SemanticFunction, SemanticGraph},
    mutation::MutationKind,
    simplify::simplify,
};

pub const GENERATED_SCHEMA_VERSION: u32 = 2;
pub const GENERATOR_VERSION: &str = "mir-logic-synth-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    Authentication,
    Authorization,
    Validation,
    ResourceLifecycle,
    Transaction,
    StateMachine,
    Initialization,
    GuardedState,
    Capability,
}

impl Domain {
    pub const ALL: [Self; 9] = [
        Self::Authentication,
        Self::Authorization,
        Self::Validation,
        Self::ResourceLifecycle,
        Self::Transaction,
        Self::StateMachine,
        Self::Initialization,
        Self::GuardedState,
        Self::Capability,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Authorization => "authorization",
            Self::Validation => "validation",
            Self::ResourceLifecycle => "resource_lifecycle",
            Self::Transaction => "transaction",
            Self::StateMachine => "state_machine",
            Self::Initialization => "initialization",
            Self::GuardedState => "guarded_state",
            Self::Capability => "capability",
        }
    }

    fn invariant(self) -> &'static str {
        match self {
            Self::Authentication => {
                "privileged context creation requires successful identity verification"
            }
            Self::Authorization => {
                "a sensitive operation requires an affirmative permission decision"
            }
            Self::Validation => {
                "untrusted input must validate before sensitive persistence or consumption"
            }
            Self::ResourceLifecycle => {
                "resource use requires a successfully opened, non-closed resource"
            }
            Self::Transaction => {
                "commit requires a valid, non-failed and non-rolled-back transaction"
            }
            Self::StateMachine => {
                "the active state is reachable only through the required valid transition"
            }
            Self::Initialization => "component use requires successful initialization",
            Self::GuardedState => "protected mutation requires the guard to be acquired",
            Self::Capability => "the feature operation requires a present capability",
        }
    }

    fn bug_type(self) -> &'static str {
        match self {
            Self::Authentication => "authentication_bypass",
            Self::Authorization => "permission_bypass",
            Self::Validation => "validation_bypass",
            Self::ResourceLifecycle => "resource_lifecycle_violation",
            Self::Transaction => "invalid_transaction_commit",
            Self::StateMachine => "invalid_state_transition",
            Self::Initialization => "use_before_initialization",
            Self::GuardedState => "guard_bypass",
            Self::Capability => "capability_bypass",
        }
    }

    fn mutation(self) -> MutationKind {
        match self {
            Self::Authentication => MutationKind::ContinueAfterError,
            Self::Authorization => MutationKind::RemovePermissionCall,
            Self::Validation => MutationKind::BypassValidation,
            Self::ResourceLifecycle => MutationKind::UseAfterClose,
            Self::Transaction => MutationKind::CommitOnFailure,
            Self::StateMachine => MutationKind::IncorrectFailureTransition,
            Self::Initialization => MutationKind::RemoveInitialization,
            Self::GuardedState => MutationKind::BypassGuard,
            Self::Capability => MutationKind::SkipRequiredState,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Template {
    MatchResult,
    IfLet,
    EarlyReturn,
    BooleanGuard,
    OptionMatch,
    NestedBranch,
    LetElse,
    DeepHelper,
}

impl Template {
    pub const ALL: [Self; 8] = [
        Self::MatchResult,
        Self::IfLet,
        Self::EarlyReturn,
        Self::BooleanGuard,
        Self::OptionMatch,
        Self::NestedBranch,
        Self::LetElse,
        Self::DeepHelper,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MatchResult => "match_result",
            Self::IfLet => "if_let",
            Self::EarlyReturn => "early_return",
            Self::BooleanGuard => "boolean_guard",
            Self::OptionMatch => "option_match",
            Self::NestedBranch => "nested_branch",
            Self::LetElse => "let_else",
            Self::DeepHelper => "deep_helper",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierMode {
    Semantic,
    Neutral,
    Randomized,
}

impl IdentifierMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Neutral => "neutral",
            Self::Randomized => "randomized",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StateRepresentation {
    UnitStruct,
    TupleNewtype,
    StructField,
    Enum,
    Boolean,
    IntegerCode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ErrorRepresentation {
    CustomEnum,
    CustomStruct,
    Unit,
    Option,
    Boolean,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum VocabularyKind {
    Primary,
    Alternate,
    Holdout,
}

impl VocabularyKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Alternate => "alternate",
            Self::Holdout => "holdout",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeSet {
    IdentifierBlind,
    UnseenVocabulary,
    UnseenTopology,
    DeepCallGraph,
    Noise,
    HardNegative,
}

impl ChallengeSet {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IdentifierBlind => "identifier_blind",
            Self::UnseenVocabulary => "unseen_vocabulary",
            Self::UnseenTopology => "unseen_topology",
            Self::DeepCallGraph => "deep_call_graph",
            Self::Noise => "noise",
            Self::HardNegative => "hard_negative",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSplit {
    Train,
    Validation,
    Test,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Scenario {
    pub id: String,
    pub seed: u64,
    pub domain: Domain,
    pub template: Template,
    pub vocabulary: VocabularyKind,
    pub identifier_mode: IdentifierMode,
    pub state_representation: StateRepresentation,
    pub error_representation: ErrorRepresentation,
    pub call_depth: usize,
    pub noise_level: usize,
    pub challenge_set: Option<ChallengeSet>,
    pub invariant: String,
    pub bug_type: String,
    pub mutation: MutationKind,
    pub origin_group: String,
    pub split: DatasetSplit,
}

impl Scenario {
    pub fn from_index(root_seed: u64, index: usize, split_enabled: bool) -> Self {
        let seed = mix64(root_seed ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut rng = StableRng::new(seed);
        let domain = Domain::ALL[(index + rng.range(Domain::ALL.len())) % Domain::ALL.len()];
        let mut template = Template::ALL
            [(index / Domain::ALL.len() + rng.range(Template::ALL.len())) % Template::ALL.len()];
        let mut vocabulary = if rng.range(3) == 0 {
            VocabularyKind::Alternate
        } else {
            VocabularyKind::Primary
        };
        let mut identifier_mode = if rng.range(5) == 0 {
            IdentifierMode::Randomized
        } else {
            IdentifierMode::Semantic
        };
        let mut call_depth = rng.range(3);
        let mut noise_level = rng.range(3);
        let mut state_representation = match rng.range(5) {
            0 => StateRepresentation::UnitStruct,
            1 => StateRepresentation::TupleNewtype,
            2 => StateRepresentation::StructField,
            3 => StateRepresentation::Enum,
            _ => StateRepresentation::IntegerCode,
        };
        let mut error_representation = match rng.range(3) {
            0 => ErrorRepresentation::CustomEnum,
            1 => ErrorRepresentation::CustomStruct,
            _ => ErrorRepresentation::Unit,
        };
        let challenge_set = if index.is_multiple_of(5) {
            Some(match (index / 5) % 6 {
                0 => ChallengeSet::IdentifierBlind,
                1 => ChallengeSet::UnseenVocabulary,
                2 => ChallengeSet::UnseenTopology,
                3 => ChallengeSet::DeepCallGraph,
                4 => ChallengeSet::Noise,
                _ => ChallengeSet::HardNegative,
            })
        } else {
            None
        };
        match challenge_set {
            Some(ChallengeSet::IdentifierBlind) => identifier_mode = IdentifierMode::Neutral,
            Some(ChallengeSet::UnseenVocabulary) => vocabulary = VocabularyKind::Holdout,
            Some(ChallengeSet::UnseenTopology) => template = Template::LetElse,
            Some(ChallengeSet::DeepCallGraph) => {
                template = Template::DeepHelper;
                call_depth = 3 + rng.range(3);
            }
            Some(ChallengeSet::Noise) => noise_level = 4 + rng.range(4),
            Some(ChallengeSet::HardNegative) | None => {}
        }
        if template == Template::BooleanGuard {
            call_depth = 0;
            state_representation = StateRepresentation::Boolean;
            error_representation = ErrorRepresentation::Boolean;
        } else if template == Template::OptionMatch {
            error_representation = ErrorRepresentation::Option;
        }
        let split = if !split_enabled {
            DatasetSplit::Train
        } else if challenge_set.is_some()
            || vocabulary == VocabularyKind::Holdout
            || matches!(template, Template::LetElse | Template::DeepHelper)
        {
            DatasetSplit::Test
        } else if matches!(template, Template::OptionMatch | Template::NestedBranch) {
            DatasetSplit::Validation
        } else {
            DatasetSplit::Train
        };
        let origin_group = format!(
            "{}:{}:{}:{}:{}",
            domain.as_str(),
            template.as_str(),
            vocabulary.as_str(),
            identifier_mode.as_str(),
            challenge_set
                .map(ChallengeSet::as_str)
                .unwrap_or("standard")
        );
        Self {
            id: format!("s{root_seed:016x}-{index:08}"),
            seed,
            domain,
            template,
            vocabulary,
            identifier_mode,
            state_representation,
            error_representation,
            call_depth,
            noise_level,
            challenge_set,
            invariant: domain.invariant().into(),
            bug_type: domain.bug_type().into(),
            mutation: domain.mutation(),
            origin_group,
            split,
        }
    }
}

#[derive(Debug, Clone)]
struct Vocabulary {
    check: String,
    action: String,
    failure: String,
    fallback: String,
    token: String,
    error: String,
}

impl Vocabulary {
    fn for_scenario(scenario: &Scenario) -> Self {
        let words = vocabulary_words(scenario.domain, scenario.vocabulary);
        match scenario.identifier_mode {
            IdentifierMode::Semantic => Self::from_words(words),
            IdentifierMode::Neutral => Self {
                check: "operation_a".into(),
                action: "operation_b".into(),
                failure: "operation_c".into(),
                fallback: "operation_d".into(),
                token: "StateA".into(),
                error: "StateB".into(),
            },
            IdentifierMode::Randomized => {
                let tag = format!("{:08x}", scenario.seed as u32);
                Self {
                    check: format!("op_{tag}_a"),
                    action: format!("op_{tag}_b"),
                    failure: format!("op_{tag}_c"),
                    fallback: format!("op_{tag}_d"),
                    token: format!("State{tag}A"),
                    error: format!("State{tag}B"),
                }
            }
        }
    }

    fn from_words(words: [&'static str; 6]) -> Self {
        Self {
            check: words[0].into(),
            action: words[1].into(),
            failure: words[2].into(),
            fallback: words[3].into(),
            token: words[4].into(),
            error: words[5].into(),
        }
    }
}

fn vocabulary_words(domain: Domain, kind: VocabularyKind) -> [&'static str; 6] {
    let index = match kind {
        VocabularyKind::Primary => 0,
        VocabularyKind::Alternate => 1,
        VocabularyKind::Holdout => 2,
    };
    const WORDS: [[[&str; 6]; 3]; 9] = [
        [
            [
                "authenticate",
                "create_session",
                "record_auth_failure",
                "anonymous_identity",
                "Identity",
                "CredentialRejected",
            ],
            [
                "verify_password",
                "establish_context",
                "audit_login_error",
                "guest_principal",
                "Principal",
                "LoginRefused",
            ],
            [
                "establish_identity",
                "materialize_principal_context",
                "trace_credential_refusal",
                "public_actor",
                "Actor",
                "ProofDeclined",
            ],
        ],
        [
            [
                "check_permission",
                "sensitive_operation",
                "audit_denied",
                "limited_capability",
                "Permission",
                "AccessDenied",
            ],
            [
                "authorize_access",
                "write_protected_record",
                "log_forbidden",
                "read_only_grant",
                "Grant",
                "Forbidden",
            ],
            [
                "evaluate_entitlement",
                "apply_restricted_change",
                "observe_entitlement_absence",
                "public_entitlement",
                "Entitlement",
                "PrivilegeMissing",
            ],
        ],
        [
            [
                "validate_payload",
                "persist_record",
                "report_invalid",
                "sanitized_default",
                "ValidatedPayload",
                "InvalidPayload",
            ],
            [
                "check_input",
                "consume_sensitive_value",
                "note_malformed",
                "safe_default",
                "CheckedInput",
                "MalformedInput",
            ],
            [
                "certify_message",
                "store_certified_message",
                "observe_rejection",
                "empty_certified_message",
                "CertifiedMessage",
                "MessageUnfit",
            ],
        ],
        [
            [
                "open_resource",
                "write_resource",
                "log_resource_closed",
                "metadata_handle",
                "OpenHandle",
                "ResourceClosed",
            ],
            [
                "acquire_stream",
                "read_stream",
                "note_unavailable_stream",
                "metadata_stream",
                "LiveStream",
                "StreamUnavailable",
            ],
            [
                "materialize_channel",
                "modify_channel",
                "observe_channel_absence",
                "descriptor_only_channel",
                "ActiveChannel",
                "ChannelDormant",
            ],
        ],
        [
            [
                "begin_transaction",
                "commit_transaction",
                "rollback_transaction",
                "empty_transaction",
                "Transaction",
                "TransactionFailed",
            ],
            [
                "start_unit_of_work",
                "publish_changes",
                "revert_changes",
                "no_op_unit",
                "UnitOfWork",
                "WorkAborted",
            ],
            [
                "stage_revision",
                "finalize_revision",
                "discard_revision",
                "blank_revision",
                "Revision",
                "RevisionRejected",
            ],
        ],
        [
            [
                "validate_transition",
                "transition_to_active",
                "log_invalid_state",
                "failed_state",
                "ReadyState",
                "InvalidState",
            ],
            [
                "check_phase",
                "enter_ready_phase",
                "record_bad_phase",
                "idle_phase",
                "VerifiedPhase",
                "PhaseRejected",
            ],
            [
                "certify_lifecycle_step",
                "advance_to_serving",
                "observe_lifecycle_refusal",
                "dormant_step",
                "CertifiedStep",
                "LifecycleBlocked",
            ],
        ],
        [
            [
                "initialize_component",
                "use_component",
                "report_init_failure",
                "disabled_component",
                "InitializedComponent",
                "InitFailed",
            ],
            [
                "prepare_service",
                "run_service",
                "note_prepare_error",
                "stub_service",
                "PreparedService",
                "PreparationFailed",
            ],
            [
                "prime_subsystem",
                "exercise_subsystem",
                "observe_priming_refusal",
                "inert_subsystem",
                "PrimedSubsystem",
                "PrimingDeclined",
            ],
        ],
        [
            [
                "acquire_guard",
                "mutate_protected_state",
                "audit_lock_failure",
                "read_guard",
                "WriteGuard",
                "LockDenied",
            ],
            [
                "lock_exclusive",
                "update_guarded_value",
                "note_lock_busy",
                "shared_guard",
                "ExclusiveLease",
                "LeaseUnavailable",
            ],
            [
                "obtain_mutation_lease",
                "alter_leased_state",
                "observe_lease_refusal",
                "inspection_lease",
                "MutationLease",
                "LeaseRefused",
            ],
        ],
        [
            [
                "check_capability",
                "perform_feature_operation",
                "report_missing_capability",
                "basic_capability",
                "Capability",
                "CapabilityMissing",
            ],
            [
                "verify_feature",
                "execute_optional_feature",
                "note_feature_disabled",
                "fallback_feature",
                "FeatureGrant",
                "FeatureDisabled",
            ],
            [
                "resolve_facility",
                "invoke_facility",
                "observe_facility_absence",
                "baseline_facility",
                "FacilityToken",
                "FacilityAbsent",
            ],
        ],
    ];
    WORDS[domain as usize][index]
}

#[derive(Debug, Clone)]
struct RenderedPair {
    scenario: Scenario,
    good_source: String,
    bad_source: String,
    good_prefix: String,
    bad_prefix: String,
    mutation_region: SourceRegion,
    transformations: Vec<String>,
    expected_bad_path: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceRegion {
    pub marker: String,
    pub start_line: usize,
    pub end_line: usize,
    pub before: String,
    pub after: String,
}

fn render_pair(scenario: Scenario, index: usize) -> RenderedPair {
    let base = format!("case_{index:08}");
    let good_prefix = format!("{base}_good");
    let bad_prefix = format!("{base}_bad");
    let vocab = Vocabulary::for_scenario(&scenario);
    let good_source = render_side(&scenario, &vocab, &good_prefix, false);
    let bad_source = render_side(&scenario, &vocab, &bad_prefix, true);
    let (start_line, before) = marker_line(&good_source, "MIR_LOGIC_MUTATION_POINT");
    let (bad_line, after) = marker_line(&bad_source, "MIR_LOGIC_MUTATION_POINT");
    let mut transformations = Vec::new();
    if scenario.noise_level > 0 {
        transformations.push("add_irrelevant_computation".into());
    }
    if scenario.call_depth > 0 {
        transformations.push("extract_helper_functions".into());
    }
    if scenario.identifier_mode != IdentifierMode::Semantic {
        transformations.push("rename_identifiers".into());
    }
    if scenario.challenge_set == Some(ChallengeSet::HardNegative) {
        transformations.push("insert_suspicious_but_benign_failure_action".into());
    }
    RenderedPair {
        scenario,
        good_source,
        bad_source,
        good_prefix,
        bad_prefix,
        mutation_region: SourceRegion {
            marker: "MIR_LOGIC_MUTATION_POINT".into(),
            start_line,
            end_line: bad_line,
            before,
            after,
        },
        transformations,
        expected_bad_path: vec![vocab.check, "failure_edge".into(), vocab.action],
    }
}

fn marker_line(source: &str, marker: &str) -> (usize, String) {
    source
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(marker))
        .map(|(line, text)| (line + 1, text.trim().into()))
        .unwrap_or((0, String::new()))
}

fn render_side(s: &Scenario, vocab: &Vocabulary, prefix: &str, bad: bool) -> String {
    let check = format!("{prefix}_{}", vocab.check);
    let action_base = format!("{prefix}_{}", vocab.action);
    let failure = if s.challenge_set == Some(ChallengeSet::HardNegative) {
        action_base.clone()
    } else {
        format!("{prefix}_{}", vocab.failure)
    };
    let action = if s.challenge_set == Some(ChallengeSet::HardNegative) {
        format!("{action_base}_privileged")
    } else {
        action_base
    };
    let fallback = format!("{prefix}_{}", vocab.fallback);
    let token = format!("{}{}", camel(prefix), vocab.token);
    let error = format!("{}{}", camel(prefix), vocab.error);
    let (token_source, token_value) = token_representation(s.state_representation, &token);
    let (error_source, error_type, error_value) =
        error_representation(s.error_representation, &error);
    let controller = format!("{prefix}_controller");
    let success = render_success_chain(prefix, &action, &token, s.call_depth, s.noise_level);
    let invoke = if s.call_depth == 0 {
        format!("{action}(token)")
    } else {
        format!("{prefix}_helper_0(token)")
    };
    let failure_call = format!("let _ = {failure}();");
    let bad_failure = format!(
        "{failure_call}\n            // MIR_LOGIC_MUTATION_POINT\n            {invoke_with_fallback}",
        invoke_with_fallback = if s.call_depth == 0 {
            format!("{action}({fallback}())")
        } else {
            format!("{prefix}_helper_0({fallback}())")
        }
    );
    let good_failure =
        format!("{failure_call}\n            // MIR_LOGIC_MUTATION_POINT\n            false");
    let result_check = format!(
        "fn {check}(accepted: bool) -> Result<{token}, {error_type}> {{ if accepted {{ Ok({token_value}) }} else {{ Err({error_value}) }} }}"
    );
    let option_check = format!(
        "fn {check}(accepted: bool) -> Option<{token}> {{ accepted.then_some({token_value}) }}"
    );
    let bool_check = format!("fn {check}(accepted: bool) -> bool {{ accepted }}");
    let controller_body = match s.template {
        Template::MatchResult | Template::DeepHelper => format!(
            "match {check}(accepted) {{\n        Ok(token) => {invoke},\n        Err(_) => {{\n            {}\n        }}\n    }}",
            if bad { &bad_failure } else { &good_failure }
        ),
        Template::IfLet => format!(
            "if let Ok(token) = {check}(accepted) {{\n        {invoke}\n    }} else {{\n        {}\n    }}",
            if bad { &bad_failure } else { &good_failure }
        ),
        Template::EarlyReturn => {
            if bad {
                format!(
                    "let token = match {check}(accepted) {{ Ok(token) => token, Err(_) => {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ {fallback}() }} }};\n    {noise}\n    {invoke}",
                    noise = noise_block(s.noise_level)
                )
            } else {
                format!(
                    "let token = match {check}(accepted) {{ Ok(token) => token, Err(_) => {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ return false; }} }};\n    {noise}\n    {invoke}",
                    noise = noise_block(s.noise_level)
                )
            }
        }
        Template::BooleanGuard => {
            if bad {
                format!(
                    "if !{check}(accepted) {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ }}\n    {noise}\n    {action}()",
                    noise = noise_block(s.noise_level)
                )
            } else {
                format!(
                    "if !{check}(accepted) {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ return false; }}\n    {noise}\n    {action}()",
                    noise = noise_block(s.noise_level)
                )
            }
        }
        Template::OptionMatch => format!(
            "match {check}(accepted) {{\n        Some(token) => {invoke},\n        None => {{\n            {}\n        }}\n    }}",
            if bad { &bad_failure } else { &good_failure }
        ),
        Template::NestedBranch => {
            if bad {
                format!(
                    "if accepted {{ if let Ok(token) = {check}(true) {{ {invoke} }} else {{ false }} }} else {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ {fallback_invoke} }}",
                    fallback_invoke = if s.call_depth == 0 {
                        format!("{action}({fallback}())")
                    } else {
                        format!("{prefix}_helper_0({fallback}())")
                    }
                )
            } else {
                format!(
                    "if accepted {{ if let Ok(token) = {check}(true) {{ {invoke} }} else {{ false }} }} else {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ false }}"
                )
            }
        }
        Template::LetElse => {
            if bad {
                format!(
                    "let token = {check}(accepted).unwrap_or_else(|_| {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ {fallback}() }});\n    {invoke}"
                )
            } else {
                format!(
                    "let Ok(token) = {check}(accepted) else {{ {failure_call} /* MIR_LOGIC_MUTATION_POINT */ return false; }};\n    {invoke}"
                )
            }
        }
    };
    let check_source = match s.template {
        Template::OptionMatch => option_check,
        Template::BooleanGuard => bool_check,
        _ => result_check,
    };
    let type_source = if matches!(s.template, Template::BooleanGuard) {
        String::new()
    } else if matches!(s.template, Template::OptionMatch) {
        token_source.clone()
    } else {
        format!("{token_source}\n{error_source}")
    };
    let action_signature = if matches!(s.template, Template::BooleanGuard) {
        format!("fn {action}() -> bool {{ true }}")
    } else {
        format!("fn {action}(_: {token}) -> bool {{ true }}")
    };
    let benign = format!("fn {failure}() -> bool {{ false }}");
    let fallback_source = if matches!(s.template, Template::BooleanGuard) {
        String::new()
    } else {
        format!("fn {fallback}() -> {token} {{ {token_value} }}")
    };
    format!(
        "\n{type_source}\n{check_source}\n{action_signature}\n{benign}\n{fallback_source}\n{success}\nfn {controller}(accepted: bool) -> bool {{\n    {controller_body}\n}}\n"
    )
}

fn token_representation(representation: StateRepresentation, token: &str) -> (String, String) {
    match representation {
        StateRepresentation::UnitStruct => (
            format!("#[derive(Clone, Copy)]\nstruct {token};"),
            token.into(),
        ),
        StateRepresentation::TupleNewtype => (
            format!("#[derive(Clone, Copy)]\nstruct {token}(u8);"),
            format!("{token}(1)"),
        ),
        StateRepresentation::StructField => (
            format!("#[derive(Clone, Copy)]\nstruct {token} {{ ready: bool }}"),
            format!("{token} {{ ready: true }}"),
        ),
        StateRepresentation::Enum => (
            format!("#[derive(Clone, Copy)]\nenum {token} {{ Ready, Dormant }}"),
            format!("{token}::Ready"),
        ),
        StateRepresentation::IntegerCode => (format!("type {token} = u16;"), "7u16".into()),
        StateRepresentation::Boolean => (format!("type {token} = bool;"), "true".into()),
    }
}

fn error_representation(
    representation: ErrorRepresentation,
    error: &str,
) -> (String, String, String) {
    match representation {
        ErrorRepresentation::CustomEnum => (
            format!("#[derive(Clone, Copy)]\nenum {error} {{ Rejected }}"),
            error.into(),
            format!("{error}::Rejected"),
        ),
        ErrorRepresentation::CustomStruct => (
            format!("#[derive(Clone, Copy)]\nstruct {error};"),
            error.into(),
            error.into(),
        ),
        ErrorRepresentation::Unit => (String::new(), "()".into(), "()".into()),
        ErrorRepresentation::Option | ErrorRepresentation::Boolean => {
            (String::new(), "()".into(), "()".into())
        }
    }
}

fn render_success_chain(
    prefix: &str,
    action: &str,
    token: &str,
    depth: usize,
    noise: usize,
) -> String {
    if depth == 0 {
        return String::new();
    }
    let mut out = String::new();
    for i in 0..depth {
        let body = if i + 1 == depth {
            format!("{action}(token)")
        } else {
            format!("{prefix}_helper_{}(token)", i + 1)
        };
        out.push_str(&format!(
            "fn {prefix}_helper_{i}(token: {token}) -> bool {{ {} {body} }}\n",
            noise_block(noise.min(1))
        ));
    }
    out
}

fn noise_block(level: usize) -> String {
    let mut out = String::new();
    for i in 0..level {
        out.push_str(&format!(
            "let noise_{i} = ({i}u64).wrapping_mul(17).rotate_left(3); let _ = noise_{i}; "
        ));
    }
    out
}

fn camel(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
struct StableRng {
    state: u64,
}

impl StableRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix64(self.state)
    }
    fn range(&mut self, upper: usize) -> usize {
        if upper == 0 {
            0
        } else {
            (self.next() % upper as u64) as usize
        }
    }
}

fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BinaryLabel {
    Good,
    Bug,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationEvidence {
    pub kind: MutationKind,
    pub invariant: String,
    pub expected_semantic_effect: String,
    pub affected_region: SourceRegion,
    pub expected_bad_path: Vec<String>,
    pub resolved_check_nodes: Vec<String>,
    pub resolved_consequence_nodes: Vec<String>,
    pub resolved_failure_edges: Vec<DeltaEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationMetadata {
    pub generator_version: String,
    pub template: Template,
    pub vocabulary: VocabularyKind,
    pub identifier_mode: IdentifierMode,
    pub state_representation: StateRepresentation,
    pub error_representation: ErrorRepresentation,
    pub paired_mutation: MutationKind,
    pub transformations: Vec<String>,
    pub call_depth: usize,
    pub noise_level: usize,
    pub origin_group: String,
    pub challenge_set: Option<ChallengeSet>,
    pub compiled: bool,
    pub invariant_tested: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedRecord {
    pub schema_version: u32,
    pub dataset_version: String,
    pub id: String,
    pub pair_id: String,
    pub paired_example: String,
    pub seed: u64,
    pub domain: Domain,
    pub scenario: String,
    pub split: DatasetSplit,
    pub label: BinaryLabel,
    pub bug_type: Option<String>,
    pub invariant: String,
    pub source: String,
    pub raw_mir: Option<String>,
    pub graph: SemanticGraph,
    pub mutation: Option<MutationEvidence>,
    pub generation: GenerationMetadata,
    pub graph_delta: GraphDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphDelta {
    pub added_nodes: Vec<String>,
    pub removed_nodes: Vec<String>,
    pub added_edges: Vec<DeltaEdge>,
    pub removed_edges: Vec<DeltaEdge>,
    pub changed_features: Vec<FeatureChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeltaEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureChange {
    pub aligned_node: String,
    pub good_features: String,
    pub bad_features: String,
}

#[derive(Debug, Clone)]
pub struct GenerateConfig {
    pub count: usize,
    pub seed: u64,
    pub output: PathBuf,
    pub split: bool,
    pub batch_size: usize,
    pub call_depth: usize,
    pub run_invariant_tests: bool,
    pub keep_workspaces: bool,
    pub overwrite: bool,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        Self {
            count: 100,
            seed: 42,
            output: PathBuf::from("generated-dataset"),
            split: false,
            batch_size: 100,
            call_depth: 5,
            run_invariant_tests: true,
            keep_workspaces: false,
            overwrite: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DatasetStatistics {
    pub pair_count: usize,
    pub record_count: usize,
    pub good_cases: usize,
    pub bad_cases: usize,
    pub by_domain: BTreeMap<String, usize>,
    pub by_mutation: BTreeMap<String, usize>,
    pub by_template: BTreeMap<String, usize>,
    pub by_split: BTreeMap<String, usize>,
    pub by_identifier_mode: BTreeMap<String, usize>,
    pub by_state_representation: BTreeMap<String, usize>,
    pub by_error_representation: BTreeMap<String, usize>,
    pub by_challenge_set: BTreeMap<String, usize>,
    pub average_graph_nodes: f64,
    pub average_graph_edges: f64,
    pub average_call_depth: f64,
    pub exact_duplicate_graphs: usize,
    pub compilation_rejections: usize,
    pub generation_failures: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TimingStatistics {
    pub total_seconds: f64,
    pub source_generation_seconds: f64,
    pub compile_test_seconds: f64,
    pub mir_extraction_seconds: f64,
    pub graph_processing_seconds: f64,
    pub pairs_per_second: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub schema_version: u32,
    pub dataset_version: String,
    pub generator_version: String,
    pub seed: u64,
    pub requested_pairs: usize,
    pub batch_size: usize,
    pub split_strategy: String,
    pub statistics: DatasetStatistics,
    pub timings: TimingStatistics,
    pub files: Vec<String>,
}

struct RecordWriters {
    train: BufWriter<File>,
    validation: Option<BufWriter<File>>,
    test: Option<BufWriter<File>>,
    challenges: HashMap<ChallengeSet, BufWriter<File>>,
}

impl RecordWriters {
    fn new(output: &Path, split: bool) -> Result<Self> {
        let train_path = output.join(if split {
            "train.jsonl"
        } else {
            "dataset.jsonl"
        });
        let train = BufWriter::new(File::create(train_path)?);
        let (validation, test) = if split {
            (
                Some(BufWriter::new(File::create(
                    output.join("validation.jsonl"),
                )?)),
                Some(BufWriter::new(File::create(output.join("test.jsonl"))?)),
            )
        } else {
            (None, None)
        };
        let challenge_dir = output.join("challenges");
        fs::create_dir_all(&challenge_dir)?;
        let mut challenges = HashMap::new();
        for challenge in [
            ChallengeSet::IdentifierBlind,
            ChallengeSet::UnseenVocabulary,
            ChallengeSet::UnseenTopology,
            ChallengeSet::DeepCallGraph,
            ChallengeSet::Noise,
            ChallengeSet::HardNegative,
        ] {
            challenges.insert(
                challenge,
                BufWriter::new(File::create(
                    challenge_dir.join(format!("{}.jsonl", challenge.as_str())),
                )?),
            );
        }
        Ok(Self {
            train,
            validation,
            test,
            challenges,
        })
    }

    fn write(&mut self, record: &GeneratedRecord, split_enabled: bool) -> Result<()> {
        let writer = if !split_enabled || record.split == DatasetSplit::Train {
            &mut self.train
        } else if record.split == DatasetSplit::Validation {
            self.validation.as_mut().expect("split writer")
        } else {
            self.test.as_mut().expect("split writer")
        };
        serde_json::to_writer(&mut *writer, record)?;
        writer.write_all(b"\n")?;
        if let Some(challenge) = record.generation.challenge_set {
            let challenge_writer = self
                .challenges
                .get_mut(&challenge)
                .expect("challenge writer");
            serde_json::to_writer(&mut *challenge_writer, record)?;
            challenge_writer.write_all(b"\n")?;
        }
        Ok(())
    }

    fn flush(mut self) -> Result<()> {
        self.train.flush()?;
        if let Some(writer) = &mut self.validation {
            writer.flush()?;
        }
        if let Some(writer) = &mut self.test {
            writer.flush()?;
        }
        for writer in self.challenges.values_mut() {
            writer.flush()?;
        }
        Ok(())
    }
}

pub fn generate_dataset(config: &GenerateConfig) -> Result<DatasetManifest> {
    if config.count == 0 {
        bail!("--count must be greater than zero");
    }
    if config.batch_size == 0 {
        bail!("--batch-size must be greater than zero");
    }
    prepare_output(config)?;
    let total_start = Instant::now();
    let dataset_version = format!(
        "{}-seed{}-pairs{}",
        GENERATOR_VERSION, config.seed, config.count
    );
    let mut writers = RecordWriters::new(&config.output, config.split)?;
    let work_root = config.output.join(".generation-work");
    fs::create_dir_all(&work_root)?;
    let shared_target = work_root.join("target-cache");
    let mut stats = DatasetStatistics::default();
    let mut timings = TimingStatistics::default();
    let mut fingerprints = HashSet::new();
    let scenarios: Vec<_> = (0..config.count)
        .map(|index| Scenario::from_index(config.seed, index, config.split))
        .collect();
    validate_split_isolation(&scenarios)?;

    for (batch_index, chunk) in scenarios.chunks(config.batch_size).enumerate() {
        let generation_start = Instant::now();
        let rendered: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(offset, scenario)| {
                render_pair(scenario.clone(), batch_index * config.batch_size + offset)
            })
            .collect();
        let batch_dir = work_root.join(format!("batch-{batch_index:06}"));
        write_batch_crate(&batch_dir, batch_index, &rendered)?;
        timings.source_generation_seconds += generation_start.elapsed().as_secs_f64();

        if config.run_invariant_tests {
            let compile_start = Instant::now();
            validate_batch(&batch_dir, &shared_target).with_context(|| {
                format!("generated batch {batch_index} failed compile/invariant validation")
            })?;
            timings.compile_test_seconds += compile_start.elapsed().as_secs_f64();
        }

        let extraction_start = Instant::now();
        let raw = MirExtractor.extract(
            &batch_dir,
            &ExtractOptions {
                call_depth: config.call_depth,
                keep_raw_mir: true,
                target_dir: Some(shared_target.clone()),
            },
        )?;
        timings.mir_extraction_seconds += extraction_start.elapsed().as_secs_f64();
        let graph_start = Instant::now();
        let full_graph = simplify(&raw, config.call_depth);
        for pair in &rendered {
            let good_graph = partition_graph(&full_graph, &pair.good_prefix);
            let bad_graph = partition_graph(&full_graph, &pair.bad_prefix);
            ensure_graphs_present(pair, &good_graph, &bad_graph)?;
            let delta = graph_delta(&good_graph, &bad_graph, &pair.good_prefix, &pair.bad_prefix);
            let raw_good = collect_raw_mir(&raw, &pair.good_prefix);
            let raw_bad = collect_raw_mir(&raw, &pair.bad_prefix);
            let mutation = resolve_mutation_evidence(pair, &bad_graph);
            let good_id = format!("{}-good", pair.scenario.id);
            let bad_id = format!("{}-bad", pair.scenario.id);
            let generation = GenerationMetadata {
                generator_version: GENERATOR_VERSION.into(),
                template: pair.scenario.template,
                vocabulary: pair.scenario.vocabulary,
                identifier_mode: pair.scenario.identifier_mode,
                state_representation: pair.scenario.state_representation,
                error_representation: pair.scenario.error_representation,
                paired_mutation: pair.scenario.mutation.clone(),
                transformations: pair.transformations.clone(),
                call_depth: pair.scenario.call_depth,
                noise_level: pair.scenario.noise_level,
                origin_group: pair.scenario.origin_group.clone(),
                challenge_set: pair.scenario.challenge_set,
                compiled: true,
                invariant_tested: config.run_invariant_tests,
            };
            let good = GeneratedRecord {
                schema_version: GENERATED_SCHEMA_VERSION,
                dataset_version: dataset_version.clone(),
                id: good_id.clone(),
                pair_id: pair.scenario.id.clone(),
                paired_example: bad_id.clone(),
                seed: pair.scenario.seed,
                domain: pair.scenario.domain,
                scenario: pair.scenario.id.clone(),
                split: pair.scenario.split,
                label: BinaryLabel::Good,
                bug_type: None,
                invariant: pair.scenario.invariant.clone(),
                source: pair.good_source.clone(),
                raw_mir: Some(raw_good),
                graph: good_graph,
                mutation: None,
                generation: generation.clone(),
                graph_delta: delta.clone(),
            };
            let bad = GeneratedRecord {
                schema_version: GENERATED_SCHEMA_VERSION,
                dataset_version: dataset_version.clone(),
                id: bad_id,
                pair_id: pair.scenario.id.clone(),
                paired_example: good_id,
                seed: pair.scenario.seed,
                domain: pair.scenario.domain,
                scenario: pair.scenario.id.clone(),
                split: pair.scenario.split,
                label: BinaryLabel::Bug,
                bug_type: Some(pair.scenario.bug_type.clone()),
                invariant: pair.scenario.invariant.clone(),
                source: pair.bad_source.clone(),
                raw_mir: Some(raw_bad),
                graph: bad_graph,
                mutation: Some(mutation),
                generation,
                graph_delta: delta,
            };
            update_stats(&mut stats, &good, &mut fingerprints)?;
            update_stats(&mut stats, &bad, &mut fingerprints)?;
            writers.write(&good, config.split)?;
            writers.write(&bad, config.split)?;
            stats.pair_count += 1;
        }
        timings.graph_processing_seconds += graph_start.elapsed().as_secs_f64();
        if !config.keep_workspaces {
            remove_known_batch_files(&batch_dir)?;
        }
    }
    writers.flush()?;
    if !config.keep_workspaces {
        // Keep the shared target outside version control only for the duration
        // of this run. Removal is scoped to the generator-owned directory.
        if shared_target.exists() {
            fs::remove_dir_all(&shared_target)?;
        }
        if work_root.exists() {
            let _ = fs::remove_dir(&work_root);
        }
    }
    stats.exact_duplicate_graphs = stats.record_count.saturating_sub(fingerprints.len());
    if stats.record_count > 0 {
        stats.average_graph_nodes /= stats.record_count as f64;
        stats.average_graph_edges /= stats.record_count as f64;
        stats.average_call_depth /= stats.record_count as f64;
    }
    timings.total_seconds = total_start.elapsed().as_secs_f64();
    timings.pairs_per_second = stats.pair_count as f64 / timings.total_seconds.max(f64::EPSILON);
    let files = manifest_files(config.split);
    let manifest = DatasetManifest {
        schema_version: GENERATED_SCHEMA_VERSION,
        dataset_version,
        generator_version: GENERATOR_VERSION.into(),
        seed: config.seed,
        requested_pairs: config.count,
        batch_size: config.batch_size,
        split_strategy: if config.split {
            "grouped by template/vocabulary/origin; holdout templates and vocabularies assigned to test".into()
        } else {
            "unsplit".into()
        },
        statistics: stats,
        timings,
        files,
    };
    fs::write(
        config.output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(manifest)
}

fn prepare_output(config: &GenerateConfig) -> Result<()> {
    fs::create_dir_all(&config.output)?;
    let known = [
        "dataset.jsonl",
        "train.jsonl",
        "validation.jsonl",
        "test.jsonl",
        "manifest.json",
    ];
    let existing: Vec<_> = known
        .iter()
        .map(|name| config.output.join(name))
        .filter(|path| path.exists())
        .collect();
    if !existing.is_empty() && !config.overwrite {
        bail!("output already contains a dataset; pass --overwrite to replace known dataset files");
    }
    if config.overwrite {
        for path in existing {
            fs::remove_file(path)?;
        }
        let challenges = config.output.join("challenges");
        if challenges.exists() {
            for challenge in [
                ChallengeSet::IdentifierBlind,
                ChallengeSet::UnseenVocabulary,
                ChallengeSet::UnseenTopology,
                ChallengeSet::DeepCallGraph,
                ChallengeSet::Noise,
                ChallengeSet::HardNegative,
            ] {
                let path = challenges.join(format!("{}.jsonl", challenge.as_str()));
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
    }
    Ok(())
}

fn write_batch_crate(
    batch_dir: &Path,
    batch_index: usize,
    rendered: &[RenderedPair],
) -> Result<()> {
    fs::create_dir_all(batch_dir.join("src"))?;
    let manifest = format!(
        "[package]\nname = \"mir-logic-generated-{batch_index}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n"
    );
    fs::write(batch_dir.join("Cargo.toml"), manifest)?;
    let mut source =
        String::from("#![allow(dead_code, non_camel_case_types, unused_variables, unused_mut)]\n");
    for pair in rendered {
        source.push_str(&pair.good_source);
        source.push_str(&pair.bad_source);
    }
    source.push_str("\n#[cfg(test)]\nmod generated_invariant_tests {\n    use super::*;\n    #[test]\n    fn all_pairs_demonstrate_the_declared_invariant() {\n");
    for pair in rendered {
        source.push_str(&format!("        assert!(!{}_controller(false));\n        assert!({}_controller(false));\n        assert!({}_controller(true));\n        assert!({}_controller(true));\n", pair.good_prefix, pair.bad_prefix, pair.good_prefix, pair.bad_prefix));
    }
    source.push_str("    }\n}\n");
    fs::write(batch_dir.join("src/lib.rs"), source)?;
    Ok(())
}

fn validate_batch(batch_dir: &Path, target_dir: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args(["test", "--quiet", "--manifest-path"])
        .arg(batch_dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .output()
        .context("failed to execute generated crate tests")?;
    if !output.status.success() {
        bail!(
            "generated crate validation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn partition_graph(graph: &SemanticGraph, prefix: &str) -> SemanticGraph {
    let functions: Vec<SemanticFunction> = graph
        .functions
        .iter()
        .filter(|function| function.name.starts_with(prefix))
        .cloned()
        .collect();
    let ids: HashSet<&str> = functions
        .iter()
        .flat_map(|function| &function.nodes)
        .map(|node| node.id.as_str())
        .collect();
    let interprocedural_edges = graph
        .interprocedural_edges
        .iter()
        .filter(|edge| ids.contains(edge.from.as_str()) && ids.contains(edge.to.as_str()))
        .cloned()
        .collect();
    SemanticGraph {
        schema_version: graph.schema_version,
        crate_name: graph.crate_name.clone(),
        functions,
        interprocedural_edges,
    }
}

fn ensure_graphs_present(
    pair: &RenderedPair,
    good: &SemanticGraph,
    bad: &SemanticGraph,
) -> Result<()> {
    if good.functions.is_empty() || bad.functions.is_empty() {
        bail!(
            "MIR partition failed for {}; good_functions={}, bad_functions={}",
            pair.scenario.id,
            good.functions.len(),
            bad.functions.len()
        );
    }
    Ok(())
}

fn collect_raw_mir(raw: &crate::graph::ProgramGraph, prefix: &str) -> String {
    raw.functions
        .iter()
        .filter(|function| function.name.starts_with(prefix))
        .filter_map(|function| function.raw_mir.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn graph_delta(
    good: &SemanticGraph,
    bad: &SemanticGraph,
    good_prefix: &str,
    bad_prefix: &str,
) -> GraphDelta {
    let good_nodes = normalized_nodes(good, good_prefix);
    let bad_nodes = normalized_nodes(bad, bad_prefix);
    let good_edges = normalized_edges(good, good_prefix);
    let bad_edges = normalized_edges(bad, bad_prefix);
    let good_keys: BTreeSet<_> = good_nodes.keys().cloned().collect();
    let bad_keys: BTreeSet<_> = bad_nodes.keys().cloned().collect();
    let added_nodes = bad_keys.difference(&good_keys).cloned().collect();
    let removed_nodes = good_keys.difference(&bad_keys).cloned().collect();
    let changed_features = good_keys
        .intersection(&bad_keys)
        .filter_map(|key| {
            let good_feature = &good_nodes[key];
            let bad_feature = &bad_nodes[key];
            (good_feature != bad_feature).then(|| FeatureChange {
                aligned_node: key.clone(),
                good_features: good_feature.clone(),
                bad_features: bad_feature.clone(),
            })
        })
        .collect();
    GraphDelta {
        added_nodes,
        removed_nodes,
        added_edges: bad_edges.difference(&good_edges).cloned().collect(),
        removed_edges: good_edges.difference(&bad_edges).cloned().collect(),
        changed_features,
    }
}

fn normalized_nodes(graph: &SemanticGraph, prefix: &str) -> BTreeMap<String, String> {
    graph
        .functions
        .iter()
        .flat_map(|function| &function.nodes)
        .map(|node| {
            let id = normalize(&node.id, prefix);
            let called = node
                .called_function
                .as_deref()
                .map(|value| normalize(value, prefix));
            let feature = format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                node.kind,
                node.operation,
                called,
                node.ty.as_deref().map(|value| normalize(value, prefix)),
                node.variant,
                node.branch_condition
            );
            (id, feature)
        })
        .collect()
}

fn normalized_edges(graph: &SemanticGraph, prefix: &str) -> BTreeSet<DeltaEdge> {
    graph
        .edges()
        .map(|edge| DeltaEdge {
            from: normalize(&edge.from, prefix),
            to: normalize(&edge.to, prefix),
            kind: format!("{:?}", edge.kind),
            label: edge.label.clone(),
        })
        .collect()
}

fn normalize(value: &str, prefix: &str) -> String {
    value.replace(prefix, "case_SIDE")
}

fn resolve_mutation_evidence(pair: &RenderedPair, graph: &SemanticGraph) -> MutationEvidence {
    let vocab = Vocabulary::for_scenario(&pair.scenario);
    let resolved_check_nodes = nodes_calling(graph, &vocab.check);
    let resolved_consequence_nodes = nodes_calling(graph, &vocab.action);
    let resolved_failure_edges = graph
        .edges()
        .filter(|edge| {
            edge.label.as_deref().is_some_and(|label| {
                let label = label.to_ascii_lowercase();
                label.contains("err") || label.contains("none") || label == "false"
            })
        })
        .map(|edge| DeltaEdge {
            from: edge.from.clone(),
            to: edge.to.clone(),
            kind: format!("{:?}", edge.kind),
            label: edge.label.clone(),
        })
        .collect();
    MutationEvidence {
        kind: pair.scenario.mutation.clone(),
        invariant: pair.scenario.invariant.clone(),
        expected_semantic_effect: format!(
            "the failure path reaches the protected consequence in the {} domain",
            pair.scenario.domain.as_str()
        ),
        affected_region: pair.mutation_region.clone(),
        expected_bad_path: pair.expected_bad_path.clone(),
        resolved_check_nodes,
        resolved_consequence_nodes,
        resolved_failure_edges,
    }
}

fn nodes_calling(graph: &SemanticGraph, needle: &str) -> Vec<String> {
    graph
        .functions
        .iter()
        .flat_map(|function| &function.nodes)
        .filter(|node| {
            node.called_function
                .as_deref()
                .is_some_and(|name| name.contains(needle))
        })
        .map(|node| node.id.clone())
        .collect()
}

fn update_stats(
    stats: &mut DatasetStatistics,
    record: &GeneratedRecord,
    fingerprints: &mut HashSet<u64>,
) -> Result<()> {
    stats.record_count += 1;
    match record.label {
        BinaryLabel::Good => stats.good_cases += 1,
        BinaryLabel::Bug => stats.bad_cases += 1,
    }
    increment(&mut stats.by_domain, record.domain.as_str());
    increment(&mut stats.by_template, record.generation.template.as_str());
    increment(
        &mut stats.by_split,
        &format!("{:?}", record.split).to_ascii_lowercase(),
    );
    increment(
        &mut stats.by_identifier_mode,
        record.generation.identifier_mode.as_str(),
    );
    increment(
        &mut stats.by_state_representation,
        &format!("{:?}", record.generation.state_representation).to_ascii_lowercase(),
    );
    increment(
        &mut stats.by_error_representation,
        &format!("{:?}", record.generation.error_representation).to_ascii_lowercase(),
    );
    if let Some(mutation) = &record.mutation {
        increment(&mut stats.by_mutation, mutation.kind.as_str());
    }
    if let Some(challenge) = record.generation.challenge_set {
        increment(&mut stats.by_challenge_set, challenge.as_str());
    }
    stats.average_graph_nodes += record
        .graph
        .functions
        .iter()
        .map(|function| function.nodes.len())
        .sum::<usize>() as f64;
    stats.average_graph_edges += record.graph.edges().count() as f64;
    stats.average_call_depth += record.generation.call_depth as f64;
    fingerprints.insert(fnv1a64(&serde_json::to_vec(&record.graph)?));
    Ok(())
}

fn increment(map: &mut BTreeMap<String, usize>, key: &str) {
    *map.entry(key.into()).or_default() += 1;
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

fn validate_split_isolation(scenarios: &[Scenario]) -> Result<()> {
    let mut groups: HashMap<&str, DatasetSplit> = HashMap::new();
    for scenario in scenarios {
        if let Some(existing) = groups.insert(&scenario.origin_group, scenario.split)
            && existing != scenario.split
        {
            bail!(
                "origin group {} leaked across {:?} and {:?}",
                scenario.origin_group,
                existing,
                scenario.split
            );
        }
    }
    Ok(())
}

fn remove_known_batch_files(batch_dir: &Path) -> Result<()> {
    let source = batch_dir.join("src/lib.rs");
    let manifest = batch_dir.join("Cargo.toml");
    let lock = batch_dir.join("Cargo.lock");
    if source.exists() {
        fs::remove_file(source)?;
    }
    let source_dir = batch_dir.join("src");
    if source_dir.exists() {
        fs::remove_dir(&source_dir)?;
    }
    if manifest.exists() {
        fs::remove_file(manifest)?;
    }
    if lock.exists() {
        fs::remove_file(lock)?;
    }
    if batch_dir.exists() {
        fs::remove_dir(batch_dir)?;
    }
    Ok(())
}

fn manifest_files(split: bool) -> Vec<String> {
    let mut files = if split {
        vec![
            "train.jsonl".into(),
            "validation.jsonl".into(),
            "test.jsonl".into(),
        ]
    } else {
        vec!["dataset.jsonl".into()]
    };
    for challenge in [
        ChallengeSet::IdentifierBlind,
        ChallengeSet::UnseenVocabulary,
        ChallengeSet::UnseenTopology,
        ChallengeSet::DeepCallGraph,
        ChallengeSet::Noise,
        ChallengeSet::HardNegative,
    ] {
        files.push(format!("challenges/{}.jsonl", challenge.as_str()));
    }
    files.push("manifest.json".into());
    files
}

pub fn load_records(path: &Path) -> Result<Vec<GeneratedRecord>> {
    let text = fs::read_to_string(path)?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(
            Scenario::from_index(42, 17, true),
            Scenario::from_index(42, 17, true)
        );
        assert_ne!(
            Scenario::from_index(42, 17, true),
            Scenario::from_index(43, 17, true)
        );
        let first = render_pair(Scenario::from_index(42, 17, true), 17);
        let second = render_pair(Scenario::from_index(42, 17, true), 17);
        assert_eq!(first.good_source, second.good_source);
        assert_eq!(first.bad_source, second.bad_source);
    }

    #[test]
    fn state_and_error_representations_are_diverse() {
        let scenarios: Vec<_> = (0..200)
            .map(|index| Scenario::from_index(77, index, true))
            .collect();
        assert!(
            scenarios
                .iter()
                .map(|scenario| scenario.state_representation)
                .collect::<HashSet<_>>()
                .len()
                >= 5
        );
        assert!(
            scenarios
                .iter()
                .map(|scenario| scenario.error_representation)
                .collect::<HashSet<_>>()
                .len()
                >= 4
        );
    }

    #[test]
    fn rendered_pair_has_controlled_mutation_and_labels() {
        let scenario = Scenario::from_index(42, 1, true);
        let pair = render_pair(scenario.clone(), 1);
        assert!(pair.good_source.contains("MIR_LOGIC_MUTATION_POINT"));
        assert!(pair.bad_source.contains("MIR_LOGIC_MUTATION_POINT"));
        assert_ne!(pair.good_source, pair.bad_source);
        assert_eq!(pair.scenario.invariant, scenario.domain.invariant());
    }

    #[test]
    fn grouped_splits_do_not_leak() {
        let scenarios: Vec<_> = (0..500)
            .map(|index| Scenario::from_index(9, index, true))
            .collect();
        validate_split_isolation(&scenarios).unwrap();
        let pairs: HashSet<_> = scenarios
            .iter()
            .map(|scenario| (&scenario.id, scenario.split))
            .collect();
        assert_eq!(pairs.len(), scenarios.len());
    }

    #[test]
    fn challenges_force_the_expected_dimensions() {
        let scenarios: Vec<_> = (0..100)
            .map(|index| Scenario::from_index(2, index, true))
            .collect();
        assert!(scenarios.iter().any(|scenario| scenario.challenge_set
            == Some(ChallengeSet::IdentifierBlind)
            && scenario.identifier_mode == IdentifierMode::Neutral));
        assert!(scenarios.iter().any(|scenario| scenario.challenge_set
            == Some(ChallengeSet::DeepCallGraph)
            && scenario.call_depth >= 3));
        assert!(
            scenarios
                .iter()
                .filter(|scenario| scenario.challenge_set.is_some())
                .all(|scenario| scenario.split == DatasetSplit::Test)
        );
    }
}
