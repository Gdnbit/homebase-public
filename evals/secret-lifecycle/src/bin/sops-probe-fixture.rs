//! Synthetic executable; no SOPS, credentials or user configuration access.
use std::{path::PathBuf, time::Duration};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args, ["--disable-version-check", "--version"]);
    assert_eq!(std::env::var("SOPS_DISABLE_VERSION_CHECK").unwrap(), "true");
    for name in [
        "SOPS_AGE_KEY",
        "SOPS_AGE_KEY_FILE",
        "HOMEBASE_SOPS_AGE_KEY_FILE",
        "AWS_SECRET_ACCESS_KEY",
        "GOOGLE_APPLICATION_CREDENTIALS",
    ] {
        assert!(std::env::var_os(name).is_none());
    }
    let root = std::env::current_dir().unwrap();
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("homebase-sops-probe-"));
    assert_eq!(PathBuf::from(std::env::var_os("HOME").unwrap()), root);
    let exe = std::env::current_exe().unwrap();
    let case = exe.file_stem().unwrap().to_string_lossy();
    match case.as_ref() {
        "recover" => {
            let marker = root.join("attempt.marker");
            if !marker.exists() {
                std::fs::write(marker, b"synthetic-attempt").unwrap();
                std::thread::sleep(Duration::from_secs(5));
            }
        }
        "hang" => std::thread::sleep(Duration::from_secs(60)),
        "invalid" => {
            println!("synthetic unrelated program");
            return;
        }
        "failed" => std::process::exit(7),
        "empty" => return,
        _ => (),
    }
    println!("sops 9.8.7-fixture");
}
