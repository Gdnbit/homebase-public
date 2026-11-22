//! Non-sensitive local capability only. No plaintext/credential export command exists.
use crate::adapters::service_credentials::{self, ServiceCredentialStatus};

/// 已有服务凭据引用的本机只读状态；DPAPI 在后端内存校验，不调用远端 API。
#[tauri::command]
pub async fn service_credentials_status() -> Result<Vec<ServiceCredentialStatus>, String> {
    service_credentials::status().await
}
