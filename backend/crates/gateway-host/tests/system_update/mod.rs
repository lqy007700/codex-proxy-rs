//! 系统更新测试入口与发行包、前置校验和本地安装夹具

mod channels;
mod downgrade;
mod download;
mod events;
mod installation;
mod operations;
mod release;
mod state;
mod swap_paths;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use filetime::FileTime;
use flate2::{Compression, write::GzEncoder};
use futures::StreamExt as _;
use gateway_admin::model::system::{
    SystemOperationAccepted, SystemOperationKind, SystemOperationStatus, SystemUpdateEventLevel,
    SystemUpdateStatus,
};
use gateway_admin::ports::system::{
    SystemOperationError, SystemOperationErrorKind, SystemOperations,
};
use gateway_core::lifecycle::CancellationToken;
use gateway_host::system_update::{
    ProcessSystemOperations, SystemUpdateConfig, validate_download_url,
};
use sha2::{Digest as _, Sha256};
use tar::{Builder, EntryType, Header};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TARGET_VERSION: &str = "1.9.9";
const CROSS_MAJOR_VERSION: &str = "2.0.0";

#[derive(Default)]
struct RecordingDiagnostics {
    failures: std::sync::Mutex<Vec<gateway_core::diagnostics::OperationalFailure>>,
    reject: bool,
}

