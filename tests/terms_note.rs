//! A terms query needs every term in one record (#103). A natural description of a past
//! conversation names words it discussed in different records, finds nothing, and the
//! result said only that nothing matched.

mod common;

use common::*;
use trovador::index::SearchOpts;
use trovador::sessions;

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.claude(
        "-w",
        ID_A,
        &[
            claude_user("/w", "the zqalpha reviewer runs first"),
            claude_assistant("then zqbeta copies its text"),
            claude_user("/w", "and zqgamma decides"),
        ],
        100,
    );
    f
}

fn search(f: &Fixture, query: &str, fts: bool) -> sessions::SearchResult {
    sessions::search(
        &f.ctx(),
        &SearchOpts {
            query: query.into(),
            fts,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn three_terms_spread_over_records_find_nothing_and_say_why() {
    let f = fixture();
    let r = search(&f, "zqalpha zqbeta zqgamma", false);
    assert_eq!(r.sessions_matched, 0);
    let note = r.note.unwrap_or_default();
    assert!(
        note.contains("all 3 terms must occur in one record"),
        "{note}"
    );
    // The query found what it asked for: no record holds all three.
    assert_eq!(r.absence_is_proof, Some(true));
}

#[test]
fn each_term_alone_finds_the_session() {
    let f = fixture();
    for t in ["zqalpha", "zqbeta", "zqgamma"] {
        assert_eq!(search(&f, t, false).sessions_matched, 1, "{t}");
    }
}

#[test]
fn two_terms_or_a_match_get_no_hint() {
    let f = fixture();
    let r = search(&f, "zqalpha zqbeta", false);
    assert_eq!(r.sessions_matched, 0);
    assert!(r.note.is_none(), "{:?}", r.note);
    let r = search(&f, "zqalpha reviewer runs", false);
    assert_eq!(r.sessions_matched, 1);
    assert!(r.note.is_none(), "{:?}", r.note);
}

#[test]
fn a_quoted_phrase_counts_as_one_term() {
    let f = fixture();
    let r = search(&f, "\"zqalpha reviewer\" zqgamma", false);
    assert_eq!(r.sessions_matched, 0);
    assert!(r.note.is_none(), "{:?}", r.note);
}

#[test]
fn fts_and_regex_queries_get_no_terms_hint() {
    let f = fixture();
    let r = search(&f, "zqalpha AND zqbeta AND zqgamma", true);
    assert!(
        !r.note.unwrap_or_default().contains("terms must occur"),
        "fts has its own syntax"
    );
}
