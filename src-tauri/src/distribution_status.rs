//! Bounded, passive reading of generated initialization metadata.
use std::{fs, io::Read, path::Path};

pub(crate) fn initialization_record(data_root: &Path) -> (Option<String>, &'static str) {
    let file = match fs::File::open(data_root.join("distribution-state.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (None, "missing"),
        Err(_) => return (None, "unreadable"),
    };
    let mut bytes = Vec::new();
    if file.take(64 * 1024 + 1).read_to_end(&mut bytes).is_err() {
        return (None, "unreadable");
    }
    if bytes.len() > 64 * 1024 {
        return (None, "invalid");
    }
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return (None, "invalid"),
    };
    match value.get("initialized_at").and_then(|value| value.as_str()) {
        Some(time) if chrono::DateTime::parse_from_rfc3339(time).is_ok() => {
            (Some(time.to_string()), "known")
        }
        _ => (None, "invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_queries_preserve_files_and_report_missing_invalid_and_oversize() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("distribution-state.json");
        assert_eq!(initialization_record(temp.path()), (None, "missing"));
        assert!(!file.exists());
        let original = br#"{"initialized_at":"2026-11-16T01:20:00+08:00"}"#;
        fs::write(&file, original).unwrap();
        for _ in 0..3 {
            assert_eq!(
                initialization_record(temp.path()),
                (Some("2026-11-16T01:20:00+08:00".into()), "known")
            );
            assert_eq!(fs::read(&file).unwrap(), original);
        }
        for invalid in [
            b"{}".to_vec(),
            br#"{"initialized_at":"not-a-date"}"#.to_vec(),
            vec![b'x'; 64 * 1024 + 1],
        ] {
            fs::write(&file, &invalid).unwrap();
            assert_eq!(initialization_record(temp.path()), (None, "invalid"));
            assert_eq!(fs::read(&file).unwrap(), invalid);
        }
        let retained = temp.path().join("retained-invalid.json");
        fs::rename(&file, &retained).unwrap();
        fs::create_dir(&file).unwrap();
        assert_eq!(initialization_record(temp.path()), (None, "unreadable"));
        assert!(file.is_dir() && retained.is_file());
    }
}
