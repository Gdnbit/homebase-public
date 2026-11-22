//! Tauri commands（v3 §8.1）— Rust → Frontend 调用入口
//!
//! 9 顶级模块（v3 §20.3）：Dashboard / Agents / Secrets / Knowledge / Projects /
//! System / Logs / Tasks / Skills；Chat Hub 是 Agents 子组件不在此列。
//! Extensions 模块管理内置/第三方 Extension（v3 §23 改名）。

pub mod agent_versions;
pub mod ai_control;
pub mod assistant;
pub mod capabilities;
pub mod development;
pub mod development_workspace;
pub mod conversations;
pub mod native_sessions;
pub mod capability_index;
pub mod model_capabilities;
pub mod task_events;
pub mod agents;
pub mod agents_history;
pub mod asr;
pub mod docs;
pub mod distribution;
pub mod extensions;
pub mod gdnbit;
pub mod governance;
pub mod inbox;
pub mod inbox_workspace;
pub mod model_center;
pub mod openrouter;
pub mod projects;
pub mod secrets;
pub mod service_credentials;
pub mod shared_mcp;
pub mod shared_secrets;
pub mod shared_skills;
pub mod skills;
pub mod studio;
pub mod system;
pub mod tasks;
pub mod topology;
pub mod wiki;
pub mod wiki_agent;
pub mod wechat;
pub(crate) mod wechat_video;
pub mod assistant_chat;
pub mod assistant_desktop;
pub mod assistant_documents;
