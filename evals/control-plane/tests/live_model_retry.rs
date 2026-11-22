//! Opt-in: synthetic HTTP faults followed by one real call through the owned backend.
use homebase_control_tests::{
    assistant::Settings,
    assistant_protocol::{self, ModelRequest},
    model_capability_io::reliability,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
#[ignore = "requires explicit live free-model authorization and isolated HOMEBASE_PERSONAL_LIVE_LAB"]
async fn retry_synthetic_http_faults_then_real_free_model_once() {
    let lab = PathBuf::from(std::env::var_os("HOMEBASE_PERSONAL_LIVE_LAB").expect("explicit lab"));
    assert!(lab.is_absolute());
    let settings: Settings =
        serde_json::from_slice(&std::fs::read(lab.join("settings.json")).unwrap()).unwrap();
    assert!(settings.free_only && settings.enabled && settings.route_id.ends_with(":free"));
    let r = ModelRequest {
        vision: None,
        route_id: settings.route_id.clone(),
        credential_source: settings.credential_source,
        free_only: true,
        max_tokens: settings.max_tokens,
        messages: vec![
            json!({"role":"user","content":"这是合成的连接恢复测试。只用一句简短中文说明：连接已恢复，原任务结果已保留。不要调用任何工具。"}),
        ],
        definitions: Some(vec![]),
    };
    let body = assistant_protocol::body("chat", "openrouter", &r).unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let root = lab.clone();
    let route = r.route_id.clone();
    let relay = tokio::spawn(async move {
        for index in 0..3 {
            let (mut stream, _) = socket.accept().await.unwrap();
            let mut bytes = vec![];
            let (end, length) = loop {
                let mut buf = [0; 8192];
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|s| {
                            s.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break (end + 4, length);
                    }
                }
            };
            seen.fetch_add(1, Ordering::SeqCst);
            let (status, content) = if index < 2 {
                (if index == 0 { 503 } else { 429 }, "{}".to_string())
            } else {
                let input: Value = serde_json::from_slice(&bytes[end..end + length]).unwrap();
                let mut cmd = tokio::process::Command::new("node");
                cmd.arg(".tmp/live-personal-model.mjs")
                    .arg("model")
                    .current_dir("C:/HomeBase/source")
                    .env("HOMEBASE_PERSONAL_LIVE_LAB", root.clone())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                #[cfg(windows)]
                cmd.creation_flags(0x08000000);
                let mut child = cmd.spawn().unwrap();
                child.stdin.take().unwrap().write_all(&serde_json::to_vec(&json!({"route_id":route,"input":input,"credential_source":settings.credential_source})).unwrap()).await.unwrap();
                let output = child.wait_with_output().await.unwrap();
                assert!(output.status.success(),"Owned real model call failed; inspect original model job; never replay automatically");
                (200, String::from_utf8(output.stdout).unwrap())
            };
            let response=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nRetry-After: 0\r\n\r\n{content}",content.len());
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let (response, transport) =
        reliability::send(reqwest::Client::new().post(url).json(&body), true)
            .await
            .unwrap();
    assert!(response.content_length().is_some_and(|n| n <= 1024 * 1024));
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() <= 1024 * 1024);
    let output: Value = serde_json::from_slice(&bytes).unwrap();
    let reply = assistant_protocol::parse("chat", &output).unwrap();
    assert!(reply.text.contains("恢复") && reply.text.contains("保留"));
    assert!(reply.calls.is_empty());
    assert_eq!(reply.usage.as_ref().unwrap().cost, Some(0.0));
    assert_eq!(transport.attempts, 3);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    relay.await.unwrap();
    let report = json!({"passed":true,"faults_injected_not_actual_provider_errors":true,"injected_http_statuses":[503,429],"relay_attempts":3,"real_model_job_submissions":1,"real_model":r.route_id,"backend_transport":reply.diagnostics,"usage":reply.usage,"final_reply":reply.text,"real_wechat_test_messages":0,"business_operations_executed":0});
    let file = lab.join("real-retry-relay-validation.json");
    assert!(!file.exists());
    std::fs::write(&file, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("REAL_RETRY_VALIDATION {}", file.display());
}
