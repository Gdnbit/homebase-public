//! Public catalog metadata only. Configuration, account entitlement and source
//! synchronization remain separate facts; none grants execution permission.
use crate::model_capabilities::{
    CapabilityCredential, ModelCapability, ModelCatalogSource, ServiceCapability,
};
use serde_json::{json, Value};

const MODEL_CHECK: &str = "按route_id执行model_capability_resolve，核对本次凭据选择、账号可用性、费用及免费限制；默认配置与目录收录不代表调用成功。";
const SERVICE_CHECK: &str = "这里只记录服务配置元数据，不证明调用适配器已开放；执行前查询实际接口，不凭名称或configured状态调用。";

fn known(value: &str) -> &str {
    if value.trim().is_empty() {
        "unknown"
    } else {
        value
    }
}

pub fn model_entry(
    model: &ModelCapability,
    function: &str,
    credentials: &[CapabilityCredential],
    sources: &[ModelCatalogSource],
) -> Value {
    let mut matches = credentials
        .iter()
        .filter(|c| c.provider_id == model.provider_id);
    let first = matches.next();
    let credential = if matches.next().is_none() {
        first
    } else {
        None
    };
    let (binding, configuration) = match credential {
        Some(c) if !c.credential_ref.trim().is_empty() => (
            c.credential_ref.clone(),
            format!(
                "默认凭据配置：{}；来源：{}；远端状态：{}；到期警告：{}。{}",
                known(&c.state), known(&c.source), known(&c.remote_state),
                if c.expiry_warning { "有，核对原到期策略" } else { "无" }, c.message
            ),
        ),
        _ => (
            "沿用route_id对应的默认凭据；调用前resolve".into(),
            "默认凭据配置未知：原目录没有唯一且完整的provider匹配记录；不选择其他来源，不据此判断可调用。".into(),
        ),
    };
    let mut matches = sources
        .iter()
        .filter(|s| s.provider_id == model.provider_id);
    let first = matches.next();
    let source = if matches.next().is_none() {
        first
    } else {
        None
    };
    let source = match source {
        Some(s) => format!("模型目录来源状态：{}。{}", known(&s.state), s.message),
        None => "模型目录来源状态未知：没有唯一provider匹配记录；不把来源缺失或同步错误当作账号无权限或模型不存在。".into(),
    };
    json!({
        "id":model.route_id,"function":function,"kind":"model","name":model.name,
        "provider":model.provider_id,"state":model.account_state,
        "credential_binding":binding,"cost":"unknown","discover":"model_capability_resolve",
        "target":model.route_id,"pricing":model.pricing,
        "prerequisites":[MODEL_CHECK,configuration,source]
    })
}

