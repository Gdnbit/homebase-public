//! Read-only query helpers: absence is allowed only where explicitly requested.
//! Errors never contain file bodies or credential values.
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::{fs, io::{self, BufRead, ErrorKind}, path::Path};

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryIssue {
    pub field: String,
    pub status: String,
    pub message: String,
}

pub fn issue(field: &str, message: &str) -> QueryIssue {
    QueryIssue {
        field: field.into(),
        status: "unknown".into(),
        message: message.into(),
    }
}

pub fn entries(path: &Path, allow_missing: bool) -> Result<Vec<fs::DirEntry>, String> {
    match fs::read_dir(path) {
        Ok(items) => items
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "QUERY_FAILED: 目录条目读取失败，结果不完整".into()),
        Err(error) if allow_missing && error.kind() == ErrorKind::NotFound => Ok(vec![]),
        Err(_) => Err("QUERY_FAILED: 目录不可读取，不能判定为空".into()),
    }
}

pub fn optional_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(raw.trim_start_matches('\u{feff}'))
            .map_err(|_| "QUERY_INVALID: 配置格式无效，未回退默认值".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(T::default()),
        Err(_) => Err("QUERY_FAILED: 配置不可读取，未回退默认值".into()),
    }
}

pub enum JsonLinesError {
    Read(io::Error),
    Invalid,
}

pub fn visit_json_lines<T: DeserializeOwned>(
    mut reader: impl BufRead,
    mut visit: impl FnMut(T),
) -> Result<(), JsonLinesError> {
    let mut line = String::new();
    let mut invalid = false;
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(JsonLinesError::Read)? == 0 {
            break;
        }
        // Read through EOF even after invalid JSON, so an unreadable file is
        // reported the same way as when the whole UTF-8 file was read first.
        if !invalid && !line.trim().is_empty() {
            match serde_json::from_str(&line) {
                Ok(item) => visit(item),
                Err(_) => invalid = true,
            }
        }
    }
    if invalid { Err(JsonLinesError::Invalid) } else { Ok(()) }
}

pub fn validate_metrics(cores: usize, used: u64, total: u64) -> Result<(), String> {
    if cores == 0 || total == 0 || used > total {
        Err("QUERY_INVALID: CPU 或内存采样缺失/无效，不能以零值代替".into())
    } else {
        Ok(())
    }
}

pub fn finished_status(reported: Option<&str>, exit_success: bool) -> (&str, Option<&'static str>) {
    if !exit_success {
        return ("failed", Some("进程非零退出，请检查业务状态与输出"));
    }
    match reported {
        Some(
            status @ ("success" | "completed" | "done" | "failed" | "error" | "cancelled"
            | "interrupted"),
        ) => (status, None),
        _ => (
            "unknown",
            Some("进程已退出，但未取得可靠的业务终态；不能依据退出码判定任务成功"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_lines_preserves_order_blank_lines_and_distinguishes_utf8_from_json() {
        let mut rows = vec![];
        assert!(visit_json_lines(&b"1\r\n \n2\n3"[..], |n: u32| rows.push(n)).is_ok());
        assert_eq!(rows, [1, 2, 3]);
        assert!(matches!(visit_json_lines(&b"1\ninvalid\n3"[..], |_: u32| {}), Err(JsonLinesError::Invalid)));
        assert!(matches!(visit_json_lines(&b"1\ninvalid\n\xff"[..], |_: u32| {}), Err(JsonLinesError::Read(error)) if error.kind() == ErrorKind::InvalidData));
    }
    #[test]
    fn query_safety_missing_empty_invalid_and_unreadable_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        assert!(entries(dir.path(), false).unwrap().is_empty());
        let absent = dir.path().join("absent");
        assert!(entries(&absent, true).unwrap().is_empty());
        assert!(entries(&absent, false).is_err());
        assert_eq!(
            optional_json::<Vec<String>>(&absent).unwrap(),
            Vec::<String>::new()
        );
        let file = dir.path().join("config.json");
        fs::write(&file, "synthetic confidential broken json").unwrap();
        let error = optional_json::<Vec<String>>(&file).unwrap_err();
        assert!(error.starts_with("QUERY_INVALID:"));
        assert!(!error.contains("confidential"));
        assert!(entries(&file, true).is_err());
        assert!(optional_json::<Vec<String>>(dir.path()).is_err());
        fs::write(&file, "\u{feff}[]").unwrap();
        assert!(optional_json::<Vec<String>>(&file).unwrap().is_empty());
    }
    #[test]
    fn query_safety_metric_failure_never_becomes_zero_or_healthy() {
        assert!(validate_metrics(0, 0, 100).is_err());
        assert!(validate_metrics(1, 0, 0).is_err());
        assert!(validate_metrics(1, 101, 100).is_err());
        assert!(validate_metrics(1, 0, 100).is_ok());
    }
    #[test]
    fn query_safety_exit_code_is_not_business_completion() {
        for state in [None, Some("running"), Some(""), Some("future_state")] {
            assert_eq!(finished_status(state, true).0, "unknown");
            assert!(finished_status(state, true).1.is_some());
        }
        assert_eq!(
            finished_status(Some("completed"), true),
            ("completed", None)
        );
        assert_eq!(finished_status(Some("success"), false).0, "failed");
    }
}
