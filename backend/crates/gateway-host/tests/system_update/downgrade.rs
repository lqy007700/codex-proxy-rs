//! 历史发行降级范围、安装校验与失败保护测试

use super::*;
use gateway_admin::model::system::{SystemUpdateChannel, SystemUpdateMode};

fn release(server: &MockServer, version: &str) -> serde_json::Value {
    let name = archive_name(version);
    let archive = release_archive(ArchiveKind::Safe);
    let checksum = format!("{}  {name}\n", hex::encode(Sha256::digest(&archive)));
    serde_json::json!({
        "tag_name": format!("v{version}"),
        "prerelease": version.contains('-'),
        "body": format!("notes for {version}"),
        "assets": [
            {"name": name, "browser_download_url": format!("{}/archive", server.uri()), "size": archive.len()},
            {"name": "checksums.txt", "browser_download_url": format!("{}/checksums", server.uri()), "size": checksum.len()}
        ]
    })
}

fn service(fixture: &Fixture, server: &MockServer, current: &str) -> ProcessSystemOperations {
    let mut config = fixture.config(&format!("{}/repos", server.uri()));
    config.version = current.into();
    ProcessSystemOperations::new(CancellationToken::new(), config)
}

async fn mount_catalog(server: &MockServer, releases: Vec<serde_json::Value>) {
    Mock::given(method("GET"))
        .and(path("/repos/owner/repository/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(releases))
        .with_priority(1)
        .mount(server)
        .await;
}

async fn start_downgrade(
    service: &ProcessSystemOperations,
    target: &str,
    preflight: Arc<dyn SystemUpdatePreflight>,
) -> Result<SystemOperationAccepted, SystemOperationError> {
    service
        .perform_update(
            Some(target.into()),
            None,
            preflight,
            SystemUpdateMode::Downgrade,
        )
        .await
}

#[tokio::test]
async fn history_should_sort_fork_revisions_across_pages_and_keep_five_even_when_assets_are_missing()
 {
    let server = MockServer::start().await;
    let fixture = Fixture::new();
    let service = service(&fixture, &server, "3.39.1+lqy007700.v12");
    let mut first_page = vec![serde_json::json!({"tag_name":"invalid", "prerelease":false}); 100];
    first_page[0] = release(&server, "3.39.1+lqy007700.v2");
    first_page[1] = release(&server, "3.39.1+lqy007700.v10");
    first_page[2] = release(&server, "3.39.1+lqy007700.v9");
    let mut missing = release(&server, "3.39.1+lqy007700.v11");
    missing["assets"] = serde_json::json!([]);
    let mut no_checksum = release(&server, "3.39.1+lqy007700.v8");
    no_checksum["assets"].as_array_mut().unwrap().pop();
    let mut draft = release(&server, "3.39.1+lqy007700.v7");
    draft["draft"] = serde_json::json!(true);
    Mock::given(method("GET"))
        .and(path("/repos/owner/repository/releases"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(first_page))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repository/releases"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![
            missing,
            no_checksum,
            draft,
            release(&server, "3.39.1+lqy007700.v6"),
            release(&server, "3.39.1+lqy007700.v10"),
            release(&server, "3.40.0"),
            release(&server, "4.0.0"),
            release(&server, "2.99.0"),
            release(&server, "3.39.0-beta.1"),
            release(&server, "3.39.1+lqy007700.v12"),
        ]))
        .mount(&server)
        .await;
    let detail = service.update_detail(true, None).await.expect("detail");
    let versions: Vec<_> = detail
        .downgrade_versions
        .iter()
        .map(|item| item.version.as_str())
        .collect();
    assert_eq!(
        versions,
        [
            "3.39.1+lqy007700.v11",
            "3.39.1+lqy007700.v10",
            "3.39.1+lqy007700.v9",
            "3.39.1+lqy007700.v8",
            "3.39.1+lqy007700.v6"
        ]
    );
    assert!(detail.downgrade_versions[0].unsupported_reason.is_some());
    assert!(
        detail.downgrade_versions[3]
            .unsupported_reason
            .as_deref()
            .unwrap()
            .contains("checksums.txt")
    );
    assert!(detail.downgrade_versions[1].unsupported_reason.is_none());
    assert_eq!(detail.latest_version, "3.40.0");
    assert_eq!(
        service
            .update_detail(false, None)
            .await
            .unwrap()
            .downgrade_versions,
        detail.downgrade_versions
    );
    assert!(!fixture.state().exists());
}

