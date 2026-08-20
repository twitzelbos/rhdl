//! Icarus Verilog is a **required** precondition for this project's tests.
//!
//! Tier 4 of the validation stack compiles every widget's emitted Verilog
//! and simulates it against the Rust model. That is the ground-truth check
//! — the one that catches a kernel which simulates correctly and emits
//! wrong hardware — so a test run without a working `iverilog` is not a
//! degraded run, it is a run that has not checked the thing that matters.
//!
//! # Why this exists rather than skipping
//!
//! Before this, a machine without the tool produced **504 individual test
//! failures**, each a bare `NotFound` panic from
//! `Command::new("iverilog")`. Three problems with that:
//!
//! 1. It reads like a code regression. Diagnosing it meant opening a
//!    failure and recognising the panic message.
//! 2. `cargo test --all` is fail-fast across test *binaries*, so which
//!    504 you saw depended on crate ordering.
//! 3. The documented workaround did not work. CLAUDE.md suggested
//!    `cargo test --all -- --skip iverilog`, but the failures include
//!    `test_vlog_generation`, `no_combinatorial_paths` and
//!    `test_synthesizable` — none of which match `iverilog` by name.
//!
//! So the precondition is checked once, explicitly, and a run that cannot
//! meet it stops immediately with an actionable message.
//!
//! # Why "working", not "present"
//!
//! `iverilog` and `vvp` are separate binaries and can break independently:
//! `iverilog` compiles the testbench, `vvp` runs it. A `PATH` lookup for
//! `iverilog` alone would pass on a machine that cannot simulate anything.
//! So the check compiles and runs a trivial module end to end and requires
//! the expected output — which also catches a version too old to accept
//! the flags used, and a broken install whose binaries exist but fail.

use std::process::Command;

/// Marker the smoke-test module prints, and the check greps for.
const SENTINEL: &str = "RHDL_IVERILOG_SMOKE_OK";

/// What is wrong with the local Icarus Verilog installation, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IverilogProblem {
    /// `iverilog` could not be executed at all.
    IverilogMissing(String),
    /// `vvp` could not be executed at all.
    VvpMissing(String),
    /// Both are runnable but the pair cannot compile-and-run a trivial
    /// module. Carries whatever diagnostic was available.
    NotWorking(String),
}

impl std::fmt::Display for IverilogProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IverilogMissing(e) => {
                write!(f, "`iverilog` could not be executed: {e}")
            }
            Self::VvpMissing(e) => write!(f, "`vvp` could not be executed: {e}"),
            Self::NotWorking(e) => write!(
                f,
                "`iverilog` and `vvp` are present but cannot compile and run \
                 a trivial module: {e}"
            ),
        }
    }
}

/// Compile and run a trivial Verilog module, end to end.
///
/// Returns `Ok(())` only if `iverilog` produced an executable and `vvp` ran
/// it and printed the sentinel. Anything else is a problem worth stopping
/// for.
pub fn check_iverilog() -> Result<(), IverilogProblem> {
    check_iverilog_with("iverilog", "vvp")
}

