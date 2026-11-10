use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use bitwig_mcp_rs::bridge::client::Bridge;
use bitwig_mcp_rs::bridge::server::BridgeServer;
use bitwig_mcp_rs::config::{Backend, Cli, Config, HELP};
use bitwig_mcp_rs::osc::client::Bitwig;
use bitwig_mcp_rs::osc::server::OscServer;
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> ExitCode {
    match Config::parse(std::env::args().skip(1), |k| std::env::var(k).ok()) {
        Ok(Cli::Help) => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
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

async fn run_bridge(cfg: &Config) -> Result<()> {
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
