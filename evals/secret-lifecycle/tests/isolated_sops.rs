use homebase_secret_lifecycle_tests::{
    adapters::{
        secret_health,
        secret_vault::{SaveRequest, Vault},
        sops::SopsAdapter,
    },
    secret_policy,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Fixture {
    _dir: tempfile::TempDir,
    vault: Vault<SopsAdapter>,
    sops: PathBuf,
    identity: PathBuf,
    config: PathBuf,
}
fn isolated_command(exe: &Path, root: &Path) -> Command {
    let mut command = Command::new(exe);
    command.env_clear();
    for name in ["SystemRoot", "WINDIR", "PATH"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.current_dir(root).stdin(Stdio::null());
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "TEMP",
        "TMP",
    ] {
        command.env(name, root);
    }
    for key in [
        "SOPS_AGE_KEY",
        "SOPS_AGE_KEY_FILE",
        "SOPS_AGE_KEY_CMD",
        "SOPS_AGE_SSH_PRIVATE_KEY_FILE",
        "SOPS_AGE_SSH_PRIVATE_KEY_CMD",
        "HOMEBASE_SOPS_AGE_KEY_FILE",
        "HOMEBASE_SECRETS_FILE",
        "HOMEBASE_SECRETS_YAML",
        "HOMEBASE_SECRETS_META",
    ] {
        command.env_remove(key);
    }
    command
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("HTTP_PROXY", "http://127.0.0.1:1");
    command
}
fn fixture() -> Fixture {
    // No default executable/vault/identity fallback is permitted in this suite.
    let tools = PathBuf::from(
        std::env::var_os("HOMEBASE_TEST_SOPS_BIN_DIR")
            .expect("Set HOMEBASE_TEST_SOPS_BIN_DIR to the bundled tools directory"),
    );
    let sops = tools.join("sops.exe");
    let age = tools.join("age-keygen.exe");
    assert!(sops.is_file() && age.is_file());
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("isolated-sops.yaml");
    let identity = dir.path().join("test-age-identity.txt");
    assert!(isolated_command(&age, dir.path())
        .arg("-o")
        .arg(&identity)
        .output()
        .unwrap()
        .status
        .success());
    let recipient = isolated_command(&age, dir.path())
        .arg("-y")
        .arg(&identity)
        .output()
        .unwrap();
    assert!(recipient.status.success());
    let recipient = String::from_utf8(recipient.stdout).unwrap();
    fs::write(
        &config,
        format!(
            "creation_rules:\n  - path_regex: .*\n    age: {}\n",
            recipient.trim()
        ),
    )
    .unwrap();
    let plain = dir.path().join("synthetic-fixture.json");
    fs::write(&plain, br#"{"sample_key":"synthetic-old-credential-only","empty_key":"  ","other_key":"synthetic-unchanged-credential"}"#).unwrap();
    let encrypted = isolated_command(&sops, dir.path())
        .arg("--config")
        .arg(&config)
        .args([
            "--encrypt",
            "--age",
            recipient.trim(),
            "--input-type",
            "json",
            "--output-type",
            "yaml",
        ])
        .arg(&plain)
        .output()
        .unwrap();
    assert!(
        encrypted.status.success(),
        "synthetic fixture encryption failed: {}",
        String::from_utf8_lossy(&encrypted.stderr)
    );
    fs::remove_file(plain).unwrap();
    let yaml = dir.path().join("secrets.yaml");
    let meta = dir.path().join("secrets-meta.json");
    fs::write(&yaml, encrypted.stdout).unwrap();
    fs::write(&meta, b"{}").unwrap();
    let adapter = SopsAdapter::with_identity(
        sops.to_string_lossy().into_owned(),
        identity.clone(),
        config.clone(),
    );
    Fixture {
        _dir: dir,
        vault: Vault::new(yaml, meta, adapter),
        sops,
        identity,
        config,
    }
}
async fn request(f: &Fixture, patch: serde_json::Value) -> SaveRequest {
    let snapshot = f.vault.snapshot().await.unwrap();
    let mut base = serde_json::json!({"short_name":"sample_key","expected_revision":snapshot.metadata.get("sample_key").map(|e|e.revision).unwrap_or(0),"expected_vault_version":snapshot.version});
    base.as_object_mut()
        .unwrap()
        .extend(patch.as_object().unwrap().clone());
    serde_json::from_value(base).unwrap()
}

#[tokio::test]
async fn real_sops_save_rotate_rollback_and_revoke_use_only_generated_identity() {
    let f = fixture();
    let key = "synthetic-new-credential-\"quotes\"-中文-\\slashes\nline\n";
    let r = request(&f, serde_json::json!({"short_name":"new_key","value":key,"display_name":"隔离测试","expires_at":"2027-01-01T15:59:59.999Z"})).await;
    let result = f.vault.save(r).await.unwrap();
    assert!(result.verified);
    let decrypted = f
        .vault
        .cipher
        .decrypt_key(&f.vault.yaml, "new_key")
        .await
        .unwrap();
    assert_eq!(decrypted.as_str(), key);
    let encrypted = fs::read_to_string(&f.vault.yaml).unwrap();
    assert!(!encrypted.contains(key));
    assert!(!encrypted.contains("synthetic-old-credential-only"));
    f.vault.save(request(&f,serde_json::json!({"operation":"rotate","candidate_verified":true,"value":"synthetic-next-credential","expires_at":"2027-01-02T15:59:59.999Z"})).await).await.unwrap();
    let id = f.vault.snapshot().await.unwrap().metadata["sample_key"].rotations[0]
        .id
        .clone();
    assert_eq!(
        f.vault
            .cipher
            .decrypt_key(&f.vault.yaml, "sample_key")
            .await
            .unwrap()
            .as_str(),
        "synthetic-next-credential"
    );
    f.vault.save(request(&f,serde_json::json!({"operation":"rollback_rotation","candidate_verified":true,"rotation_id":id})).await).await.unwrap();
    assert_eq!(
        f.vault
            .cipher
            .decrypt_key(&f.vault.yaml, "sample_key")
            .await
            .unwrap()
            .as_str(),
        "synthetic-old-credential-only"
    );
    assert_eq!(
        f.vault
            .cipher
            .decrypt_key(&f.vault.yaml, "other_key")
            .await
            .unwrap()
            .as_str(),
        "synthetic-unchanged-credential"
    );
    f.vault.save(request(&f,serde_json::json!({"operation":"rotate","candidate_verified":true,"value":"synthetic-third-credential","expires_at":""})).await).await.unwrap();
    let id = f.vault.snapshot().await.unwrap().metadata["sample_key"].rotations[1]
        .id
        .clone();
    f.vault
        .save(
            request(
                &f,
                serde_json::json!({"operation":"confirm_revocation","rotation_id":id}),
            )
            .await,
        )
        .await
        .unwrap();
    assert!(f.vault.save(request(&f,serde_json::json!({"operation":"rollback_rotation","candidate_verified":true,"rotation_id":id})).await).await.is_err());
    for file in fs::read_dir(f.vault.yaml.parent().unwrap()).unwrap() {
        let path = file.unwrap().path();
        if path.is_file() && path != f.identity {
            let bytes = fs::read(path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains(key));
        }
    }
}

#[tokio::test]
async fn expired_block_policy_is_enforced_by_adapter_and_real_reveal_helper() {
    let f = fixture();
    f.vault
        .save(
            request(
                &f,
                serde_json::json!({"expires_at":"2000-01-01T00:00:00Z","expiry_policy":"block"}),
            )
            .await,
        )
        .await
        .unwrap();
    assert!(f
        .vault
        .cipher
        .decrypt_key(&f.vault.yaml, "sample_key")
        .await
        .is_err());
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_homebase-reveal-isolated"));
    let mut command = isolated_command(&helper, f._dir.path());
    command
        .arg("sample_key")
        .env("HOMEBASE_SECRETS_FILE", &f.vault.yaml)
        .env("HOMEBASE_SECRETS_META", &f.vault.meta)
        .env("HOMEBASE_SOPS_EXE", &f.sops)
        .env("HOMEBASE_SOPS_CONFIG", &f.config)
        .env("SOPS_AGE_KEY_FILE", &f.identity);
    let result = command.output().unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("已到期"));
    f.vault
        .save(request(&f, serde_json::json!({"expiry_policy":"warn"})).await)
        .await
        .unwrap();
    assert!(f
        .vault
        .cipher
        .decrypt_key(&f.vault.yaml, "sample_key")
        .await
        .is_ok());
    let result = command.output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "synthetic-old-credential-only"
    );
}

