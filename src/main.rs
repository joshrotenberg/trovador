use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
use trovador::consent::{self, Decision, Level};
use trovador::index::{Order, SearchOpts};
use trovador::run::{self, AskOpts};
use trovador::sessions::{self, Ctx, ListOpts};
use trovador::{Backend, archive};

/// Local Claude Code and Codex sessions as one MCP server and CLI.
#[derive(Parser)]
#[command(name = "trovador", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

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

fn print(value: &impl Serialize) -> ExitCode {
    match serde_json::to_string_pretty(value) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn fail(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("trovador: {e}");
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
    let ctx = Ctx::from_env();
    match cli.command {
        Command::Serve => serve(ctx),
        Command::Sessions { action } => match action {
            SessionsCmd::List {
                backend,
                project,
                limit,
                origin,
                include_subagents,
            } => print(&sessions::list(
                &ctx,
                &ListOpts {
                    backend,
                    project,
                    limit,
                    origin,
                    include_subagents,
                },
            )),
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
        Command::Doctor => {
            let claude = archive::scan_claude(&ctx.paths.claude_archive);
            let codex = archive::scan_codex(&ctx.paths.codex_archive, false);
            let archived = archive::scan_codex(&ctx.paths.codex_archived, true);
            print(&json!({
                "version": env!("CARGO_PKG_VERSION"),
                "paths": {
                    "claude_archive": ctx.paths.claude_archive,
                    "codex_archive": ctx.paths.codex_archive,
                    "codex_archived": ctx.paths.codex_archived,
                    "home": ctx.paths.home,
                },
                "sessions": {
                    "claude": claude.len(),
                    "codex": codex.len(),
                    "codex_archived": archived.len(),
                },
                "index": sessions::index_status(&ctx),
                "consent": consent::status(&ctx.paths),
                "limits": ctx.limits,
            }))
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

fn serve(ctx: Ctx) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let router = trovador::mcp::router(ctx);
    match runtime.block_on(tower_mcp::StdioTransport::new(router).run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}
