//! 验证网关特化恢复与普通上游错误透传的边界

use super::precommit::stream_failure;
use super::*;
use gateway_core::error::TransientRetryPolicy;

#[tokio::test]
async fn transient_failure_recovery_requires_open_replay_boundary() {
    for websocket in [false, true] {
        for (status, code) in [
            (Some(500), Some("server_error")),
            (Some(500), Some("Vendor.Future.v2")),
            (Some(500), None),
            (Some(503), Some("server_error")),
            (Some(503), Some("Vendor.Future.v2")),
            (Some(503), None),
            (None, Some("server_error")),
            (Some(503), Some("server_is_overloaded")),
            (None, Some("server_is_overloaded")),
            (None, Some("slow_down")),
        ] {
            let capacity = matches!(code, Some("server_is_overloaded" | "slow_down"));
            for (response_failed, boundary_event) in [false, true].into_iter().flat_map(|failed| {
                [
                    None,
                    Some(json!({"type":"response.output_text.delta","response_id":"resp_503","item_id":"msg_503","output_index":0,"content_index":0,"delta":"hello"})),
                    Some(json!({"type":"response.function_call_arguments.delta","item_id":"call_1","output_index":0,"delta":"{}"})),
                    Some(json!({"type":"vendor.future.event","data":{"unknown":true}})),
                ].map(|event| (failed, event))
            }) {
                let replay_closed = boundary_event.is_some();
                let store = Arc::new(MemoryAccountStore::default());
                create_account(&store, "acct_provider_contract").await;
                let mut error =
                    json!({"type":"server_error","message":"You can retry your request."});
                if let Some(code) = code {
                    error["code"] = json!(code);
                }
                let mut failure = if response_failed {
                    json!({"type":"response.failed","response":{"id":"resp_503","status":"failed","error":error}})
                } else {
                    json!({"type":"error","sequence_number":3,"error":error})
                };
                if let Some(status) = status {
                    failure["status"] = json!(status);
                }
                let mut upstream_events = vec![
                    json!({"type":"codex.response.metadata","metadata":{}}),
                    json!({"type":"response.created","response":{"id":"resp_503","status":"in_progress","output":[]}}),
                    json!({"type":"response.in_progress","response":{"id":"resp_503","status":"in_progress","output":[]}}),
                ];
                if let Some(event) = boundary_event {
                    upstream_events.push(event);
                }
                // 元数据可替换，但不能重新打开已被输出或未知事件关闭的恢复窗口
                upstream_events.push(json!({
                    "type": "response.metadata",
                    "response_id": "resp_503",
                    "headers": {"x-codex-turn-state": "turn-503"},
                    "metadata": {"type": "safety_buffering", "use_cases": ["cyber"]}
                }));
                let last_prelude_type = upstream_events.last().unwrap()["type"].as_str().unwrap().to_owned();
                let terminal = failure.clone();
                let (release, released) = oneshot::channel();
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base = format!("http://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    if websocket {
                        let mut socket = accept_codex_test_websocket(socket).await;
                        socket.next().await.unwrap().unwrap();
                        for event in upstream_events {
                            socket
                                .send(Message::Text(event.to_string().into()))
                                .await
                                .unwrap();
                        }
                        released.await.unwrap();
                        socket.send(Message::Text(terminal.to_string().into())).await.unwrap();
                    } else {
                        read_http_request(&mut socket).await;
                        let body = upstream_events
                            .iter()
                            .map(|event| {
                                gateway_protocol::openai::sse::encode_sse_event(
                                    event["type"].as_str().unwrap(),
                                    &event.to_string(),
                                )
                            })
                            .collect::<String>();
                        let terminal = gateway_protocol::openai::sse::encode_sse_event(
                            terminal["type"].as_str().unwrap(), &terminal.to_string(),
                        );
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len() + terminal.len()).as_bytes()).await.unwrap();
                        released.await.unwrap();
                        socket.write_all(terminal.as_bytes()).await.unwrap();
                    }
                });
                let operation = if websocket {
                    capacity_websocket_operation()
                } else {
                    http_generate_operation()
                };
                let mut stream = provider_with_base_url(&store, base)
                    .execute(
                        planned_request("openai", operation),
                        context("req_service_unavailable", CancellationToken::new()),
                    )
                    .await
                    .unwrap();
                let mut events = Vec::new();
                // 明确验证已交付事件的屏障，不让 HTTP chunk 合并把事件留在原子失败批次
                loop {
                    let event = timeout(Duration::from_secs(3), stream.next()).await.unwrap().unwrap().unwrap();
                    let last_prelude = event.wire_event().is_some_and(|wire| wire.event_type() == Some(last_prelude_type.as_str()));
                    events.push(event);
                    if last_prelude { break; }
                }
                release.send(()).unwrap();
                let (terminal_events, mut error) = stream_failure(&mut stream).await;
                events.extend(terminal_events);
                assert_eq!(
                    error.kind(),
                    if capacity {
                        ProviderErrorKind::UpstreamCapacityUnavailable
                    } else {
                        ProviderErrorKind::Unavailable
                    }
                );
                assert_eq!(error.upstream_status(), status);
                assert_eq!(error.send_state(), UpstreamSendState::Sent);
                assert_eq!(error.replay_is_safe(), !replay_closed);
                if replay_closed {
                    assert!(error.pre_delivery_retry().is_none());
                    assert!(events.iter().any(
                        |event| event.has_client_event() && !event.is_replayable_prelude()
                    ));
                } else {
                    assert!(!error.retry_is_prohibited());
                    let Some(PreDeliveryRetry::SameAccountTransientRetry {
                        max_retries,
                        initial_delay,
                        max_delay,
                        jitter,
                        policy:
                            TransientRetryPolicy::RetryThenRotateAccountWithBudget {
                                max_total_retries,
                                track_client_retry_failure,
                            },
                    }) = error.pre_delivery_retry()
                    else {
                        panic!("expected shared bounded service failure recovery")
                    };
                    assert_eq!(
                        (max_retries.get(), max_total_retries.get()),
                        (2, 5)
                    );
                    assert_eq!(track_client_retry_failure, !capacity);
                    assert_eq!(initial_delay, Duration::from_millis(500));
                    assert_eq!(max_delay, Duration::from_secs(2));
                    assert!(
                        (Duration::from_millis(50)..Duration::from_millis(250)).contains(&jitter)
                    );
                    assert!(
                        events
                            .iter()
                            .filter(|event| event.has_client_event())
                            .all(|event| event.is_replayable_prelude())
                    );
                }
                assert_eq!(
                    serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str())
                        .unwrap(),
                    failure
                );
                events.extend(error.take_atomic_client_events());
                assert_eq!(
                    events.last().unwrap().wire_event().unwrap().data(),
                    &failure
                );
                let account = store.account("acct_provider_contract").unwrap();
                assert_eq!(account.credential_state(), CredentialState::Ready);
                assert_eq!(account.quota().access(), QuotaAccessState::Unknown);
                server.await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn service_failure_opening_rejections_request_bounded_retry_and_keep_response() {
    for (websocket, status) in [false, true]
        .into_iter()
        .flat_map(|ws| [500, 503].map(|status| (ws, status)))
    {
        for body in [
            r#"{ "error":{"code":"server_error","type":"server_error","message":"retry later"} }"#,
            r#"{"error":{"code":"Vendor.Future.v2","message":"retry later"}}"#,
            r#"{"error":{"message":"retry later"}}"#,
            "Service temporarily unavailable",
        ] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let server = MockServer::start().await;

            Mock::given(method(if websocket { "GET" } else { "POST" }))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("retry-after", "17")
                        .set_body_string(body),
                )
                .expect(1)
                .mount(&server)
                .await;
            let operation = if websocket {
                capacity_websocket_operation()
            } else {
                http_generate_operation()
            };
            let mut stream = provider_with_base_url(&store, server.uri())
                .execute(
                    planned_request("openai", operation),
                    context("req_server_error_opening", CancellationToken::new()),
                )
                .await
                .unwrap();
            let (_, error) = stream_failure(&mut stream).await;
            assert!(error.replay_is_safe());
            assert!(!error.retry_is_prohibited());
            assert!(matches!(
                error.pre_delivery_retry(),
                Some(PreDeliveryRetry::SameAccountTransientRetry {
                    policy: TransientRetryPolicy::RetryThenRotateAccountWithBudget { max_total_retries, track_client_retry_failure: true },
                    max_retries,
                    ..
                }) if max_retries.get() == 2 && max_total_retries.get() == 5
            ));
            assert_eq!(error.retry_after(), Some(Duration::from_secs(17)));
            let response = error.client_visible_upstream_response().unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.body().as_ref(), body.as_bytes());
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn ordinary_http_and_websocket_opening_errors_remain_client_decisions() {
    for websocket in [false, true] {
        for status in [409, 502, 504] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let server = MockServer::start().await;
            let body = r#"{ "error":{"code":"Vendor.Future.v2","type":"server_error","message":"upstream failed","future":9007199254740993} }"#;
            Mock::given(method(if websocket { "GET" } else { "POST" }))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("content-type", "application/json")
                        .insert_header("retry-after", "17")
                        .set_body_string(body),
                )
                .expect(2)
                .mount(&server)
                .await;
            let provider = provider_with_base_url_and_retry_budget(&store, server.uri(), 0);
            // 同一会话的下一次请求仍使用原传输，普通拒绝不污染 WS 回退状态
            for _ in 0..2 {
                let operation = if websocket {
                    capacity_websocket_operation()
                } else {
                    http_generate_operation()
                };
                let mut stream = provider
                    .clone()
                    .execute(
                        planned_request("openai", operation),
                        context("req_passthrough_recovery", CancellationToken::new()),
                    )
                    .await
                    .unwrap();
                let (_, error) = stream_failure(&mut stream).await;
                assert!(
                    error.retry_is_prohibited(),
                    "websocket={websocket}, status={status}"
                );
                assert!(!error.replay_is_safe());
                assert!(error.pre_delivery_retry().is_none());
                let response = error.client_visible_upstream_response().unwrap();
                assert_eq!(response.status(), status);
                assert_eq!(response.body().as_ref(), body.as_bytes());
                assert!(response.headers().iter().any(
                    |header| header.name() == "retry-after" && header.value().as_ref() == b"17"
                ));
            }
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn ordinary_stream_failures_preserve_original_events_without_proxy_retry() {
    for websocket in [false, true] {
        for (status, code) in [
            (None, Some("Vendor.Future.v2")),
            (None, None),
            (Some(500), Some("invalid_prompt")),
            (Some(503), Some("invalid_prompt")),
            (Some(502), Some("server_error")),
            (Some(504), Some("server_error")),
        ] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let created = json!({"type":"response.created","response":{"id":"resp_failure","status":"in_progress","output":[]}});
            let mut failed = json!({"type":"response.failed","response":{"id":"resp_failure","status":"failed","output":[],"error":{"type":"server_error","message":"original failure","headers":{"Retry-After":"17"},"future":{"keep":true}}}});
            if let Some(code) = code {
                failed["response"]["error"]["code"] = json!(code);
            }
            if let Some(status) = status {
                failed["status_code"] = json!(status);
            }
            let expected = vec![created.clone(), failed.clone()];
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                if websocket {
                    let mut socket = accept_codex_test_websocket(socket).await;
                    socket.next().await.unwrap().unwrap();
                    for event in [created, failed] {
                        socket
                            .send(Message::Text(event.to_string().into()))
                            .await
                            .unwrap();
                    }
                } else {
                    read_http_request(&mut socket).await;
                    let body = [created, failed]
                        .iter()
                        .map(|event| {
                            gateway_protocol::openai::sse::encode_sse_event(
                                event["type"].as_str().unwrap(),
                                &event.to_string(),
                            )
                        })
                        .collect::<String>();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            });
            let operation = if websocket {
                capacity_websocket_operation()
            } else {
                http_generate_operation()
            };
            let mut stream = provider_with_base_url(&store, base)
                .execute(
                    planned_request("openai", operation),
                    context("req_stream_passthrough", CancellationToken::new()),
                )
                .await
                .unwrap();
            let (mut events, mut error) = stream_failure(&mut stream).await;
            assert!(
                events
                    .iter()
                    .filter(|event| event.has_client_event())
                    .all(|event| event.is_replayable_prelude())
            );
            assert!(error.retry_is_prohibited());
            assert_eq!(error.upstream_status(), status);
            assert!(error.pre_delivery_retry().is_none());
            assert_eq!(error.retry_after(), Some(Duration::from_secs(17)));
            events.extend(error.take_atomic_client_events());
            let atomic = events;
            assert_eq!(
                atomic
                    .iter()
                    .filter_map(|event| event.wire_event().map(|wire| wire.data().clone()))
                    .collect::<Vec<_>>(),
                expected
            );
            server.await.unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
mod native_tls {
    use super::*;

    fn run_isolated_default_tls() -> bool {
        const CHILD: &str = "CPR_TEST_NATIVE_TLS_RECOVERY";
        if std::env::var_os(CHILD).is_some() {
            return false;
        }
        // Cargo 的 SSL_CERT_FILE 会选择 rustls；只在子进程中固定默认 native TLS 路径
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", std::thread::current().name().unwrap()])
            .env(CHILD, "1")
            .env_remove(provider_openai::transport::tls::CODEX_CA_CERT_ENV)
            .env_remove(provider_openai::transport::tls::SSL_CERT_FILE_ENV)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        true
    }

    async fn failure_at(base: String) -> gateway_core::error::ProviderError {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let mut stream = provider_with_base_url(&store, base)
            .execute(
                planned_request("openai", http_generate_operation()),
                context("req_tls_recovery", CancellationToken::new()),
            )
            .await
            .unwrap();
        let (_, error) = timeout(Duration::from_secs(5), stream_failure(&mut stream))
            .await
            .unwrap();
        assert_eq!(error.send_state(), UpstreamSendState::NotSent);
        error
    }

    async fn rejected_handshake(alert: bool) -> gateway_core::error::ProviderError {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut header = [0_u8; 5];
            socket.read_exact(&mut header).await.unwrap();
            assert_eq!(header[0], 22);
            let mut hello = vec![0; usize::from(u16::from_be_bytes([header[3], header[4]]))];
            socket.read_exact(&mut hello).await.unwrap();
            if alert {
                socket.write_all(&[21, 3, 3, 0, 2, 2, 40]).await.unwrap();
            }
            socket.shutdown().await.unwrap();
        });
        let error = failure_at(base).await;
        server.await.unwrap();
        error
    }

    #[tokio::test]
    async fn handshake_eof_requests_bounded_same_account_connection_recovery() {
        if run_isolated_default_tls() {
            return;
        }
        let error = rejected_handshake(false).await;
        assert!(error.error_details().unwrap().contains("unexpected EOF"));
        assert_eq!(
            error.pre_delivery_retry(),
            Some(PreDeliveryRetry::SameAccountConnectionRetry {
                transport: AttemptTransport::Fallback
            })
        );
    }

    #[tokio::test]
    async fn handshake_alert_does_not_request_connection_recovery() {
        if run_isolated_default_tls() {
            return;
        }
        let error = rejected_handshake(true).await;
        assert!(error.error_details().unwrap().contains("handshake failure"));
        assert!(error.pre_delivery_retry().is_none());
    }

    #[tokio::test]
    async fn untrusted_certificate_does_not_request_connection_recovery() {
        if run_isolated_default_tls() {
            return;
        }
        use openssl::{
            asn1::Asn1Time,
            hash::MessageDigest,
            pkey::PKey,
            rsa::Rsa,
            ssl::{SslAcceptor, SslMethod},
            x509::{X509, X509NameBuilder},
        };
        let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut certificate = X509::builder().unwrap();
        certificate.set_version(2).unwrap();
        certificate.set_subject_name(&name).unwrap();
        certificate.set_issuer_name(&name).unwrap();
        certificate.set_pubkey(&key).unwrap();
        certificate
            .set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        certificate
            .set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        certificate.sign(&key, MessageDigest::sha256()).unwrap();
        let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&key).unwrap();
        acceptor.set_certificate(&certificate.build()).unwrap();
        let acceptor = acceptor.build();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let server = tokio::task::spawn_blocking(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert!(acceptor.accept(socket).is_err());
        });
        let error = failure_at(base).await;
        assert!(
            error
                .error_details()
                .unwrap()
                .contains("certificate verify failed")
        );
        assert!(error.pre_delivery_retry().is_none());
        server.await.unwrap();
    }
}
