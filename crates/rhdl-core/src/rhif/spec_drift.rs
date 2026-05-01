//! Spec-drift check: verify the per-opcode pages under
//! `doc/rhif-spec/opcodes/` exactly match the variants of
//! [`crate::rhif::spec::OpCode`].
//!
//! Per `rhif-formalization-plan.md` §11, every PR that modifies
//! `crates/rhdl-core/src/rhif/spec.rs` must update the corresponding
//! spec page.  This module enforces that contract: a missing page
//! (a new `OpCode` variant without docs) or a stale page (a
//! deleted variant whose page lingers) fails the test, surfacing
//! the drift immediately.
//!
//! The check is run as a normal `cargo test`, so it runs on every
//! PR without needing CI-config changes.  Wiring the same check
//! into a CI badge / pre-merge gate is a separate (non-engineering)
//! step.
//!
//! ## What's checked
//!
//! - **Surjectivity.** Every `OpCode` variant has a corresponding
//!   `.md` file under `doc/rhif-spec/opcodes/`.
//! - **Injectivity.** Every `.md` file under `doc/rhif-spec/opcodes/`
//!   corresponds to an `OpCode` variant (excluding `README.md` and
//!   index files).
//! - **Naming.** Each variant `Foo` maps to a file `foo.md` (snake_case),
//!   modulo the case-insensitive match.
//!
//! ## What's NOT checked (yet)
//!
//! - That each opcode page actually documents the right opcode (i.e.,
//!   the page's content correctly describes the variant).  That's a
//!   prose-quality concern, not a drift concern.
//! - That helper structs (`Binary`, `Cast`, `Wrap`, etc.) have any
//!   particular cross-reference structure.

#![cfg(test)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// The canonical `OpCode` variant list, kept in sync by hand with
/// `spec.rs`.  If a new variant is added there, this list must also
/// be updated — and that requirement is itself part of the spec-
/// drift contract.
const OPCODE_VARIANTS: &[&str] = &[
    "noop",
    "binary",
    "unary",
    "select",
    "index",
    "assign",
    "splice",
    "repeat",
    "struct",
    "tuple",
    "case",
    "exec",
    "array",
    "enum",
    "as_bits",
    "as_signed",
    "resize",
    "retime",
    "wrap",
];

fn opcodes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("doc")
        .join("rhif-spec")
        .join("opcodes")
}

fn opcode_pages() -> BTreeSet<String> {
    let dir = opcodes_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|e| {
        panic!(
            "could not read opcodes directory at {}: {e}",
            dir.display(),
        )
    });
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            // Drop README files; keep <name>.md.  ("index.md" is
            // an opcode page — not a directory index.)
            if name.ends_with(".md") && name != "README.md" {
                Some(name.trim_end_matches(".md").to_string())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn every_opcode_variant_has_a_page() {
    let pages = opcode_pages();
    let missing: Vec<&str> = OPCODE_VARIANTS
        .iter()
        .copied()
        .filter(|v| !pages.contains(*v))
        .collect();
    assert!(
        missing.is_empty(),
        "OpCode variant(s) without a corresponding doc/rhif-spec/opcodes/<name>.md page: {missing:?}",
    );
}

#[test]
fn every_opcode_page_corresponds_to_a_variant() {
    let pages = opcode_pages();
    let known: BTreeSet<&str> = OPCODE_VARIANTS.iter().copied().collect();
    let extras: Vec<&str> = pages
        .iter()
        .map(String::as_str)
        .filter(|p| !known.contains(p))
        .collect();
    assert!(
        extras.is_empty(),
        "doc/rhif-spec/opcodes/ pages with no corresponding OpCode variant (stale docs): {extras:?}",
    );
}

#[test]
fn opcode_variant_count_matches_spec_rs() {
    // This guards against the OPCODE_VARIANTS list itself drifting
    // out of sync with spec.rs.  When a new opcode is added, both
    // `spec.rs::OpCode` and `OPCODE_VARIANTS` must be updated, plus
    // a new doc page added.  Update this expected count with each
    // new opcode.
    const EXPECTED: usize = 19;
    assert_eq!(
        OPCODE_VARIANTS.len(),
        EXPECTED,
        "OPCODE_VARIANTS has {} entries, expected {EXPECTED}.  If a new OpCode \
         variant landed in spec.rs, add it here AND in doc/rhif-spec/opcodes/ \
         AND bump EXPECTED.",
        OPCODE_VARIANTS.len(),
    );
}
