mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::*;
use serde::Serialize;
use serde_json::{Value, json};
use trovador::archive;
use trovador::index::{Index, RefreshOpts, SearchOpts};
use trovador::sessions::{self, ListOpts};
use trovador::transcript::Projection;
use trovador::{Backend, Ctx, Limits};

fn records() -> Vec<Value> {
    vec![
        codex_meta("/work/zstd", json!("cli")),
        codex_user_item("find the compressed history"),
        codex_agent_item("zstdonlyneedle lives here"),
        codex_user_item("and the next turn"),
        codex_agent_item("the next answer"),
    ]
}

fn jsonl_len(records: &[Value]) -> u64 {
    records.iter().map(|r| r.to_string().len() as u64 + 1).sum()
}

fn set_mtime(path: &Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

fn refresh(ctx: &Ctx) -> trovador::index::RefreshReport {
    Index::open(&ctx.paths, &ctx.limits)
        .unwrap()
        .refresh(&ctx.paths, &ctx.limits, &RefreshOpts::default())
        .unwrap()
}

fn page(ctx: &Ctx, offset: usize, limit: usize) -> Value {
    let mut page =
        serde_json::to_value(sessions::transcript(ctx, ID_A, None, offset, Some(limit)).unwrap())
            .unwrap();
    // The revision names the stored file, which differs between a plain and a compressed
    // rollout with the same records (tests/revision.rs checks that it does).
    page.as_object_mut().unwrap().remove("revision");
    page
}

fn logical_output(value: impl Serialize) -> Value {
    let mut value = serde_json::to_value(value).unwrap();
    let object = value.as_object_mut().unwrap();
    // Storage facts differ even when all session content and addressing agree.
    for key in ["file", "size_bytes", "size_kb"] {
        object.remove(key);
    }
    value
}

fn indexed(ctx: &Ctx) -> Vec<(usize, String, String)> {
    Index::open(&ctx.paths, &ctx.limits)
        .unwrap()
        .session_records(Backend::Codex, ID_A)
        .unwrap()
}

fn assert_index_matches_transcript(ctx: &Ctx) {
    let page = sessions::transcript(ctx, ID_A, None, 0, Some(200))
        .unwrap()
        .page;
    assert!(!page.has_more);
    let expected: Vec<_> = page
        .records
        .into_iter()
        .filter_map(|r| {
            r.text.map(|text| {
                (
                    r.index,
                    serde_json::to_value(r.kind)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    text,
                )
            })
        })
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(indexed(ctx), expected);
}

#[test]
fn compressed_active_and_archived_rollouts_match_plain_list_show_and_pages() {
    for archived in [false, true] {
        let plain = Fixture::new();
        let compressed = Fixture::new();
        let records = records();
        let plain_path = if archived {
            plain.codex_archived(ID_A, &records, 0)
        } else {
            plain.codex(ID_A, &records, 0)
        };
        let compressed_path = compressed.codex_zstd(ID_A, &records, 0, archived);
        for path in [&plain_path, &compressed_path] {
            set_mtime(path, 100);
        }
        let plain_ctx = plain.ctx();
        let compressed_ctx = compressed.ctx();
        let plain_list = sessions::list(&plain_ctx, &ListOpts::default()).unwrap();
        let compressed_list = sessions::list(&compressed_ctx, &ListOpts::default()).unwrap();
        assert_eq!(plain_list.total_sessions, 1);
        assert_eq!(compressed_list.total_sessions, 1);
        assert_eq!(compressed_list.returned, 1);
        assert_eq!(compressed_list.sessions[0].archived, archived);
        assert_eq!(
            logical_output(&plain_list.sessions[0]),
            logical_output(&compressed_list.sessions[0])
        );
        let plain_show = sessions::show(&plain_ctx, ID_A, None).unwrap();
        let compressed_show = sessions::show(&compressed_ctx, ID_A, None).unwrap();
        assert_eq!(compressed_show.file, compressed_path.to_str().unwrap());
        assert_eq!(
            compressed_show.size_bytes,
            fs::metadata(&compressed_path).unwrap().len()
        );
        assert_eq!(logical_output(plain_show), logical_output(compressed_show));
        let mut offset = 0;
        loop {
            let expected = page(&plain_ctx, offset, 2);
            assert_eq!(page(&compressed_ctx, offset, 2), expected);
            let Some(next) = expected["next_offset"].as_u64() else {
                break;
            };
            offset = next as usize;
        }
        assert!(offset > 0);
    }
}

#[test]
fn search_finds_compressed_only_text_and_indexes_each_rollout_once() {
    let f = Fixture::new();
    f.codex_zstd(ID_A, &records(), 0, false);
    f.codex_zstd(
        ID_B,
        &[
            codex_meta("/work/archived", json!("cli")),
            codex_user_item("archivedonlyneedle"),
        ],
        0,
        true,
    );
    let ctx = f.ctx();
    for (query, id) in [("zstdonlyneedle", ID_A), ("archivedonlyneedle", ID_B)] {
        let result = sessions::search(
            &ctx,
            &SearchOpts {
                query: query.into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].row.session_id, id);
    }
    let index = Index::open(&ctx.paths, &ctx.limits).unwrap();
    assert_eq!(index.status().unwrap().sessions, 2);
    assert_eq!(index.status().unwrap().text_indexed, 2);
    assert_eq!(refresh(&ctx).unchanged, 2);
    assert_index_matches_transcript(&ctx);
}

#[test]
fn plain_and_compressed_duplicates_choose_newest_then_plain_then_path() {
    let f = Fixture::new();
    let plain = f.codex(ID_A, &records(), 0);
    let compressed = f.codex_zstd(ID_A, &records(), 0, false);
    let ctx = f.ctx();
    for (plain_time, compressed_time, expected) in [
        (100, 100, &plain),
        (100, 101, &compressed),
        (102, 101, &plain),
    ] {
        set_mtime(&plain, plain_time);
        set_mtime(&compressed, compressed_time);
        assert_eq!(archive::scan_all(&ctx.paths).len(), 1);
        assert_eq!(
            archive::find_session(&ctx.paths, "aaaa", None)
                .unwrap()
                .file,
            *expected
        );
        assert_eq!(
            sessions::list(&ctx, &ListOpts::default()).unwrap().returned,
            1
        );
        assert_eq!(refresh(&ctx).sessions, 1);
        assert_eq!(
            Index::open(&ctx.paths, &ctx.limits)
                .unwrap()
                .status()
                .unwrap()
                .sessions,
            1
        );
    }
    fs::remove_file(&plain).unwrap();
    let archived = f.codex_zstd(ID_A, &records(), 0, true);
    set_mtime(&archived, 101);
    assert_eq!(
        archive::find_session(&ctx.paths, ID_A, None).unwrap().file,
        compressed.min(archived)
    );
}

#[test]
fn compressing_an_indexed_session_preserves_its_row_transcript_and_decoded_checkpoint() {
    let f = Fixture::new();
    let records = records();
    let plain = f.codex(ID_A, &records, 0);
    let ctx = f.ctx();
    assert_eq!(refresh(&ctx).reindexed, 1);
    let expected_page = page(&ctx, 0, 200);
    let expected_records = indexed(&ctx);
    let conn = rusqlite::Connection::open(trovador::index::db_path(&ctx.paths)).unwrap();
    let checkpoint = || {
        conn.query_row(
            "SELECT id, offset, head_hash, tail_hash FROM sessions",
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap()
    };
    let original = checkpoint();
    assert_eq!(original.1 as u64, jsonl_len(&records));
    let compressed = f.codex_zstd(ID_A, &records, 0, false);
    fs::remove_file(plain).unwrap();
    let report = refresh(&ctx);
    assert_eq!(
        (
            report.sessions,
            report.reindexed,
            report.appended,
            report.removed,
            report.failed
        ),
        (1, 1, 0, 0, 0)
    );
    assert_eq!(checkpoint(), original);
    assert_eq!(page(&ctx, 0, 200), expected_page);
    assert_eq!(indexed(&ctx), expected_records);
    let (file, size): (String, i64) = conn
        .query_row("SELECT file, size FROM sessions", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(file, compressed.to_str().unwrap());
    assert_eq!(size as u64, fs::metadata(compressed).unwrap().len());
    assert_eq!(
        Index::open(&ctx.paths, &ctx.limits)
            .unwrap()
            .status()
            .unwrap()
            .sessions,
        1
    );
    assert_eq!(refresh(&ctx).unchanged, 1);
}

#[test]
fn nonzero_pages_and_projection_checkpoints_use_decompressed_offsets() {
    let plain = Fixture::new();
    let compressed = Fixture::new();
    let mut records = records();
    records.insert(1, codex_response("developer", &"context".repeat(20_000)));
    plain.codex(ID_A, &records[..4], 0);
    let plain_ctx = plain.ctx();
    let prefix = archive::find_session(&plain.paths, ID_A, None).unwrap();
    let mut projection = Projection::for_index(&prefix, &plain_ctx.limits, None).unwrap();
    let prefix_records: Vec<_> = projection.by_ref().collect();
    let checkpoint = projection.state().clone();
    assert_eq!(checkpoint.offset, jsonl_len(&records[..4]));
    assert!(checkpoint.offset > 100_000);
    plain.codex(ID_A, &records, 0);
    compressed.codex_zstd(ID_A, &records, 0, false);
    let plain_entry = archive::find_session(&plain.paths, ID_A, None).unwrap();
    let compressed_entry = archive::find_session(&compressed.paths, ID_A, None).unwrap();
    assert!(checkpoint.offset > compressed_entry.size_bytes);
    assert_eq!(page(&plain_ctx, 2, 2), page(&compressed.ctx(), 2, 2));
    let mut resumed_plain =
        Projection::for_index(&plain_entry, &plain_ctx.limits, Some(checkpoint.clone())).unwrap();
    let mut resumed_compressed =
        Projection::for_index(&compressed_entry, &plain_ctx.limits, Some(checkpoint)).unwrap();
    let expected: Vec<_> = resumed_plain.by_ref().collect();
    assert!(!expected.is_empty());
    assert_eq!(
        serde_json::to_value(resumed_compressed.by_ref().collect::<Vec<_>>()).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(resumed_plain.state(), resumed_compressed.state());
    assert_eq!(resumed_compressed.state().offset, jsonl_len(&records));
    let combined: Vec<_> = prefix_records.into_iter().chain(expected).collect();
    assert_eq!(
        serde_json::to_value(combined).unwrap(),
        page(&compressed.ctx(), 0, 200)["records"]
    );
    assert_eq!(
        archive::read_meta(&compressed_entry, &plain_ctx.limits),
        archive::read_meta(&plain_entry, &plain_ctx.limits)
    );
    assert_eq!(
        archive::read_meta(&compressed_entry, &plain_ctx.limits)
            .snippet
            .as_deref(),
        Some("find the compressed history")
    );
}

#[test]
fn every_compressed_file_change_reindexes_even_growth_with_an_unchanged_prefix() {
    let f = Fixture::new();
    let original = records();
    let path = f.codex_zstd(ID_A, &original, 0, false);
    set_mtime(&path, 100);
    let ctx = f.ctx();
    assert_eq!(refresh(&ctx).reindexed, 1);
    set_mtime(&path, 101);
    assert_eq!(refresh(&ctx).reindexed, 1);
    let mut grown = original.clone();
    grown.push(codex_agent_item(
        &(0..4_000).map(|n| format!("word{n} ")).collect::<String>(),
    ));
    f.codex_zstd(ID_A, &grown, 0, false);
    assert!(fs::metadata(&path).unwrap().len() > jsonl_len(&original));
    let report = refresh(&ctx);
    assert_eq!(
        (report.reindexed, report.appended, report.failed),
        (1, 0, 0)
    );
    assert_index_matches_transcript(&ctx);
    f.codex_zstd(ID_A, &original, 0, false);
    let report = refresh(&ctx);
    assert_eq!(
        (report.reindexed, report.appended, report.failed),
        (1, 0, 0)
    );
    assert_index_matches_transcript(&ctx);
    assert!(!indexed(&ctx).iter().any(|r| r.2.contains("word3999")));
}

#[test]
fn corrupt_zstd_is_reported_without_hiding_or_aborting_other_sessions() {
    for corruption in ["header", "truncated", "trailing"] {
        let f = Fixture::new();
        let path = f.codex_zstd(ID_A, &records(), 0, false);
        match corruption {
            "header" => fs::write(&path, b"not a zstd stream").unwrap(),
            "truncated" => {
                let mut bytes = fs::read(&path).unwrap();
                bytes.pop();
                fs::write(&path, bytes).unwrap();
            }
            _ => {
                fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"invalid next zstd frame")
                    .unwrap();
            }
        }
        f.codex(ID_B, &records(), 0);
        let ctx = f.ctx();
        let listed = sessions::list(&ctx, &ListOpts::default()).unwrap();
        assert_eq!(listed.total_sessions, 2);
        assert!(
            listed
                .sessions
                .iter()
                .any(|s| s.session_id == ID_B && s.meta.snippet.is_some())
        );
        assert!(
            matches!(
                sessions::transcript(&ctx, ID_A, None, 0, Some(200)),
                Err(sessions::Error::Io { .. })
            ),
            "{corruption}"
        );
        assert!(
            matches!(
                sessions::text(&ctx, ID_A, None, 0),
                Err(sessions::Error::Io { .. })
            ),
            "{corruption}: a partial decode must not pass for the whole session's text"
        );
        let report = refresh(&ctx);
        assert_eq!((report.failed, report.reindexed), (1, 1), "{corruption}");
        assert_eq!(
            Index::open(&ctx.paths, &ctx.limits)
                .unwrap()
                .status()
                .unwrap()
                .sessions,
            1
        );
    }
}

#[test]
fn compressed_fork_boundaries_hide_parent_history_and_keep_child_metadata() {
    for archived in [false, true] {
        let plain = Fixture::new();
        let compressed = Fixture::new();
        let mut header = codex_meta("/child", json!({"subagent": {"thread_spawn": {}}}));
        header["payload"]["forked_from_id"] = json!(ID_B);
        header["payload"]["subagent_history_start_ordinal"] = json!(5);
        let records = vec![
            header,
            codex_meta("/parent", json!("cli")),
            codex_user_item("parentonlyneedle"),
            codex_response("developer", &"inherited".repeat(20_000)),
            codex_agent_item("parentonlyanswer"),
            json!({"type":"response_item", "payload":{"type":"agent_message", "author":"/root", "recipient":"/root/child", "content":[{"type":"input_text", "text":"child assignment"}]}}),
            codex_response("assistant", "child answer"),
        ];
        if archived {
            plain.codex_archived(ID_A, &records, 0);
        } else {
            plain.codex(ID_A, &records, 0);
        }
        compressed.codex_zstd(ID_A, &records, 0, archived);
        let plain_ctx = plain.ctx();
        let compressed_ctx = compressed.ctx();
        assert_eq!(page(&plain_ctx, 0, 200), page(&compressed_ctx, 0, 200));
        let shown = sessions::show(&compressed_ctx, ID_A, None).unwrap();
        assert_eq!(
            shown.row.meta,
            sessions::show(&plain_ctx, ID_A, None).unwrap().row.meta
        );
        assert_eq!(shown.row.meta.snippet.as_deref(), Some("child assignment"));
        assert_eq!(shown.row.meta.cwd.as_deref(), Some("/child"));
        assert_eq!(refresh(&compressed_ctx).reindexed, 1);
        let indexed = indexed(&compressed_ctx);
        assert_eq!(indexed.len(), 2);
        assert_eq!(indexed[0].2, "child assignment");
        assert_eq!(indexed[1].2, "child answer");
        assert_index_matches_transcript(&compressed_ctx);
    }
}

#[test]
fn compressed_resumability_falls_back_to_disk_size_when_the_frame_has_no_content_size() {
    let f = Fixture::new();
    let records = vec![
        codex_meta("/work", json!("cli")),
        codex_user_item(&"x".repeat(100_000)),
    ];
    f.codex_zstd(ID_A, &records, 0, false);
    let entry = archive::find_session(&f.paths, ID_A, None).unwrap();
    assert!(entry.size_bytes < jsonl_len(&records));
    let limits = Limits {
        resume_limit_bytes: entry.size_bytes + 1,
        ..limits()
    };
    assert!(archive::resumability(&entry, &limits).resumable);
    let limits = Limits {
        resume_limit_bytes: entry.size_bytes,
        ..limits
    };
    assert!(!archive::resumability(&entry, &limits).resumable);
}

#[test]
fn excluded_compressed_text_still_uses_a_validated_decoded_checkpoint() {
    let f = Fixture::new();
    let records = vec![
        codex_meta("/review", json!({"subagent":{"other":"guardian"}})),
        codex_user_item(&"x".repeat(100_000)),
    ];
    let path = f.codex_zstd(ID_A, &records, 0, false);
    let ctx = f.ctx();
    assert_eq!(refresh(&ctx).reindexed, 1);
    let index = Index::open(&ctx.paths, &ctx.limits).unwrap();
    assert_eq!(index.status().unwrap().text_indexed, 0);
    assert!(indexed(&ctx).is_empty());
    let conn = rusqlite::Connection::open(trovador::index::db_path(&ctx.paths)).unwrap();
    let offset: i64 = conn
        .query_row("SELECT offset FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(offset as u64, jsonl_len(&records));
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"invalid next zstd frame")
        .unwrap();
    assert_eq!(refresh(&ctx).failed, 1);
}

#[test]
fn compressed_resumability_uses_the_frame_content_size_not_the_smaller_disk_size() {
    // A 749 KB rollout on the development machine held 3.0 MB of JSONL: resumable by its
    // disk size and over the ceiling by content.
    let f = Fixture::new();
    let records = vec![
        codex_meta("/work", json!("cli")),
        codex_user_item(&"x".repeat(100_000)),
    ];
    let path = f.codex_zstd(ID_A, &records, 0, false);
    let body: String = records.iter().map(|r| format!("{r}\n")).collect();
    // bulk compression writes the content size into the frame header, as Codex does.
    fs::write(&path, zstd::bulk::compress(body.as_bytes(), 3).unwrap()).unwrap();
    let entry = archive::find_session(&f.paths, ID_A, None).unwrap();
    assert!(entry.size_bytes < jsonl_len(&records) / 10);
    assert_eq!(archive::transcript_bytes(&entry), jsonl_len(&records));
    let limits = Limits {
        resume_limit_bytes: entry.size_bytes + 1,
        ..limits()
    };
    let r = archive::resumability(&entry, &limits);
    assert!(!r.resumable);
    assert!(r.reason.unwrap().contains("sessions action=text"));
}

#[test]
fn compressed_text_matches_the_same_session_uncompressed() {
    let plain = Fixture::new();
    plain.codex(ID_A, &records(), 0);
    let compressed = Fixture::new();
    compressed.codex_zstd(ID_A, &records(), 0, false);
    let a = sessions::text(&plain.ctx(), ID_A, None, 0).unwrap();
    let b = sessions::text(&compressed.ctx(), ID_A, None, 0).unwrap();
    assert!(a.text.contains("zstdonlyneedle"));
    assert_eq!(a.text, b.text);
}
