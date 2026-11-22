//! Offline metadata wrapper: no Engine/Bridge/client is created, no auth is exported.
use chrono::{DateTime, Utc};
use homebase_control_tests::wechat_crypto;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Assistant {
    sessions: Vec<Session>,
    #[serde(default)]
    personal: Personal,
    #[serde(default)]
    followups: Followups,
    #[serde(default)]
    matters: Matters,
}
#[derive(Deserialize)]
struct Session {
    turns: Vec<Turn>,
    jobs: Vec<Job>,
    #[serde(default)]
    queue_paused: bool,
}
#[derive(Deserialize)]
struct Turn {
    state: String,
    steps: Vec<Step>,
    #[serde(default)]
    queued: Vec<serde::de::IgnoredAny>,
    #[serde(default)]
    runtime_uncertain_model: bool,
    #[serde(default)]
    pending: Option<serde::de::IgnoredAny>,
}
#[derive(Deserialize)]
struct Step {
    state: String,
}
#[derive(Deserialize)]
struct Job {
    state: String,
    #[serde(default)]
    remote: Option<Remote>,
}
#[derive(Deserialize)]
struct Remote {
    finished: bool,
    #[serde(default)]
    query_job: Option<serde::de::IgnoredAny>,
}
#[derive(Default, Deserialize)]
struct Personal {
    #[serde(default)]
    memories: Vec<Memory>,
    #[serde(default)]
    reminders: Vec<Reminder>,
}
#[derive(Deserialize)]
struct Memory {
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    expiry_notified: bool,
}
#[derive(Deserialize)]
struct Reminder {
    enabled: bool,
    delivery: String,
    due_at: String,
    #[serde(default)]
    source: String,
}
#[derive(Default, Deserialize)]
struct Followups {
    entries: Vec<Followup>,
}
#[derive(Deserialize)]
struct Followup {
    enabled: bool,
    state: String,
    due_at: String,
    #[serde(default)]
    notice_delivery: String,
    runs: Vec<FollowupRun>,
}
#[derive(Deserialize)]
struct FollowupRun {
    state: String,
}
#[derive(Default, Deserialize)]
struct Matters {
    entries: Vec<Matter>,
}
#[derive(Deserialize)]
struct Matter {
    state: String,
    delivery: String,
    #[serde(default)]
    review_at: Option<String>,
}

