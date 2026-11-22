//! Public summary contract; omitted text is retrieved through its original owner.
use super::progress::Progress;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct UsageSummary {
    pub scope: String,
    pub source: String,
    pub turns: usize,
    pub active: usize,
    pub samples: usize,
    pub reported_cost_usd: Option<f64>,
    pub cost_unknown_samples: usize,
    pub prompt_tokens: Option<u64>,
    pub prompt_unknown_samples: usize,
    pub completion_tokens: Option<u64>,
    pub completion_unknown_samples: usize,
    pub unreported_turns: usize,
    pub complete: bool,
    pub groups: Vec<UsageGroup>,
}

#[derive(Serialize)]
pub struct UsageGroup {
    pub kind: UsageGroupKind,
    pub model: Option<String>,
    pub model_count: usize,
    pub totals: UsageTotals,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageGroupKind {
    Model,
    Unidentified,
    Other,
}

#[derive(Serialize)]
pub struct UsageTotals {
    pub samples: usize,
    pub reported_cost_usd: Option<f64>,
    pub cost_unknown_samples: usize,
    pub prompt_tokens: Option<u64>,
    pub prompt_unknown_samples: usize,
    pub completion_tokens: Option<u64>,
    pub completion_unknown_samples: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProjection {
    pub version: String,
    pub projection_schema_revision: u32,
    pub summary_only: bool,
    pub context_byte_limit: usize,
    pub fingerprint: String,
    pub state_revision: u64,
    pub policy_revision: u64,
    pub assembled_at: String,
    pub session: Option<ContextSession>,
    pub provider: String,
    pub workspace: String,
    pub authorized_workspaces: Vec<ContextWorkspace>,
    pub authorized_workspaces_total: usize,
    pub authorized_workspaces_has_more: bool,
    pub memories: Vec<ContextMemory>,
    pub memory_candidates: usize,
    pub memory_scope: String,
    pub memories_has_more: bool,
    pub tasks: Vec<ContextTask>,
    pub tasks_total: usize,
    pub tasks_has_more: bool,
    pub queued_tasks: usize,
    pub discussion_turns: usize,
    pub context_has_more: bool,
    pub shared_model_defaults: ContextModelDefaults,
    pub capabilities: Vec<ContextCapability>,
    pub interfaces: ContextInterfaces,
    pub workflows: ContextWorkflows,
    pub rules: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSession {
    pub id: String,
    pub source: String,
    pub title: String,
    pub title_truncated: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextWorkspace {
    pub id: String,
    pub access: String,
    pub auto_write: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextMemory {
    pub id: String,
    pub title: String,
    pub title_truncated: bool,
    pub content: String,
    pub content_truncated: bool,
    pub updated_at: String,
    pub trust: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextTask {
    pub id: String,
    pub state: String,
    pub runtime_version: String,
    pub runtime_features: ContextRuntimeFeatures,
    pub route_id: String,
    pub requirements_revision: u64,
    pub requirements: Vec<ContextRequirement>,
    pub requirements_total: usize,
    pub requirements_has_more: bool,
    pub progress: Progress,
    pub progress_truncated: bool,
    pub result: String,
    pub result_truncated: bool,
    pub artifacts: Vec<ContextArtifact>,
    pub artifacts_total: usize,
    pub artifacts_has_more: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRuntimeFeatures {
    pub steering: bool,
    pub documents: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequirement {
    pub revision: u64,
    pub text: String,
    pub text_truncated: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextArtifact {
    pub id: String,
    pub name: String,
    pub name_truncated: bool,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextModelDefaults {
    pub scope: String,
    pub revision: u64,
    pub enabled: bool,
    pub route_id: String,
    pub free_only: bool,
    pub runtime: String,
    pub credential_source: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextCapability {
    pub function: String,
    pub discover: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub describe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextInterfaces {
    pub discover: String,
    pub describe: String,
    pub call: String,
    pub jobs: String,
    pub results: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextWorkflows {
    pub skill: String,
    pub capture: String,
    pub writing: String,
    pub quick_tasks: String,
}
