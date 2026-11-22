//! Read-only observations. Lack of an owned child handle never proves interruption.
use serde_json::Value;
use std::path::Path;

pub fn valid_job_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub fn same_file(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub fn observe(mut value: Value, owned: bool) -> Result<Value, String> {
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .ok_or("QUERY_INVALID: 任务缺少 status，无法恢复监控")?;
    let active = matches!(
        status,
        "queued" | "preflight" | "preprocessing" | "running" | "fusing"
    );
    let terminal = matches!(
        status,
        "completed" | "succeeded" | "failed" | "cancelled" | "interrupted" | "verification_failed"
    );
    let (state, message) = if active && owned {
        (
            "managed",
            "任务由当前 HomeBase 实例管理，已恢复监控；未修改原始任务记录。",
        )
    } else if active {
        ("unmanaged", "任务状态未知：当前实例未持有此任务，无法确认是否仍在运行。已保留原始状态，不会自动中断、重启或取消。")
    } else if terminal {
        ("terminal", "任务记录已是终态，仅只读展示。")
    } else {
        (
            "unknown",
            "任务状态未知：无法识别记录状态，已保留原始记录，请核对任务结果。",
        )
    };
    let object = value
        .as_object_mut()
        .ok_or("QUERY_INVALID: 任务记录不是对象")?;
    object.insert("runtime_state".into(), state.into());
    object.insert("runtime_message".into(), message.into());
    Ok(value)
}

#[cfg(test)]
pub fn snapshot(
    root: &Path,
) -> std::collections::BTreeMap<std::path::PathBuf, (Vec<u8>, std::time::SystemTime)> {
    let mut result = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(snapshot(&path));
        } else {
            result.insert(
                path.clone(),
                (
                    std::fs::read(&path).unwrap(),
                    std::fs::metadata(&path).unwrap().modified().unwrap(),
                ),
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_recovery_observations_preserve_status_and_unknowns() {
        for status in ["queued", "preflight", "preprocessing", "running", "fusing"] {
            let source = serde_json::json!({"status":status,"pid":42,"revision":8,"error":null});
            let observed = observe(source.clone(), false).unwrap();
            assert_eq!(observed["runtime_state"], "unmanaged");
            for key in ["status", "pid", "revision", "error"] {
                assert_eq!(observed[key], source[key]);
            }
            assert_eq!(observe(source, true).unwrap()["runtime_state"], "managed");
        }
        for status in [
            "completed",
            "succeeded",
            "failed",
            "cancelled",
            "interrupted",
            "verification_failed",
        ] {
            assert_eq!(
                observe(serde_json::json!({"status":status}), true).unwrap()["runtime_state"],
                "terminal"
            );
        }
        assert_eq!(
            observe(serde_json::json!({"status":"future-state"}), false).unwrap()["runtime_state"],
            "unknown"
        );
        assert!(observe(serde_json::json!({}), false).is_err());
        for id in ["", "../outside", "a/b", "a\\b", ".."] {
            assert!(!valid_job_id(id));
        }
        assert!(valid_job_id("synthetic-job_1"));
    }

    #[test]
    fn task_recovery_ownership_is_bound_to_existing_file_not_just_id() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("a.json");
        let second = temp.path().join("b.json");
        std::fs::write(&first, "synthetic").unwrap();
        std::fs::write(&second, "synthetic").unwrap();
        assert!(same_file(&first, &first));
        assert!(!same_file(&first, &second));
        assert!(!same_file(&first, &temp.path().join("absent.json")));
    }
}