// Authentication, identity, cursor, message bodies, args, results and IDs are
// ignored by serde, never allocated as metadata strings or serialized again.
#[derive(Deserialize)]
struct Channel {
    enabled: bool,
    #[serde(default)]
    records: Vec<Record>,
}
#[derive(Deserialize)]
struct Record {
    command: String,
    status: String,
    delivery: String,
    #[serde(default)]
    job_id: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    progress_delivery: String,
}
fn fixed_bucket<'a>(value: &str, allowed: &'a [&'a str]) -> &'a str {
    allowed
        .iter()
        .copied()
        .find(|item| *item == value)
        .unwrap_or("other")
}
fn due(value: &str, at: DateTime<Utc>, invalid: &mut usize) -> bool {
    match DateTime::parse_from_rfc3339(value) {
        Ok(value) => value <= at,
        Err(_) => {
            *invalid += 1;
            false
        }
    }
}
fn metadata(a: Assistant, c: Channel) -> Result<Value, &'static str> {
    if a.sessions.len() > 100 || c.records.len() > 8192 {
        return Err("STARTUP_METADATA_LIMIT");
    }
    let at = Utc::now();
    let mut turns = BTreeMap::<&str, usize>::new();
    let mut steps = BTreeMap::<&str, usize>::new();
    let (
        mut total_turns,
        mut jobs,
        mut pending_jobs,
        mut pending_approvals,
        mut uncertain,
        mut queued_calls,
        mut constructor_candidates,
        mut queue_pauses,
    ) = (0, 0, 0, 0, 0, 0, 0, 0);
    for s in &a.sessions {
        if s.turns.len() > 8192 || s.jobs.len() > 8192 {
            return Err("STARTUP_METADATA_LIMIT");
        }
        if !s.queue_paused && s.turns.iter().any(|t| t.state == "waiting_input_queue") {
            queue_pauses += 1;
        }
        for t in &s.turns {
            total_turns += 1;
            *turns
                .entry(fixed_bucket(
                    &t.state,
                    &[
                        "queued",
                        "running",
                        "waiting_jobs",
                        "awaiting_confirmation",
                        "waiting_input_queue",
                        "completed",
                        "failed",
                        "unknown",
                        "interrupted_unknown",
                        "paused_budget",
                        "needs_input",
                        "canceled",
                        "cancelled",
                        "rejected",
                        "disabled",
                        "timed_out",
                    ],
                ))
                .or_default() += 1;
            if matches!(t.state.as_str(), "queued" | "running" | "waiting_jobs") {
                constructor_candidates += 1;
            }
            if t.runtime_uncertain_model {
                uncertain += 1;
            }
            if t.pending.is_some() {
                pending_approvals += 1;
            }
            queued_calls += t.queued.len();
            for step in &t.steps {
                *steps
                    .entry(fixed_bucket(
                        &step.state,
                        &[
                            "completed",
                            "failed",
                            "unknown",
                            "interrupted_unknown",
                            "running",
                            "pending",
                            "planned",
                            "queued",
                            "awaiting_confirmation",
                            "canceled",
                            "cancelled",
                            "rejected",
                        ],
                    ))
                    .or_default() += 1;
            }
        }
        for job in &s.jobs {
            jobs += 1;
            if !matches!(
                job.state.as_str(),
                "succeeded" | "failed" | "canceled" | "cancelled" | "timed_out"
            ) || job
                .remote
                .as_ref()
                .is_some_and(|r| !r.finished || r.query_job.is_some())
            {
                pending_jobs += 1;
            }
        }
    }
    let mut invalid_dates = 0;
    let expired_memories = a
        .personal
        .memories
        .iter()
        .filter(|m| {
            !m.expiry_notified
                && m.expires_at
                    .as_ref()
                    .is_some_and(|v| due(v, at, &mut invalid_dates))
        })
        .count();
    let due_reminders = a
        .personal
        .reminders
        .iter()
        .filter(|r| {
            r.enabled && r.delivery == "scheduled" && due(&r.due_at, at, &mut invalid_dates)
        })
        .count();
    let claimed_notices = a
        .personal
        .reminders
        .iter()
        .filter(|r| r.delivery == "claimed")
        .count()
        + a.followups
            .entries
            .iter()
            .filter(|f| f.notice_delivery == "claimed")
            .count()
        + a.matters
            .entries
            .iter()
            .filter(|m| m.delivery == "claimed")
            .count();
    let enabled_followups = a.followups.entries.iter().filter(|f| f.enabled).count();
    let due_followups = a
        .followups
        .entries
        .iter()
        .filter(|f| f.enabled && f.state == "scheduled" && due(&f.due_at, at, &mut invalid_dates))
        .count();
    let unfinished_followup_runs = a
        .followups
        .entries
        .iter()
        .flat_map(|f| &f.runs)
        .filter(|r| {
            !matches!(
                r.state.as_str(),
                "completed" | "failed" | "canceled" | "cancelled" | "not_submitted"
            )
        })
        .count();
    let due_matters = a
        .matters
        .entries
        .iter()
        .filter(|m| {
            m.state != "closed"
                && m.delivery == "scheduled"
                && m.review_at
                    .as_ref()
                    .is_some_and(|v| due(v, at, &mut invalid_dates))
        })
        .count();
    // Pending notifications are distinct from schedules that become due later.
    let reminder_deliveries = a
        .personal
        .reminders
        .iter()
        .map(|r| {
            fixed_bucket(
                &r.delivery,
                &[
                    "scheduled",
                    "pending",
                    "claimed",
                    "accepted",
                    "unknown",
                    "desktop_due",
                    "canceled",
                    "",
                ],
            )
        })
        .fold(BTreeMap::<&str, usize>::new(), |mut out, key| {
            *out.entry(key).or_default() += 1;
            out
        });
    let pending_wechat_reminders = a
        .personal
        .reminders
        .iter()
        .filter(|r| r.source == "wechat" && r.delivery == "pending")
        .count();
    let pending_followup_notices = a
        .followups
        .entries
        .iter()
        .filter(|f| f.notice_delivery == "pending")
        .count();
    let pending_matter_notices = a
        .matters
        .entries
        .iter()
        .filter(|m| m.delivery == "pending")
        .count();
    let mut record_states = BTreeMap::<&str, usize>::new();
    let mut deliveries = BTreeMap::<&str, usize>::new();
    let (mut can_submit, mut can_poll, mut missing_job, mut pending_progress) = (0, 0, 0, 0);
    for r in &c.records {
        *record_states
            .entry(fixed_bucket(
                &r.status,
                &[
                    "received",
                    "running",
                    "awaiting_confirmation",
                    "waiting_input_queue",
                    "succeeded",
                    "failed",
                    "unknown",
                    "continued",
                    "canceled",
                    "cancelled",
                    "completed",
                    "waiting_remote",
                ],
            ))
            .or_default() += 1;
        *deliveries
            .entry(fixed_bucket(
                &r.delivery,
                &[
                    "pending",
                    "accepted",
                    "unknown",
                    "suppressed",
                    "not_requested",
                    "none",
                    "failed",
                    "sent",
                    "",
                ],
            ))
            .or_default() += 1;
        if r.command != "message" && r.status == "received" {
            can_submit += 1;
        }
        if r.command != "message"
            && matches!(
                r.status.as_str(),
                "running" | "awaiting_confirmation" | "waiting_input_queue"
            )
        {
            if r.job_id.is_some() {
                can_poll += 1;
            } else {
                missing_job += 1;
            }
        }
        if r.progress_delivery == "pending" {
            pending_progress += 1;
        }
    }
    Ok(
        json!({"assistant":{"sessions":a.sessions.len(),"turns":total_turns,"jobs":jobs,"pending_jobs":pending_jobs,"turn_states":turns,"step_states":steps,"uncertain_model_turns":uncertain,"queued_call_references":queued_calls,"pending_approvals":pending_approvals,"constructor_turn_candidates_upper_bound":constructor_candidates,"constructor_queue_pauses":queue_pauses,"constructor_claimed_notice_changes":claimed_notices},
      "scheduled":{"memories":a.personal.memories.len(),"expired_memory_updates":expired_memories,"reminders":a.personal.reminders.len(),"due_reminders":due_reminders,"reminder_deliveries":reminder_deliveries,"pending_wechat_reminders":pending_wechat_reminders,"pending_followup_notices":pending_followup_notices,"pending_matter_notices":pending_matter_notices,"followups":a.followups.entries.len(),"enabled_followups":enabled_followups,"due_followups":due_followups,"unfinished_followup_runs":unfinished_followup_runs,"matters":a.matters.entries.len(),"due_matters":due_matters,"invalid_dates":invalid_dates},
      "wechat":{"enabled":c.enabled,"records":c.records.len(),"raw_record_states":record_states,"delivery_states":deliveries,"received_that_monitor_can_submit":can_submit,"original_jobs_monitor_can_poll":can_poll,"monitored_records_missing_job":missing_job,"pending_progress_records":pending_progress,"binding_and_network":"not_inspected","authentication_fields":"ignored_not_exported"}}),
    )
}
fn safe_path(file: &Path) -> Result<(), &'static str> {
    for p in file.ancestors() {
        match fs::symlink_metadata(p) {
            Ok(m) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if m.file_attributes() & 0x400 != 0 {
                        return Err("STARTUP_LINKED_PATH");
                    }
                }
                if m.file_type().is_symlink() {
                    return Err("STARTUP_LINKED_PATH");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("STARTUP_PATH_UNREADABLE"),
        }
    }
    Ok(())
}
fn read_cipher(file: &Path, expected: &str) -> Result<Vec<u8>, &'static str> {
    safe_path(file)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut input = options
        .open(file)
        .map_err(|_| "STARTUP_SOURCE_UNREADABLE")?;
    let len = input
        .metadata()
        .map_err(|_| "STARTUP_SOURCE_UNREADABLE")?
        .len();
    if len == 0 || len > 32 * 1024 * 1024 {
        return Err("STARTUP_SOURCE_SIZE");
    }
    let mut bytes = vec![0; len as usize];
    input
        .read_exact(&mut bytes)
        .map_err(|_| "STARTUP_SOURCE_UNREADABLE")?;
    if input
        .metadata()
        .map_err(|_| "STARTUP_SOURCE_UNREADABLE")?
        .len()
        != len
        || format!("{:x}", Sha256::digest(&bytes)) != expected
    {
        return Err("STARTUP_SOURCE_CHANGED");
    }
    Ok(bytes)
}
fn run() -> Result<Value, &'static str> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if !matches!(args.len(), 6 | 7)
        || args[0] != "--profile-root"
        || args[2] != "--assistant-sha256"
        || args[4] != "--wechat-sha256"
    {
        return Err("STARTUP_INVALID_INPUT");
    }
    let root = PathBuf::from(&args[1]);
    if !root.is_absolute() {
        return Err("STARTUP_INVALID_INPUT");
    }
    let hash = |n: usize| {
        args[n]
            .to_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|v| v.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase)
            .ok_or("STARTUP_INVALID_INPUT")
    };
    let (ah, ch) = (hash(3)?, hash(5)?);
    let test = args.len() == 7;
    if test
        && (args[6] != "--test-gate"
            || root.parent() != Some(std::env::temp_dir().as_path())
            || !root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("startup-inspect-"))
            || std::env::var_os("HOMEBASE_TEST_HOME").is_none_or(|v| PathBuf::from(v) != root))
    {
        return Err("STARTUP_TEST_GATE_REJECTED");
    }
    let af = root.join(".homebase/assistant/state.dpapi");
    let cf = root.join(".homebase/wechat/channel.dpapi");
    let ab = read_cipher(&af, &ah)?;
    let cb = read_cipher(&cf, &ch)?;
    let ap = wechat_crypto::crypt(&ab, false).map_err(|_| "STARTUP_STATE_UNREADABLE")?;
    let cp = wechat_crypto::crypt(&cb, false).map_err(|_| "STARTUP_STATE_UNREADABLE")?;
    if ap.len() > 32 * 1024 * 1024 || cp.len() > 32 * 1024 * 1024 {
        return Err("STARTUP_SOURCE_SIZE");
    }
    let a: Assistant = serde_json::from_slice(&ap).map_err(|_| "STARTUP_METADATA_INVALID")?;
    let c: Channel = serde_json::from_slice(&cp).map_err(|_| "STARTUP_METADATA_INVALID")?;
    let result = metadata(a, c)?;
    if test {
        eprintln!("startup-inspect:test-gate");
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|_| "STARTUP_TEST_GATE_UNRELEASED")?;
        if line.trim() != "inspect" {
            return Err("STARTUP_TEST_GATE_UNRELEASED");
        }
    }
    read_cipher(&af, &ah)?;
    read_cipher(&cf, &ch)?;
    Ok(
        json!({"schema_version":1,"inspection_complete":true,"consistent":true,"metadata":result,"local_operational_ledger_unprotection":true,"authentication_values_exported":false,"network_or_executor_started":false,"writes_performed":false,"safe_to_update":false}),
    )
}
fn main() {
    match run() {
        Ok(value) => println!("{}", value),
        Err(code) => {
            println!(
                "{}",
                json!({"schema_version":1,"inspection_complete":false,"code":code,"metadata":null,"authentication_values_exported":false,"network_or_executor_started":false,"writes_performed":false,"safe_to_update":false})
            );
            std::process::exit(1);
        }
    }
}
