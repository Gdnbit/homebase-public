use homebase_secret_lifecycle_tests::adapters::sops_probe::{probe, ProbeFailure, ProbePolicy};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
fn executable(case: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let exe = directory.path().join(format!("{case}.exe"));
    std::fs::copy(env!("CARGO_BIN_EXE_sops-probe-fixture"), &exe).unwrap();
    (directory, exe)
}
#[tokio::test]
async fn fast_success_uses_only_offline_flags_and_credential_free_environment() {
    let (_dir, exe) = executable("fast");
    let result = probe(&exe, ProbePolicy::default())
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.message()));
    assert_eq!(result.version, "9.8.7-fixture");
    assert_eq!(result.attempts, 1);
}
#[tokio::test]
async fn first_slow_start_is_killed_then_recovers_on_second_attempt() {
    let (_dir, exe) = executable("recover");
    let result = probe(
        &exe,
        ProbePolicy {
            first: Duration::from_millis(500),
            retry: Duration::from_secs(2),
        },
    )
    .await
    .unwrap_or_else(|failure| panic!("{}", failure.message()));
    assert_eq!(result.attempts, 2);
    assert_eq!(result.version, "9.8.7-fixture");
    assert!(result.elapsed_ms >= 500);
}
#[tokio::test]
async fn permanent_hang_remains_a_retryable_failure_with_bounded_wait() {
    let (_dir, exe) = executable("hang");
    let start = Instant::now();
    let failure = probe(
        &exe,
        ProbePolicy {
            first: Duration::from_millis(100),
            retry: Duration::from_millis(200),
        },
    )
    .await
    .err()
    .expect("must not pass");
    assert!(matches!(failure, ProbeFailure::Timeout { attempts: 2, .. }));
    assert!(failure.retryable());
    assert!(start.elapsed() < Duration::from_secs(10));
}
#[tokio::test]
async fn invalid_empty_and_nonzero_exit_never_pass_or_retry_as_transient() {
    for case in ["invalid", "empty", "failed"] {
        let (_dir, exe) = executable(case);
        let failure = probe(&exe, ProbePolicy::default())
            .await
            .err()
            .expect("must not pass");
        assert!(!failure.retryable());
        assert!(!failure.message().contains("synthetic unrelated"));
    }
}
#[tokio::test]
async fn bundled_real_sops_probe_runs_without_any_identity_or_vault() {
    let exe =
        std::env::var_os("HOMEBASE_TEST_SOPS_EXE").expect("explicit test executable required");
    let result = probe(Path::new(&exe), ProbePolicy::default())
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.message()));
    assert!(result.version.starts_with("3."));
    println!(
        "Real offline SOPS probe: {} attempt(s), {} ms",
        result.attempts, result.elapsed_ms
    );
}
