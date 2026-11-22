use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn server(
    responses: Vec<String>,
    pause: Duration,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let worker = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = socket.accept().await.unwrap();
            let mut input = [0; 8192];
            let _ = stream.read(&mut input).await;
            seen.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(pause).await;
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (url, count, worker)
}
fn http(code: u16, headers: &str) -> String {
    format!(
        "HTTP/1.1 {code} Fixture\r\nContent-Length: 2\r\nConnection: close\r\n{headers}\r\n{{}}"
    )
}

#[tokio::test]
async fn transient_http_retries_are_bounded_honor_headers_and_preserve_request() {
    let (url, count, worker) = server(
        vec![
            http(429, "Retry-After: 0\r\n"),
            http(503, "Retry-After: 0\r\n"),
            http(200, ""),
        ],
        Duration::ZERO,
    )
    .await;
    let (response, report) = send(
        reqwest::Client::new()
            .post(url)
            .json(&json!({"synthetic":true})),
        true,
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(report.attempts, 3);
    assert_eq!(report.retries.len(), 2);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    worker.await.unwrap();
}
#[tokio::test]
async fn rejection_exhaustion_is_known_and_diagnostics_do_not_include_payload() {
    let (url, count, worker) =
        server(vec![http(503, "Retry-After: 0\r\n"); 3], Duration::ZERO).await;
    let error = send(
        reqwest::Client::new()
            .post(url)
            .header("authorization", "synthetic-secret")
            .body("private-test-payload"),
        true,
    )
    .await
    .err()
    .unwrap();
    let failure = Failure::decode(&error).unwrap();
    assert!(failure.safe_to_resume());
    assert_eq!(failure.attempts, 3);
    assert_eq!(failure.http_status, Some(503));
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert!(!error.contains("synthetic-secret"));
    assert!(!error.contains("private-test-payload"));
    worker.await.unwrap();
}
#[tokio::test]
async fn auth_long_retry_after_opt_out_and_noninference_do_not_retry() {
    for (code, headers, eligible) in [
        (401, "", true),
        (429, "Retry-After: 3600\r\n", true),
        (503, "x-should-retry: false\r\n", true),
        (503, "", false),
    ] {
        let (url, count, worker) = server(vec![http(code, headers)], Duration::ZERO).await;
        let failure = Failure::decode(
            &send(reqwest::Client::new().post(url), eligible)
                .await
                .err()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(failure.attempts, 1);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        worker.await.unwrap();
    }
}
#[tokio::test]
async fn timeout_after_submission_is_unknown_and_never_retried() {
    let (url, count, worker) = server(vec![http(200, "")], Duration::from_millis(300)).await;
    let error = send_limited(
        reqwest::Client::new().post(url),
        true,
        Duration::from_secs(1),
        Duration::from_millis(50),
    )
    .await
    .err()
    .unwrap();
    let failure = Failure::decode(&error).unwrap();
    assert_eq!(failure.kind, Kind::Timeout);
    assert!(!failure.safe_to_resume());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    worker.abort();
}
#[tokio::test]
async fn cancellation_during_backoff_stops_the_next_network_request() {
    let (url, count, worker) =
        server(vec![http(429, "Retry-After: 1\r\n"); 2], Duration::ZERO).await;
    let request = tokio::spawn(async move { send(reqwest::Client::new().post(url), true).await });
    while count.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(40)).await;
    request.abort();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    worker.abort();
}
#[test]
fn only_pinned_zero_price_text_function_inference_is_retry_eligible() {
    assert!(text_inference(&["text".into(), "vision".into()]));
    assert!(!text_inference(&["text".into(), "image".into()]));
    let mut input = json!({"provider":{"allow_fallbacks":false,"max_price":{"prompt":0,"completion":0}},"tools":[{"type":"function"}]});
    assert!(retry_eligible(
        "openrouter",
        "chat",
        "openrouter::fixture:free",
        &input,
        false
    ));
    let mut plugin = input.clone();
    plugin["plugins"] = json!([{"id":"web"}]);
    assert!(!retry_eligible(
        "openrouter",
        "chat",
        "fixture:free",
        &plugin,
        false
    ));
    assert!(!retry_eligible(
        "openrouter",
        "chat",
        "openrouter::fixture:free",
        &input,
        true
    ));
    assert!(!retry_eligible(
        "openrouter",
        "chat",
        "openrouter::paid",
        &input,
        false
    ));
    input["tools"] = json!([{"type":"web_search"}]);
    assert!(!retry_eligible(
        "openrouter",
        "chat",
        "fixture:free",
        &input,
        false
    ));
    input["tools"] = json!([]);
    input["modalities"] = json!(["image"]);
    assert!(!retry_eligible(
        "openrouter",
        "chat",
        "fixture:free",
        &input,
        false
    ));
}
#[test]
fn forged_safe_acceptance_is_rejected_and_http_dates_are_understood() {
    let mut f = Failure::unknown(Kind::Timeout);
    f.acceptance = Acceptance::NotSubmitted;
    assert!(Failure::decode(&f.encode()).is_none());
    let header = reqwest::header::HeaderValue::from_str(
        &(chrono::Utc::now() + chrono::Duration::seconds(4)).to_rfc2822(),
    )
    .unwrap();
    assert!(retry_after(Some(&header)).is_some_and(|n| n > 1000 && n <= 4000));
}
