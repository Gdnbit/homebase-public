//! Credential-free, bounded JSON-lines transport for the local agent worker.
use crate::assistant::Records;
use serde::Serialize;
use serde_json::{json, Value};
use std::{future::Future, path::Path, pin::Pin, process::Stdio, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
pub type Handler =
    Arc<dyn Fn(Value) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>> + Send + Sync>;
const FRAME_LIMIT: u64 = 4 * 1024 * 1024;
pub(super) const CHECKPOINT_LIMIT: usize = 512;
const CHECKPOINT_BYTES: usize = 3 * 1024 * 1024;

pub(super) fn append_checkpoint(
    operations: &mut Records<Value>,
    payload: Value,
) -> Result<(), String> {
    let limit = "本轮检查点达到上限，原状态保留";
    if operations.len() >= CHECKPOINT_LIMIT {
        return Err(limit.into());
    }
    operations.push(payload);
    // Callers write a transaction snapshot. Also undo the local append on an
    // encoding or budget error so no caller can publish an oversized journal.
    let bytes = operations.encoded_extra_bytes();
    if !matches!(bytes, Ok(bytes) if bytes <= CHECKPOINT_BYTES - 2) {
        operations.discard_last();
        return Err(if bytes.is_err() {
            "检查点无效"
        } else {
            limit
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_journal_checks_the_new_bytes_and_preserves_its_snapshot() {
        let original = json!({"kind":"put","label":"合成🙂"});
        let mut operations: Records<_> = vec![original.clone()].into();
        let before = operations.clone();
        let empty = json!({"kind":"writes","value":""});
        let used = serde_json::to_vec(&[&original, &empty]).unwrap().len();
        let exact = json!({"kind":"writes","value":"x".repeat(CHECKPOINT_BYTES - used)});
        let oversized = json!({"kind":"writes","value":"x".repeat(CHECKPOINT_BYTES - used + 1)});
        assert!(append_checkpoint(&mut operations, oversized)
            .unwrap_err()
            .contains("达到上限"));
        assert_eq!(operations.len(), 1);
        assert!(operations.shares_record(&before, 0));
        append_checkpoint(&mut operations, exact).unwrap();
        assert_eq!(
            operations.encoded_extra_bytes().unwrap() + 2,
            CHECKPOINT_BYTES
        );
        assert_eq!(
            serde_json::to_vec(&operations).unwrap().len(),
            CHECKPOINT_BYTES
        );
        assert!(operations.shares_record(&before, 0));
        assert!(append_checkpoint(&mut operations, empty).is_err());
        assert_eq!(operations.len(), 2);
        assert_eq!(before.len(), 1);
    }

    #[tokio::test]
    async fn replay_sends_large_saved_records_in_order_before_work_can_begin() {
        let lab = tempfile::tempdir().unwrap();
        let worker = lab.path().join("mock-worker.mjs");
        std::fs::write(
            &worker,
            r#"
import assert from 'node:assert/strict';
import readline from 'node:readline';
const input=readline.createInterface({input:process.stdin,crlfDelay:Infinity});
const send=value=>console.log(JSON.stringify(value));
let index=-1;
input.on('line', line=>{
  const frame=JSON.parse(line);
  if(index===-1){
    assert.equal(frame.checkpoint_count,2);
    assert(!Object.hasOwn(frame,'operations'));
    index=0;
    if(frame.skip) {send({kind:'tool',id:1,payload:{}});return;}
    if(frame.gap) {send({kind:'replay',id:1,payload:{index:1}});return;}
  }else{
    assert.equal(frame.id,index+1);
    assert.equal(frame.result.ordinal,index);
    assert.equal(frame.result.body.length,2_200_000);
    index++;
  }
  if(index===2){send({kind:'complete',result:{text:'resumed'}});return;}
  send({kind:'replay',id:index+1,payload:{index}});
});
"#,
        )
        .unwrap();
        let operations: Records<Value> = (0..2)
            .map(|ordinal| json!({"ordinal":ordinal,"body":"a".repeat(2_200_000)}))
            .collect();
        assert!(operations.encoded_extra_bytes().unwrap() as u64 > FRAME_LIMIT);
        let handler: Handler = Arc::new(|_| panic!("checkpoint replay must not invoke the task"));
        let result = run(
            &worker,
            &lab.path().join("profile"),
            json!({"protocol":1}),
            operations.clone(),
            handler.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result, "resumed");
        let error = run(
            &worker,
            &lab.path().join("profile"),
            json!({"protocol":1,"skip":true}),
            operations.clone(),
            handler.clone(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("尚未按顺序恢复原检查点"));
        let error = run(
            &worker,
            &lab.path().join("profile"),
            json!({"protocol":1,"gap":true}),
            operations,
            handler,
        )
        .await
        .unwrap_err();
        assert!(error.contains("读取顺序不符"));
    }
}

#[derive(Serialize)]
struct Reply<'a> {
    id: u64,
    result: &'a Value,
}

pub async fn run(
    worker: &Path,
    profile: &Path,
    mut init: Value,
    operations: Records<Value>,
    handler: Handler,
) -> Result<String, String> {
    if !worker.is_file() {
        return Err("所选助理运行包缺失，请安装固定版本依赖；原任务记录已保留".into());
    }
    if operations.len() > CHECKPOINT_LIMIT {
        return Err("运行时检查点数量超过上限，原记录保留".into());
    }
    init["checkpoint_count"] = json!(operations.len());
    let bytes = serde_json::to_vec(&init).map_err(|_| "运行时初始状态无法编码")?;
    if bytes.len() as u64 > FRAME_LIMIT {
        return Err("运行时状态超过上限，未截断或覆盖".into());
    }
    std::fs::create_dir_all(profile).map_err(|_| "运行时隔离目录不可创建")?;
    let bundled_node = worker.with_file_name("node.exe");
    let mut command = tokio::process::Command::new(if bundled_node.is_file() {
        bundled_node
    } else {
        "node".into()
    });
    command
        .arg(worker)
        .current_dir(profile)
        .env_clear()
        .env("HOME", profile)
        .env("USERPROFILE", profile)
        .env("TEMP", profile)
        .env("TMP", profile)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in ["SystemRoot", "WINDIR", "PATH", "PATHEXT"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|_| "助理引擎启动失败，请检查 Node 和固定版本依赖")?;
    let mut writer = child.stdin.take().ok_or("运行时输入不可用")?;
    let mut reader = BufReader::new(child.stdout.take().ok_or("运行时输出不可用")?);
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| "运行时初始化失败")?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|_| "运行时初始化失败")?;
    let mut next_checkpoint = 0;
    loop {
        let mut frame = Vec::new();
        let n = tokio::time::timeout(
            Duration::from_secs(180),
            (&mut reader)
                .take(FRAME_LIMIT + 1)
                .read_until(b'\n', &mut frame),
        )
        .await
        .map_err(|_| "运行时无响应，已停止等待；请查询原任务")?
        .map_err(|_| "运行时读取失败；请查询原任务")?;
        if n == 0 {
            return Err("运行时提前退出，已有任务未重提".into());
        }
        if frame.len() as u64 > FRAME_LIMIT {
            return Err("运行时响应超过上限".into());
        }
        let msg: Value = serde_json::from_slice(&frame).map_err(|_| "运行时协议无效")?;
        let bytes = match msg["kind"].as_str() {
            Some("complete") => {
                if next_checkpoint != operations.len() {
                    return Err("运行时尚未按顺序恢复原检查点，未采纳结果".into());
                }
                let text = msg["result"]["text"]
                    .as_str()
                    .ok_or("运行时结果无效")?
                    .to_string();
                let _ = child.kill().await;
                return Ok(text);
            }
            Some("failed") => {
                return Err(format!(
                    "助理引擎：{}",
                    msg["error"].as_str().unwrap_or("执行失败")
                ))
            }
            Some("replay") => {
                let id = msg["id"].as_u64().ok_or("检查点请求编号无效")?;
                if msg["payload"]["index"].as_u64() != Some(next_checkpoint as u64) {
                    return Err("运行时检查点读取顺序不符，未执行原任务".into());
                }
                let operation = operations
                    .get(next_checkpoint)
                    .ok_or("运行时检查点读取超出原编号，未执行原任务")?;
                next_checkpoint += 1;
                serde_json::to_vec(&Reply {
                    id,
                    result: operation,
                })
                .map_err(|_| "原检查点无法编码")?
            }
            Some("model" | "tool" | "checkpoint" | "summarize") => {
                if next_checkpoint != operations.len() {
                    return Err("运行时尚未按顺序恢复原检查点，未执行原任务".into());
                }
                let id = msg["id"].as_u64().ok_or("运行时请求编号无效")?;
                let result = handler(msg).await?;
                serde_json::to_vec(&Reply {
                    id,
                    result: &result,
                })
                .map_err(|_| "运行时响应编码失败")?
            }
            _ => return Err("运行时消息类型不受支持".into()),
        };
        if bytes.len() as u64 > FRAME_LIMIT {
            return Err("单条检查点或工具结果超过运行时传输上限，原记录保留".into());
        }
        writer
            .write_all(&bytes)
            .await
            .map_err(|_| "运行时已断开；未重提任务")?;
        writer.write_all(b"\n").await.map_err(|_| "运行时已断开")?;
    }
}
