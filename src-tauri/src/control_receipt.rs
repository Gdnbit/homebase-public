//! Current-user protection for small, explicitly reviewed control receipts.
use super::Job;
use crate::control_protocol::ControlError;
use crate::wechat::{self, clip_video, publish_package, repository_search, video_cover};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::Zeroizing;

const MAX_PLAIN: usize = 4096;
const MAX_ENCODED: usize = 8192;

pub(super) fn encoded_limit(command: &str) -> u64 {
    if commands().contains(&command) {
        MAX_ENCODED as u64
    } else {
        0
    }
}

pub(super) fn commands() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            "assistant_send", "inbox_submit", "repo_project_analyze",
            repository_search::COMMAND, "capability_clip_video", "capability_assemble_video",
            clip_video::TRIM_ARTIFACT_COMMAND,
            video_cover::COMMAND,
            publish_package::COMMAND,
        ]
    } else {
        &[]
    }
}

pub(super) fn verifies_artifact_files(command: &str) -> bool {
    clip_video::is_clip_command(command) || matches!(command, video_cover::COMMAND | publish_package::COMMAND)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u8,
    id: String,
    request_id: String,
    command: String,
    fingerprint: String,
    requires: Vec<String>,
    result: Value,
}

pub(super) fn seal(job: &Job, result: &Value) -> Option<String> {
    if !commands().contains(&job.command.as_str()) {
        return None;
    }
    let result = if job.command == "repo_project_analyze" {
        let project = &result["project"];
        json!({"status":result["status"],"registered":result["registered"],"project":{
            "id":project["id"],"phase":project["phase"],"revision":project["revision"]
        }})
    } else if job.command == repository_search::COMMAND {
        match repository_search::search_error(result) {
            Some((code, message)) => json!({"status":"failed","error":{"code":code,"message":message}}),
            None => result.clone(),
        }
    } else {
        result.clone()
    };
    if !is_receipt(&job.command, &result) {
        return None;
    }
    let receipt = Receipt {
        version: 1,
        id: job.id.clone(),
        request_id: job.request_id.clone(),
        command: job.command.clone(),
        fingerprint: job.fingerprint.clone(),
        requires: job.requires.clone(),
        result,
    };
    let plain = Zeroizing::new(serde_json::to_vec(&receipt).ok()?);
    if plain.len() > MAX_PLAIN {
        return None;
    }
    let ciphertext = crate::wechat_crypto::crypt(&plain, true).ok()?;
    let encoded = STANDARD.encode(ciphertext);
    (encoded.len() <= MAX_ENCODED).then_some(encoded)
}

pub(super) fn open(job: &Job, sealed: &str) -> Result<Value, ControlError> {
    fn unavailable() -> ControlError {
        ControlError::new(
            "RESULT_UNAVAILABLE",
            "无法恢复受保护的原回执，请按原任务编号核对业务结果。",
        )
    }
    if !commands().contains(&job.command.as_str()) || sealed.len() > MAX_ENCODED {
        return Err(unavailable());
    }
    let ciphertext = STANDARD.decode(sealed).map_err(|_| unavailable())?;
    let plain = crate::wechat_crypto::crypt(&ciphertext, false).map_err(|_| unavailable())?;
    if plain.len() > MAX_PLAIN {
        return Err(unavailable());
    }
    let receipt: Receipt = serde_json::from_slice(&plain).map_err(|_| unavailable())?;
    if receipt.version != 1
        || receipt.id != job.id
        || receipt.request_id != job.request_id
        || receipt.command != job.command
        || receipt.fingerprint != job.fingerprint
        || receipt.requires != job.requires
        || !is_receipt(&job.command, &receipt.result)
    {
        return Err(unavailable());
    }
    if clip_video::is_clip_command(&job.command) {
        let path = clip_path(&receipt.result).ok_or_else(unavailable)?;
        let root = path.parent().and_then(Path::parent).ok_or_else(unavailable)?;
        let bytes = wechat::artifact_bytes_limited(path, root, clip_video::OUTPUT_LIMIT)
            .map_err(|_| unavailable())?;
        let artifact = &receipt.result["artifact"];
        if artifact["bytes"] != bytes.len()
            || artifact["sha256"] != format!("{:x}", Sha256::digest(&bytes))
            || !wechat::video::is_mp4(&bytes)
        {
            return Err(unavailable());
        }
        if let Some(artifact) = receipt.result.get("subtitles") {
            use wechat::subtitles;
            let path = Path::new(artifact["path"].as_str().ok_or_else(unavailable)?);
            let bytes = wechat::artifact_bytes_limited(path, root, subtitles::LIMIT).map_err(|_| unavailable())?;
            if artifact["bytes"] != bytes.len() || artifact["sha256"] != format!("{:x}", Sha256::digest(&bytes))
                || !subtitles::is_timed_srt(&bytes) {
                return Err(unavailable());
            }
        }
    }
    if job.command == video_cover::COMMAND && !video_cover::valid_receipt(&receipt.result, true) {
        return Err(unavailable());
    }
    if job.command == publish_package::COMMAND && !publish_package::valid_receipt(&receipt.result, true) {
        return Err(unavailable());
    }
    Ok(receipt.result)
}

