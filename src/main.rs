use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use trovador::Provider;
use trovador::index::{Order, SearchOpts};
use trovador::sessions::{self, Ctx, ListOpts};

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
    Serve,
    /// List, inspect, and read sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionsCmd,
    },
    /// Full-text search across every session. Brings the index up to date first, within
    /// TROVADOR_INDEX_BUDGET_MS.
    Search {
        /// Words and "quoted phrases", all required in the same record.
        query: String,
        /// Treat the query as an FTS5 expression.
        #[arg(long, conflicts_with = "regex")]
        fts: bool,
        /// Treat the query as a case-insensitive regular expression.
        #[arg(long)]
        regex: bool,
        #[arg(long)]
        provider: Option<Provider>,
        /// Only sessions whose project path contains this text (ASCII case-insensitive).
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        origin: Option<String>,
        /// Include sessions started by another session; =false hides them. Default:
        /// TROVADOR_INCLUDE_SUBAGENTS, hidden unless set.
        #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
        include_subagents: Option<bool>,
        /// Include sessions written in the last few minutes (TROVADOR_ACTIVE_WINDOW_S);
        /// =false hides them. Default: TROVADOR_INCLUDE_ACTIVE, hidden unless set.
        #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
        include_active: Option<bool>,
        /// Include claude -p and codex exec sessions; =false hides them. Default:
        /// TROVADOR_INCLUDE_AUTOMATED, hidden unless set.
        #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
        include_automated: Option<bool>,
        /// Leave out this session id.
        #[arg(long)]
        exclude_session: Option<String>,
        /// Only sessions last written at or after this: 2026-10-01, an RFC 3339 time (Z or numeric offset), or 2d.
        #[arg(long)]
        since: Option<String>,
        /// Only sessions last written before this, in the same forms.
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        /// Matching sessions to skip, from `next_offset` of the previous page.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// true: only Codex sessions from the archived root. false: only sessions not from it.
        #[arg(long)]
        archived: Option<bool>,
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
    /// Install the plugin (MCP server, recall skill, SessionStart hook) into Claude Code and
    /// Codex: writes a local marketplace under TROVADOR_HOME/plugin and registers it with
    /// each host's own plugin commands. Edits no host configuration file.
    Init {
        /// Report the files and host commands without writing or running anything that changes state.
        #[arg(long)]
        dry_run: bool,
        /// Which hosts to install into. Under both, a host that is not on PATH is skipped.
        #[arg(long, value_enum, default_value = "both")]
        host: InitHost,
        /// The trovador binary the plugin runs. Defaults to this one.
        #[arg(long)]
        binary: Option<std::path::PathBuf>,
        /// The claude executable, when it is not the one on PATH.
        #[arg(long)]
        claude_bin: Option<std::path::PathBuf>,
        /// The codex executable, when it is not the one on PATH.
        #[arg(long)]
        codex_bin: Option<std::path::PathBuf>,
    },
    /// Report paths, limits, the index, and what the archives contain.
    Doctor,
    /// Handlers for Claude Code and Codex hooks, run by a host's hook configuration rather than by hand.
    Hook {
        #[command(subcommand)]
        event: HookCmd,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum InitHost {
    Both,
    Claude,
    Codex,
}

#[derive(Subcommand)]
enum HookCmd {
    /// Reads a SessionStart payload on stdin and prints context naming this session's id,
    /// so search can leave out exactly this session. Prints nothing for any other input
    /// and always exits 0, so it can never block a session from starting.
    SessionStart {
        /// The host: claude or codex. Inferred from the transcript path when omitted.
        // `--backend` was this flag's name before 0.1. Hook command lines live in host
        // configuration trovador does not rewrite (~/.codex/hooks.json, an older plugin
        // install), and an unknown flag there fails under `|| true`, silently dropping
        // the session's self-exclusion, so the old name stays accepted.
        #[arg(long, alias = "backend")]
        provider: Option<Provider>,
    },
}

#[derive(Subcommand)]
enum SessionsCmd {
    /// Sessions newest first.
    List {
        #[arg(long)]
        provider: Option<Provider>,
        /// Only sessions whose project path contains this text (ASCII case-insensitive).
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        limit: Option<usize>,
        /// Matching sessions to skip, from `next_offset` of the previous page.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// true: only Codex sessions from the archived root. false: only sessions not from it.
        #[arg(long)]
        archived: Option<bool>,
        /// Only sessions with this origin (cli, claude-desktop, sdk-cli, exec, vscode).
        #[arg(long)]
        origin: Option<String>,
        /// Include sessions started by another session; =false hides them. Default:
        /// TROVADOR_INCLUDE_SUBAGENTS, hidden unless set.
        #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
        include_subagents: Option<bool>,
        /// Include claude -p and codex exec sessions; =false hides them. Default:
        /// TROVADOR_INCLUDE_AUTOMATED, hidden unless set.
        #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
        include_automated: Option<bool>,
        /// Only sessions last written at or after this: 2026-10-01, an RFC 3339 time (Z or numeric offset), or 2d.
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
        provider: Option<Provider>,
    },
    /// One page of a session's projected transcript.
    Transcript {
        id: String,
        #[arg(long)]
        provider: Option<Provider>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        limit: Option<usize>,
        /// The `revision` the previous page reported. A session that has only grown is read as
        /// it was at that revision; anything else fails.
        #[arg(long)]
        expected_revision: Option<String>,
    },
    /// The whole session as plain text, in parts of TROVADOR_RESPONSE_MAX_CHARS, for
    /// reading or summarizing.
    Text {
        id: String,
        #[arg(long)]
        provider: Option<Provider>,
        /// The character to start from, for the next part (truncated.next_offset).
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// The `revision` the previous part reported. A session that has only grown is read as
        /// it was at that revision; anything else fails.
        #[arg(long)]
        expected_revision: Option<String>,
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

/// Progress lines as a person reads them: the message alone, no time, level, or target.
struct Plain;

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for Plain
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut w: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        write!(w, "trovador: ")?;
        ctx.field_format().format_fields(w.by_ref(), event)?;
        writeln!(w)
    }
}

/// Whether progress lines are printed: TROVADOR_PROGRESS when it is a recognized boolean,
/// otherwise when stderr is a terminal. Never under serve, whose stderr is a host's log.
fn progress_on(serving: bool) -> bool {
    if serving {
        return false;
    }
    match std::env::var("TROVADOR_PROGRESS")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => std::io::IsTerminal::is_terminal(&std::io::stderr()),
    }
}

/// Two outputs on stderr, never stdout, which belongs to JSON: the MCP stream under serve,
/// the result everywhere else. Diagnostics follow TROVADOR_LOG (default warn). Progress
/// (the `trovador::progress` target) gets its own plain layer when it is on, and is then
/// kept out of the diagnostic layer so TROVADOR_LOG=info at a terminal does not print each
/// line twice.
fn init_tracing(serving: bool) {
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::{FilterExt, filter_fn};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let progress = progress_on(serving);
    let env = tracing_subscriber::EnvFilter::try_from_env("TROVADOR_LOG")
        .unwrap_or_else(|_| "warn".into());
    let diagnostics = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_filter(env.and(filter_fn(move |m| {
            !(progress && m.target() == trovador::PROGRESS)
        })));
    let plain = progress.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .event_format(Plain)
            .with_filter(filter_fn(|m| {
                m.target() == trovador::PROGRESS && *m.level() <= tracing::Level::INFO
            }))
    });
    tracing_subscriber::registry()
        .with(diagnostics)
        .with(plain)
        .init();
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(matches!(cli.command, Command::Serve));
    let _ = REDACT.set(!cli.no_redact);
    let mut ctx = Ctx::from_env();
    ctx.limits.redact = !cli.no_redact;
    match cli.command {
        Command::Serve => serve(
            ctx,
            trovador::mcp::ServeOpts {
                redact: !cli.no_redact,
            },
        ),
        Command::Sessions { action } => match action {
            SessionsCmd::List {
                provider,
                project,
                limit,
                offset,
                archived,
                origin,
                include_subagents,
                include_automated,
                since,
                until,
            } => match sessions::list(
                &ctx,
                &ListOpts {
                    provider,
                    project,
                    limit,
                    offset,
                    archived,
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
            SessionsCmd::Show { id, provider } => match sessions::show(&ctx, &id, provider) {
                Ok(d) => print(&d),
                Err(e) => fail(e),
            },
            SessionsCmd::Transcript {
                id,
                provider,
                offset,
                limit,
                expected_revision,
            } => match sessions::transcript_at(
                &ctx,
                &id,
                provider,
                offset,
                limit,
                expected_revision.as_deref(),
            ) {
                Ok(t) => print(&t),
                Err(e) => fail(e),
            },
            SessionsCmd::Text {
                id,
                provider,
                offset,
                expected_revision,
            } => {
                match sessions::text_at(&ctx, &id, provider, offset, expected_revision.as_deref()) {
                    Ok(t) => print(&t),
                    Err(e) => fail(e),
                }
            }
        },
        Command::Search {
            query,
            fts,
            regex,
            provider,
            project,
            origin,
            include_subagents,
            include_active,
            include_automated,
            exclude_session,
            since,
            until,
            limit,
            offset,
            archived,
            per_session,
            recent,
        } => {
            let opts = SearchOpts {
                query,
                fts,
                regex,
                provider,
                project,
                origin,
                include_subagents,
                include_active,
                include_automated,
                exclude_session,
                since,
                until,
                limit,
                offset,
                archived,
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
        Command::Hook {
            event: HookCmd::SessionStart { provider },
        } => {
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
            if let Some(out) = trovador::hook::session_start(&input, provider) {
                println!("{out}");
            }
            ExitCode::SUCCESS
        }
        Command::Init {
            dry_run,
            host,
            binary,
            claude_bin,
            codex_bin,
        } => {
            let opts = trovador::init::InitOpts {
                dry_run,
                host: match host {
                    InitHost::Both => trovador::init::Host::Both,
                    InitHost::Claude => trovador::init::Host::Claude,
                    InitHost::Codex => trovador::init::Host::Codex,
                },
                binary,
                claude_bin,
                codex_bin,
            };
            let report = trovador::init::init(&ctx.paths, &opts);
            let code = print(&report);
            if report.ok { code } else { ExitCode::FAILURE }
        }
        Command::Doctor => {
            let report = trovador::doctor::report(&ctx);
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
    tracing::debug!(
        version = env!("CARGO_PKG_VERSION"),
        "serving MCP over stdio"
    );
    match runtime.block_on(tower_mcp::StdioTransport::new(router).run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}