#[tokio::test]
async fn pending_journal_blocks_consumers_and_mutated_metadata_fails_closed() {
    let f = fixture();
    fs::write(
        secret_policy::journal_path(&f.vault.yaml),
        b"interrupted journal",
    )
    .unwrap();
    assert!(f
        .vault
        .cipher
        .decrypt_key(&f.vault.yaml, "sample_key")
        .await
        .is_err());
    fs::remove_file(secret_policy::journal_path(&f.vault.yaml)).unwrap();
    fs::write(&f.vault.meta, b"{broken").unwrap();
    assert!(f
        .vault
        .cipher
        .decrypt_key(&f.vault.yaml, "sample_key")
        .await
        .is_err());
}

#[tokio::test]
async fn excluded_names_and_non_string_values_never_escape_as_credentials() {
    let f = fixture();
    let before_yaml = fs::read(&f.vault.yaml).unwrap();
    let before_meta = fs::read(&f.vault.meta).unwrap();
    let r = request(&f, serde_json::json!({"short_name":"sample_unencrypted","value":"synthetic-must-never-be-plaintext"})).await;
    assert!(f.vault.save(r).await.is_err());
    assert_eq!(fs::read(&f.vault.yaml).unwrap(), before_yaml);
    assert_eq!(fs::read(&f.vault.meta).unwrap(), before_meta);
    assert!(f
        .vault
        .cipher
        .decrypt_key(&f.vault.yaml, "empty_key")
        .await
        .is_err());
    let changed = String::from_utf8(before_yaml)
        .unwrap()
        .replace("type:str]", "type:int]");
    fs::write(&f.vault.yaml, changed).unwrap();
    assert!(f
        .vault
        .cipher
        .decrypt_key(&f.vault.yaml, "sample_key")
        .await
        .is_err());
}