#[tokio::test]
async fn history_should_follow_selected_channel_and_isolate_experimental_lines() {
    let server = MockServer::start().await;
    let fixture = Fixture::new();
    mount_catalog(
        &server,
        [
            "1.4.0",
            "1.5.0-rc.1",
            "1.5.0-beta.1",
            "1.5.0-alpha.1",
            "1.5.0-exp.1",
            "1.4.0-exp.9",
        ]
        .iter()
        .map(|version| release(&server, version))
        .collect(),
    )
    .await;
    let regular = service(&fixture, &server, "1.6.0");
    for (channel, count) in [
        (SystemUpdateChannel::Stable, 1),
        (SystemUpdateChannel::Rc, 2),
        (SystemUpdateChannel::Beta, 3),
        (SystemUpdateChannel::Alpha, 4),
    ] {
        assert_eq!(
            regular
                .update_detail(true, Some(channel))
                .await
                .unwrap()
                .downgrade_versions
                .len(),
            count
        );
    }
    let experimental = service(&fixture, &server, "1.5.0-exp.2");
    let detail = experimental.update_detail(true, None).await.unwrap();
    assert_eq!(detail.downgrade_versions.len(), 1);
    assert_eq!(detail.downgrade_versions[0].version, "1.5.0-exp.1");
}

#[tokio::test]
async fn downgrade_should_install_the_fifth_target_and_preserve_a_complete_rollback_backup() {
    let server = MockServer::start().await;
    let fixture = Fixture::new();
    fixture
        .mount_release(
            &server,
            TARGET_VERSION,
            ArchiveKind::Safe,
            ChecksumKind::Valid,
        )
        .await;
    mount_catalog(
        &server,
        [
            "1.9.13",
            "1.9.12",
            "1.9.11",
            "1.9.10",
            TARGET_VERSION,
            "1.9.8",
        ]
        .iter()
        .map(|version| release(&server, version))
        .collect(),
    )
    .await;
    let service = service(&fixture, &server, "1.10.0");
    assert!(
        service
            .perform_test_update(Some(TARGET_VERSION.into()))
            .await
            .is_err(),
        "普通升级不能接受旧版本"
    );
    start_downgrade(&service, TARGET_VERSION, Arc::new(AllowingUpdatePreflight))
        .await
        .expect("accepted");
    let status = wait_for_update(&service).await;
    assert_eq!(status.operation.status, SystemOperationStatus::Succeeded);
    assert_eq!(status.current_version.as_deref(), Some(TARGET_VERSION));
    assert_eq!(status.previous_version.as_deref(), Some("1.10.0"));
    assert!(status.need_restart);
    assert_eq!(fs::read(fixture.executable()).unwrap(), b"new-binary");
    assert_eq!(
        fs::read(fixture.web().join("index.html")).unwrap(),
        b"new-web"
    );
    assert_eq!(
        fs::read(fixture.official().join("new-plugin.tar.gz")).unwrap(),
        b"new-plugin"
    );
    assert_eq!(
        fs::read(fixture.executable().with_extension("backup")).unwrap(),
        b"old-binary"
    );
    assert!(
        start_downgrade(&service, "1.9.8", Arc::new(AllowingUpdatePreflight))
            .await
            .is_err(),
        "待重启时禁止再次安装"
    );
}

#[tokio::test]
async fn downgrade_should_reject_a_sixth_or_removed_target_before_downloading() {
    for target in ["1.9.8", "1.8.0"] {
        let server = MockServer::start().await;
        let fixture = Fixture::new();
        let service = service(&fixture, &server, "1.10.0");
        mount_catalog(&server, vec![release(&server, target)]).await;
        let shown = service.update_detail(true, None).await.unwrap();
        assert_eq!(shown.downgrade_versions[0].version, target);
        server.reset().await;
        let mut newer: Vec<_> = ["1.9.13", "1.9.12", "1.9.11", "1.9.10", "1.9.9"]
            .iter()
            .map(|version| release(&server, version))
            .collect();
        if target == "1.9.8" {
            newer.push(release(&server, target));
        }
        mount_catalog(&server, newer).await;
        start_downgrade(&service, target, Arc::new(AllowingUpdatePreflight))
            .await
            .expect("accepted for revalidation");
        let status = wait_for_update(&service).await;
        assert_eq!(status.operation.status, SystemOperationStatus::Failed);
        assert!(
            status
                .operation
                .error
                .as_deref()
                .unwrap()
                .contains("前 5 个版本")
        );
        assert!(!status.need_restart);
        assert_eq!(fs::read(fixture.executable()).unwrap(), b"old-binary");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.url.path().ends_with("/releases"))
        );
    }
}

