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
        token_names: Vec::new(),
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
fn incomplete_or_invalid_identity_falls_back_without_guessing_a_proxy_owner() {
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
        assert_eq!(
            proxy(false)
                .resolve(&key, &headers(&[value], &["2"]))
                .unwrap(),
            CallerIdentity::direct(&key),
            "user {value:?}"
        );
        assert!(
            proxy(true)
                .resolve(&key, &headers(&["1"], &[value]))
                .unwrap()
                .is_unidentified(),
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
        assert_eq!(
            proxy(false).resolve(&key, &invalid).unwrap(),
            CallerIdentity::direct(&key)
        );
        assert!(
            proxy(true)
                .resolve(&key, &invalid)
                .unwrap()
                .is_unidentified()
        );
    }
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
fn isolated_fallback_has_fresh_identity_even_when_headers_are_malformed() {
    let key = ClientApiKeyId::new("key_shared").unwrap();
    let first = proxy(true).resolve(&key, &headers(&[], &[])).unwrap();
    let second = proxy(true).resolve(&key, &headers(&[], &[])).unwrap();
    assert!(first.is_unidentified());
    assert_ne!(first.session_namespace(), second.session_namespace());
    assert!(
        proxy(true)
            .resolve(&key, &headers(&["bad"], &["bad"]))
            .unwrap()
            .is_unidentified()
    );
}

#[test]
fn invalid_source_configuration_still_fails_validation() {
    let policy = CallerIdentityPolicy::NewApi {
        source_id: "invalid-source".into(),
        allow_unidentified: false,
    };
    let key = ClientApiKeyId::new("key_shared").unwrap();
    assert!(policy.validate().is_err());
    assert!(policy.resolve(&key, &headers(&[], &[])).is_err());
}

#[test]
fn optional_token_names_never_change_caller_identity_and_only_trusted_callers_use_them() {
    let key = ClientApiKeyId::new("key_shared").unwrap();
    let expected = proxy(false)
        .resolve(&key, &headers(&["1"], &["2"]))
        .unwrap();
    let cases = [
        (Vec::new(), None),
        (vec![b"  developer  ".to_vec()], Some("developer")),
        (vec!["中文名称".as_bytes().to_vec()], Some("中文名称")),
        (
            vec![b"%E4%B8%AD%E6%96%87+Key%20100%25".to_vec()],
            Some("中文+Key 100%"),
        ),
        (vec![b"literal%2520name".to_vec()], Some("literal%20name")),
        (vec![b"%20name%20".to_vec()], Some("name")),
        (vec![b"%e4%b8%ad".to_vec()], Some("中")),
        (vec![b"%".to_vec()], None),
        (vec![b"%2".to_vec()], None),
        (vec![b"%GG".to_vec()], None),
        (vec![b"%FF".to_vec()], None),
        (vec![b"%0D%0AX-Injected:%20yes".to_vec()], None),
        (vec![b"%20%20".to_vec()], None),
        (vec![b"  ".to_vec()], None),
        (vec![b"bad\tname".to_vec()], None),
        (vec![vec![0xff]], None),
        (vec![vec![b'a'; 257]], None),
        (vec![b"one".to_vec(), b"two".to_vec()], None),
    ];
    for (token_names, name) in cases {
        let raw = ForwardedCallerHeaders {
            token_names,
            ..headers(&["1"], &["2"])
        };
        let caller = proxy(false).resolve(&key, &raw).unwrap();
        assert_eq!(caller, expected);
        assert_eq!(caller.session_namespace(), expected.session_namespace());
        assert_eq!(raw.token_name(&caller).as_deref(), name);
        assert_eq!(raw.token_name(&CallerIdentity::direct(&key)), None);
    }
    for allow_unidentified in [false, true] {
        let raw = ForwardedCallerHeaders {
            token_names: vec![b"ignored".to_vec()],
            ..headers(&["1"], &[])
        };
        let caller = proxy(allow_unidentified).resolve(&key, &raw).unwrap();
        assert_eq!(raw.token_name(&caller), None);
    }
}

#[test]
fn forwarded_token_name_limits_apply_after_decoding_without_reinterpreting_stored_names() {
    let key = ClientApiKeyId::new("key_shared").unwrap();
    let caller = proxy(false)
        .resolve(&key, &headers(&["1"], &["2"]))
        .unwrap();
    let name = format!("{}a", "中".repeat(85));
    let raw = ForwardedCallerHeaders {
        token_names: vec![format!("{}a", "%E4%B8%AD".repeat(85)).into_bytes()],
        ..headers(&["1"], &["2"])
    };
    assert_eq!(raw.token_name(&caller).as_deref(), Some(name.as_str()));
    for value in [format!("{}ab", "%E4%B8%AD".repeat(85)), "%41".repeat(257)] {
        let raw = ForwardedCallerHeaders {
            token_names: vec![value.into_bytes()],
            ..headers(&["1"], &["2"])
        };
        assert_eq!(raw.token_name(&caller), None);
    }
    assert_eq!(
        gateway_core::policy::normalize_caller_token_name(b"literal%20name"),
        Some("literal%20name")
    );
}
