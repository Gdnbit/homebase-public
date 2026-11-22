use crate::wechat::video::{is_silk, VOICE_LIMIT, VOICE_SECONDS};
use std::{io, path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

const MAX_WAVE: usize = 44 + VOICE_SECONDS * 32_000;

pub(super) async fn decode(
    node: &Path,
    runtime: &Path,
    profile: &Path,
    input: &[u8],
) -> Result<Zeroizing<Vec<u8>>, String> {
    if input.len() > VOICE_LIMIT || !is_silk(input) {
        return Err("微信语音不是可处理的 SILK 文件。".into());
    }
    let script = runtime.join("local/wechat-voice.mjs");
    if !script.is_file() {
        return Err("本机 SILK 解码器尚未安装。".into());
    }
    let mut permission = std::ffi::OsString::from("--allow-fs-read=");
    permission.push(runtime);
    let mut command = tokio::process::Command::new(node);
    command
        .args(["--permission", "--max-old-space-size=64"])
        .arg(permission)
        .arg(script)
        .env_clear()
        .current_dir(profile)
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
    let mut child = command.spawn().map_err(|_| "本机 SILK 解码器无法启动。")?;
    let mut writer = child.stdin.take().ok_or("语音输入通道不可用。")?;
    let mut reader = child
        .stdout
        .take()
        .ok_or("语音输出通道不可用。")?
        .take(MAX_WAVE as u64 + 1);
    let mut wave = Zeroizing::new(Vec::new());
    let exit = tokio::time::timeout(Duration::from_secs(15), async {
        let send = async move {
            writer.write_all(input).await?;
            writer.shutdown().await
        };
        tokio::try_join!(send, reader.read_to_end(&mut wave))?;
        if !valid_wave(&wave) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid WAV"));
        }
        child.wait().await
    })
    .await
    .map_err(|_| "本机 SILK 解码超时。")?
    .map_err(|_| "未能解码这条语音，或语音超过 60 秒。")?;
    if !exit.success() {
        return Err("未能解码这条语音，或语音超过 60 秒。".into());
    }
    Ok(wave)
}

fn valid_wave(wave: &[u8]) -> bool {
    if !(45..=MAX_WAVE).contains(&wave.len()) || wave.len() % 2 != 0 {
        return false;
    }
    let read = |start| u32::from_le_bytes(wave[start..start + 4].try_into().unwrap());
    &wave[..4] == b"RIFF"
        && &wave[8..16] == b"WAVEfmt "
        && &wave[36..40] == b"data"
        && read(4) as usize == wave.len() - 8
        && read(16) == 16
        && read(20) == 0x0001_0001
        && read(24) == 16_000
        && read(28) == 32_000
        && read(32) == 0x0010_0002
        && read(40) as usize == wave.len() - 44
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "explicit fixed Node, decoder package and synthetic SILK fixture required"]
async fn wechat_voice_decoder_writes_bounded_wave_with_real_silk() {
    use std::path::PathBuf;
    let node = PathBuf::from(std::env::var_os("HOMEBASE_VOICE_NODE").unwrap());
    let runtime = PathBuf::from(std::env::var_os("HOMEBASE_VOICE_RUNTIME").unwrap());
    let source = PathBuf::from(std::env::var_os("HOMEBASE_VOICE_FIXTURE").unwrap());
    assert!(node.is_absolute() && runtime.is_absolute() && source.is_absolute());
    let profile = tempfile::tempdir().unwrap();
    let data = std::fs::read(source).unwrap();
    let decoded = decode(&node, &runtime, profile.path(), &data)
        .await
        .unwrap();
    assert!(valid_wave(&decoded));
    assert_eq!(decoded.len(), 44 + 224_640);
    assert!(decode(&node, &runtime, profile.path(), b"not media")
        .await
        .is_err());
    assert!(
        decode(&node, &runtime, profile.path(), b"\x02#!SILK_V3broken")
            .await
            .is_err()
    );
}
