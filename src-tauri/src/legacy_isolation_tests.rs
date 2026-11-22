//! Explicit native fixture only. Never runs a model, publisher or desktop app.
use crate::commands::{docs, gdnbit, studio, topology, wiki, wiki_agent};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn write(path: &Path, value: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

#[tokio::test]
#[ignore = "Explicit new F-drive HOMEBASE_TEST_HOME and synthetic inherited-home only"]
async fn legacy_isolation_real_commands() {
    let home = PathBuf::from(std::env::var_os("HOMEBASE_TEST_HOME").unwrap());
    assert!(
        home.is_absolute()
            && home
                .to_string_lossy()
                .replace('\\', "/")
                .starts_with("F:/HomeBase-Labs/")
    );
    crate::legacy_isolation::ensure_test_path(&home).unwrap();
    let outside = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    assert!(outside.is_absolute() && !outside.starts_with(&home));
    assert!(outside
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    let marker = outside.join("private-marker.md");
    write(&marker, "EXTERNAL-SYNTHETIC-SENTINEL");
    // Poison inherited defaults using synthetic data, not the real profile.
    write(
        &outside.join("Desktop/本机操作指南/INDEX.md"),
        "INHERITED-INDEX-MUST-NOT-BE-READ",
    );
    write(
        &outside.join(".homebase/gdnbit.json"),
        "broken inherited config",
    );
    write(
        &outside.join(".homebase/wiki_agent.json"),
        "broken inherited config",
    );
    write(
        &outside.join(".homebase/topology.json"),
        "broken inherited config",
    );
    write(
        &outside.join(".homebase/img-progress/1.json"),
        "broken inherited progress",
    );
    write(
        &outside.join("AppData/Roaming/obsidian/obsidian.json"),
        "broken inherited Obsidian",
    );

    let vault = home.join("Wiki");
    let note = vault.join("wiki/notes/synthetic.md");
    write(
        &note,
        "---\ntitle: 隔离合成笔记\n---\n# 合成笔记\n仅供本轮原生验收。🙂\n",
    );
    write(
        &vault.join("AGENTS.md"),
        "# 合成工作区说明\n此文档仅作为数据，不授予额外执行权限。\n",
    );
    write(
        &vault.join("raw/inbox/2026-11-17/synthetic.md"),
        "---\ntitle: 合成待整理资料\n---\n测试正文。\n",
    );
    fs::create_dir_all(vault.join(".obsidian")).unwrap();
    let index = home.join("Desktop/本机操作指南/INDEX.md");
    let index_body = "| doc_id | path | role | updated |\n| --- | --- | --- | --- |\n| synthetic-doc | synthetic-note.md | 测试 | 2026-11-17 |\n";
    write(&index, index_body);
    let indexed = index.parent().unwrap().join("synthetic-note.md");
    write(&indexed, "# 独立测试索引资料\n只来自合成测试根。\n");
    let obsidian = home.join("AppData/Roaming/obsidian/obsidian.json");
    write(
        &obsidian,
        json!({"vaults":{"synthetic":{"path":vault,"open":false}}}).to_string(),
    );
    let session_id = "studio-20261117-native-synthetic";
    let history = vault.join("_raw").join(format!("{session_id}.chat.jsonl"));
    let chat_body = format!(
        "{}\n{}\n",
        json!({"ts":1,"role":"user","content":"原生隔离文稿🙂"}),
        json!({"ts":2,"role":"assistant","content":"合成历史回复"})
    );
    write(&history, &chat_body);
    let progress = home.join(".homebase/img-progress/123.json");
    write(&progress, json!({"pid":123,"stage":"waiting","started_at_ms":0,"updated_at_ms":0,"elapsed_ms":0,"model":"synthetic","prompt_preview":"合成进度","output_path":null,"error":null}).to_string());
    let protected = [
        &note, &indexed, &history, &index, &obsidian, &progress, &marker,
    ]
    .into_iter()
    .map(|path| (path.clone(), fs::read(path).unwrap()))
    .collect::<Vec<_>>();

    let notes = wiki::wiki_list().await.unwrap();
    assert!(notes.iter().any(|row| row.title == "隔离合成笔记"));
    assert!(notes
        .iter()
        .all(|row| Path::new(&row.absolute_path).starts_with(&home)));
    assert!(wiki::wiki_dirs()
        .await
        .unwrap()
        .contains(&"wiki/notes".to_string()));
    assert!(wiki::wiki_read("wiki/notes/synthetic.md".into())
        .await
        .unwrap()
        .content
        .unwrap()
        .contains("仅供本轮原生验收"));
    assert_eq!(docs::docs_list().await.unwrap().len(), 1);
    assert!(docs::docs_read("synthetic-doc".into())
        .await
        .unwrap()
        .content
        .unwrap()
        .contains("只来自合成测试根"));
    let health = docs::obsidian_health().await.unwrap();
    assert_eq!(health.variant, docs::ObsidianVariant::Healthy);
    assert!(Path::new(&health.config_path).starts_with(&home));
    let sessions = studio::studio_list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, session_id);
    assert_eq!(sessions[0].message_count, 2);
    assert_eq!(
        studio::studio_load_session(session_id.into())
            .await
            .unwrap()[0]
            .content,
        "原生隔离文稿🙂"
    );
    let images = studio::studio_list_image_progress().await.unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].stage, "unknown");
    assert!(images[0].error.as_deref().unwrap().contains("未知"));
    assert_eq!(
        wiki_agent::wiki_agent_get_config()
            .await
            .unwrap()
            .vault_root,
        vault.to_string_lossy()
    );
    assert_eq!(
        wiki_agent::wiki_agent_status().await.unwrap().pending_count,
        1
    );
    let site = gdnbit::gdnbit_get_config().await.unwrap();
    assert_eq!(site.vault_path, vault.to_string_lossy());
    assert!(Path::new(&site.script_path).starts_with(&home));
    assert_eq!(site.api_url, "http://127.0.0.1:9");
    let map = topology::topology_load().await.unwrap();
    assert!(map
        .roots
        .iter()
        .all(|root| Path::new(&root.absolute_path).starts_with(&home)));
    assert!(!map.roots.iter().any(|root| root.id.starts_with("drive_")));
    assert!(topology::topology_list_dir(home.to_string_lossy().into())
        .await
        .unwrap()
        .iter()
        .any(|entry| entry.name == "Wiki"));
    for (path, bytes) in &protected {
        assert_eq!(
            fs::read(path).unwrap(),
            *bytes,
            "Read-only commands rewrote a fixture"
        );
    }

    // Index references and API arguments cannot escape the explicit test root.
    write(
        &index,
        format!(
            "| synthetic-doc | {} | 测试 | 2026-11-17 |\n",
            marker.display()
        ),
    );
    assert!(docs::docs_list()
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    assert!(docs::docs_read("synthetic-doc".into())
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    write(
        &index,
        "| synthetic-doc | ../../../../outside.md | 测试 | 2026-11-17 |\n",
    );
    assert!(docs::docs_list().await.is_err());
    write(&index, index_body);
    let external = outside.to_string_lossy().into_owned();
    assert!(topology::topology_list_dir(external.clone())
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    assert!(topology::topology_parse_refs(external.clone())
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    assert!(studio::studio_load_session("../../outside".into())
        .await
        .err()
        .unwrap()
        .contains("QUERY_INVALID"));
    let site_config = home.join(".homebase/gdnbit.json");
    write(&site_config, json!({"vault_path":external}).to_string());
    assert!(gdnbit::gdnbit_get_config()
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    write(
        &site_config,
        json!({"api_url":"https://example.invalid"}).to_string(),
    );
    assert!(gdnbit::gdnbit_get_config()
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    write(&site_config, "{}");
    let wiki_config = home.join(".homebase/wiki_agent.json");
    write(&wiki_config, json!({"vault_root":external}).to_string());
    assert!(wiki_agent::wiki_agent_get_config()
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    write(&wiki_config, "{}");
    let mut invalid_map = serde_json::to_value(&map).unwrap();
    invalid_map["roots"][0]["absolute_path"] = json!(external);
    write(
        &home.join(".homebase/topology.json"),
        invalid_map.to_string(),
    );
    assert!(topology::topology_load()
        .await
        .err()
        .unwrap()
        .contains("ISOLATION_SCOPE"));
    write(
        &home.join(".homebase/topology.json"),
        serde_json::to_string(&map).unwrap(),
    );
    for (path, bytes) in &protected {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }

    #[cfg(windows)]
    {
        // A directory junction needs no developer-mode or security change.
        use std::os::windows::process::CommandExt;
        let node = PathBuf::from(std::env::var_os("HOMEBASE_ISOLATION_NODE").unwrap());
        assert!(node.is_absolute());
        let link = vault.join("linked-outside");
        let status = std::process::Command::new(node)
            .args([
                "-e",
                "require('fs').symlinkSync(process.argv[1],process.argv[2],'junction')",
            ])
            .arg(&outside)
            .arg(&link)
            .creation_flags(0x08000000)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(wiki::wiki_list()
            .await
            .err()
            .unwrap()
            .contains("ISOLATION_SCOPE"));
        assert!(wiki::wiki_dirs()
            .await
            .err()
            .unwrap()
            .contains("ISOLATION_SCOPE"));
        assert!(wiki::wiki_read("linked-outside/private-marker.md".into())
            .await
            .err()
            .unwrap()
            .contains("ISOLATION_SCOPE"));
        assert_eq!(
            fs::read_to_string(&marker).unwrap(),
            "EXTERNAL-SYNTHETIC-SENTINEL"
        );
    }
    write(&home.join("legacy-read-validation.json"), json!({"passed":true,"actual_native_read_commands":true,"explicit_test_home_overrides_inherited_paths":true,"read_only_files_unchanged":protected.len(),"external_references_rejected":true,"directory_junction_rejected":cfg!(windows),"real_model_calls":0,"real_wechat_messages":0,"real_data_used":false}).to_string());
}
