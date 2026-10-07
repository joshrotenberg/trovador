mod common;

use common::*;
use serde_json::json;
use trovador::consent::{self, Decision, Level, Source};
use trovador::run::{self, AskOpts, AskResult, RunError};
use trovador::sessions::{self, Ctx};
use trovador::{Backend, Limits};

fn ask(ctx: &Ctx, opts: AskOpts) -> Result<AskResult, RunError> {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run::ask(ctx, &opts))
}

fn prompt(p: &str) -> AskOpts {
    AskOpts {
        prompt: p.into(),
        ..Default::default()
    }
}

fn has_arg(call: &[String], arg: &str) -> bool {
    call.iter().any(|l| l == &format!("arg={arg}"))
}

fn arg_after(call: &[String], flag: &str) -> Option<String> {
    let i = call.iter().position(|l| l == &format!("arg={flag}"))?;
    call.get(i + 1)?.strip_prefix("arg=").map(str::to_string)
}

// --- starting and continuing -----------------------------------------------

#[test]
fn a_new_claude_session_uses_the_id_trovador_chose_and_can_be_continued() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let r = ask(&ctx, prompt("hello")).unwrap();
    assert_eq!(r.result, "echo: hello");
    assert_eq!(r.verb, "started");
    assert!(!r.is_error && !r.timed_out);
    assert_eq!(r.cost_usd, Some(0.01));

    let id = r.session_id.clone().unwrap();
    let calls = f.calls("claude");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        arg_after(&calls[0], "--session-id").as_deref(),
        Some(id.as_str())
    );
    assert!(has_arg(&calls[0], "stream-json"));
    // The prompt is not on the command line: variadic flags would swallow it.
    assert!(!has_arg(&calls[0], "hello"));
    assert!(r.continuation.unwrap().allowed);

    let again = ask(
        &ctx,
        AskOpts {
            session: Some(id[..8].to_string()),
            ..prompt("again")
        },
    )
    .unwrap();
    assert_eq!(again.verb, "continued");
    assert_eq!(again.session_id.as_deref(), Some(id.as_str()));
    let calls = f.calls("claude");
    // A prefix is resolved; the provider only ever sees the canonical id.
    assert_eq!(
        arg_after(&calls[1], "--resume").as_deref(),
        Some(id.as_str())
    );
    assert!(!has_arg(&calls[1], "--session-id"));
}

#[test]
fn a_continuation_runs_where_the_session_lives() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let project = f.dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let r = ask(
        &ctx,
        AskOpts {
            cwd: Some(project.display().to_string()),
            ..prompt("start here")
        },
    )
    .unwrap();
    ask(
        &ctx,
        AskOpts {
            session: r.session_id.clone(),
            ..prompt("continue")
        },
    )
    .unwrap();
    let calls = f.calls("claude");
    let cwd_of = |c: &Vec<String>| c[0].strip_prefix("cwd=").unwrap().to_string();
    let canon = |p: &str| std::fs::canonicalize(p).unwrap();
    assert_eq!(
        canon(&cwd_of(&calls[1])),
        canon(&project.display().to_string())
    );
}

#[test]
fn options_reach_claude_as_flags() {
    let f = Fixture::new().with_fakes();
    let extra = f.dir.path().join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    ask(
        &f.ctx(),
        AskOpts {
            model: Some("opus".into()),
            effort: Some("high".into()),
            allowed_tools: vec!["Read".into(), "Bash(git:*)".into()],
            add_dirs: vec![extra.display().to_string()],
            max_turns: Some(30),
            permission_mode: Some("acceptEdits".into()),
            hermetic: true,
            ..prompt("go")
        },
    )
    .unwrap();
    let call = &f.calls("claude")[0];
    assert_eq!(arg_after(call, "--model").as_deref(), Some("opus"));
    assert_eq!(arg_after(call, "--effort").as_deref(), Some("high"));
    assert_eq!(arg_after(call, "--max-turns").as_deref(), Some("30"));
    assert_eq!(
        arg_after(call, "--permission-mode").as_deref(),
        Some("acceptEdits")
    );
    assert!(has_arg(call, "--strict-mcp-config"));
    assert!(call.iter().any(|l| l.contains("Bash(git:*)")));
    assert!(
        call.iter()
            .any(|l| l == &format!("arg={}", extra.display()))
    );
    // The child knows it is one level deeper.
    assert!(call.iter().any(|l| l == "depth=1"));
}