fn clip_path(value: &Value) -> Option<&Path> {
    let path = Path::new(value["artifact"]["path"].as_str()?);
    clip_video::is_clip_output(path).then_some(path)
}

fn is_clip_receipt(command: &str, value: &Value, fields: usize) -> bool {
    let artifact = &value["artifact"];
    let duration = value["duration_ms"].as_u64().unwrap_or(0);
    let timed = value.get("subtitles");
    if fields != 4 + usize::from(timed.is_some())
        || value["status"] != "succeeded"
        || !(150..=30_300).contains(&duration)
        || clip_path(value).is_none()
        || artifact["mime_type"] != "video/mp4"
        || !artifact.as_object().is_some_and(|map| map.len() == 4)
        || !artifact["bytes"].as_u64().is_some_and(|size| (1..=clip_video::OUTPUT_LIMIT as u64).contains(&size))
        || !artifact["sha256"].as_str().is_some_and(|sha| {
            sha.len() == 64 && sha.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    {
        return false;
    }
    if command != "capability_assemble_video" {
        return timed.is_none() && value["start_ms"].as_u64().is_some_and(|start| start <= 119_800)
            && duration <= 30_050;
    }
    if let Some(timed) = timed {
        use wechat::subtitles;
        if timed.as_object().map(|m| m.len()) != Some(4)
            || timed["mime_type"] != subtitles::MIME
            || timed["path"].as_str().map(Path::new).is_none_or(|path| path != clip_path(value).unwrap().with_file_name(subtitles::NAME))
            || !timed["bytes"].as_u64().is_some_and(|size| (1..=subtitles::LIMIT as u64).contains(&size))
            || !timed["sha256"].as_str().is_some_and(|sha| sha.len() == 64 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        { return false; }
    }
    value["segments"].as_array().is_some_and(|segments| {
        (2..=6).contains(&segments.len())
            && serde_json::from_value::<Vec<clip_video::ClipRange>>(value["segments"].clone())
                .is_ok_and(|ranges| clip_video::validate_ranges(&ranges).is_ok())
    })
}

fn is_receipt(command: &str, value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    fn tag(value: &Value, limit: usize) -> bool {
        value.as_str().is_some_and(|text| {
            !text.is_empty()
                && text.len() <= limit
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    }
    match command {
        command if clip_video::is_clip_command(command) => is_clip_receipt(command, value, fields.len()),
        video_cover::COMMAND => video_cover::valid_receipt(value, false),
        publish_package::COMMAND => publish_package::valid_receipt(value, false),
        "assistant_send" => {
            fields.len() == 3
                && tag(&value["session_id"], 128)
                && tag(&value["turn_id"], 128)
                && tag(&value["state"], 64)
        }
        "inbox_submit" => {
            fields.len() == 4
                && value["path"].as_str().is_some_and(|path| {
                    let path = Path::new(path);
                    path.is_absolute()
                        && path.file_name().and_then(|name| name.to_str())
                            == value["filename"].as_str()
                        && value["filename"].is_string()
                })
                && value["day"].as_str().is_some_and(|day| {
                    day.len() == 10 && chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok()
                })
                && value["char_count"].is_u64()
        }
        "repo_project_analyze" => {
            let project = &value["project"];
            if fields.len() != 3 || !project.as_object().is_some_and(|map| map.len() == 3) || !tag(&project["id"], 160)
                || !project["id"].as_str().unwrap_or_default().starts_with("project_")
                || !project["revision"].is_u64()
            {
                return false;
            }
            let phase = project["phase"].as_str().unwrap_or_default();
            let status = match phase {
                "registered" | "analyzed" | "needs_adapter" => "succeeded",
                "analyzing" | "installing" | "wrapping" | "verifying" | "registering" => "running",
                "canceled" => "canceled",
                "failed" | "interrupted_unknown" => "failed",
                _ => return false,
            };
            value["status"] == status && value["registered"].as_bool() == Some(phase == "registered")
        }
        repository_search::COMMAND => {
            repository_search::candidates(value).is_some() || fields.len() == 2
                && value["error"].as_object().is_some_and(|error| error.len() == 2)
                && repository_search::search_error(value).is_some_and(|(_, message)| value["error"]["message"] == message)
        }
        _ => false,
    }
}
