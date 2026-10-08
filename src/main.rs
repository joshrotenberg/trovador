use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use trovador::consent::{self, Decision, Level};
use trovador::index::{Order, SearchOpts};
use trovador::run::{self, AskOpts};
use trovador::sessions::{self, Ctx, ListOpts};
use trovador::summary;
use trovador::{Backend, archive};

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
    /// Run one turn: start a session, or continue or fork one. Costs model tokens.
    Ask {
        /// The prompt, or - to read it from stdin.
        prompt: String,
        #[arg(long)]
        backend: Option<Backend>,
        /// Continue this session (id or unambiguous prefix).
        #[arg(long)]
        session: Option<String>,
        /// With --session, resume its context into a new session (claude only).
        #[arg(long, requires = "session")]
        fork: bool,
        /// Consult about this session: summarize it and ask a new session, on the other
        /// backend unless --backend says otherwise.
        #[arg(long, conflicts_with_all = ["session", "fork"])]
        about: Option<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long = "add-dir")]
        add_dirs: Vec<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
        #[arg(long = "allowed-tool")]
        allowed_tools: Vec<String>,
        #[arg(long = "disallowed-tool")]
        disallowed_tools: Vec<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        append_system_prompt: Option<String>,
        #[arg(long)]
        max_turns: Option<u32>,
        #[arg(long)]
        timeout_s: Option<u64>,
        #[arg(long)]
        hermetic: bool,
        /// codex sandbox: read-only (default) or workspace-write.
        #[arg(long)]
        sandbox: Option<String>,
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
    },
    /// Import consent and summaries from spike (agent-sessions). Reads, never writes, the
    /// spike directory.
    Import {
        /// spike's state directory.
        #[arg(long)]
        from: Option<std::path::PathBuf>,
        /// Report what would be imported without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Which existing sessions trovador may continue. Only this command changes it.
    Consent {
        #[command(subcommand)]
        action: ConsentCmd,
    },
    /// Report paths, limits, the index, and what the archives contain.
    Doctor,
}

#[derive(Subcommand)]
enum ConsentCmd {
    /// The current level and counts.
    Status,
    /// Whether one session may be continued, and why.
    Check {
        id: String,
        #[arg(long)]
        backend: Option<Backend>,
    },
    /// Allow continuing a session trovador did not start (needs level approved).
    Approve {
        id: String,
        #[arg(long)]
        backend: Option<Backend>,
    },
    /// Never continue this session, whatever the level.
    Deny {
        id: String,
        #[arg(long)]
        backend: Option<Backend>,
    },
    /// Remove an approval or denial.
    Clear {
        id: String,
        #[arg(long)]
        backend: Option<Backend>,
    },
    /// Set the level: deny, spawned, approved, or all.
    Policy { level: Level },
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
    let ctx = Ctx::from_env();
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
        Command::Ask {
            prompt,
            backend,
            session,
            fork,
            about,
            cwd,
            add_dirs,
            model,
            effort,
            allowed_tools,
            disallowed_tools,
            permission_mode,
            append_system_prompt,
            max_turns,
            timeout_s,
            hermetic,
            sandbox,
        } => {
            let prompt = if prompt == "-" {
                let mut s = String::new();
                if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s) {
                    return fail(e);
                }
                s
            } else {
                prompt
            };
            let opts = AskOpts {
                prompt,
                backend,
                session,
                fork,
                about,
                cwd,
                add_dirs,
                model,
                effort,
                allowed_tools,
                disallowed_tools,
                permission_mode,
                append_system_prompt,
                max_turns,
                timeout_s,
                hermetic,
                sandbox,
            };
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            match runtime.block_on(run::ask(&ctx, &opts)) {
                Ok(r) => print(&r),
                Err(e) => fail(e),
            }
        }
        Command::Consent { action } => consent_cmd(&ctx, action),
        Command::Summarize {
            id,
            backend,
            force,
            model,
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
                Ok(s) => print(&s),
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

/// Consent decisions are keyed by the canonical archive id, so a prefix given here is
/// resolved first, the same way `ask` resolves it.
fn resolve(ctx: &Ctx, id: &str, backend: Option<Backend>) -> Result<(Backend, String), String> {
    archive::find_session(&ctx.paths, id, backend)
        .map(|e| (e.backend, e.session_id))
        .map_err(|e| e.to_string())
}

fn consent_cmd(ctx: &Ctx, action: ConsentCmd) -> ExitCode {
    let decide = |id: &str, backend, d| match resolve(ctx, id, backend) {
        Ok((b, id)) => match consent::set_decision(&ctx.paths, b, &id, d) {
            Ok(c) => print(&c),
            Err(e) => fail(e),
        },
        Err(e) => fail(e),
    };
    match action {
        ConsentCmd::Status => print(&consent::status(&ctx.paths)),
        ConsentCmd::Check { id, backend } => match resolve(ctx, &id, backend) {
            Ok((b, id)) => print(&consent::check(&ctx.paths, b, &id)),
            Err(e) => fail(e),
        },
        ConsentCmd::Approve { id, backend } => decide(&id, backend, Some(Decision::Approved)),
        ConsentCmd::Deny { id, backend } => decide(&id, backend, Some(Decision::Denied)),
        ConsentCmd::Clear { id, backend } => decide(&id, backend, None),
        ConsentCmd::Policy { level } => match consent::set_level(&ctx.paths, level) {
            Ok(s) => print(&s),
            Err(e) => fail(e),
        },
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
