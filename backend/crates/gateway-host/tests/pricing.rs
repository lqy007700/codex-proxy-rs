//! 验证外部价格源的 Provider 选择、免费价格与无效文档拒绝

use gateway_admin::ports::pricing::{PricingCatalogSource, PricingSourceScope};
use gateway_host::pricing::decode_catalog;
use serde_json::json;
use std::collections::BTreeMap;

fn default_scope() -> PricingSourceScope {
    PricingSourceScope {
        catalogs: BTreeMap::from([
            ("openai".to_owned(), PricingCatalogSource::Direct),
            ("xai".to_owned(), PricingCatalogSource::Direct),
        ]),
    }
}

#[test]
fn source_selects_supported_providers_and_preserves_free_prices() {
    let catalog = json!({
        "openai":{"models":{
            "free":{"modalities":{"output":["text"]},"cost":{"input":0,"output":0}},
            "normal":{"modalities":{"output":["text"]},"cost":{"input":2.5,"output":10,"cache_read":0.25}},
            "image":{"modalities":{"output":["image"]},"cost":{"input":1,"output":2}},
            "invalid":{"modalities":{"output":["text"]},"cost":{"input":-1,"output":2}},
            "wrong-tier":{"modalities":{"output":["text"]},"cost":{"input":1,"output":2,"tiers":[{"tier":{"size":200000},"input":2,"output":4}]}}
        }},
        "xai":{"models":{}},
        "unrelated":{"models":{"secret":{}}}
    });
    let result = decode_catalog(&serde_json::to_vec(&catalog).unwrap(), &default_scope()).unwrap();
    let models = &result.prices["openai"];
    assert_eq!(models.len(), 2);
    assert_eq!(models["free"].bands["standard"].input.ticks_per_token(), 0);
    assert_eq!(
        models["normal"].bands["standard"]
            .cache_read
            .ticks_per_token(),
        2500
    );
    assert_eq!(result.skipped.len(), 3);
    assert!(!result.prices.contains_key("unrelated"));
}

#[test]
fn source_rejects_empty_partial_or_malformed_documents() {
    let scope = default_scope();
    for document in [
        json!({}),
        json!({"openai":{"models":{}}}),
        json!({"openai":{"models":{}},"xai":{"models":{}}}),
    ] {
        assert!(decode_catalog(&serde_json::to_vec(&document).unwrap(), &scope).is_err());
    }
    assert!(decode_catalog(b"not json", &scope).is_err());
}
