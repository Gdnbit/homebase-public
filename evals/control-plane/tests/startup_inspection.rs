//! Maintenance inspection uses only encrypted synthetic ledgers and no runtime.
#![cfg(windows)]
use homebase_control_tests::wechat_crypto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const PRIVATE: &str = "PRIVATE-启动账本-token-message-不得输出";
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
struct Fixture {
    root: tempfile::TempDir,
    assistant: PathBuf,
    channel: PathBuf,
}
impl Fixture {
    fn new(a: Value, c: Value) -> Self {
        let root = tempfile::Builder::new()
            .prefix("startup-inspect-")
            .tempdir()
            .unwrap();
        let assistant = root.path().join(".homebase/assistant/state.dpapi");
        let channel = root.path().join(".homebase/wechat/channel.dpapi");
        fs::create_dir_all(assistant.parent().unwrap()).unwrap();
        fs::create_dir_all(channel.parent().unwrap()).unwrap();
        fs::write(
            &assistant,
            wechat_crypto::crypt(&serde_json::to_vec(&a).unwrap(), true).unwrap(),
        )
        .unwrap();
        fs::write(
            &channel,
            wechat_crypto::crypt(&serde_json::to_vec(&c).unwrap(), true).unwrap(),
        )
        .unwrap();
        Self {
            root,
            assistant,
            channel,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_startup-inspect"));
        cmd.arg("--profile-root").arg(self.root.path()).args([
            "--assistant-sha256",
            &digest(&fs::read(&self.assistant).unwrap()),
            "--wechat-sha256",
            &digest(&fs::read(&self.channel).unwrap()),
        ]);
        cmd
    }
    fn unchanged(&self, before: &[Vec<u8>; 2]) {
        assert_eq!(fs::read(&self.assistant).unwrap(), before[0]);
        assert_eq!(fs::read(&self.channel).unwrap(), before[1]);
        assert_eq!(
            fs::read_dir(self.assistant.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(self.channel.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(fs::read_dir(self.root.path()).unwrap().count(), 1);
    }
    fn snapshot(&self) -> [Vec<u8>; 2] {
        [
            fs::read(&self.assistant).unwrap(),
            fs::read(&self.channel).unwrap(),
        ]
    }
}
fn a() -> Value {
    json!({"sessions":[],"settings":{"key":PRIVATE},"private":PRIVATE})
}
fn c() -> Value {
    json!({"enabled":true,"token":PRIVATE,"bot":PRIVATE,"owner":PRIVATE,"base":PRIVATE,"cursor":PRIVATE,"records":[]})
}
fn inspect(out: Output, code: Option<&str>) -> Value {
    for bytes in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains(PRIVATE));
    }
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["network_or_executor_started"], false);
    assert_eq!(value["authentication_values_exported"], false);
    assert_eq!(value["writes_performed"], false);
    assert_eq!(value["safe_to_update"], false);
    assert_eq!(out.status.success(), code.is_none());
    if let Some(code) = code {
        assert_eq!(value["code"], code);
        assert!(value["metadata"].is_null());
    } else {
        assert_eq!(value["consistent"], true);
        assert_eq!(value["inspection_complete"], true);
    }
    value
}
#[test]
fn empty_ledgers_are_read_only_and_never_echo_authentication() {
    let f = Fixture::new(a(), c());
    let before = f.snapshot();
    let v = inspect(f.command().output().unwrap(), None);
    assert_eq!(v["metadata"]["assistant"]["turns"], 0);
    assert_eq!(
        v["metadata"]["wechat"]["binding_and_network"],
        "not_inspected"
    );
    f.unchanged(&before);
}
#[test]
fn full_ledger_includes_old_records_outside_status_window_and_unknown_buckets() {
    let mut channel = c();
    let mut records = vec![
        json!({"command":"assistant","status":"received","delivery":"pending","job_id":null,"args":PRIVATE,"context":PRIVATE,"result":PRIVATE}),
    ];
    records.extend((0..60).map(|_|json!({"command":"assistant","status":"succeeded","delivery":"accepted","job_id":PRIVATE})));
    records.push(json!({"command":"assistant","status":"running","delivery":"unknown","job_id":PRIVATE,"progress_delivery":"pending"}));
    records.push(json!({"command":"assistant","status":"waiting_input_queue","delivery":"pending","job_id":null}));
    records.push(json!({"command":"message","status":"received","delivery":"accepted"}));
    records.push(json!({"command":PRIVATE,"status":PRIVATE,"delivery":PRIVATE}));
    channel["records"] = json!(records);
    let f = Fixture::new(a(), channel);
    let before = f.snapshot();
    let v = inspect(f.command().output().unwrap(), None);
    let w = &v["metadata"]["wechat"];
    assert_eq!(w["records"], 65);
    assert_eq!(w["received_that_monitor_can_submit"], 1);
    assert_eq!(w["original_jobs_monitor_can_poll"], 1);
    assert_eq!(w["monitored_records_missing_job"], 1);
    assert_eq!(w["pending_progress_records"], 1);
    assert_eq!(w["raw_record_states"]["other"], 1);
    assert_eq!(w["delivery_states"]["other"], 1);
    f.unchanged(&before);
}
#[test]
fn uncertain_originals_and_startup_timers_are_distinguished_without_resuming() {
    let mut state = a();
    let turn = |status: &str| json!({"state":status,"steps":[{"state":"unknown","arguments":PRIVATE}],"pending":null,"runtime_uncertain_model":true,"queued":[PRIVATE],"text":PRIVATE});
    state["sessions"] = json!([{"queue_paused":true,"turns":[turn("failed"),turn("interrupted_unknown"),turn("unknown")],"jobs":[{"state":"succeeded","remote":null}]}]);
    state["personal"] = json!({"memories":[{"expires_at":"2000-01-01T00:00:00Z"},{"expires_at":"2000-01-01T00:00:00Z","expiry_notified":true}],"reminders":[{"enabled":true,"delivery":"scheduled","due_at":"2000-01-01T00:00:00Z"},{"enabled":true,"delivery":"scheduled","due_at":"2099-01-01T00:00:00Z"},{"enabled":true,"delivery":"scheduled","due_at":PRIVATE},{"enabled":true,"delivery":"pending","source":"wechat","due_at":"2000-01-01T00:00:00Z"}]});
    state["followups"] = json!({"entries":[{"enabled":true,"state":"scheduled","due_at":"2000-01-01T00:00:00Z","notice_delivery":"pending","runs":[{"state":"running"}]}]});
    state["matters"] = json!({"entries":[{"state":"active","delivery":"scheduled","review_at":"2000-01-01T00:00:00Z"},{"state":"active","delivery":"claimed"}]});
    let f = Fixture::new(state, c());
    let before = f.snapshot();
    let v = inspect(f.command().output().unwrap(), None);
    let s = &v["metadata"]["assistant"];
    assert_eq!(s["constructor_turn_candidates_upper_bound"], 0);
    assert_eq!(s["uncertain_model_turns"], 3);
    assert_eq!(s["step_states"]["unknown"], 3);
    assert_eq!(s["queued_call_references"], 3);
    let t = &v["metadata"]["scheduled"];
    for name in [
        "expired_memory_updates",
        "due_reminders",
        "due_followups",
        "unfinished_followup_runs",
        "due_matters",
        "invalid_dates",
        "pending_wechat_reminders",
        "pending_followup_notices",
    ] {
        assert_eq!(t[name], 1, "{name}");
    }
    assert_eq!(s["constructor_claimed_notice_changes"], 1);
    f.unchanged(&before);
}
#[test]
fn active_candidates_pauses_approvals_and_remote_jobs_are_counted_conservatively() {
    let mut state = a();
    state["sessions"] = json!([{"turns":[{"state":"waiting_input_queue","steps":[]},{"state":"running","steps":[],"pending":{"private":PRIVATE}},{"state":"queued","steps":[]}],"jobs":[{"state":"succeeded","remote":{"finished":false}},{"state":"succeeded","remote":{"finished":true,"query_job":PRIVATE}},{"state":"running"}]}]);
    let f = Fixture::new(state, c());
    let before = f.snapshot();
    let v = inspect(f.command().output().unwrap(), None);
    let s = &v["metadata"]["assistant"];
    assert_eq!(s["constructor_turn_candidates_upper_bound"], 2);
    assert_eq!(s["constructor_queue_pauses"], 1);
    assert_eq!(s["pending_approvals"], 1);
    assert_eq!(s["pending_jobs"], 3);
    f.unchanged(&before);
}
#[test]
fn malformed_schema_plaintext_wrong_hash_and_missing_sources_fail_closed() {
    for (kind, code) in [
        ("plain", "STARTUP_STATE_UNREADABLE"),
        ("schema", "STARTUP_METADATA_INVALID"),
        ("empty", "STARTUP_SOURCE_SIZE"),
        ("large", "STARTUP_SOURCE_SIZE"),
        ("wronghash", "STARTUP_SOURCE_CHANGED"),
        ("missing", "STARTUP_SOURCE_UNREADABLE"),
    ] {
        let f = Fixture::new(a(), c());
        match kind {
            "plain" => fs::write(&f.assistant, PRIVATE).unwrap(),
            "schema" => fs::write(
                &f.assistant,
                wechat_crypto::crypt(
                    &serde_json::to_vec(&json!({"sessions":PRIVATE})).unwrap(),
                    true,
                )
                .unwrap(),
            )
            .unwrap(),
            "empty" => fs::write(&f.assistant, []).unwrap(),
            "large" => fs::OpenOptions::new()
                .write(true)
                .open(&f.assistant)
                .unwrap()
                .set_len(32 * 1024 * 1024 + 1)
                .unwrap(),
            _ => (),
        }
        let mut cmd = f.command();
        let before = f.snapshot();
        if kind == "missing" {
            fs::remove_file(&f.channel).unwrap();
        }
        if kind == "wronghash" {
            fs::write(&f.assistant, wechat_crypto::crypt(b"{}", true).unwrap()).unwrap();
        }
        inspect(cmd.output().unwrap(), Some(code));
        if !matches!(kind, "wronghash" | "missing") {
            f.unchanged(&before);
        }
    }
}
#[test]
fn invalid_paths_and_test_gate_context_are_rejected() {
    let f = Fixture::new(a(), c());
    let before = f.snapshot();
    inspect(
        Command::new(env!("CARGO_BIN_EXE_startup-inspect"))
            .arg(PRIVATE)
            .output()
            .unwrap(),
        Some("STARTUP_INVALID_INPUT"),
    );
    inspect(
        f.command()
            .arg("--test-gate")
            .env("HOMEBASE_TEST_HOME", f.root.path().join("different"))
            .output()
            .unwrap(),
        Some("STARTUP_TEST_GATE_REJECTED"),
    );
    inspect(
        f.command()
            .arg("--other")
            .env("HOMEBASE_TEST_HOME", f.root.path())
            .output()
            .unwrap(),
        Some("STARTUP_TEST_GATE_REJECTED"),
    );
    inspect(
        f.command()
            .arg("--test-gate")
            .env("HOMEBASE_TEST_HOME", f.root.path())
            .env("TMP", f.root.path())
            .env("TEMP", f.root.path())
            .stdin(Stdio::null())
            .output()
            .unwrap(),
        Some("STARTUP_TEST_GATE_REJECTED"),
    );
    inspect(
        f.command()
            .arg("--test-gate")
            .env("HOMEBASE_TEST_HOME", f.root.path())
            .stdin(Stdio::null())
            .output()
            .unwrap(),
        Some("STARTUP_TEST_GATE_UNRELEASED"),
    );
    f.unchanged(&before);
}
#[test]
fn deterministic_change_after_inspection_and_before_final_check_is_rejected() {
    for which in ["assistant", "channel"] {
        let f = Fixture::new(a(), c());
        let mut child = f
            .command()
            .arg("--test-gate")
            .env("HOMEBASE_TEST_HOME", f.root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut line = String::new();
        stderr.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "startup-inspect:test-gate");
        let target: &Path = if which == "assistant" {
            &f.assistant
        } else {
            &f.channel
        };
        fs::write(target, wechat_crypto::crypt(b"{}", true).unwrap()).unwrap();
        child.stdin.take().unwrap().write_all(b"inspect\n").unwrap();
        inspect(
            child.wait_with_output().unwrap(),
            Some("STARTUP_SOURCE_CHANGED"),
        );
    }
}
#[test]
fn locked_source_is_unreadable_without_partial_metadata() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new(a(), c());
    let mut cmd = f.command();
    let before = f.snapshot();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&f.channel)
        .unwrap();
    inspect(cmd.output().unwrap(), Some("STARTUP_SOURCE_UNREADABLE"));
    drop(held);
    f.unchanged(&before);
}
