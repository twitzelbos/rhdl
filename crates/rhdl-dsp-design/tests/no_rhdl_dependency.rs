//! The property this crate exists for.
//!
//! `rhdl-dsp-design` is separate from `rhdl-fpga` for exactly one
//! reason: a proc macro must be able to evaluate filter design at
//! expansion time, and `rhdl-macro-core` may not depend on `rhdl-core`
//! (architecture.md §2). That only works while this crate has **no
//! RHDL dependency of its own**.
//!
//! The moment someone adds one — for a `Digital` bound, a `bits()`
//! call, anything — the crate stops being reachable from the macro
//! layer and the split becomes pointless overhead. That failure is
//! silent: everything still compiles, and the macro simply cannot be
//! written. So it is checked.

use std::path::PathBuf;

#[test]
fn this_crate_depends_on_nothing_from_rhdl() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).expect("read our own manifest");

    // Crude on purpose: a TOML parser would be a dependency, and this
    // crate having no dependencies is the thing being tested.
    //
    // Only dependency sections count. The first version scanned the
    // whole file and tripped on `name = "rhdl-dsp-design"`, which is
    // the crate declaring itself.
    let mut in_deps = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            // `[dependencies]`, `[dev-dependencies]`,
            // `[build-dependencies]`, `[target.*.dependencies]`, and
            // the `[dependencies.foo]` table form.
            in_deps = line.contains("dependencies");
            // The table form names the crate in the header itself.
            if in_deps && line.contains("dependencies.") {
                assert!(
                    !line.contains("rhdl"),
                    "{}:{}: dependency table names an RHDL crate:\n  {line}",
                    manifest.display(),
                    n + 1
                );
            }
            continue;
        }
        if !in_deps {
            continue;
        }
        assert!(
            !line.contains("rhdl"),
            "{}:{}: this crate must not depend on anything from RHDL, but sees:\n  {line}\n\
             See the module docs: the macro layer cannot reach a crate that does.",
            manifest.display(),
            n + 1
        );
    }
}

/// And the code must not reach for RHDL either, however it got there.
#[test]
fn no_source_file_refers_to_rhdl() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            checked += 1;
            for (n, line) in text.lines().enumerate() {
                let t = line.trim_start();
                // Prose mentions RHDL constantly and should; only real
                // code counts.
                if t.starts_with("//") || t.starts_with("#![doc") {
                    continue;
                }
                assert!(
                    !t.contains("rhdl::") && !t.contains("rhdl_core") && !t.contains("rhdl_fpga"),
                    "{}:{}: refers to RHDL in code:\n  {line}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
    assert!(
        checked >= 5,
        "expected to scan the crate, saw {checked} files"
    );
}
