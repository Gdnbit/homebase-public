//! Derived functional index. Existing registries remain the only source of truth.
use crate::capability_discovery::{
    self as discovery, DiscoveryPage, DiscoveryRequest, DiscoveryStore, Index,
};
use serde_json::{json, Value};
static SNAPSHOTS: tokio::sync::Mutex<DiscoveryStore> =
    tokio::sync::Mutex::const_new(DiscoveryStore::new());

#[tauri::command]
pub async fn capability_index_query(request: DiscoveryRequest) -> Result<DiscoveryPage, String> {
    let mut request = request;
    request.filters()?;
    // Only an explicit new catalog reads sources. Reusing a catalog for another
    // filter or continuing its view never rereads config or probes a provider.
    let catalog = if request.catalog_id.is_none() && request.snapshot_id.is_none() {
        Some(collect_index().await?)
    } else {
        None
    };
    let mut snapshots = SNAPSHOTS.lock().await;
    if let Some(catalog) = catalog {
        request.catalog_id = Some(snapshots.insert(catalog, std::time::Instant::now())?);
    }
    snapshots.page(&request, std::time::Instant::now())
}

#[tauri::command]
pub async fn capability_index_get() -> Result<Value, String> {
    serde_json::to_value(collect_index().await?).map_err(|_| "能力索引无法编码".into())
}

async fn collect_index() -> Result<Index, String> {
    let mut entries = vec![];
    let mut issues = vec![];
    let (models, repositories, assistant) = tokio::join!(
        super::model_capabilities::model_capabilities_snapshot(),
        super::capabilities::repo_capability_index_entries(),
        super::assistant::service(),
    );
    match models {
        Ok(catalog) => {
            const MAX_MODELS: usize = 10_200;
            if catalog.models.len() > MAX_MODELS {
                issues.push(json!({"source":"models","message":"目录超过模型索引上限，当前索引不完整"}));
            }
            for model in catalog.models.iter().take(MAX_MODELS) {
                for function in &model.functions {
                    entries.push(crate::capability_catalog_projection::model_entry(
                        model,
                        function,
                        &catalog.credentials,
                        &catalog.sources,
                    ));
                }
            }
            for service in &catalog.services {
                entries.push(crate::capability_catalog_projection::service_entry(service));
            }
            for issue in &catalog.issues {
                issues.push(json!({"source":"models","message":issue}));
            }
        }
        Err(_) => {
            issues.push(json!({"source":"models","message":"模型目录读取失败，未按空目录处理"}))
        }
    }
    match repositories {
        Ok(repositories) => {
            for cap in repositories {
                let text = format!("{} {}", cap["name"], cap["description"]).to_lowercase();
                let function = if text.contains("markdown") {
                    "document"
                } else if text.contains("video") {
                    "video"
                } else {
                    "tools"
                };
                entries.push(json!({"id":cap["id"],"function":function,"kind":"repository","name":cap["name"],"provider":cap["repository"],"state":cap["state"],"cost":"local_compute","discover":"repo_capability_describe","target":cap["id"],"provenance":cap["provenance"],"search_description":cap["description"].as_str().unwrap_or_default()}));
            }
        }
        Err(_) => issues.push(json!({"source":"repositories","message":"开源能力目录暂不可用；原注册内容未修改"})),
    }
    let config = assistant?.local_config().await;
    let (mcp, skills) = tokio::join!(
        super::assistant::local::mcp_index(&config),
        super::assistant::local::installed_skill_index(),
    );
    discovery::append_mcp_servers(mcp, config.enabled, &mut entries, &mut issues);
    discovery::append_installed_skills(skills, &mut entries, &mut issues);
    entries.push(json!({"id":"hyperframes","function":"video","kind":"local","name":"代码动画","provider":"HyperFrames","state":"check_before_call","cost":"local_compute","discover":"capabilities_catalog","target":"hyperframes"}));
    entries.push(json!({"id":"homebase.video_clip","function":"video","kind":"local","name":"微信原片剪辑","provider":"随包 FFmpeg","state":"check_before_call","cost":"local_compute","discover":"capability_clip_video","target":"capability_clip_video","prerequisites":["在当前微信会话发送已校验的 MP4，再说剪刚发的视频，也可直接引用原片。助理按时间范围新建一段 MP4。"],"evidence":"单段本机剪辑按实际产物登记。"}));
    entries.push(json!({"id":"homebase.video_assembly","function":"video","kind":"local","name":"微信视频多段拼接","provider":"随包 FFmpeg","state":"check_before_call","cost":"local_compute","discover":"capability_assemble_video","target":"capability_assemble_video","prerequisites":["同一当前微信会话核验过的原 MP4。每段位于前 120 秒，输入按成片顺序提供 2 至 6 段，合计不超过 30 秒。"],"evidence":"本机核对完整成片后登记附件。"}));
    entries.push(json!({"id":"homebase.video_clip_trim","function":"video","kind":"local","name":"微信成片再剪辑","provider":"随包 FFmpeg","state":"check_before_call","cost":"local_compute","discover":"capability_trim_clip_artifact","target":"capability_trim_clip_artifact","prerequisites":["引用当前微信会话内已登记的剪辑成片，按成片时间指定一个范围。执行前核对归属、时长及文件摘要。"],"evidence":"生成独立 MP4，保留引用的成片。"}));
    entries.push(json!({"id":"homebase.video_cover","function":"image","kind":"local","name":"视频取帧做封面","provider":"随包 FFmpeg","state":"check_before_call","cost":"local_compute","discover":"capability_video_cover","target":"capability_video_cover","prerequisites":["当前微信会话已核验的原 MP4、封面时间点和标题。需要本机微软雅黑字体。"],"evidence":"一次生成三种 PNG 尺寸，保持原片。"}));
    entries.push(json!({"id":"homebase.video_publish_package","function":"video","kind":"local","name":"短视频发布包","provider":"随包 FFmpeg","state":"check_before_call","cost":"local_compute","discover":"capability_video_publish_package","target":"capability_video_publish_package","prerequisites":["当前微信会话已核验的原 MP4 和已登记的 SRT。需要栏目片段范围、封面时间点、标题简介和本机微软雅黑字体。"],"evidence":"一个 ZIP 交付竖版成片、对齐字幕、三张封面、发布草稿及来源摘要。"}));
    entries.push(discovery::document_entry());
    entries.push(json!({"id":"homebase.development","function":"development","kind":"local","name":"开发","provider":"Agent Autoloop","state":"check_before_call","cost":"unknown","discover":"development_catalog / development_run / development_result","target":"homebase.development","prerequisites":["先查询development_catalog，指定项目目录、需求、流程和验收argv；模型沿用已登记角色，费用未知不等于免费。","固定request_id提交；按原task_id查询与回答，只有state=verified才是宿主验收通过。无检查为needs_acceptance。"],"evidence":"复用本机已部署工作流；每次任务单独验收，不以中台在线代替成功。"}));
    Ok(discovery::normalize(entries, issues))
}
