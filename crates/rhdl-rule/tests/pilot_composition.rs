//! Pilot rewrite #4 — composition: rule-kernel widget + traditional
//! widget inside a hand-written `Synchronous` wrapper.
//!
//! Validates `rule-architecture.md` §9.1: "A `RuleKernel` widget is
//! structurally a `Synchronous` widget.  It composes with every
//! other RHDL widget by virtue of implementing `Synchronous` +
//! `SynchronousIO` + `SynchronousDQ`."  This pilot proves that
//! claim end-to-end: a hand-written `MonitoredArbiter` widget
//! contains both a rule-kernel sub-circuit (`RuleRoundRobinArbiter`
//! from Pilot #1) and a traditional sub-circuit (`dff::DFF<Bits<32>>`),
//! and its hand-written `#[kernel]` function wires them together.
//!
//! ## What the composition demonstrates
//!
//! - The two sub-circuit flavours have the same `D`/`Q` shape from
//!   the wrapper's perspective.  No special handling for the rule
//!   kernel.
//! - The wrapper's kernel reads the rule kernel's output (`q.arbiter`
//!   would be the arbiter's `Q` type — but `RuleRoundRobinArbiter`'s
//!   output is `Option<Bits<W>>`, not its register state, so we
//!   read it via the synthesized `<Name>D` to feed the next-cycle
//!   inputs).  The wrapper drives the arbiter's input via `d.arbiter`.
//! - Same widget composition idiom as any other multi-sub-circuit
//!   widget — confirms there's no impedance mismatch.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

// ---- Rule-kernel sub-circuit (one rule, one register) ----

rule_kernel! {
    /// Single-rule arbiter: grants whichever requester has the lowest
    /// index this cycle (priority-fixed, not round-robin — chosen for
    /// the composition demo because rotation needs no internal state
    /// here).  Captures the grant index in `last_idx` for visibility.
    pub struct PriorityArbiter<const N: usize, const W: usize>
    where
        rhdl::bits::W<N>: BitWidth,
        rhdl::bits::W<W>: BitWidth,
    {
        last_idx: dff::DFF<Bits<W>>,
    }

    impl PriorityArbiter {
        #[rule]
        fn arbitrate(ctx: &mut RuleCtx<Self>, requests: Bits<N>) {
            // Preamble — find the first set request bit.
            let mut winner: Bits<W> = bits::<W>(0);
            let mut found = false;
            for i in 0..N {
                let idx: Bits<W> = bits::<W>(i as u128);
                let bit_at_idx = (requests >> idx) & bits::<N>(1);
                if bit_at_idx != bits::<N>(0) && !found {
                    winner = idx;
                    found = true;
                }
            }

            // Direct-assignment write — hold prior value when no requester.
            ctx.last_idx = if found { winner } else { *ctx.last_idx };
        }

        // No `self_q` parameter — output is purely a function of
        // input.  The PR #25 workaround `let _ = *self_q.last_idx;`
        // (needed back when every struct field had to be touched and
        // `self_q` had to be declared even when unused) is gone.
        #[output]
        fn output(requests: Bits<N>) -> Option<Bits<W>> {
            let mut winner: Bits<W> = bits::<W>(0);
            let mut found = false;
            for i in 0..N {
                let idx: Bits<W> = bits::<W>(i as u128);
                let bit_at_idx = (requests >> idx) & bits::<N>(1);
                if bit_at_idx != bits::<N>(0) && !found {
                    winner = idx;
                    found = true;
                }
            }
            if found { Some(winner) } else { None }
        }
    }
}

// ---- Hand-written wrapper widget composing rule kernel + traditional widget ----

/// `MonitoredArbiter` — hand-written `Synchronous` widget that
/// contains both a rule-kernel sub-circuit (`PriorityArbiter`)
/// and a traditional sub-circuit (`dff::DFF<Bits<32>>` as a grant
/// counter).
///
/// Composition pattern: the wrapper's struct lists both sub-circuits
/// as fields; the auto-derived `Q`/`D` types reflect them; the
/// hand-written `#[kernel]` function reads `q.arbiter` and
/// `q.grant_count`, drives `d.arbiter` and `d.grant_count`, and
/// emits the wrapper's output.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct MonitoredArbiter {
    /// Rule-kernel sub-circuit.  Contributes its own register
    /// (`last_idx`) but exposes its output as `Option<Bits<2>>`.
    arbiter: PriorityArbiter<4, 2>,
    /// Traditional sub-circuit — a 32-bit counter of granted cycles.
    grant_count: dff::DFF<Bits<32>>,
}

