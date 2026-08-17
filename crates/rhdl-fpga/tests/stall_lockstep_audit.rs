//! Guard against two throttled streams stalling in lockstep.
//!
//! The default seed for [`stalling`] and [`SinkFromFn::new_from_iter`] is
//! a function of the stall probability alone. That decorrelates streams
//! running at *different* rates for free, which is the common case. Two
//! streams at the **same** rate, live in the same run, draw the identical
//! sequence and therefore stall on identical cycles — so the case where
//! one channel is blocked while the other flows never occurs. For a
//! request/response pair that is the case worth testing. The
//! `*_with_seed` constructors exist to break the tie.
//!
//! # Why this is a test and not a review checklist
//!
//! This defect was introduced twice in one afternoon, in the change whose
//! stated purpose was to make the stimulus reproducible:
//!
//! 1. Seeding `new_from_iter` from a single hard-coded constant made
//!    *every* sink in a run draw the same sequence. Four fixtures build
//!    two sinks each; all four pairs went from independently random to
//!    perfectly lockstep.
//! 2. Migrating example sources from `stalling(x, 0.23)` to
//!    `stalling_periodic(x, 4)` gave both channels of the AXI read and
//!    write fixtures a phase counter starting at zero — lockstep again,
//!    by a different mechanism.
//!
//! Both were found by reading the code, after the fact, one at a time.
//! Reading is what let them in, so the check is mechanical now.
//! Determinism and independence are separate properties, and a change
//! that establishes the first can silently destroy the second.
//!
//! # Suppressing a deliberate pair
//!
//! A test that exists to *demonstrate* the collision is not a defect. Put
//! the marker `lockstep-audit: intentional` in the function body to
//! exempt it.

use std::path::{Path, PathBuf};

/// Constructors whose default seed derives from their last argument.
const THROTTLES: &[&str] = &["stalling(", "new_from_iter(", "stalling_periodic("];

/// Marker exempting a function that collides on purpose.
const ALLOW: &str = "lockstep-audit: intentional";

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// True if `needle` at `pos` is a whole identifier, not the tail of a
/// longer one — so `new_from_iter(` does not match inside some
/// hypothetical `raw_new_from_iter(`.
fn is_boundary(text: &str, pos: usize) -> bool {
    text[..pos]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

/// The last top-level argument of a call whose `(` sits at `open`.
///
/// Depth-counted so that a nested call in an earlier argument — say
/// `stalling(rng.clone(), 0.23)` — does not confuse the split.
fn last_argument(text: &str, open: usize) -> Option<String> {
    let mut depth = 0usize;
    let mut last_comma = None;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    let start = last_comma.map_or(open + 1, |p: usize| p + 1);
                    return Some(text[start..open + i].trim().to_string());
                }
            }
            ',' if depth == 1 => last_comma = Some(open + i),
            _ => {}
        }
    }
    None
}

/// Split a file into `(name, body)` per function.
///
/// The unit is one function body because two calls only stall in lockstep
/// if they are live in the same run: two calls in two different `#[test]`
/// functions never coexist and must not be flagged.
fn scopes(text: &str) -> Vec<(String, &str)> {
    let mut starts = Vec::new();
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let after_vis = t
            .strip_prefix("pub(crate) ")
            .or_else(|| t.strip_prefix("pub "))
            .unwrap_or(t);
        let after_async = after_vis.strip_prefix("async ").unwrap_or(after_vis);
        if let Some(rest) = after_async.strip_prefix("fn ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                starts.push((offset, name));
            }
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, (pos, name))| {
            let end = starts.get(i + 1).map_or(text.len(), |(p, _)| *p);
            (name.clone(), &text[*pos..end])
        })
        .collect()
}

#[test]
fn no_same_rate_throttles_share_a_seed() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for sub in ["src", "examples", "tests"] {
        collect_rs(&root.join(sub), &mut files);
    }
    assert!(
        files.len() > 100,
        "expected to scan the whole crate, found only {} files — \
         a guard that silently scans nothing is worse than no guard",
        files.len()
    );

    let mut findings = Vec::new();
    for path in &files {
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        // Neutralise the seeded forms; they are the fix, not the defect.
        let text = raw.replace("_with_seed(", "_withseed(");
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();

        for (fn_name, body) in scopes(&text) {
            if body.contains(ALLOW) {
                continue;
            }
            for needle in THROTTLES {
                let mut rates: Vec<String> = Vec::new();
                let mut from = 0usize;
                while let Some(rel_pos) = body[from..].find(needle) {
                    let pos = from + rel_pos;
                    from = pos + needle.len();
                    if !is_boundary(body, pos) {
                        continue;
                    }
                    if let Some(rate) = last_argument(body, pos + needle.len() - 1) {
                        rates.push(rate);
                    }
                }
                for rate in &rates {
                    if rates.iter().filter(|r| *r == rate).count() > 1
                        && !findings.iter().any(|f: &String| {
                            f.contains(&rel) && f.contains(fn_name.as_str()) && f.contains(rate)
                        })
                    {
                        findings.push(format!(
                            "{rel}::{fn_name} — two `{}` at rate {rate}",
                            needle.trim_end_matches('(')
                        ));
                    }
                }
            }
        }
    }

    assert!(
        findings.is_empty(),
        "these streams share a rate, so they share a seed and stall on \
         identical cycles — use the `*_with_seed` constructor with \
         distinct seeds, or mark the function `{ALLOW}` if the collision \
         is the point:\n  {}",
        findings.join("\n  ")
    );
}
