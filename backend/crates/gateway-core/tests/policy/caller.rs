//! 验证共享 Key 的可信身份校验、命名空间隔离与直连兼容

use gateway_core::policy::{
    CallerIdentity, CallerIdentityPolicy, ClientApiKeyId, ForwardedCallerHeaders,
};

const SOURCE: &str = "b7bb7de9-7b06-4caa-8070-352af86a8a4a";
fn proxy(allow_unidentified: bool) -> CallerIdentityPolicy {
    CallerIdentityPolicy::NewApi {
        source_id: SOURCE.into(),
        allow_unidentified,
    }
}
fn headers(users: &[&str], tokens: &[&str]) -> ForwardedCallerHeaders {
    ForwardedCallerHeaders {
        users: users.iter().map(|value| (*value).into()).collect(),
        tokens: tokens.iter().map(|value| (*value).into()).collect(),
    }
}
#[test]
fn direct_keys_ignore_spoofed_identity_and_keep_legacy_namespace() {
    let key = ClientApiKeyId::new("key_direct").unwrap();
    let caller = CallerIdentityPolicy::Direct
        .resolve(&key, &headers(&["0", "1"], &["bad"]))
        .unwrap();
    assert_eq!(caller, CallerIdentity::direct(&key));
    assert_eq!(caller.session_namespace(), key.as_str());
}
#[test]
fn trusted_identity_rejects_missing_partial_duplicate_multivalue_and_noncanonical_ids() {
    let key = ClientApiKeyId::new("key_shared").unwrap();
    for value in [
        "",
        "0",
        "01",
        "+1",
        "-1",
        " 1",
        "1 ",
        "1,2",
        "1.0",
        "1e2",
        "١",
        "18446744073709551616",
    ] {
        assert!(
            proxy(false)
                .resolve(&key, &headers(&[value], &["2"]))
                .is_err(),
            "user {value:?}"
        );
        assert!(
            proxy(true)
                .resolve(&key, &headers(&["1"], &[value]))
                .is_err(),
            "token {value:?}"
        );
    }
    for invalid in [
        headers(&[], &[]),
        headers(&["1"], &[]),
        headers(&[], &["2"]),
        headers(&["1", "1"], &["2"]),
        headers(&["1"], &["2", "2"]),
    ] {
        assert!(proxy(false).resolve(&key, &invalid).is_err());
    }
    assert!(proxy(true).resolve(&key, &headers(&["1"], &[])).is_err());
    assert!(
        proxy(false)
            .resolve(
                &key,
                &headers(&["18446744073709551615"], &["9007199254740993"])
            )
            .is_ok()
    );
}
#[test]
fn source_user_token_isolate_sessions_but_cpr_key_rotation_preserves_identity() {
    let a = ClientApiKeyId::new("key_a").unwrap();
    let b = ClientApiKeyId::new("key_b").unwrap();
    let caller = proxy(false).resolve(&a, &headers(&["1"], &["2"])).unwrap();
    assert_eq!(
        caller,
        proxy(false).resolve(&b, &headers(&["1"], &["2"])).unwrap()
    );
    for other in [
        proxy(false).resolve(&a, &headers(&["2"], &["2"])).unwrap(),
        proxy(false).resolve(&a, &headers(&["1"], &["3"])).unwrap(),
        CallerIdentity::NewApi {
            source_id: "other".into(),
            user_id: "1".into(),
            token_id: "2".into(),
        },
        CallerIdentity::direct(&a),
    ] {
        assert_ne!(caller.session_namespace(), other.session_namespace());
    }
    assert!(
        ClientApiKeyId::new(caller.session_namespace()).is_err(),
        "proxy namespace cannot collide with any valid legacy Key ID"
    );
}
#[test]
fn unidentified_probe_has_fresh_identity_and_malformed_headers_never_downgrade() {
    let key = ClientApiKeyId::new("key_shared").unwrap();
    let first = proxy(true).resolve(&key, &headers(&[], &[])).unwrap();
    let second = proxy(true).resolve(&key, &headers(&[], &[])).unwrap();
    assert!(first.is_unidentified());
    assert_ne!(first.session_namespace(), second.session_namespace());
    assert!(
        proxy(true)
            .resolve(&key, &headers(&["bad"], &["bad"]))
            .is_err()
    );
}