#[tokio::test]
async fn conventional_identity_is_retained_for_backup_reads() {
    const CHILD_VAULT: &str = "HOMEBASE_ISOLATED_CONVENTIONAL_VAULT";
    if let Some(path) = std::env::var_os(CHILD_VAULT) {
        let yaml = PathBuf::from(path);
        let backup = yaml.parent().unwrap().join("backup").join("synthetic-backup.yaml");
        let value = SopsAdapter::for_vault(&yaml).decrypt_key(&backup, "sample_key").await.unwrap();
        assert_eq!(value.as_str(), "synthetic-old-credential-only");
        return;
    }
    let f = fixture();
    let root = f._dir.path();
    fs::create_dir(root.join(".age-keys")).unwrap();
    fs::copy(&f.identity, root.join(".age-keys").join("primary.txt")).unwrap();
    fs::create_dir(root.join("backup")).unwrap();
    fs::copy(&f.vault.yaml, root.join("backup").join("synthetic-backup.yaml")).unwrap();
    let child = isolated_command(&std::env::current_exe().unwrap(), root)
        .args(["--exact", "conventional_identity_is_retained_for_backup_reads", "--test-threads=1"])
        .env(CHILD_VAULT, &f.vault.yaml)
        .env("HOMEBASE_SOPS_EXE", &f.sops)
        .env("HOMEBASE_SOPS_CONFIG", &f.config)
        .output().unwrap();
    assert!(child.status.success(), "isolated conventional identity regression failed: {} {}", String::from_utf8_lossy(&child.stdout), String::from_utf8_lossy(&child.stderr));
}

async fn probe_fixture(code: u16) -> (secret_health::ProbeResult, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = server.accept().await.unwrap();
        let mut bytes = vec![0; 8192];
        let n = socket.read(&mut bytes).await.unwrap();
        socket.write_all(format!("HTTP/1.1 {code} Fixture\r\nContent-Length: 0\r\nLocation: http://127.0.0.1:1/never-follow\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        String::from_utf8_lossy(&bytes[..n]).into_owned()
    });
    let result = secret_health::probe(
        &format!("http://{address}/probe"),
        false,
        "Authorization",
        "Bearer synthetic-test-only",
    )
    .await
    .unwrap();
    (result, task.await.unwrap())
}
#[tokio::test]
async fn mock_service_checks_status_and_never_follows_credential_redirects() {
    for (code, expected) in [
        (200, "valid"),
        (401, "unauthorized"),
        (403, "forbidden"),
        (422, "invalid_request"),
        (302, "redirect_refused"),
    ] {
        let (result, request) = probe_fixture(code).await;
        assert_eq!(result.status, expected);
        assert_eq!(result.success, code == 200);
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-test-only"));
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("synthetic-test-only"));
    }
}