/// [`check_iverilog`], with the binary names injected.
///
/// Exists so the failure path can be tested by naming a binary that does
/// not exist, rather than by mutating `PATH`. The environment is
/// process-wide and cargo runs tests in parallel, so a test that clears
/// `PATH` races every other test that reads it — which is exactly how the
/// first version of this module broke its own precondition test.
pub fn check_iverilog_with(iverilog_bin: &str, vvp_bin: &str) -> Result<(), IverilogProblem> {
    let dir = tempfile::tempdir()
        .map_err(|e| IverilogProblem::NotWorking(format!("no temp dir: {e}")))?;
    let src = dir.path().join("smoke.v");
    let exe = dir.path().join("smoke");
    let source = format!("module main;\n initial $display(\"{SENTINEL}\");\nendmodule\n");
    std::fs::write(&src, source)
        .map_err(|e| IverilogProblem::NotWorking(format!("cannot write temp source: {e}")))?;

    let compile = Command::new(iverilog_bin)
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .map_err(|e| IverilogProblem::IverilogMissing(e.to_string()))?;
    if !compile.status.success() {
        return Err(IverilogProblem::NotWorking(format!(
            "iverilog exited {}: {}",
            compile.status,
            String::from_utf8_lossy(&compile.stderr).trim()
        )));
    }

    let run = Command::new(vvp_bin)
        .arg(&exe)
        .output()
        .map_err(|e| IverilogProblem::VvpMissing(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&run.stdout);
    if !run.status.success() {
        return Err(IverilogProblem::NotWorking(format!(
            "vvp exited {}: {}",
            run.status,
            String::from_utf8_lossy(&run.stderr).trim()
        )));
    }
    if !stdout.contains(SENTINEL) {
        return Err(IverilogProblem::NotWorking(format!(
            "vvp ran but did not print the expected output; got {stdout:?}"
        )));
    }
    Ok(())
}

/// The message shown when the precondition is not met.
///
/// Separate from the exit so it can be asserted on without terminating the
/// test process.
pub fn precondition_failure_message(problem: &IverilogProblem) -> String {
    format!(
        "\n\
         ==================================================================\n\
         REQUIRED TOOL MISSING OR BROKEN: Icarus Verilog\n\
         ==================================================================\n\
         \n\
         {problem}\n\
         \n\
         This project REQUIRES a working Icarus Verilog. Tier 4 of the\n\
         validation stack compiles every widget's emitted Verilog and\n\
         simulates it against the Rust model; without it, the check that\n\
         catches wrong emitted hardware does not run, so the suite would\n\
         report success while proving much less than it appears to.\n\
         \n\
         Install it:\n\
         \n\
         Debian/Ubuntu   sudo apt install iverilog\n\
         macOS           brew install icarus-verilog\n\
         Fedora          sudo dnf install iverilog\n\
         \n\
         Both `iverilog` and `vvp` must be on PATH -- they are separate\n\
         binaries and this check requires the pair to work together.\n\
         \n\
         The test run is being aborted rather than allowed to produce\n\
         hundreds of individual failures that all share this one cause.\n\
         See CLAUDE.md section 8.\n\
         ==================================================================\n"
    )
}

/// Abort the test process unless a working Icarus Verilog is present.
///
/// Call this from one `#[test]` per crate whose tests reach Tier 4. On
/// failure it prints [`precondition_failure_message`] and exits non-zero,
/// which fails that test binary immediately — so the run stops at the
/// precondition instead of restating it a few hundred times.
///
/// Exiting rather than panicking is deliberate: a panic fails one test and
/// lets the remaining tests in the binary produce the same failure again
/// with a less useful message.
pub fn require_iverilog() {
    if let Err(problem) = check_iverilog() {
        eprint!("{}", precondition_failure_message(&problem));
        // stderr is not always flushed before exit.
        use std::io::Write;
        let _ = std::io::stderr().flush();
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The precondition itself. Also the crate's own Tier-4 gate.
    #[test]
    fn iverilog_precondition() {
        require_iverilog();
    }

    /// The check actually exercises the toolchain rather than returning
    /// `Ok` unconditionally.
    ///
    /// A check that always passes is worse than no check: it makes the
    /// precondition look enforced while every Tier-4 test fails downstream
    /// for a reason nothing has named.
    ///
    /// Injects a nonexistent binary name rather than clearing `PATH`. The
    /// first version did the latter and broke `iverilog_precondition`,
    /// which was running in parallel and saw the empty environment — the
    /// same shared-mutable-state race as two tests sharing a file.
    #[test]
    fn check_detects_a_missing_iverilog() {
        let result = check_iverilog_with("iverilog-does-not-exist-9f3a2b", "vvp");
        assert!(
            matches!(result, Err(IverilogProblem::IverilogMissing(_))),
            "a nonexistent iverilog must be reported as missing, got {result:?}"
        );
    }

    /// `vvp` is checked separately, because it is a separate binary.
    ///
    /// A check that only looked for `iverilog` would pass on a machine that
    /// can compile a testbench and not run it, which is a working compiler
    /// and a useless test suite.
    #[test]
    fn check_detects_a_missing_vvp() {
        let result = check_iverilog_with("iverilog", "vvp-does-not-exist-9f3a2b");
        assert!(
            matches!(result, Err(IverilogProblem::VvpMissing(_))),
            "a nonexistent vvp must be reported separately, got {result:?}"
        );
    }

    /// The failure message names the tool and how to install it.
    ///
    /// The whole point is that someone hitting this does not have to
    /// diagnose it, so the message is part of the contract.
    #[test]
    fn failure_message_is_actionable() {
        let msg = precondition_failure_message(&IverilogProblem::IverilogMissing(
            "No such file or directory".into(),
        ));
        for needle in ["Icarus Verilog", "apt install iverilog", "vvp", "CLAUDE.md"] {
            assert!(msg.contains(needle), "message must mention {needle:?}");
        }
    }
}
