use std::{env, sync::Arc, time::Duration};

use clap::{Parser, Subcommand};
use tosi::{
    backend::CliRunner, config, diagnostics::check_environment, manager::Manager, server::router,
    types::Backend,
};
use tower_mcp::StdioTransport;

#[derive(Debug, Parser)]
#[command(
    version,
    about,
    args_conflicts_with_subcommands = true,
    after_help = include_str!("../resources/cli/after-help.txt").trim_end()
)]
struct Cli {
    /// Serve MCP over stdio (also the default when no arguments are supplied)
    #[arg(long)]
    stdio: bool,
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// List durable task history as JSON
    Tasks {
        #[arg(long, value_enum)]
        status: Option<tosi::manager::Status>,
        #[arg(long)]
        since: Option<u64>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        full_result: bool,
    },
    /// Report CLI discovery and authentication visible in this launch context as JSON; no model call
    Doctor {
        /// Check only this provider (omitted checks both)
        #[arg(long, value_enum)]
        backend: Option<Backend>,
    },
}

fn main() -> Result<(), tower_mcp::BoxError> {
    let cli = Cli::parse();
    let state = bottega_core::state::prepare(bottega_core::state::from_env())?;
    if let Some(warning) = &state.warning {
        eprintln!("{warning}");
    }
    for diagnostic in bottega_core::state::diagnostics(&state) {
        eprintln!("{}; {}", diagnostic.what, diagnostic.fix);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let outcome = runtime.block_on(async {
        match cli.command {
            Some(Action::Doctor { backend }) => {
                let report = check_environment(&CliRunner::new(), backend).await;
                println!("{}", serde_json::to_string_pretty(&report)?);
                if report.is_ready() {
                    Ok(())
                } else {
                    Err("one or more provider checks are not ready in this launch context".into())
                }
            }
            Some(Action::Tasks {
                status,
                since,
                limit,
                full_result,
            }) => {
                let tasks = tokio::task::spawn_blocking(move || {
                    tosi::registry::Registry::open(&state)?.list(tosi::registry::ListTasks {
                        status,
                        since,
                        limit,
                        full_result: Some(full_result),
                    })
                })
                .await??;
                println!("{}", serde_json::to_string_pretty(&tasks)?);
                Ok(())
            }
            None => run(state).await,
        }
    });
    // Worker cleanup is already awaited by run(). Tokio's stdin reader can
    // still be blocked when a signal stops the server with stdin left open.
    runtime.shutdown_timeout(Duration::from_millis(100));
    outcome
}

async fn run(state: bottega_core::state::State) -> Result<(), tower_mcp::BoxError> {
    if config::is_delegated_child() {
        return Err("tosi cannot start inside a delegated worker".into());
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_ansi(false)
        .init();
    let default_backend = match config::var("DEFAULT_BACKEND").as_deref() {
        Ok("claude") | Err(env::VarError::NotPresent) => Backend::Claude,
        Ok("codex") => Backend::Codex,
        _ => {
            return Err(
                "TOSI_DEFAULT_BACKEND (or legacy TASK_RUNNERS_DEFAULT_BACKEND) must be claude or codex"
                    .into(),
            );
        }
    };
    let max_concurrency: usize = match config::var("MAX_CONCURRENCY") {
        Ok(value) => value.parse().map_err(|_| {
            "TOSI_MAX_CONCURRENCY (or legacy TASK_RUNNERS_MAX_CONCURRENCY) must contain a valid number"
        })?,
        Err(env::VarError::NotPresent) => 4,
        Err(_) => {
            return Err(
                "TOSI_MAX_CONCURRENCY (or legacy TASK_RUNNERS_MAX_CONCURRENCY) must contain a valid number"
                    .into(),
            );
        }
    };
    if !(1..=32).contains(&max_concurrency) {
        return Err(
            "TOSI_MAX_CONCURRENCY (or legacy TASK_RUNNERS_MAX_CONCURRENCY) must be between 1 and 32"
                .into(),
        );
    }
    let runner = Arc::new(CliRunner::default());
    let registry_state = state.clone();
    let registry =
        tokio::task::spawn_blocking(move || tosi::registry::Registry::open(&registry_state))
            .await??;
    let manager = Manager::new(runner.clone(), default_backend, max_concurrency)
        .with_sessions(tosi::manager::Sessions::from_prepared(&state))
        .with_registry(registry)
        // Only the new name: a legacy TASK_RUNNERS_ alias for a security control would be a
        // switch nobody auditing the environment looks for (found in review).
        .with_redaction(std::env::var("TOSI_NO_REDACT").as_deref() != Ok("1"));
    let mut transport = StdioTransport::new(router(manager.clone(), runner));
    let handle = transport.handle();
    let run = transport.run();
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => {
            manager.shutdown().await;
            result?;
        }
        _ = handle.stopping() => {
            // EOF starts the transport's request drain. Cancel workers now so
            // pending wait_tasks calls finish instead of delaying cleanup.
            manager.shutdown().await;
            run.await?;
        }
        signal = shutdown_signal() => {
            let stop = handle.shutdown();
            manager.shutdown().await;
            let drained = run.await;
            signal?;
            stop?;
            drained?;
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    Ok(())
}

#[cfg(not(unix))]
async fn shutdown_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}