#[test]
fn a_fork_starts_a_new_spawned_session_from_an_old_one() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    // A session trovador did not start, which may not be continued but may be forked.
    f.claude("-w", ID_A, &[claude_user("/", "original")], 0);
    let r = ask(
        &ctx,
        AskOpts {
            session: Some(ID_A.into()),
            fork: true,
            ..prompt("branch off")
        },
    )
    .unwrap();
    assert_eq!(r.verb, "forked");
    assert_eq!(r.session_id.as_deref(), Some(FORK_ID));
    let call = &f.calls("claude")[0];
    assert_eq!(arg_after(call, "--resume").as_deref(), Some(ID_A));
    assert!(has_arg(call, "--fork-session"));
    assert!(consent::check(&ctx.paths, Backend::Claude, FORK_ID).spawned);
    assert!(!consent::check(&ctx.paths, Backend::Claude, ID_A).allowed);
}

#[test]
fn a_new_codex_session_is_recorded_from_its_first_event_and_unsupported_options_are_reported() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let r = ask(
        &ctx,
        AskOpts {
            backend: Some(Backend::Codex),
            max_turns: Some(30),
            allowed_tools: vec!["Read".into()],
            effort: Some("high".into()),
            ..prompt("hi codex")
        },
    )
    .unwrap();
    assert_eq!(r.session_id.as_deref(), Some(CODEX_NEW_ID));
    assert_eq!(r.result, "echo: hi codex");
    assert_eq!(r.usage.unwrap()["output_tokens"], 2);
    assert_eq!(r.warnings.len(), 2);
    assert!(r.warnings.iter().any(|w| w.starts_with("max_turns")));
    assert!(consent::check(&ctx.paths, Backend::Codex, CODEX_NEW_ID).spawned);
    let call = &f.calls("codex")[0];
    assert_eq!(arg_after(call, "--sandbox").as_deref(), Some("read-only"));
    assert!(
        call.iter()
            .any(|l| l.contains("model_reasoning_effort=\"high\""))
    );

    let again = ask(
        &ctx,
        AskOpts {
            session: Some(CODEX_NEW_ID.into()),
            ..prompt("more")
        },
    )
    .unwrap();
    assert_eq!(again.verb, "continued");
    let call = &f.calls("codex")[1];
    assert!(has_arg(call, "resume"));
    assert!(has_arg(call, CODEX_NEW_ID));
}

// --- spike#107 and spike#108 -----------------------------------------------

#[test]
fn a_claude_run_killed_by_timeout_returns_its_id_and_can_be_continued_without_approval() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let r = ask(
        &ctx,
        AskOpts {
            timeout_s: Some(1),
            ..prompt("slow work")
        },
    )
    .unwrap();
    assert!(r.timed_out && r.is_error);
    assert!(r.error.unwrap().contains("timeout"));
    let id = r
        .session_id
        .expect("a timed-out run still names its session");
    assert!(r.continuation.unwrap().allowed);

    let again = ask(
        &ctx,
        AskOpts {
            session: Some(id.clone()),
            ..prompt("finish up")
        },
    )
    .unwrap();
    assert!(!again.is_error);
    assert_eq!(again.result, "echo: finish up");
}

#[test]
fn a_codex_run_killed_by_timeout_returns_its_id_and_can_be_continued_without_approval() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let r = ask(
        &ctx,
        AskOpts {
            backend: Some(Backend::Codex),
            timeout_s: Some(1),
            ..prompt("slow work")
        },
    )
    .unwrap();
    assert!(r.timed_out);
    assert_eq!(r.session_id.as_deref(), Some(CODEX_NEW_ID));
    let again = ask(
        &ctx,
        AskOpts {
            session: Some(CODEX_NEW_ID.into()),
            ..prompt("finish up")
        },
    )
    .unwrap();
    assert_eq!(again.result, "echo: finish up");
}

#[test]
fn codex_gets_a_longer_default_timeout_than_claude() {
    let l = Limits::default();
    assert!(l.codex_timeout_s > l.claude_timeout_s);
    assert!(
        l.claude_timeout_s >= 600,
        "spike's 120 s default killed real turns"
    );
}

#[test]
fn a_provider_error_is_a_result_with_the_session_not_a_failure() {
    let f = Fixture::new().with_fakes();
    let r = ask(&f.ctx(), prompt("please fail")).unwrap();
    assert!(r.is_error && !r.timed_out);
    assert_eq!(r.error.as_deref(), Some("it failed"));
    assert!(r.session_id.is_some());
}

// --- consent ---------------------------------------------------------------