#[async_trait::async_trait]
impl gateway_core::diagnostics::OperationalDiagnostics for RecordingDiagnostics {
    async fn record_failure(
        &self,
        failure: gateway_core::diagnostics::OperationalFailure,
    ) -> Result<(), gateway_core::error::StoreError> {
        self.failures
            .lock()
            .expect("diagnostic failures")
            .push(failure);
        if self.reject {
            Err(gateway_core::error::StoreError::new(
                gateway_core::error::StoreErrorKind::Unavailable,
            ))
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
trait TestSystemUpdate {
    async fn perform_test_update(
        &self,
        target: Option<String>,
    ) -> Result<SystemOperationAccepted, SystemOperationError>;
}

#[async_trait::async_trait]
impl TestSystemUpdate for ProcessSystemOperations {
    async fn perform_test_update(
        &self,
        target: Option<String>,
    ) -> Result<SystemOperationAccepted, SystemOperationError> {
        SystemOperations::perform_update(
            self,
            target,
            None,
            gateway_admin::model::system::SystemUpdateMode::Upgrade,
        )
        .await
    }
}

async fn wait_for_update(service: &ProcessSystemOperations) -> SystemUpdateStatus {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = service.update_status().await.expect("update status");
            if status.operation.status != SystemOperationStatus::Running {
                assert!(status.operation.finished_at.is_some(), "terminal timestamp");
                return status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("update reaches terminal state")
}

async fn complete_update(service: &ProcessSystemOperations, target: &str) -> SystemUpdateStatus {
    let SystemOperationAccepted::Update { operation_id, .. } = service
        .perform_test_update(Some(target.to_owned()))
        .await
        .expect("update accepted")
    else {
        panic!("expected update operation")
    };
    let status = wait_for_update(service).await;
    assert_eq!(
        status.operation.operation_id.as_deref(),
        Some(operation_id.as_str())
    );
    status
}

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::new_in(std::env::temp_dir())
    }

    fn new_in(parent: impl AsRef<std::path::Path>) -> Self {
        let root = tempfile::tempdir_in(parent).expect("system update root");
        let fixture = Self { root };
        fixture.write_executable("old-binary");
        fs::create_dir_all(fixture.web()).expect("web dir");
        fs::write(fixture.web().join("index.html"), "old-web").expect("web");
        fixture
    }

    fn executable(&self) -> PathBuf {
        self.root.path().join("codex-proxy-rs")
    }

    fn web(&self) -> PathBuf {
        self.root.path().join("web/dist")
    }

    fn state(&self) -> PathBuf {
        self.root.path().join("update-state.json")
    }

    fn lock(&self) -> PathBuf {
        self.root.path().join("update.lock")
    }

    fn write_executable(&self, content: &str) {
        fs::write(self.executable(), content).expect("binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(self.executable(), fs::Permissions::from_mode(0o755))
                .expect("permissions");
        }
    }

    fn config(&self, api_base: &str) -> SystemUpdateConfig {
        SystemUpdateConfig {
            version: "1.0.0".to_owned(),
            git_sha: "test-sha".to_owned(),
            build_time: "2026-07-19T00:00:00Z".to_owned(),
            deployment_mode: "binary".to_owned(),
            build_type: "release".to_owned(),
            update_repository: Some("owner/repository".to_owned()),
            github_api_base: api_base.to_owned(),
            executable_path: Some(self.executable()),
            web_dist_dir: Some(self.web()),
            update_state_file: self.state(),
            update_lock_file: self.lock(),
            update_temp_dir: self.root.path().join("tmp"),
            self_restart_enabled: false,
        }
    }

    fn service(&self, server: &MockServer) -> ProcessSystemOperations {
        self.service_for_url(&format!("{}/repos", server.uri()))
    }

    fn service_for_url(&self, api_base: &str) -> ProcessSystemOperations {
        ProcessSystemOperations::new(CancellationToken::new(), self.config(api_base))
    }

    async fn mount_release(
        &self,
        server: &MockServer,
        version: &str,
        archive_kind: ArchiveKind,
        checksum_kind: ChecksumKind,
    ) {
        let archive = release_archive(archive_kind);
        let name = archive_name(version);
        let checksum = match checksum_kind {
            ChecksumKind::Valid => format!("{}  {name}\n", hex::encode(Sha256::digest(&archive))),
            ChecksumKind::Mismatch => format!("{}  {name}\n", "0".repeat(64)),
            ChecksumKind::Missing => String::new(),
        };
        let checksum_asset = (!matches!(checksum_kind, ChecksumKind::Missing)).then(|| {
            serde_json::json!({
                "name": "checksums.txt",
                "browser_download_url": format!("{}/checksums", server.uri()),
                "size": checksum.len(),
            })
        });
        let mut assets = vec![serde_json::json!({
            "name": name,
            "browser_download_url": format!("{}/archive", server.uri()),
            "size": archive.len(),
        })];
        assets.extend(checksum_asset);
        mount_release_json(server, version, assets, None).await;
        Mock::given(method("GET"))
            .and(path("/archive"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(archive))
            .mount(server)
            .await;
        if !checksum.is_empty() {
            Mock::given(method("GET"))
                .and(path("/checksums"))
                .respond_with(ResponseTemplate::new(200).set_body_string(checksum))
                .mount(server)
                .await;
        }
    }

    async fn mount_release_once(&self, server: &MockServer, version: &str) {
        let assets = Vec::new();
        Mock::given(method("GET"))
            .and(path("/repos/owner/repository/releases"))
            .respond_with(release_response(version, assets))
            .up_to_n_times(1)
            .mount(server)
            .await;
    }

    async fn mount_custom_release(
        &self,
        server: &MockServer,
        archive_url: &str,
        checksum_url: Option<&str>,
    ) {
        let archive = release_archive(ArchiveKind::Safe);
        let name = archive_name(TARGET_VERSION);
        let mut assets = vec![serde_json::json!({
            "name": name,
            "browser_download_url": archive_url,
            "size": archive.len(),
        })];
        if let Some(checksum_url) = checksum_url {
            assets.push(serde_json::json!({
                "name": "checksums.txt",
                "browser_download_url": checksum_url,
                "size": 80,
            }));
        } else {
            assets.push(serde_json::json!({
                "name": "checksums.txt",
                "browser_download_url": format!("{}/checksums", server.uri()),
                "size": 80,
            }));
        }
        mount_release_json(server, TARGET_VERSION, assets, None).await;
    }
}

#[derive(Clone, Copy)]
enum ArchiveKind {
    Safe,
    UnsafePath,
}

#[derive(Clone, Copy)]
enum ChecksumKind {
    Valid,
    Mismatch,
    Missing,
}

fn release_archive(kind: ArchiveKind) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = Builder::new(encoder);
    append_file(&mut tar, "codex-proxy-rs", b"new-binary", false);
    append_file(&mut tar, "web/dist/index.html", b"new-web", false);
    if matches!(kind, ArchiveKind::UnsafePath) {
        append_file(&mut tar, "safe", b"escape", true);
    }
    let encoder = tar.into_inner().expect("tar");
    encoder.finish().expect("gzip")
}

fn append_file(tar: &mut Builder<GzEncoder<Vec<u8>>>, name: &str, data: &[u8], unsafe_path: bool) {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o755);
    header.set_size(u64::try_from(data.len()).expect("size"));
    header.set_path(name).expect("path");
    if unsafe_path {
        let bytes = header.as_mut_bytes();
        bytes[..100].fill(0);
        bytes[..9].copy_from_slice(b"../escape");
    }
    header.set_cksum();
    tar.append(&header, data).expect("append");
}

fn archive_name(version: &str) -> String {
    format!(
        "codex-proxy-rs-{version}-{}-{}.tar.gz",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

async fn mount_release_json(
    server: &MockServer,
    version: &str,
    assets: Vec<serde_json::Value>,
    times: Option<u64>,
) {
    let mock = Mock::given(method("GET"))
        .and(path("/repos/owner/repository/releases"))
        .respond_with(release_response(version, assets));
    match times {
        Some(times) => mock.up_to_n_times(times).mount(server).await,
        None => mock.mount(server).await,
    }
}

fn release_response(version: &str, assets: Vec<serde_json::Value>) -> ResponseTemplate {
    let prerelease = !semver::Version::parse(version)
        .expect("version")
        .pre
        .is_empty();
    let release = serde_json::json!({
        "tag_name": format!("v{version}"),
        "name": format!("Release {version}"),
        "body": "notes",
        "html_url": "https://github.com/owner/repository/releases/latest",
        "prerelease": prerelease,
        "published_at": "2026-07-19T00:00:00Z",
        "assets": assets,
    });
    ResponseTemplate::new(200).set_body_json(serde_json::json!([release]))
}
