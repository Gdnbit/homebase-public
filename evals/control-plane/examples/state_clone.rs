//! Isolated synthetic benchmark of the real State clone path; no provider/files.
use homebase_control_tests::{
    assistant::{Control, Model, SendRequest, State},
    test_support::{legacy_engine, Save},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{hint::black_box, sync::Arc, time::Instant};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let saved = Arc::new(Mutex::new(State::default()));
    let save: Save = Arc::new({
        let saved = saved.clone();
        move |state| {
            let saved = saved.clone();
            Box::pin(async move {
                *saved.lock().await = state;
                Ok(())
            })
        }
    });
    let model: Model = Arc::new(|_| {
        Box::pin(async { panic!("disabled synthetic benchmark cannot call a model") })
    });
    let control: Control =
        Arc::new(|_| Box::pin(async { panic!("synthetic benchmark cannot submit a task") }));
    let root = tempfile::tempdir().unwrap();
    let engine = legacy_engine(
        State::default(),
        save,
        model,
        control,
        root.path().join("workspace"),
    )
    .await
    .unwrap();
    let request: SendRequest = serde_json::from_value(
        json!({"text":"合成基准","request_id":"clone-fixture","mode":"task"}),
    )
    .unwrap();
    engine.send(request).await.unwrap();
    let mut input = serde_json::to_value(saved.lock().await.clone()).unwrap();
    fn normalize(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, item) in map {
                    if item.is_string() && (key == "id" || key == "session_id" || key == "turn_id")
                    {
                        *item = json!("fixed-fixture-id");
                    } else if item.is_string() && key.ends_with("_at") {
                        *item = json!("2026-11-15T00:00:00Z");
                    } else if key == "runtime_prompt" {
                        *item = json!("synthetic-prompt");
                    } else {
                        normalize(item);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    normalize(item);
                }
            }
            _ => (),
        }
    }
    normalize(&mut input);
    // Seed through the original JSON wire format rather than binding the
    // benchmark to either the old Vec or the new internal representation.
    let session = &mut input["sessions"][0];
    let template = session["turns"][0].clone();
    let payload = "合成检查点🙂".repeat(400);
    session["turns"]=json!((0..80).map(|n|{
        let mut turn=template.clone();turn["id"]=json!(format!("turn-{n}"));turn["request_id"]=json!(format!("request-{n}"));
        turn["protocol"]=json!([{"role":"user","content":payload},{"role":"assistant","content":payload},{"role":"tool","content":payload}]);
        turn["runtime_operations"]=json!([{"fixture":payload},{"fixture":payload}]);turn
    }).collect::<Vec<Value>>());
    session["messages"]=json!((0..160).map(|n|json!({"id":format!("message-{n}"),"role":if n%2==0{"user"}else{"assistant"},"content":payload,"created_at":"2026-11-15T00:00:00Z","turn_id":format!("turn-{}",n/2)})).collect::<Vec<_>>());
    session["jobs"]=json!((0..80).map(|n|json!({"id":format!("job-{n}"),"command":"system_info","turn_id":format!("turn-{n}"),"state":"succeeded","result":{"status":"succeeded","fixture":payload},"owned":true,"deliver":false,"artifacts":[],"remote":null})).collect::<Vec<_>>());
    let state: State = serde_json::from_value(input).unwrap();
    let bytes = serde_json::to_vec(&state).unwrap();
    let iterations = 80;
    let mut samples = Vec::new();
    for _ in 0..iterations {
        let start = Instant::now();
        let copy = black_box(state.clone());
        samples.push(start.elapsed().as_nanos());
        black_box(copy);
    }
    samples.sort();
    println!(
        "{}",
        json!({"synthetic_only":true,"real_model_calls":0,"real_wechat_messages":0,"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(&bytes)),"turns":state.sessions[0].turns.len(),"messages":state.sessions[0].messages.len(),"jobs":state.sessions[0].jobs.len(),"iterations":iterations,"clone_median_ns":samples[iterations/2],"clone_p95_ns":samples[iterations*95/100]})
    );
}
