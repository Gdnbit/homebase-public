//! Public bootstrap metadata only; never reads endpoint tokens or secret files.
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
pub const TEMPLATE: &str = include_str!("../../control/bootstrap.template.txt");
const REQUIRED: &[&str] = &[
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
];

#[derive(Serialize)]
pub struct BootstrapFiles {
    pub ready: bool,
    pub project_root: Option<String>,
    pub startup_file: Option<String>,
    pub cli_file: Option<String>,
    pub prompt: Option<String>,
    pub issue: Option<String>,
}
pub fn render(root: &Path) -> String {
    TEMPLATE
        .replace("{{PROJECT_ROOT}}", &root.to_string_lossy())
        .replace(
            "{{START_FILE}}",
            &root.join("control").join("START.md").to_string_lossy(),
        )
        .replace(
            "{{CLI_FILE}}",
            &root.join("scripts").join("homebase.mjs").to_string_lossy(),
        )
}
fn valid_root(root: &Path, hash: &str) -> bool {
    if !root.is_absolute()
        || REQUIRED
            .iter()
            .any(|relative| !root.join(relative).is_file())
    {
        return false;
    }
    let Ok(template) = std::fs::read_to_string(root.join("control/bootstrap.template.txt")) else {
        return false;
    };
    if template != TEMPLATE {
        return false;
    }
    std::fs::read(root.join("control/commands.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .is_some_and(|catalog| catalog["catalog_hash"].as_str() == Some(hash))
}
pub fn discover(explicit: Option<PathBuf>, candidates: &[PathBuf], hash: &str) -> BootstrapFiles {
    let explicit_set = explicit.is_some();
    let candidates = explicit
        .map(|path| vec![path])
        .unwrap_or_else(|| candidates.to_vec());
    for root in candidates {
        if !valid_root(&root, hash) {
            continue;
        }
        let Ok(canonical) = root.canonicalize() else {
            continue;
        };
        let text = canonical.to_string_lossy();
        #[cfg(windows)]
        let text = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
        };
        let root = PathBuf::from(text.as_ref() as &str);
        return BootstrapFiles {
            ready: true,
            project_root: Some(root.to_string_lossy().into_owned()),
            startup_file: Some(root.join("control/START.md").to_string_lossy().into_owned()),
            cli_file: Some(
                root.join("scripts/homebase.mjs")
                    .to_string_lossy()
                    .into_owned(),
            ),
            prompt: Some(render(&root)),
            issue: None,
        };
    }
    BootstrapFiles { ready:false,project_root:None,startup_file:None,cli_file:None,prompt:None,issue:Some(if explicit_set {"指定的 AI 操作目录缺少必要文件，或与运行中的应用版本不一致。请核对 HOMEBASE_CONTROL_PROJECT_ROOT。"} else {"未找到与当前应用匹配的 AI 操作文件。请从当前项目构建运行，或配置 HOMEBASE_CONTROL_PROJECT_ROOT。"}.into()) }
}
