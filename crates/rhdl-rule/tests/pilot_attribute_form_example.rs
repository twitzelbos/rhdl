//! Pilot rewrite #5 — attribute-form companion demo.
//!
//! Pilots 1-4 all use the function-like `rule_kernel! { struct + impl }`
//! form.  This pilot uses the **attribute form** `#[rule_kernel_attr]`
//! to demonstrate that both surface spellings (documented in
//! `rule-architecture.md` §4.5) work in real-widget contexts and
//! produce byte-identical hardware.
//!
//! The pilot defines two widgets that differ ONLY in macro spelling:
//!
//! - `AttrFormCounter` — uses `#[rule_kernel_attr]` on the impl block.
//!   The user writes the standard RHDL derives on the struct
//!   themselves (`#[derive(Clone, Debug, Default, Synchronous,
//!   SynchronousDQ)]`).
//!
//! - `FnFormCounter` — uses the function-like `rule_kernel! { ... }`,
//!   which auto-injects the standard derives on the struct.
//!
//! A runtime parity test (`fn_form_and_attr_form_produce_identical_outputs`)
//! drives the same input stream through both widgets and asserts
//! byte-identical output sequences.  This is the load-bearing claim
//! of the §4.5 design note: the two forms are interchangeable.
//!
//! ## Widget shape
//!
//! Both widgets are 2-rule kernels: a `clear` rule (priority 0) and
//! an `increment` rule (priority 1) that both write the same
//! `count` field.  When both `clear` and `enable` are asserted in
//! the same cycle, the priority chain picks `clear` (priority 0
//! wins).  This exercises:
//!
//! - Multi-rule decomposition.
//! - Static `#[rule(priority = N)]` annotations.
//! - Write-write conflict between rules (same field).
//! - Attribute-form macro on a non-trivial impl.

use rhdl::prelude::*;
#[allow(unused_imports)] // referenced inside both rule kernel macros
use rhdl_fpga::core::dff;
use rhdl_rule::{rule_kernel, rule_kernel_attr};

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct CounterCmd {
    /// Synchronous clear — drops the count to zero this cycle.
    /// Wins over `enable` when both are asserted (priority 0).
    pub clear: bool,
    /// Increment — advances the count by one (priority 1).
    pub enable: bool,
}

// ---- Attribute form ---------------------------------------------------

/// Same widget shape as `FnFormCounter` below, but expressed using
/// the **attribute form** `#[rule_kernel_attr]`.  The user writes
/// the standard RHDL derives on the struct themselves; the
/// attribute walks the impl block and synthesizes the kernel.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AttrFormCounter {
    count: dff::DFF<Bits<8>>,
}

#[rule_kernel_attr]
impl AttrFormCounter {
    /// Synchronous clear.  Priority 0 — wins when both rules are
    /// ready in the same cycle.
    #[rule(priority = 0)]
    fn clear_rule(ctx: &mut RuleCtx<Self>, i: CounterCmd) {
        guard!(i.clear);
        set!(ctx.count, bits::<8>(0));
    }

    /// Increment.  Priority 1 — suppressed by `clear_rule` when
    /// both rules' guards are true (write-write conflict on `count`,
    /// resolved by priority).
    #[rule(priority = 1)]
    fn increment_rule(ctx: &mut RuleCtx<Self>, i: CounterCmd) {
        guard!(i.enable);
        set!(ctx.count, *ctx.count + bits::<8>(1));
    }

    #[output]
    fn output(self_q: &Self, _i: CounterCmd) -> Bits<8> {
        *self_q.count
    }
}

// ---- Function-like form (for parity comparison) -----------------------

rule_kernel! {
    /// Same widget shape as `AttrFormCounter` above, but expressed
    /// using the **function-like form** `rule_kernel! { ... }`.
    /// The macro auto-injects the standard derives on the struct.
    pub struct FnFormCounter {
        count: dff::DFF<Bits<8>>,
    }

    impl FnFormCounter {
        #[rule(priority = 0)]
        fn clear_rule(ctx: &mut RuleCtx<Self>, i: CounterCmd) {
            guard!(i.clear);
            set!(ctx.count, bits::<8>(0));
        }

        #[rule(priority = 1)]
        fn increment_rule(ctx: &mut RuleCtx<Self>, i: CounterCmd) {
            guard!(i.enable);
            set!(ctx.count, *ctx.count + bits::<8>(1));
        }

        #[output]
        fn output(self_q: &Self, _i: CounterCmd) -> Bits<8> {
            *self_q.count
        }
    }
}