#[test]
fn a_session_trovador_did_not_start_needs_a_person_to_approve_it() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    f.claude("-w", ID_A, &[claude_user("/", "someone else's")], 0);
    let cont = || {
        ask(
            &ctx,
            AskOpts {
                session: Some("aaaa".into()),
                ..prompt("hi")
            },
        )
    };
    match cont() {
        Err(RunError::Denied(check)) => {
            assert_eq!(check.level, Level::Spawned);
            assert!(
                check
                    .next_steps
                    .iter()
                    .any(|s| s.contains("trovador consent approve"))
            );
        }
        other => panic!("expected a denial, got {other:?}"),
    }
    assert!(
        f.calls("claude").is_empty(),
        "nothing may start before consent"
    );

    consent::set_decision(&ctx.paths, Backend::Claude, ID_A, Some(Decision::Approved)).unwrap();
    assert!(
        matches!(cont(), Err(RunError::Denied(_))),
        "approval needs level approved"
    );
    consent::set_level(&ctx.paths, Level::Approved).unwrap();
    assert!(cont().is_ok());

    // An explicit denial wins over every level, and a prefix cannot get around it.
    consent::set_decision(&ctx.paths, Backend::Claude, ID_A, Some(Decision::Denied)).unwrap();
    consent::set_level(&ctx.paths, Level::All).unwrap();
    assert!(matches!(cont(), Err(RunError::Denied(_))));
    consent::set_decision(&ctx.paths, Backend::Claude, ID_A, None).unwrap();
    assert!(cont().is_ok());
}

#[test]
fn invalid_consent_state_denies_everything_and_is_never_overwritten() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let r = ask(&ctx, prompt("mine")).unwrap();
    let path = consent::path(&ctx.paths);
    std::fs::write(&path, "{ not json").unwrap();

    let check = consent::check(
        &ctx.paths,
        Backend::Claude,
        r.session_id.as_deref().unwrap(),
    );
    assert!(!check.allowed);
    assert_eq!(check.source, Source::Invalid);
    assert!(consent::set_level(&ctx.paths, Level::All).is_err());
    assert!(consent::record_spawned(&ctx.paths, Backend::Claude, ID_B).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    assert!(matches!(
        ask(
            &ctx,
            AskOpts {
                session: r.session_id,
                ..prompt("again")
            }
        ),
        Err(RunError::Denied(_))
    ));
}

#[cfg(unix)]
#[test]
fn the_consent_file_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    ask(&ctx, prompt("x")).unwrap();
    let mode = std::fs::metadata(consent::path(&ctx.paths))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn show_reports_whether_a_session_may_be_continued() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let r = ask(&ctx, prompt("x")).unwrap();
    let mine = sessions::show(&ctx, r.session_id.as_deref().unwrap(), None).unwrap();
    assert!(mine.consent.allowed);
    f.claude("-w", ID_A, &[claude_user("/", "theirs")], 0);
    let theirs = sessions::show(&ctx, ID_A, None).unwrap();
    assert!(!theirs.consent.allowed);
}

// --- refusals before anything starts ---------------------------------------

#[test]
fn an_oversized_transcript_is_refused_before_spawning() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    consent::set_level(&ctx.paths, Level::All).unwrap();
    f.claude("-w", ID_A, &[claude_user("/", &"z".repeat(950_000))], 0);
    let r = ask(
        &ctx,
        AskOpts {
            session: Some(ID_A.into()),
            ..prompt("hi")
        },
    );
    assert!(matches!(r, Err(RunError::NotResumable { .. })));
    assert!(f.calls("claude").is_empty());
}

#[test]
fn the_depth_limit_refuses_a_run() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx_with(Limits {
        max_depth: 0,
        ..Default::default()
    });
    assert!(matches!(
        ask(&ctx, prompt("x")),
        Err(RunError::Depth { .. })
    ));
    assert!(f.calls("claude").is_empty());
}

#[test]
fn unsafe_or_unknown_options_are_refused_before_anything_is_recorded() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    for opts in [
        AskOpts {
            permission_mode: Some("bypassPermissions".into()),
            ..prompt("x")
        },
        AskOpts {
            effort: Some("extreme".into()),
            ..prompt("x")
        },
        AskOpts {
            backend: Some(Backend::Codex),
            sandbox: Some("danger-full-access".into()),
            ..prompt("x")
        },
        AskOpts {
            fork: true,
            ..prompt("x")
        },
        prompt("   "),
    ] {
        assert!(matches!(ask(&ctx, opts), Err(RunError::Invalid(_))));
    }
    assert!(f.calls("claude").is_empty() && f.calls("codex").is_empty());
    assert_eq!(consent::status(&ctx.paths).spawned, 0);
}

#[test]
fn every_run_is_logged_at_start_and_end() {
    let f = Fixture::new().with_fakes();
    let ctx = f.ctx();
    let r = ask(&ctx, prompt("x")).unwrap();
    let log = std::fs::read_to_string(ctx.paths.home.join("runs.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["phase"], "start");
    assert_eq!(lines[1]["phase"], "end");
    assert_eq!(lines[1]["outcome"], "ok");
    assert_eq!(lines[1]["session"], json!(r.session_id));
}
