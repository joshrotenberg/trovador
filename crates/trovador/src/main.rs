use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use trovador::Backend;
use trovador::index::{Order, SearchOpts};
use trovador::sessions::{self, Ctx, ListOpts};
use trovador::summary;

/// Local Claude Code and Codex sessions as one MCP server and CLI.
#[derive(Parser)]
#[command(name = "trovador", version)]
struct Cli {
    /// Show secrets in output instead of [redacted:<kind>]. For serve, this is the only way
    /// to turn redaction off; no tool argument can.
    #[arg(long, global = true)]
    no_redact: bool,
    #[command(subcommand)]
    command: Command,
}

// Parsed once per process; boxing the large variants would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Run the MCP server over stdio.
    Serve {
        /// Offer only sessions and search: nothing that runs a model.
        #[arg(long)]
        read_only: bool,
    },
    /// List, inspect, and read sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionsCmd,
    },
    /// Full-text search across every session. Brings the index up to date first, within
    /// TROVADOR_INDEX_BUDGET_MS.
    Search {
        /// Words and "quoted phrases", all required.
        query: String,
        /// Treat the query as an FTS5 expression.
        #[arg(long, conflicts_with = "regex")]
        fts: bool,
        /// Treat the query as a case-insensitive regular expression.
        #[arg(long)]
        regex: bool,
        #[arg(long)]
        backend: Option<Backend>,
        /// Only sessions whose project path contains this text.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        origin: Option<String>,
        #[arg(long)]
        include_subagents: bool,
        /// Include sessions written in the last few minutes (TROVADOR_ACTIVE_WINDOW_S).
        #[arg(long)]
        include_active: bool,
        /// Include claude -p and codex exec sessions trovador did not start.
        #[arg(long)]
        include_automated: bool,
        /// Leave out this session id.
        #[arg(long)]
        exclude_session: Option<String>,
        /// Only sessions last written at or after this: 2026-10-01, an RFC 3339 time, or 2d.
        #[arg(long)]
        since: Option<String>,
        /// Only sessions last written before this, in the same forms.
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        /// Matching records shown per session.
        #[arg(long)]
        per_session: Option<usize>,
        /// Order sessions by most recent instead of best match.
        #[arg(long)]
        recent: bool,
    },
    /// Bring the search index fully up to date, with no time budget.
    Index {
        /// Discard the index and build it again.
        #[arg(long)]
        rebuild: bool,
    },
    /// Summarize a session: cached until the session changes. Costs tokens on a miss.
    Summarize {
        id: String,
        #[arg(long)]
        backend: Option<Backend>,
        /// Make a new summary even if a current one is cached.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        model: Option<String>,
        /// The character to start from, for the next part of a summary longer than the
        /// response budget.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Import spawned-session records and summaries from spike (agent-sessions). Reads, never writes, the
    /// spike directory.
    Import {
        /// spike's state directory.
        #[arg(long)]
        from: Option<std::path::PathBuf>,
        /// Report what would be imported without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove cached summaries and run-log lines that will never be used again. A dry run
    /// unless --apply. The record of sessions the toolkit started is never touched.
    Gc {
        /// Also remove summaries made, and run-log lines written, longer ago than this.
        #[arg(long, default_value = "90d")]
        older_than: String,
        /// Remove; without this, only report.
        #[arg(long)]
        apply: bool,
    },
    /// Report paths, limits, the index, and what the archives contain.
    Doctor,
    /// Handlers for Claude Code and Codex hooks, run by a plugin rather than by hand.
    Hook {
        #[command(subcommand)]
        event: HookCmd,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    /// Reads a SessionStart payload on stdin and prints context naming this session's id,
    /// so search can leave out exactly this session. Prints nothing for any other input
    /// and always exits 0, so it can never block a session from starting.
    SessionStart {
        /// The host: claude or codex. Inferred from the transcript path when omitted.
        #[arg(long)]
        backend: Option<Backend>,
    },
}

#[derive(Subcommand)]
enum SessionsCmd {
    /// Sessions newest first.
    List {
        #[arg(long)]
        backend: Option<Backend>,
        /// Only sessions whose project path contains this text.
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        /// Only sessions with this origin (cli, claude-desktop, sdk-cli, exec, vscode).
        #[arg(long)]
        origin: Option<String>,
        /// Include sessions started by another session.
        #[arg(long)]
        include_subagents: bool,
        /// Include claude -p and codex exec sessions trovador did not start.
        #[arg(long)]
        include_automated: bool,
        /// Only sessions last written at or after this: 2026-10-01, an RFC 3339 time, or 2d.
        #[arg(long)]
        since: Option<String>,
        /// Only sessions last written before this, in the same forms.
        #[arg(long)]
        until: Option<String>,
    },
    /// One session's metadata, resumability, and resume command.
    Show {
        /// Session id or unambiguous prefix.
        id: String,
        #[arg(long)]
        backend: Option<Backend>,
    },
    /// One page of a session's projected transcript.
    Transcript {
        id: String,
        #[arg(long)]
        backend: Option<Backend>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        limit: Option<usize>,
    },
}

/// Whether output is redacted, set once from the command line before anything prints.
static REDACT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn print(value: &impl Serialize) -> ExitCode {
    let mut v = match serde_json::to_value(value) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    if *REDACT.get().unwrap_or(&true) {
        trovador::redact::redact_value(&mut v);
    }
    match serde_json::to_string_pretty(&v) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    let msg = e.to_string();
    let msg = if *REDACT.get().unwrap_or(&true) {
        trovador::redact::redact_str(&msg).into_owned()
    } else {
        msg
    };
    eprintln!("trovador: {msg}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    // stdout belongs to JSON: the MCP stream under serve, the result everywhere else.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("TROVADOR_LOG")
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let _ = REDACT.set(!cli.no_redact);
    let mut ctx = Ctx::from_env();
    ctx.limits.redact = !cli.no_redact;
    match cli.command {
        Command::Serve { read_only } => serve(
            ctx,
            trovador::mcp::ServeOpts {
                read_only,
                redact: !cli.no_redact,
            },
        ),
        Command::Sessions { action } => match action {
            SessionsCmd::List {
                backend,
                project,
                limit,
                origin,
                include_subagents,
                include_automated,
                since,
                until,
            } => match sessions::list(
                &ctx,
                &ListOpts {
                    backend,
                    project,
                    limit,
                    origin,
                    include_subagents,
                    include_automated,
                    since,
                    until,
                },
            ) {
                Ok(r) => print(&r),
                Err(e) => fail(e),
            },
            SessionsCmd::Show { id, backend } => match sessions::show(&ctx, &id, backend) {
                Ok(d) => print(&d),
                Err(e) => fail(e),
            },
            SessionsCmd::Transcript {
                id,
                backend,
                offset,
                limit,
            } => match sessions::transcript(&ctx, &id, backend, offset, limit) {
                Ok(t) => print(&t),
                Err(e) => fail(e),
            },
        },
        Command::Search {
            query,
            fts,
            regex,
            backend,
            project,
            origin,
            include_subagents,
            include_active,
            include_automated,
            exclude_session,
            since,
            until,
            limit,
            per_session,
            recent,
        } => {
            let opts = SearchOpts {
                query,
                fts,
                regex,
                backend,
                project,
                origin,
                include_subagents,
                include_active,
                include_automated,
                exclude_session,
                since,
                until,
                limit,
                per_session,
                order: if recent {
                    Order::Recent
                } else {
                    Order::Relevance
                },
            };
            match sessions::search(&ctx, &opts) {
                Ok(r) => print(&r),
                Err(e) => fail(e),
            }
        }
        Command::Index { rebuild } => match sessions::reindex(&ctx, rebuild) {
            Ok(r) => print(&json!({ "refresh": r, "index": sessions::index_status(&ctx) })),
            Err(e) => fail(e),
        },
        Command::Gc { older_than, apply } => match humantime::parse_duration(&older_than) {
            Ok(d) => match trovador::gc::gc(&ctx, d, apply) {
                Ok(r) => print(&r),
                Err(e) => fail(e),
            },
            Err(_) => fail(format!(
                "--older-than \"{older_than}\" is not a duration such as 90d or 12w"
            )),
        },
        Command::Summarize {
            id,
            backend,
            force,
            model,
            offset,
        } => {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            match runtime.block_on(summary::summarize(
                &ctx,
                &id,
                backend,
                force,
                model.as_deref(),
            )) {
                Ok(s) => print(&summary::part(s, offset, &ctx.limits)),
                Err(e) => fail(e),
            }
        }
        Command::Import { from, dry_run } => {
            let from = from.unwrap_or_else(trovador::import::default_from);
            match trovador::import::import(&ctx, &from, dry_run) {
                Ok(r) => print(&r),
                Err(e) => fail(e),
            }
        }
        Command::Hook {
            event: HookCmd::SessionStart { backend },
        } => {
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
            if let Some(out) = trovador::hook::session_start(&input, backend) {
                println!("{out}");
            }
            ExitCode::SUCCESS
        }
        Command::Doctor => {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            let report = runtime.block_on(trovador::doctor::report(&ctx));
            let code = print(&report);
            if report.ok { code } else { ExitCode::FAILURE }
        }
    }
}

fn serve(ctx: Ctx, opts: trovador::mcp::ServeOpts) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let router = trovador::mcp::router_with(ctx, opts);
    match runtime.block_on(tower_mcp::StdioTransport::new(router).run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}