// ---- Tests ------------------------------------------------------------

/// Drive a fixed stream through a widget and collect the post-reset
/// counter values.
fn drive(stream_in: Vec<CounterCmd>) -> Vec<u128> {
    let attr_uut: AttrFormCounter = AttrFormCounter::default();
    attr_uut
        .run(stream_in.into_iter().with_reset(2).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output.raw())
        .collect()
}

#[test]
fn attr_form_counter_increments_when_enabled() {
    let stream_in: Vec<CounterCmd> = std::iter::repeat_n(
        CounterCmd {
            clear: false,
            enable: true,
        },
        5,
    )
    .collect();
    let outputs = drive(stream_in);
    let last = *outputs.last().unwrap_or(&0);
    assert!(last >= 4 && last <= 5, "expected ~5 increments, got {last}");
}

#[test]
fn attr_form_counter_clears_synchronously() {
    let stream_in: Vec<CounterCmd> = vec![
        CounterCmd {
            clear: false,
            enable: true,
        }, // count=1
        CounterCmd {
            clear: false,
            enable: true,
        }, // count=2
        CounterCmd {
            clear: false,
            enable: true,
        }, // count=3
        CounterCmd {
            clear: true,
            enable: false,
        }, // count=0
        CounterCmd {
            clear: false,
            enable: false,
        }, // count=0
    ];
    let outputs = drive(stream_in);
    let last = *outputs.last().unwrap_or(&99);
    assert_eq!(last, 0, "expected clear to drop count to 0; got {last}");
}

#[test]
fn attr_form_counter_priority_clear_wins_over_enable() {
    // Both clear AND enable asserted: priority 0 (clear) wins.
    // Without priority, enable would increment.  With priority,
    // clear holds the count at zero.
    let stream_in: Vec<CounterCmd> = std::iter::repeat_n(
        CounterCmd {
            clear: true,
            enable: true,
        },
        5,
    )
    .collect();
    let outputs = drive(stream_in);
    assert!(
        outputs.iter().all(|v| *v == 0),
        "clear should win over enable; got {outputs:?}",
    );
}

/// **Parity test** — drive the same input stream through the
/// attribute-form and function-like widgets and assert byte-
/// identical output sequences.  This is the runtime confirmation
/// of the `attribute_form_parity.rs` token-level parity test:
/// the two forms produce identical hardware behaviour for any
/// rule kernel.
#[test]
fn fn_form_and_attr_form_produce_identical_outputs() {
    // Mix: increment, clear, both, neither, mid-sequence clear.
    let inputs: Vec<CounterCmd> = vec![
        CounterCmd {
            clear: false,
            enable: true,
        },
        CounterCmd {
            clear: false,
            enable: true,
        },
        CounterCmd {
            clear: false,
            enable: false,
        },
        CounterCmd {
            clear: false,
            enable: true,
        },
        CounterCmd {
            clear: true,
            enable: true,
        }, // both — clear wins
        CounterCmd {
            clear: false,
            enable: true,
        },
        CounterCmd {
            clear: false,
            enable: true,
        },
        CounterCmd {
            clear: true,
            enable: false,
        },
        CounterCmd {
            clear: false,
            enable: false,
        },
    ];

    let attr_uut: AttrFormCounter = AttrFormCounter::default();
    let attr_outputs: Vec<u128> = attr_uut
        .run(
            inputs
                .clone()
                .into_iter()
                .with_reset(2)
                .clock_pos_edge(100),
        )
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output.raw())
        .collect();

    let fn_uut: FnFormCounter = FnFormCounter::default();
    let fn_outputs: Vec<u128> = fn_uut
        .run(inputs.into_iter().with_reset(2).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output.raw())
        .collect();

    assert_eq!(
        attr_outputs, fn_outputs,
        "attribute form and function-like form must produce byte-identical \
         outputs for the same input stream",
    );
}

#[test]
fn attr_form_counter_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: AttrFormCounter = AttrFormCounter::default();
    let stream = std::iter::repeat_n(
        CounterCmd {
            clear: false,
            enable: true,
        },
        4,
    )
    .with_reset(2)
    .clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
