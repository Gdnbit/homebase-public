//! Legacy service catalogs already use Chinese functional group identifiers.
use homebase_control_tests::capability_discovery::{normalize, DiscoveryRequest, DiscoveryStore};
use serde_json::json;
use std::time::Instant;
#[test]
fn real_service_function_labels_remain_queryable_without_rekeying_configuration() {
    let index = normalize(
        vec![
            json!({"id":"fixture-service","function":"内容分发","kind":"service","name":"合成内容服务","provider":"fixture-ref","state":"metadata_only","cost":"unknown","discover":"model_capabilities_catalog","target":"fixture-service"}),
        ],
        vec![],
    );
    let now = Instant::now();
    let mut store = DiscoveryStore::new();
    let id = store.insert(index, now).unwrap();
    let request: DiscoveryRequest = serde_json::from_value(
        json!({"catalog_id":id,"function":"内容分发","offset":0,"limit":10}),
    )
    .unwrap();
    assert!(request.filters().is_ok(),"an existing service's function identifier must not be rejected merely because it is Chinese");
    let page = store.page(&request, now).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].target, "fixture-service");
    assert_eq!(page.entries[0].function, "内容分发");
    let mut invalid = request;
    invalid.function = Some("bad\nfunction".into());
    assert!(invalid.filters().is_err());
}
