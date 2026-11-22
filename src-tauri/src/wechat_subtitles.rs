use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::path::Path;

use super::clip_video::ClipRange;

pub(crate) const LIMIT: usize = 256 * 1024;
pub(crate) const MIME: &str = "application/x-subrip";
pub(crate) const NAME: &str = "subtitles.srt";
pub(crate) const WHISPER_ENGINE: &str = "faster_whisper_large_v3_cpu";
const INVALID: &str = "原转写字幕格式无效，请核对原视频的转写结果。";

pub(super) fn source_suffix(kind: &str) -> Option<&'static Path> {
    Some(Path::new(match kind {
        "export" => "exports/transcript.srt",
        "whisper_run" => "runs/faster_whisper_large_v3_cpu/subtitles.srt",
        _ => return None,
    }))
}

pub(crate) fn observe_timed_source(task: &Path, output: &Path) -> Option<(&'static str, String)> {
    ["export", "whisper_run"].into_iter().find_map(|kind| {
        let path = task.join(source_suffix(kind)?);
        let bytes = super::artifact_bytes_limited(&path, output, LIMIT).ok()?;
        is_timed_srt(&bytes).then(|| (kind, format!("{:x}", Sha256::digest(&bytes))))
    })
}

pub(super) struct Cue<'a> {
    start: u64,
    end: u64,
    text: &'a str,
}

fn timestamp(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 12 || bytes[2] != b':' || bytes[5] != b':' || bytes[8] != b',' {
        return None;
    }
    let part = |start, end| {
        let s: &str = value.get(start..end)?;
        s.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| s.parse::<u64>().ok())
            .flatten()
    };
    let (hours, minutes, seconds, ms) = (part(0, 2)?, part(3, 5)?, part(6, 8)?, part(9, 12)?);
    (minutes < 60 && seconds < 60).then_some(((hours * 60 + minutes) * 60 + seconds) * 1000 + ms)
}

fn number(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn parse(normalized: &str) -> Result<Vec<Cue<'_>>, &'static str> {
    if normalized.len() > LIMIT
        || normalized
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(INVALID);
    }
    let mut cues = Vec::new();
    for block in normalized.trim().split("\n\n").filter(|s| !s.is_empty()) {
        let (index, body) = block.split_once('\n').ok_or(INVALID)?;
        let (range, text) = body.split_once('\n').ok_or(INVALID)?;
        let (start, end) = range.split_once(" --> ").ok_or(INVALID)?;
        let (start, end) = (
            timestamp(start).ok_or(INVALID)?,
            timestamp(end).ok_or(INVALID)?,
        );
        if index.parse::<usize>().ok() != Some(cues.len() + 1)
            || end <= start
            || text.trim().is_empty()
            || cues.len() >= 4096
        {
            return Err(INVALID);
        }
        cues.push(Cue { start, end, text });
    }
    cues.sort_by_key(|cue| cue.start);
    Ok(cues)
}

fn normalize(bytes: &[u8]) -> Result<String, &'static str> {
    if bytes.len() > LIMIT {
        return Err(INVALID);
    }
    Ok(std::str::from_utf8(bytes)
        .map_err(|_| INVALID)?
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n"))
}

pub(crate) fn is_timed_srt(bytes: &[u8]) -> bool {
    normalize(bytes)
        .and_then(|text| parse(&text).map(|cues| !cues.is_empty()))
        .unwrap_or(false)
}

pub(super) fn rebase(
    bytes: &[u8],
    ranges: &[ClipRange],
    frame_counts: &[u64],
    fps: u64,
) -> Result<String, String> {
    let normalized = normalize(bytes)?;
    let cues = parse(&normalized)?;
    if ranges.len() != frame_counts.len() || fps == 0 {
        return Err("成片时间轴无法核对，未登记字幕。".into());
    }
    let mut output = String::new();
    let mut ordinal = 0;
    let mut previous_frames = 0;
    for (range, &frames) in ranges.iter().zip(frame_counts) {
        let offset = previous_frames * 1000 / fps;
        previous_frames += frames;
        let duration = (previous_frames * 1000 / fps - offset).min(range.duration_ms);
        let clip_end = range.start_ms + duration;
        for cue in &cues {
            let left = cue.start.max(range.start_ms);
            let right = cue.end.min(clip_end);
            if left >= right {
                continue;
            }
            ordinal += 1;
            let start = number(offset + left - range.start_ms);
            let end = number(offset + right - range.start_ms);
            write!(output, "{ordinal}\n{start} --> {end}\n{}\n\n", cue.text).unwrap();
            if output.len() > LIMIT || ordinal > 4096 {
                return Err("剪辑字幕超过本机输出上限。".into());
            }
        }
    }
    if output.is_empty() {
        return Err("所选片段没有转写字幕。请核对时间范围。".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordered_ranges_clip_cues_and_use_actual_frame_offsets() {
        let original = "\u{feff}1\r\n00:00:00,200 --> 00:00:00,800\r\n前段\r\n\r\n2\r\n00:00:01,800 --> 00:00:02,650\r\n跨越切点\r\n";
        let ranges = [
            ClipRange {
                start_ms: 2000,
                duration_ms: 640,
            },
            ClipRange {
                start_ms: 100,
                duration_ms: 350,
            },
        ];
        let output = rebase(original.as_bytes(), &ranges, &[19, 11], 30).unwrap();
        assert_eq!(output, "1\n00:00:00,000 --> 00:00:00,633\n跨越切点\n\n2\n00:00:00,733 --> 00:00:00,983\n前段\n\n");
        assert!(is_timed_srt(output.as_bytes()));
        assert!(!is_timed_srt(b"1\n00:00:00,150 --> 00:00:00,100\ninvalid"));
        assert!(rebase(
            original.as_bytes(),
            &[ClipRange {
                start_ms: 8000,
                duration_ms: 500
            }],
            &[15],
            30
        )
        .is_err());
    }
}