impl Default for MonitoredArbiter {
    fn default() -> Self {
        Self {
            arbiter: PriorityArbiter::default(),
            grant_count: dff::DFF::new(bits::<32>(0)),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MonitoredArbiterOut {
    pub grant: Option<Bits<2>>,
    pub total_grants: Bits<32>,
}

impl SynchronousIO for MonitoredArbiter {
    type I = Bits<4>;
    type O = MonitoredArbiterOut;
    type Kernel = monitored_arbiter_kernel;
}

#[kernel]
/// Wrapper kernel — hand-written, composes the rule-kernel arbiter
/// with the traditional grant counter.
pub fn monitored_arbiter_kernel(
    cr: ClockReset,
    requests: Bits<4>,
    q: Q,
) -> (MonitoredArbiterOut, D) {
    let mut d = D::dont_care();
    // Drive the rule-kernel sub-circuit with the wrapper's input.
    d.arbiter = requests;

    // Read the rule-kernel's output (Option<Bits<2>> — its declared
    // SynchronousIO::O).  Increment the traditional counter when a
    // grant fires.
    let grant: Option<Bits<2>> = q.arbiter;
    let increment: Bits<32> = match grant {
        Some(_) => bits::<32>(1),
        None => bits::<32>(0),
    };
    d.grant_count = q.grant_count + increment;

    let mut o = MonitoredArbiterOut::dont_care();
    o.grant = grant;
    o.total_grants = q.grant_count;

    if cr.reset.any() {
        d.grant_count = bits::<32>(0);
        o.grant = None;
        o.total_grants = bits::<32>(0);
    }
    (o, d)
}

// ---- Tests ----

#[test]
fn monitored_arbiter_compiles_and_runs() {
    let uut = MonitoredArbiter::default();
    let stream = std::iter::repeat_n(bits::<4>(0b0001), 4)
        .with_reset(2)
        .clock_pos_edge(100);
    let outputs: Vec<MonitoredArbiterOut> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    // Rule-kernel grant should be Some(0) every cycle (only requester 0 active).
    for o in &outputs {
        assert_eq!(o.grant, Some(bits::<2>(0)), "outputs: {outputs:?}");
    }
}

#[test]
fn monitored_arbiter_counter_advances_on_grants() {
    let uut = MonitoredArbiter::default();
    // 6 cycles with requester active → 6 grants.
    let stream = std::iter::repeat_n(bits::<4>(0b0010), 6)
        .with_reset(2)
        .clock_pos_edge(100);
    let outputs: Vec<MonitoredArbiterOut> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    let final_count = outputs.last().map(|o| o.total_grants.raw()).unwrap_or(0);
    assert!(
        final_count >= 4 && final_count <= 6,
        "expected ~6 grants counted, got {final_count}",
    );
}

#[test]
fn monitored_arbiter_counter_holds_when_no_grants() {
    let uut = MonitoredArbiter::default();
    // No requests for 6 cycles — counter must stay at 0.
    let stream = std::iter::repeat_n(bits::<4>(0b0000), 6)
        .with_reset(2)
        .clock_pos_edge(100);
    let outputs: Vec<MonitoredArbiterOut> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    let final_count = outputs.last().map(|o| o.total_grants.raw()).unwrap_or(99);
    assert_eq!(
        final_count, 0,
        "no grants → counter must stay at 0; got {final_count}",
    );
    for o in &outputs {
        assert_eq!(o.grant, None, "no requester → no grant");
    }
}

#[test]
fn monitored_arbiter_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut = MonitoredArbiter::default();
    let inputs = vec![
        bits::<4>(0b0001),
        bits::<4>(0b0010),
        bits::<4>(0b0100),
        bits::<4>(0b1000),
        bits::<4>(0b0000),
        bits::<4>(0b1111),
    ];
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
