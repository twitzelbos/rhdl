//! Does the book still hold together?
//!
//! `mdbook` is not installed in every environment this repository is
//! built in, and there is no CI to put it in, so the book's structural
//! integrity was being checked by hand — which means it was being
//! checked whenever someone remembered.
//!
//! These tests check it in `cargo test`, where everything else is
//! checked. They do not render anything; they verify the two things
//! that silently rot:
//!
//! - every chapter `SUMMARY.md` lists exists;
//! - every `{{#rustdoc_include path:anchor}}` points at a real file
//!   with a real `ANCHOR:` marker.
//!
//! A broken link renders as a dead entry in the sidebar and a missing
//! anchor renders as an empty code block — neither fails a build, and
//! both look like the author meant it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `doc/book/src`, from this crate at `doc/book/src/code`.
fn book_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("code/ has a parent")
        .to_path_buf()
}

/// Every markdown file the book contains, chapters and all.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                // `code/` is this crate; `book/` is build output.
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name != "code" && name != "book" {
                    stack.push(p);
                }
            } else if p.extension().is_some_and(|x| x == "md") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Every chapter `SUMMARY.md` points at must exist.
#[test]
fn every_summary_link_resolves() {
    let src = book_src();
    let summary = std::fs::read_to_string(src.join("SUMMARY.md")).expect("read SUMMARY.md");
    let mut checked = 0;
    let mut missing = Vec::new();
    for line in summary.lines() {
        // `- [Title](path/to/chapter.md)`, sometimes with a `./`.
        let Some(open) = line.find("](") else {
            continue;
        };
        let rest = &line[open + 2..];
        let Some(close) = rest.find(')') else {
            continue;
        };
        let target = rest[..close].trim_start_matches("./");
        if target.is_empty() || target.starts_with("http") {
            continue;
        }
        checked += 1;
        if !src.join(target).is_file() {
            missing.push(target.to_string());
        }
    }
    assert!(
        checked > 40,
        "expected to check the whole summary, saw {checked}"
    );
    assert!(
        missing.is_empty(),
        "SUMMARY.md lists chapters that do not exist: {missing:?}"
    );
}

/// Every `rustdoc_include` must name a real file and a real anchor.
#[test]
fn every_include_anchor_resolves() {
    let src = book_src();
    let mut checked = 0;
    let mut broken: Vec<String> = Vec::new();

    for md in markdown_files(&src) {
        let text = std::fs::read_to_string(&md).expect("read chapter");
        let dir = md.parent().expect("chapter has a directory");
        for line in text.lines() {
            for tag in ["{{#rustdoc_include ", "{{#include "] {
                let Some(at) = line.find(tag) else { continue };
                let rest = &line[at + tag.len()..];
                let Some(end) = rest.find("}}") else { continue };
                let spec = rest[..end].trim();
                let (rel, anchor) = match spec.split_once(':') {
                    Some((f, a)) => (f, Some(a)),
                    None => (spec, None),
                };
                checked += 1;
                let target = dir.join(rel);
                let Ok(body) = std::fs::read_to_string(&target) else {
                    broken.push(format!("{}: missing file {rel}", md.display()));
                    continue;
                };
                if let Some(a) = anchor {
                    // mdbook accepts `ANCHOR: name` and `ANCHOR:name`.
                    let has = body.lines().any(|l| {
                        let t = l.trim();
                        t.ends_with(&format!("ANCHOR: {a}")) || t.ends_with(&format!("ANCHOR:{a}"))
                    });
                    if !has {
                        broken.push(format!("{}: no `ANCHOR: {a}` in {rel}", md.display()));
                    }
                }
            }
        }
    }
    assert!(checked > 10, "expected to find includes, saw {checked}");
    assert!(
        broken.is_empty(),
        "broken includes:\n  {}",
        broken.join("\n  ")
    );
}

/// Chapters that predate this test and are known to be dead.
///
/// These are not deleted here because they are not this fork's files to
/// delete — they are abandoned drafts from upstream, and removing them
/// would make every future merge noisier for no reader's benefit. They
/// are named individually so the allowlist cannot quietly grow: adding
/// an entry means writing down why the chapter is dead.
const KNOWN_ORPHANS: &[(&str, &str)] = &[
    // The mdbook scaffold the book started from. Superseded by the
    // `bits/`, `digital/`, `kernels/` and `timed/` trees, all of which
    // `SUMMARY.md` does list.
    ("chapter_1/bits.md", "superseded by bits/"),
    ("chapter_1/circuit.md", "stub, superseded by circuits/"),
    ("chapter_1/digital.md", "stub, superseded by digital/"),
    ("chapter_1/foundation.md", "superseded by the introduction"),
    ("chapter_1/kernel.md", "stub, superseded by kernels/"),
    ("chapter_1/summary.md", "scaffold index"),
    (
        "chapter_1/synchronous.md",
        "stub, superseded by synchronous/",
    ),
    ("chapter_1/timed.md", "stub, superseded by timed/"),
    // Split into `kernels/tracing/{summary,simple,complex,keys,nesting,enums}.md`,
    // which is what `SUMMARY.md` points at. The single file is the pre-split copy.
    ("kernels/tracing.md", "split into kernels/tracing/"),
    // A scratch pad holding two emoji for copy-pasting into chapters.
    ("unicode.md", "scratch pad, not prose"),
];

/// Every chapter is reachable from `SUMMARY.md`.
///
/// An orphaned chapter is invisible in the rendered book, so it gets
/// written, goes stale, and nobody notices — the failure mode is that
/// the work was wasted rather than that anything looks wrong.
#[test]
fn no_chapter_is_orphaned() {
    let src = book_src();
    let summary = std::fs::read_to_string(src.join("SUMMARY.md")).expect("read SUMMARY.md");
    let listed: BTreeSet<PathBuf> = summary
        .lines()
        .filter_map(|line| {
            let open = line.find("](")?;
            let rest = &line[open + 2..];
            let close = rest.find(')')?;
            let t = rest[..close].trim_start_matches("./");
            (!t.is_empty() && !t.starts_with("http")).then(|| src.join(t))
        })
        .collect();

    let orphans: Vec<String> = markdown_files(&src)
        .into_iter()
        .filter(|p| p.file_name().and_then(|s| s.to_str()) != Some("SUMMARY.md"))
        .filter(|p| !listed.contains(p))
        .map(|p| p.strip_prefix(&src).unwrap_or(&p).display().to_string())
        .filter(|rel| !KNOWN_ORPHANS.iter().any(|(k, _)| k == rel))
        .collect();
    assert!(
        orphans.is_empty(),
        "chapters not reachable from SUMMARY.md: {orphans:?}"
    );
}

/// The allowlist may not outlive the files it excuses.
///
/// Without this, a chapter named in `KNOWN_ORPHANS` could be deleted or
/// finally linked into `SUMMARY.md` and the stale entry would sit there
/// silently excusing nothing.
#[test]
fn the_orphan_allowlist_is_still_accurate() {
    let src = book_src();
    let summary = std::fs::read_to_string(src.join("SUMMARY.md")).expect("read SUMMARY.md");
    for (rel, why) in KNOWN_ORPHANS {
        assert!(
            src.join(rel).is_file(),
            "{rel} is allowlisted as an orphan ({why}) but no longer exists \u{2014} drop the entry"
        );
        assert!(
            !summary.contains(rel),
            "{rel} is allowlisted as an orphan ({why}) but SUMMARY.md now links it \u{2014} drop the entry"
        );
    }
}
