//! 网关命令行入口，选择启动模式并创建异步运行时

use std::{error::Error, io, process::ExitCode};

use provider_openai::ensure_rustls_provider;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn Error + Send + Sync>> {
    ensure_rustls_provider();
    let mut arguments = gateway_host::system_update::startup_arguments()?;
    let command = arguments
        .next()
        .map(|value| {
            value
                .into_string()
                .map_err(|_| invalid_cli("command must be UTF-8"))
        })
        .transpose()?;
    match command.as_deref() {
        None | Some("serve") => {
            reject_extra_arguments(arguments)?;
            runtime()?.block_on(codex_proxy_rs::bootstrap::run())?;
            Ok(ExitCode::SUCCESS)
        }
        Some("help" | "--help" | "-h") => {
            reject_extra_arguments(arguments)?;
            println!(
                "Usage: codex-proxy-rs [serve]\n\n  --help     Show help without loading configuration\n  --version  Show version"
            );
            Ok(ExitCode::SUCCESS)
        }
        Some("--version" | "-V") => {
            reject_extra_arguments(arguments)?;
            println!(
                "{}",
                option_env!("CPR_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
            );
            Ok(ExitCode::SUCCESS)
        }
        Some(_) => Err(invalid_cli("unknown command; expected serve or --help").into()),
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, io::Error> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
}

fn reject_extra_arguments(mut arguments: impl Iterator) -> Result<(), io::Error> {
    if arguments.next().is_some() {
        return Err(invalid_cli("unexpected extra command arguments"));
    }
    Ok(())
}

fn invalid_cli(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