#[tokio::test]
async fn downgrade_should_reject_invalid_direction_major_and_channel_without_network_or_file_changes()
 {
    let server = MockServer::start().await;
    let fixture = Fixture::new();
    let service = service(&fixture, &server, "1.10.0");
    for target in [
        "1.10.0",
        "1.11.0",
        "0.9.0",
        "2.0.0",
        "1.9.0-beta.1",
        "1.9.0-exp.1",
    ] {
        assert!(
            start_downgrade(&service, target, Arc::new(AllowingUpdatePreflight))
                .await
                .is_err(),
            "{target}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(!fixture.state().exists());
    assert_eq!(fs::read(fixture.executable()).unwrap(), b"old-binary");
}

struct RejectingDowngrade;

#[async_trait::async_trait]
impl SystemUpdatePreflight for RejectingDowngrade {
    async fn validate(&self, _: SystemUpdateCandidate) -> Result<Revision, SystemOperationError> {
        panic!("降级不能走普通升级的宽松校验")
    }
    async fn validate_downgrade(
        &self,
        _: SystemUpdateCandidate,
    ) -> Result<Revision, SystemOperationError> {
        Err(SystemOperationError::new(
            SystemOperationErrorKind::Conflict,
            "fixture incompatible plugin",
        ))
    }
    async fn confirm_revision(&self, _: Revision) -> Result<(), SystemOperationError> {
        unreachable!()
    }
}

#[tokio::test]
async fn failed_downgrade_should_preserve_all_running_files_and_release_locks() {
    for reject_plugin in [false, true] {
        let server = MockServer::start().await;
        let fixture = Fixture::new();
        fixture
            .mount_release(
                &server,
                TARGET_VERSION,
                ArchiveKind::Safe,
                if reject_plugin {
                    ChecksumKind::Valid
                } else {
                    ChecksumKind::Mismatch
                },
            )
            .await;
        let service = service(&fixture, &server, "1.10.0");
        let preflight: Arc<dyn SystemUpdatePreflight> = if reject_plugin {
            Arc::new(RejectingDowngrade)
        } else {
            Arc::new(AllowingUpdatePreflight)
        };
        start_downgrade(&service, TARGET_VERSION, preflight)
            .await
            .unwrap();
        let status = wait_for_update(&service).await;
        assert_eq!(status.operation.status, SystemOperationStatus::Failed);
        assert!(!status.need_restart);
        assert_eq!(fs::read(fixture.executable()).unwrap(), b"old-binary");
        assert_eq!(
            fs::read(fixture.web().join("index.html")).unwrap(),
            b"old-web"
        );
        assert_eq!(
            fs::read(fixture.official().join("old-plugin.tar.gz")).unwrap(),
            b"old-plugin"
        );
        assert!(!fixture.lock().exists());
    }
}

#[tokio::test]
async fn history_should_keep_numeric_fork_order_when_other_build_metadata_shares_the_baseline() {
    let server = MockServer::start().await;
    let fixture = Fixture::new();
    let service = service(&fixture, &server, "3.40.0+lqy007700.v1");
    mount_catalog(
        &server,
        [
            "3.39.1",
            "3.39.1+lqy007700.v2",
            "3.39.1+other",
            "3.39.1+lqy007700.v10",
            "3.39.1+lqy007700.v9",
            "3.39.1+lqy007700.v8",
        ]
        .iter()
        .map(|version| release(&server, version))
        .collect(),
    )
    .await;
    let versions: Vec<_> = service
        .update_detail(true, None)
        .await
        .unwrap()
        .downgrade_versions
        .into_iter()
        .map(|item| item.version)
        .collect();
    assert_eq!(
        versions,
        [
            "3.39.1+lqy007700.v10",
            "3.39.1+lqy007700.v9",
            "3.39.1+lqy007700.v8",
            "3.39.1+lqy007700.v2",
            "3.39.1+other",
        ]
    );
}
