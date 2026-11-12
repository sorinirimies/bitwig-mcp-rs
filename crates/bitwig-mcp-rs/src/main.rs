use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use bitwig_mcp_rs::bridge::client::Bridge;
use bitwig_mcp_rs::bridge::server::BridgeServer;
use bitwig_mcp_rs::config::{Backend, Cli, Config, Dirs, HELP};
use bitwig_mcp_rs::extension;
use bitwig_mcp_rs::osc::client::Bitwig;
use bitwig_mcp_rs::osc::server::OscServer;
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> ExitCode {
    match Config::parse(
        std::env::args().skip(1),
        |k| std::env::var(k).ok(),
        &Dirs::system(),
    ) {
        Ok(Cli::Help) => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Cli::InstallExtension(dir)) => install_extension(dir),
        Ok(Cli::Version) => {
            println!("bitwig-mcp-rs {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Cli::Run(cfg)) => match run(cfg).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e:#}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("{e}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

async fn run(cfg: Config) -> Result<()> {
    // stdout carries the MCP protocol, so logs go to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match cfg.backend {
        Backend::Bridge => run_bridge(&cfg).await,
        Backend::Osc => run_osc(&cfg).await,
    }
}

/// `bitwig-mcp-rs install-extension`: install into `dir` or Bitwig's Extensions folder.
fn install_extension(dir: Option<std::path::PathBuf>) -> ExitCode {
    let dir = dir.or_else(|| extension::system_bitwig_dir().map(|b| b.join("Extensions")));
    let Some(dir) = dir else {
        eprintln!("error: cannot find Bitwig's user folder; pass --dir <Extensions folder>");
        return ExitCode::FAILURE;
    };
    match extension::install(&dir, extension::BWEXTENSION) {
        Ok(outcome) => {
            println!("{outcome}");
            println!("{}", extension::ADD_CONTROLLER);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// At start: keep the installed extension in step with this binary, if Bitwig is set up
/// for this user (its user folder exists). Never fatal.
fn ensure_extension(cfg: &Config) {
    if !cfg.install_extension {
        return;
    }
    let Some(bitwig) = cfg.bitwig_dir.as_ref().filter(|d| d.is_dir()) else {
        tracing::info!("no Bitwig user folder found: not installing the extension");
        return;
    };
    match extension::install(&bitwig.join("Extensions"), extension::BWEXTENSION) {
        Ok(extension::Outcome::Current(_)) => tracing::debug!("the Bitwig extension is current"),
        Ok(outcome) => tracing::info!("{outcome}. {}", extension::ADD_CONTROLLER),
        Err(e) => tracing::warn!("could not install the Bitwig extension: {e}"),
    }
}

async fn run_bridge(cfg: &Config) -> Result<()> {
    ensure_extension(cfg);
    let bridge = Arc::new(Bridge::new(&cfg.host, cfg.bridge_port));
    tracing::info!(
        bridge = bridge.addr(),
        instruments = %cfg.instruments_dir.display(),
        "bitwig-mcp-rs {} ready (bridge backend)",
        env!("CARGO_PKG_VERSION")
    );
    let service = BridgeServer::new(bridge, cfg.instruments_dir.clone())
        .serve(stdio())
        .await
        .context("MCP handshake failed")?;
    service.waiting().await.context("MCP session failed")?;
    Ok(())
}

async fn run_osc(cfg: &Config) -> Result<()> {
    let bw = Arc::new(Bitwig::connect(cfg).await?);
    tracing::info!(
        host = %cfg.host,
        send_port = cfg.send_port,
        listen_port = cfg.listen_port,
        listening = bw.listening(),
        "bitwig-mcp-rs {} ready (osc backend)",
        env!("CARGO_PKG_VERSION")
    );
    // Prime the state cache; the answer arrives while the client initializes.
    if let Err(e) = bw.refresh().await {
        tracing::warn!("initial refresh failed: {e:#}");
    }
    let service = OscServer::new(bw)
        .serve(stdio())
        .await
        .context("MCP handshake failed")?;
    service.waiting().await.context("MCP session failed")?;
    Ok(())
}
