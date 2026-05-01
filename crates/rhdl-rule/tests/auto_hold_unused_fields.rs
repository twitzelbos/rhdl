//! Auto-hold for unused struct fields in the function-like
//! `rule_kernel!` form.
//!
//! Before this change: the macro derived its field list purely from
//! the union of every rule's read+write set and the `#[output]`
//! method's field reads.  A struct field that no rule touched and no
//! output method referenced would be missing from the emitted `D`
//! constructor, and the user got a cryptic Rust compile error
//! ("missing field `xyz` in initializer of D").  The Pilot 4
//! composition demo had to add `let _ = *self_q.last_idx;` purely
//! to satisfy the constraint.
//!
//! After this change: the function-like form passes the struct's
//! actual field list to the lowering, which unions it into the
//! field-name set.  Fields no rule touches get `_next_<field> =
//! q.<field>` (hold) initialization, with no overwrite — the
//! field stays at its current value forever.  The user can declare
//! DFF fields without being forced to add dummy reads.
//!
//! The attribute form `#[rule_kernel_attr]` can't see the struct,
//! so it's unchanged; users of that form keep the every-field-
//! touched constraint and the Rust "missing field" error if they
//! violate it.  Documented in `rule-architecture.md` §4.5.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

// ---- Test 1: struct field never read or written by any rule ------

rule_kernel! {
    /// `aux_state` is declared but never touched by any rule or
    /// output reference.  Pre-PR-#27 this would have failed to
    /// compile with "missing field `aux_state` in initializer of D".
    /// Post-PR-#27 the macro auto-holds it.
    pub struct AutoHoldUnusedField {
        active: dff::DFF<Bits<8>>,
        aux_state: dff::DFF<bool>,
    }

    impl AutoHoldUnusedField {
        #[rule]
        fn bump_active(ctx: &mut RuleCtx<Self>, enable: bool) {
            guard!(enable);
            ctx.active = *ctx.active + bits::<8>(1);
        }

        #[output]
        fn output(self_q: &Self, _enable: bool) -> Bits<8> {
            *self_q.active
        }
    }
}

#[test]
fn auto_hold_unused_field_compiles() {
    let _uut: AutoHoldUnusedField = AutoHoldUnusedField::default();
}

#[test]
fn auto_hold_active_increments_normally() {
    let uut: AutoHoldUnusedField = AutoHoldUnusedField::default();
    let stream = std::iter::repeat_n(true, 5)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert!(last >= 4 && last <= 5, "active should increment; got {last}");
}

#[test]
fn auto_hold_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: AutoHoldUnusedField = AutoHoldUnusedField::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}

// ---- Test 2: multiple unused fields, all auto-held ---------------

rule_kernel! {
    /// Two unused fields plus one active field.  All three must be
    /// in the emitted D struct; only `count` gets a non-hold update.
    pub struct ManyUnusedFields {
        count: dff::DFF<Bits<8>>,
        flag_a: dff::DFF<bool>,
        flag_b: dff::DFF<bool>,
        spare: dff::DFF<Bits<4>>,
    }

    impl ManyUnusedFields {
        #[rule]
        fn bump(ctx: &mut RuleCtx<Self>, _i: bool) {
            ctx.count = *ctx.count + bits::<8>(1);
        }

        #[output]
        fn output(self_q: &Self, _i: bool) -> Bits<8> {
            *self_q.count
        }
    }
}

#[test]
fn many_unused_fields_compile_and_run() {
    let uut: ManyUnusedFields = ManyUnusedFields::default();
    let stream = std::iter::repeat_n(true, 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let last = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .last()
        .map(|s| s.output.raw())
        .unwrap_or(0);
    assert!(last >= 3 && last <= 4, "got {last}");
}

// ---- Test 3: #[output] with no `self_q` parameter ---------------
//
// When the output is purely a function of the input, the user can
// drop the receiver parameter entirely.  The macro accepts both
// `fn output(self_q: &Self, i: I) -> O` and `fn output(i: I) -> O`.
// All struct fields auto-hold; the kernel function still has a
// `q: <Name>Q` parameter even though the output body doesn't read it.

rule_kernel! {
    pub struct StatelessOutput {
        unused_a: dff::DFF<bool>,
        unused_b: dff::DFF<Bits<8>>,
    }

    impl StatelessOutput {
        // The rule writes one field with a no-op (hold) so the
        // macro has at least one action to lower.  The other field
        // (`unused_b`) is auto-held since no rule mentions it.
        #[rule]
        fn noop(ctx: &mut RuleCtx<Self>, _i: bool) {
            ctx.unused_a = *ctx.unused_a;
        }

        /// Output takes no `self_q`.  Pure function of input.
        #[output]
        fn output(i: bool) -> bool {
            !i
        }
    }
}

#[test]
fn no_receiver_output_method_compiles_and_runs() {
    let uut: StatelessOutput = StatelessOutput::default();
    let stream = vec![true, false, true, false, true]
        .into_iter()
        .with_reset(2)
        .clock_pos_edge(100);
    let outputs: Vec<bool> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    // Output is `!i` for each input.
    assert_eq!(
        outputs,
        vec![false, true, false, true, false],
        "stateless output should be NOT of input",
    );
}

// ---- Test 4: unused field that the output method DOES reference --
// (Distinguishes "rule never touches but output reads" from "nothing
// touches" — both should compile, but the field-reads-from-output
// case was already handled even before auto-hold.)

rule_kernel! {
    pub struct OutputOnlyField {
        active: dff::DFF<Bits<8>>,
        observed: dff::DFF<bool>,
    }

    impl OutputOnlyField {
        #[rule]
        fn bump(ctx: &mut RuleCtx<Self>, _i: bool) {
            ctx.active = *ctx.active + bits::<8>(1);
        }

        #[output]
        fn output(self_q: &Self, _i: bool) -> (Bits<8>, bool) {
            (*self_q.active, *self_q.observed)
        }
    }
}

#[test]
fn output_only_field_compiles_and_runs() {
    let uut: OutputOnlyField = OutputOnlyField::default();
    let stream = std::iter::repeat_n(true, 3)
        .with_reset(2)
        .clock_pos_edge(100);
    let outputs: Vec<(Bits<8>, bool)> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    // `observed` is never written by any rule but IS read by output;
    // it should hold its default (false) forever.
    assert!(
        outputs.iter().all(|(_, obs)| !obs),
        "observed should always be false; got {outputs:?}",
    );
}
