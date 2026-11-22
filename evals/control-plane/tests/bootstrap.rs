use homebase_control_tests::control_bootstrap::{discover, render, TEMPLATE};
use std::path::PathBuf;
fn fixture() -> tempfile::TempDir {
    let root = tempfile::Builder::new().prefix("hb-bootstrap 中文 $& ").tempdir().unwrap();
    for file in [
        "AGENTS.md",
        "control/START.md",
        "control/OPERATIONS.md",
        "control/reference.md",
        "control/commands.json",
        "control/bootstrap.template.txt",
        "scripts/homebase.mjs",
        "scripts/control-bootstrap.mjs",
        "scripts/control-validation.mjs",
        "scripts/control-result.mjs",
        "hb.cmd",
    ] {
        let path = root.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            match file {
                "control/bootstrap.template.txt" => TEMPLATE.to_string(),
                "control/commands.json" => "{\"catalog_hash\":\"fixture\"}".into(),
                _ => "synthetic-only".into(),
            },
        )
        .unwrap();
    }
    root
}
#[test]
fn verified_files_generate_prompt_without_credentials() {
    let root = fixture();
    let data = discover(None, &[root.path().into()], "fixture");
    assert!(data.ready);
    let prompt = data.prompt.unwrap();
    assert!(!prompt.contains("{{"));
    assert!(prompt.contains("START.md"));
    assert!(prompt.contains("doctor"));
    assert!(!prompt.contains("endpoint.json"));
    assert!(!data.project_root.unwrap().starts_with(r"\\?\"));
}
#[test]
fn missing_files_do_not_create_a_fake_copyable_prompt() {
    let root = fixture();
    std::fs::remove_file(root.path().join("scripts/homebase.mjs")).unwrap();
    let data = discover(None, &[root.path().into()], "fixture");
    assert!(!data.ready);
    assert!(data.prompt.is_none());
    assert!(data.issue.is_some());
}
#[test]
fn stale_catalog_or_template_is_rejected() {
    let root = fixture();
    assert!(!discover(None, &[root.path().into()], "different").ready);
    std::fs::write(root.path().join("control/bootstrap.template.txt"), "stale").unwrap();
    assert!(!discover(None, &[root.path().into()], "fixture").ready);
}
#[test]
fn explicit_invalid_location_never_falls_back_to_other_project() {
    let root = fixture();
    assert!(
        !discover(
            Some(root.path().join("absent")),
            &[root.path().into()],
            "fixture"
        )
        .ready
    );
    assert!(
        !discover(
            Some(PathBuf::from("relative")),
            &[root.path().into()],
            "fixture"
        )
        .ready
    );
}
#[test]
fn rendering_preserves_unicode_and_spaces() {
    let prompt = render(&PathBuf::from("E:/Projects/中文 目录"));
    assert!(prompt.contains("E:/Projects/中文 目录"));
    assert!(prompt.contains("scripts"));
    assert!(!prompt.contains("{{"));
}

#[test]
fn native_and_cli_render_the_identical_shared_prompt() {
    let root = fixture();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/control-bootstrap.mjs")
        .canonicalize()
        .unwrap();
    let mut command = std::process::Command::new("node");
    command.env_clear();
    for name in ["PATH", "SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
    ] {
        command.env(name, root.path());
    }
    let result=command.args(["--input-type=module","-e","import {pathToFileURL} from 'node:url'; const {bootstrapPrompt}=await import(pathToFileURL(process.argv[1])); process.stdout.write(bootstrapPrompt(process.argv[2]));"])
        .arg(script).arg(root.path()).output().unwrap();
    assert!(result.status.success());
    let native = discover(None, &[root.path().into()], "fixture")
        .prompt
        .unwrap();
    assert_eq!(String::from_utf8(result.stdout).unwrap(), native);
}
