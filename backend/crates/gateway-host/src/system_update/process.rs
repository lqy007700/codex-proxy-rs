//! 宿主进程替换与容器内关闭语义

use std::env;
use std::fs;
use std::io;
use std::process::{ChildStdin, Command, Stdio};

use super::{OperationError, SystemUpdateConfig, internal};

const RESTART_HANDOFF_ARGUMENT: &str = "--restart-handoff";

/// 组合根在创建运行时前接收重启交接，并取得不含内部交接标记的 CLI 参数
pub fn startup_arguments() -> Result<impl Iterator<Item = std::ffi::OsString>, OperationError> {
    let mut arguments = env::args_os().skip(1).peekable();
    if arguments
        .peek()
        .is_some_and(|value| value == RESTART_HANDOFF_ARGUMENT)
    {
        arguments.next();
        // 旧服务释放操作锁后关闭管道；旧进程异常退出也会关闭管道，避免固定延迟抢占端口
        io::copy(&mut io::stdin().lock(), &mut io::sink())
            .map_err(|error| internal("无法等待旧服务释放资源").with_source(error))?;
    }
    Ok(arguments)
}

pub(crate) async fn prepare_replacement(
    config: &SystemUpdateConfig,
) -> Result<Command, OperationError> {
    let executable = config.executable_path()?;
    let metadata = fs::metadata(&executable)
        .map_err(|error| internal("无法启动替换进程").with_source(error))?;
    if !metadata.is_file() {
        return Err(internal("当前程序不是普通文件，无法在线重启"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(internal("当前程序没有执行权限，无法在线重启"));
        }
    }

    // 在旧服务仍可用且持有操作锁时检查目标，避免门控 shell 掩盖无效解释器等 spawn 失败
    // Unix 只使用已有的帮助合同；其他平台同时核对目标实际支持内部交接标记
    let mut probe = tokio::process::Command::new(&executable);
    #[cfg(not(unix))]
    probe.arg(RESTART_HANDOFF_ARGUMENT);
    let mut probe = probe
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| internal("无法启动替换进程").with_source(error))?;
    let status = match tokio::time::timeout(std::time::Duration::from_secs(5), probe.wait()).await {
        Ok(result) => result.map_err(|error| internal("无法检查替换进程").with_source(error))?,
        Err(_) => {
            let _ = probe.kill().await;
            return Err(internal("替换进程启动检查超时，旧服务继续运行"));
        }
    };
    if !status.success() {
        return Err(internal("替换进程启动检查失败，旧服务继续运行"));
    }

    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        // 旧发行不认识内部 CLI 标记，由启动器等待 EOF 再 exec，参数仍按原生编码透传
        command
            .arg("-c")
            .arg("while IFS= read -r line; do :; done; exec \"$@\" </dev/null")
            .arg("codex-proxy-rs-restart")
            .arg(&executable);
        command
    };
    #[cfg(not(unix))]
    let mut command = {
        let mut command = Command::new(&executable);
        command.arg(RESTART_HANDOFF_ARGUMENT);
        command
    };
    command
        // 连续重启时移除当前进程已消费的交接标记，保留原始参数及编码
        .args(
            env::args_os()
                .skip(1)
                .skip_while(|value| value == RESTART_HANDOFF_ARGUMENT),
        )
        .stdin(Stdio::piped());
    Ok(command)
}

pub(crate) fn spawn_replacement(
    mut command: Command,
) -> Result<Option<ChildStdin>, OperationError> {
    command
        .spawn()
        .map(|mut child| child.stdin.take())
        .map_err(|error| internal("无法启动替换进程").with_source(error))
}