pub fn service_entry(service: &ServiceCapability) -> Value {
    json!({
        "id":service.id,"function":service.function,"kind":"service","name":service.name,
        "provider":service.credential_ref,"state":service.state,"cost":"unknown",
        "credential_binding":service.credential_ref,
        "discover":"model_capabilities_catalog（服务条目需按实际接口核对，配置不代表可执行）",
        "target":service.id,"prerequisites":[SERVICE_CHECK,service.message]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> ModelCapability {
        crate::model_capabilities::seed_models()
            .into_iter()
            .find(|m| m.provider_id == "openrouter")
            .unwrap()
    }
    fn credential(state: &str) -> CapabilityCredential {
        CapabilityCredential {
            provider_id: "openrouter".into(),
            provider_name: "Synthetic".into(),
            credential_ref: "sops:synthetic_ref".into(),
            source: "sops".into(),
            state: state.into(),
            message: "远端认证、余额及模型权限未验证".into(),
            remote_state: "not_checked".into(),
            expires_at: None,
            expiry_warning: false,
        }
    }
    fn source() -> ModelCatalogSource {
        ModelCatalogSource {
            provider_id: "openrouter".into(),
            state: "not_synced".into(),
            message: "参考目录不是账号权限清单".into(),
            fetched_at: None,
        }
    }
    #[test]
    fn preserves_default_configuration_without_upgrading_account_or_cost() {
        let m = model();
        let e = model_entry(&m, "text", &[credential("configured")], &[source()]);
        assert_eq!(e["state"], m.account_state);
        assert_eq!(e["cost"], "unknown");
        assert_eq!(e["credential_binding"], "sops:synthetic_ref");
        assert!(e["prerequisites"][1]
            .as_str()
            .unwrap()
            .contains("默认凭据配置：configured"));
        assert!(e["prerequisites"][1]
            .as_str()
            .unwrap()
            .contains("远端状态：not_checked"));
        assert!(e["prerequisites"][2]
            .as_str()
            .unwrap()
            .contains("not_synced"));
    }
    #[test]
    fn blocked_unknown_and_not_configured_are_independent_of_account_state() {
        let m = model();
        for state in ["blocked", "unknown", "not_configured"] {
            let e = model_entry(&m, "text", &[credential(state)], &[source()]);
            assert_eq!(e["state"], m.account_state);
            assert!(e["prerequisites"][1].as_str().unwrap().contains(state));
            assert_eq!(e["target"], m.route_id);
        }
    }
    #[test]
    fn expired_warning_does_not_invent_a_block_or_change_credential_source() {
        let mut c = credential("configured");
        c.expiry_warning = true;
        c.source = "api_key_manager".into();
        c.credential_ref = "api-key-manager:synthetic".into();
        let e = model_entry(&model(), "text", &[c], &[source()]);
        assert_eq!(e["credential_binding"], "api-key-manager:synthetic");
        let detail = e["prerequisites"][1].as_str().unwrap();
        assert!(
            detail.contains("配置：configured") && detail.contains("到期警告：有，核对原到期策略")
        );
        assert!(!detail.contains("blocked"));
    }
    #[test]
    fn absent_ambiguous_empty_or_different_provider_never_picks_a_credential() {
        let mut other = credential("configured");
        other.provider_id = "openai".into();
        let mut empty = credential("configured");
        empty.credential_ref.clear();
        for cs in [
            vec![],
            vec![credential("blocked"), credential("configured")],
            vec![other],
            vec![empty],
        ] {
            let e = model_entry(&model(), "text", &cs, &[source()]);
            assert!(e["prerequisites"][1].as_str().unwrap().contains("配置未知"));
            assert!(!e.to_string().contains("synthetic_ref"));
        }
    }
    #[test]
    fn failed_and_ambiguous_sources_do_not_replace_permission_or_hide_old_catalog() {
        let mut failed = source();
        failed.state = "error".into();
        failed.message = "同步失败，保留旧目录".into();
        let e = model_entry(&model(), "text", &[credential("configured")], &[failed]);
        assert!(e["prerequisites"][2]
            .as_str()
            .unwrap()
            .contains("error。同步失败，保留旧目录"));
        let e = model_entry(&model(), "text", &[], &[source(), source()]);
        assert!(e["prerequisites"][2]
            .as_str()
            .unwrap()
            .contains("来源状态未知"));
    }
    #[test]
    fn service_purpose_and_known_state_remain_metadata_only() {
        for state in [
            "configured",
            "expired",
            "blocked",
            "unknown",
            "not_configured",
            "linked",
        ] {
            let e = service_entry(&ServiceCapability {
                id: "fixture".into(),
                name: "已授权免费（仅名称）".into(),
                function: "tools".into(),
                credential_ref: "sops:fixture".into(),
                state: state.into(),
                message: "网页搜索服务，不是独立生成模型".into(),
            });
            assert_eq!(e["state"], state);
            assert_eq!(e["credential_binding"], "sops:fixture");
            assert_eq!(e["prerequisites"][1], "网页搜索服务，不是独立生成模型");
            assert!(e["prerequisites"][0]
                .as_str()
                .unwrap()
                .contains("不证明调用适配器已开放"));
            assert_eq!(e["cost"], "unknown");
        }
    }
    #[test]
    fn extra_private_properties_and_input_examples_never_cross_projection() {
        let mut raw = serde_json::to_value(model()).unwrap();
        raw["api_key"] = json!("SYNTHETIC-PRIVATE-EXTRA");
        raw["input_example"] = json!({"secret":"SYNTHETIC-PRIVATE-EXTRA"});
        let m = serde_json::from_value(raw).unwrap();
        assert!(!model_entry(&m, "text", &[], &[])
            .to_string()
            .contains("SYNTHETIC-PRIVATE-EXTRA"));
    }
    #[test]
    fn normalized_identity_stays_stable_and_oversized_prose_is_marked() {
        let m = model();
        let mut c = credential("configured");
        c.message = "中文说明🙂".repeat(300);
        let old = json!({"id":m.route_id,"target":m.route_id,"function":"text","kind":"model","name":m.name,
            "provider":m.provider_id,"state":m.account_state,"cost":"unknown","discover":"model_capability_resolve"});
        let old = crate::capability_discovery::normalize(vec![old], vec![]);
        let next = crate::capability_discovery::normalize(
            vec![model_entry(&m, "text", &[c], &[source()])],
            vec![],
        );
        assert_eq!(old.entries[0].entry_key, next.entries[0].entry_key);
        assert!(next.entries[0].summary_truncated);
        assert_eq!(next.entries[0].target, m.route_id);
    }

    #[test]
    #[ignore = "explicit isolated output for frontend/shared-graph synthetic fixture"]
    fn write_synthetic_model_selection_fixture() {
        use std::io::Write;
        let file = std::path::PathBuf::from(
            std::env::var_os("HOMEBASE_SELECTION_PROJECTED_OUTPUT").expect("explicit test output"),
        );
        assert!(
            file.is_absolute()
                && file
                    .to_string_lossy()
                    .replace('\\', "/")
                    .starts_with("F:/HomeBase-Labs/")
        );
        let mut failed = source();
        failed.state = "error".into();
        failed.message = "同步失败，保留原参考目录；未探测账号".into();
        let index = crate::capability_discovery::normalize(
            vec![model_entry(
                &model(),
                "text",
                &[credential("blocked")],
                &[failed],
            )],
            vec![],
        );
        assert_eq!(index.entries.len(), 1);
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(file)
            .unwrap();
        output
            .write_all(
                &serde_json::to_vec(&json!({"synthetic_only":true,"entry":index.entries[0]}))
                    .unwrap(),
            )
            .unwrap();
    }
}
