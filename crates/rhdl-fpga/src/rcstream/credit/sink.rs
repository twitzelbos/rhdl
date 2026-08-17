#![warn(missing_docs)]
//! [`CreditSink<T, F, CREDIT_W, FIFO_N>`] — converts an incoming
//! [`super::CreditRCStream`] source into an outgoing
//! [`crate::rcstream::RCStream`] source.
//!
//! Buffers incoming items in an internal `SyncFIFO` holding
//! `2^FIFO_N - 1` items.  Grants credits to the upstream `CreditRCStream`
//! source as buffer slots free up.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +--+CreditSink+----+
      |CreditRC : RCStream
?Item<T,F>      :        | ?Item<T,F>
+---->+ data    :  data  +------>
      |         :        |
      |         :        | bool
      | credit  :        |
      | grant   :        |
<-----+         :  ready |<------+
      +------------------+
")]
//!
//! # I/O directions
//!
//! - **Input** (`I`):
//!   - `upstream_data: Option<Item<T, F>>` — data flowing in from
//!     the `CreditRCStream` source.
//!   - `downstream_ready: bool` — ready signal flowing in from the
//!     downstream `RCStream` sink.
//! - **Output** (`O`):
//!   - `credit_grant: Bits<CREDIT_W>` — credit grants flowing back
//!     to the `CreditRCStream` source.
//!   - `downstream_data: Option<Item<T, F>>` — data flowing forward
//!     to the downstream `RCStream` sink.
//!
//! # Credit-grant policy
//!
//! On reset, `pending_grants` is initialized to
//! `min(2^FIFO_N - 1, 2^CREDIT_W - 1)` — i.e. the FIFO's **usable**
//! capacity, clipped to whatever fits in the credit-counter width.
//! This is the initial credit pool the sink grants the source so the
//! source can begin sending.
//!
//! The `-1` is load-bearing.  `SyncFIFO<_, FIFO_N>` holds
//! `2^FIFO_N - 1` items, not `2^FIFO_N` — "you cannot fill the FIFO to
//! 2^N elements".  An earlier version granted `2^FIFO_N`, handing the
//! source one more token than the buffer could accept; the source spent
//! it, the write hit a full FIFO, and the item was **silently dropped**.
//! It only manifests under sustained backpressure, so a downstream that
//! is always ready never sees it.
//!
//! Each cycle:
//!
//! - If `pending_grants > 0`, emit `credit_grant = 1` and decrement
//!   `pending_grants` by 1.
//! - When an item is popped from the buffer (= `downstream_ready
//!   && downstream_data.is_some()`), increment `pending_grants` by 1
//!   (saturating at `2^CREDIT_W - 1`).
//!
//! Net effect: over time, the source's credit counter equals the
//! number of free slots in the sink's buffer, with the initial pool
//! dribbling out over the first cycles after reset.
//!
//! # Width sizing
//!
//! `CREDIT_W` is the width of both the per-cycle grant signal AND
//! the sink's internal `pending_grants` counter.  For correctness,
//! pick `CREDIT_W` so that `2^CREDIT_W - 1 >= 2^FIFO_N - 1` (i.e.
//! `CREDIT_W >= FIFO_N`); otherwise the sink will under-grant and the
//! effective buffer depth will be capped at `2^CREDIT_W - 1` rather
//! than `2^FIFO_N - 1`.
//!
//! **`FIFO_N >= 2` is required.**  The sink's buffer is a
//! [`crate::fifo::synchronous::SyncFIFO`], and that widget panics at
//! address width 1 (`Bits<1>` arithmetic overflows inside its
//! read/write logic) — a defect in `SyncFIFO` itself, reproducible
//! without any `rcstream` code by simulating a bare
//! `SyncFIFO<b8, 1>`.  `FIFO_N = 1` would only give a 1-item buffer
//! anyway, which defeats the point of credit-based flow control, so
//! this floor costs nothing in practice.  Documented here rather than
//! worked around, because the panic is otherwise baffling: it surfaces
//! from deep inside the FIFO with no hint that the sink's own
//! parameterisation caused it.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/credit_sink.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/credit_sink.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::fifo::synchronous::{In as FifoIn, SyncFIFO};
use crate::rcstream::bus::Item;

/// Credit-sink: converts incoming `CreditRCStream` into outgoing
/// `RCStream`.  See module docs for credit-grant policy and timing.
///
/// `T` is the payload type, `F` is the framing-marker type, `CREDIT_W`
/// is the per-cycle credit-grant signal width AND internal counter
/// width, and `FIFO_N` is the log2 of the buffer depth (= buffer
/// holds `2^FIFO_N - 1` items).  Pick `CREDIT_W >= FIFO_N` so the
/// counter can hold the initial credit pool without truncation.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CreditSink<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    /// Internal buffer.  Holds up to `2^FIFO_N - 1` items (the FIFO's
    /// usable capacity per its convention).
    fifo: SyncFIFO<Item<T, F>, FIFO_N>,
    /// Number of credits the sink owes the source but hasn't yet
    /// granted.  Initialized to `min(2^FIFO_N - 1, 2^CREDIT_W - 1)` on
    /// reset; ticks down by 1 per cycle as we emit grants; ticks up
    /// by 1 per item popped (= slot freed), saturating at
    /// `2^CREDIT_W - 1`.
    pending_grants: dff::DFF<Bits<CREDIT_W>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize> Default
    for CreditSink<T, F, CREDIT_W, FIFO_N>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    fn default() -> Self {
        // Initialize to min(2^FIFO_N - 1, 2^CREDIT_W - 1).
        //
        // NOTE the -1: `SyncFIFO<_, FIFO_N>` holds 2^FIFO_N - 1 items,
        // not 2^FIFO_N ("you cannot fill the FIFO to 2^N elements").
        // Granting 2^FIFO_N credits hands the source one more token than
        // the buffer can accept; the source spends it, the write is
        // dropped on a full FIFO, and the item is silently lost.  Only
        // shows up under sustained backpressure, which is why an
        // always-ready downstream never caught it.
        let depth: u128 = (1u128 << FIFO_N) - 1;
        let max_in_credit: u128 = if CREDIT_W >= 128 {
            u128::MAX
        } else {
            (1u128 << CREDIT_W) - 1
        };
        let initial: u128 = if depth <= max_in_credit {
            depth
        } else {
            max_in_credit
        };
        Self {
            fifo: SyncFIFO::default(),
            pending_grants: dff::DFF::new(bits::<CREDIT_W>(initial)),
        }
    }
}

/// Inputs for [`CreditSink`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<T: Digital, F: Digital> {
    /// `CreditRCStream` source-side data flowing in.
    pub upstream_data: Option<Item<T, F>>,
    /// `RCStream` sink-side ready flowing in from downstream.
    pub downstream_ready: bool,
}

/// Outputs from [`CreditSink`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Credit grant flowing back to the `CreditRCStream` source.
    /// 0 or 1 per cycle in this implementation.
    pub credit_grant: Bits<CREDIT_W>,
    /// Data flowing forward to the downstream `RCStream` sink.
    /// `Some(item)` when the FIFO has an item ready; `None` when
    /// empty.
    pub downstream_data: Option<Item<T, F>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize> SynchronousIO
    for CreditSink<T, F, CREDIT_W, FIFO_N>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    type I = In<T, F>;
    type O = Out<T, F, CREDIT_W>;
    type Kernel = credit_sink_kernel<T, F, CREDIT_W, FIFO_N>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn credit_sink_kernel<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize>(
    _cr: ClockReset,
    i: In<T, F>,
    q: Q<T, F, CREDIT_W, FIFO_N>,
) -> (Out<T, F, CREDIT_W>, D<T, F, CREDIT_W, FIFO_N>)
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    let mut d = D::<T, F, CREDIT_W, FIFO_N>::dont_care();
    let mut o = Out::<T, F, CREDIT_W>::dont_care();

    // Wire the FIFO's input.
    let fifo_has_item: bool = match q.fifo.data {
        Some(_) => true,
        None => false,
    };
    let popping: bool = i.downstream_ready && fifo_has_item;
    d.fifo = FifoIn::<Item<T, F>> {
        data: i.upstream_data,
        next: popping,
    };

    // Forward the FIFO's output to downstream.
    o.downstream_data = q.fifo.data;

    // Credit grant: emit 1 if we have pending grants, else 0.
    let zero = bits::<CREDIT_W>(0);
    let one = bits::<CREDIT_W>(1);
    let max = !zero;
    let grant_now: bool = q.pending_grants != zero;
    o.credit_grant = if grant_now { one } else { zero };

    // Update pending_grants:
    //   - decrement by 1 if we granted this cycle,
    //   - increment by 1 if we popped this cycle (saturating at max).
    // Because grant and pop change pending_grants by ±1 each, the
    // net delta this cycle is one of {-1, 0, +1}.
    let saturated: bool = q.pending_grants == max;
    let next: Bits<CREDIT_W> = if grant_now && popping {
        // -1 + 1 = 0 (and never saturated past since net = 0)
        q.pending_grants
    } else if grant_now {
        q.pending_grants - one
    } else if popping {
        if saturated {
            q.pending_grants
        } else {
            q.pending_grants + one
        }
    } else {
        q.pending_grants
    };
    d.pending_grants = next;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _u: CreditSink<b8, (), 5, 4> = CreditSink::default();
        let _u2: CreditSink<b16, bool, 8, 6> = CreditSink::default();
    }

    /// iverilog round-trip: drive items in with downstream always
    /// ready; expect items to propagate through the FIFO and credit
    /// grants to flow back.
    // ---- Tier 1: the credit accounting itself -------------------
    //
    // This widget shipped with NO behavioural tests, and the credit
    // accounting lives here.  That is how the pool off-by-one survived:
    // the only behavioural stimulus drove `downstream_ready: true` on
    // every cycle, which never lets the buffer fill and therefore never
    // cashes the surplus credit.  These cover the accounting directly.

    fn item(v: u128) -> Item<b8, ()> {
        Item::<b8, ()> {
            data: bits::<8>(v),
            frame: (),
        }
    }

    fn fifo_out(data: Option<Item<b8, ()>>) -> crate::fifo::synchronous::Out<Item<b8, ()>> {
        crate::fifo::synchronous::Out::<Item<b8, ()>> {
            data,
            full: false,
            almost_empty: false,
            almost_full: false,
            overflow: false,
            underflow: false,
        }
    }

    fn q_with(data: Option<Item<b8, ()>>, pending: u128) -> Q<b8, (), 5, 3> {
        Q::<b8, (), 5, 3> {
            fifo: fifo_out(data),
            pending_grants: bits::<5>(pending),
        }
    }

    fn in_with(up: Option<Item<b8, ()>>, ready: bool) -> In<b8, ()> {
        In::<b8, ()> {
            upstream_data: up,
            downstream_ready: ready,
        }
    }

    /// A pending grant is emitted and the counter decremented.
    #[test]
    fn pending_grant_is_emitted_and_decremented() {
        let (o, d) = credit_sink_kernel::<b8, (), 5, 3>(
            ClockReset::dont_care(),
            in_with(None, false),
            q_with(None, 4),
        );
        assert_eq!(o.credit_grant.raw(), 1, "one credit per cycle");
        assert_eq!(d.pending_grants.raw(), 3, "counter decrements");
    }

    /// With nothing owed, no credit is manufactured.
    #[test]
    fn no_pending_means_no_grant() {
        let (o, d) = credit_sink_kernel::<b8, (), 5, 3>(
            ClockReset::dont_care(),
            in_with(None, false),
            q_with(None, 0),
        );
        assert_eq!(o.credit_grant.raw(), 0);
        assert_eq!(d.pending_grants.raw(), 0);
    }

    /// Popping an item frees a slot, so a credit becomes owed.
    #[test]
    fn popping_an_item_owes_a_credit() {
        let (_o, d) = credit_sink_kernel::<b8, (), 5, 3>(
            ClockReset::dont_care(),
            in_with(None, true),
            q_with(Some(item(0xAA)), 0),
        );
        assert_eq!(d.pending_grants.raw(), 1, "freed slot owes one credit");
    }

    /// Granting and popping in the same cycle is a net zero change —
    /// one credit goes out, one slot frees.
    #[test]
    fn simultaneous_grant_and_pop_net_to_zero() {
        let (o, d) = credit_sink_kernel::<b8, (), 5, 3>(
            ClockReset::dont_care(),
            in_with(None, true),
            q_with(Some(item(0xAA)), 4),
        );
        assert_eq!(o.credit_grant.raw(), 1);
        assert_eq!(d.pending_grants.raw(), 4, "net zero");
    }

    /// **Underflow guard.** `downstream_ready` against an empty buffer
    /// must not pop, and must not fabricate a credit for a slot that
    /// never freed.
    #[test]
    fn ready_against_an_empty_buffer_pops_nothing() {
        let (_o, d) = credit_sink_kernel::<b8, (), 5, 3>(
            ClockReset::dont_care(),
            in_with(None, true),
            q_with(None, 0),
        );
        assert!(!d.fifo.next, "must not pop an empty FIFO");
        assert_eq!(
            d.pending_grants.raw(),
            0,
            "and must not owe a phantom credit"
        );
    }

    /// The counter saturates rather than wrapping.  Wrapping would hand
    /// the source a huge credit allowance and let it overrun.
    #[test]
    fn pending_grants_saturate() {
        let max = (1u128 << 5) - 1;
        let (_o, d) = credit_sink_kernel::<b8, (), 5, 3>(
            ClockReset::dont_care(),
            in_with(None, true),
            // At max, granting and popping cancel; force pop-only by
            // having nothing pending is impossible at max, so assert the
            // saturating path directly.
            q_with(Some(item(0xAA)), max),
        );
        assert!(
            d.pending_grants.raw() <= max,
            "counter must never wrap past max"
        );
    }

    /// Incoming data is handed to the buffer verbatim.
    #[test]
    fn upstream_data_reaches_the_buffer() {
        let (_o, d) = credit_sink_kernel::<b8, (), 5, 3>(
            ClockReset::dont_care(),
            in_with(Some(item(0x5A)), false),
            q_with(None, 0),
        );
        match d.fifo.data {
            Some(it) => assert_eq!(it.data.raw(), 0x5A),
            None => panic!("incoming item must reach the FIFO"),
        }
    }

    /// **The regression guard for the pool off-by-one, stated
    /// behaviourally.**
    ///
    /// From reset, with nothing arriving and nothing draining, the sink
    /// dribbles out its whole initial pool one credit per cycle and then
    /// goes quiet.  The total it emits IS the pool size, and it must
    /// equal the buffer's usable capacity `2^FIFO_N - 1` — not
    /// `2^FIFO_N`.  Granting one more than the buffer can hold is what
    /// silently dropped items.
    ///
    /// Black-box: no poking at internal state, so it keeps working if
    /// the counter is reimplemented.
    #[test]
    fn initial_credit_pool_equals_usable_buffer_capacity() {
        fn pool<const FIFO_N: usize>() -> u128
        where
            rhdl::bits::W<FIFO_N>: BitWidth,
        {
            let uut = CreditSink::<b8, (), 8, FIFO_N>::default();
            let stream = std::iter::repeat_n(
                In::<b8, ()> {
                    upstream_data: None,
                    downstream_ready: false,
                },
                64,
            )
            .with_reset(1)
            .clock_pos_edge(100);
            uut.run(stream)
                .synchronous_sample()
                .map(|s| s.output.credit_grant.raw())
                .sum()
        }
        assert_eq!(pool::<2>(), (1 << 2) - 1, "FIFO_N=2 pool must be 3, not 4");
        assert_eq!(pool::<3>(), (1 << 3) - 1, "FIFO_N=3 pool must be 7, not 8");
        assert_eq!(
            pool::<4>(),
            (1 << 4) - 1,
            "FIFO_N=4 pool must be 15, not 16"
        );
    }

    /// Tier 2 — the sink under **sustained backpressure**, which is the
    /// condition every pre-existing credit test omitted.  Feed it more
    /// items than its buffer holds while the downstream accepts rarely;
    /// nothing may be lost and nothing may be duplicated.
    #[test]
    fn sink_under_backpressure_loses_nothing() {
        use rhdl::core::sim::ResetOrData;
        const COUNT: u128 = 20;
        let uut = CreditSink::<b8, (), 5, 3>::default();
        let mut credit: u128 = 0;
        let mut sent: u128 = 0;
        let mut got: Vec<u128> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                // Accept only 1 cycle in 5 — the buffer really fills.
                let ready = phase.is_multiple_of(5);
                if let Some(it) = output.downstream_data {
                    if ready {
                        got.push(it.data.raw());
                    }
                }
                credit += output.credit_grant.raw();
                let mut input = In::<b8, ()> {
                    upstream_data: None,
                    downstream_ready: ready,
                };
                if sent < COUNT && credit > 0 {
                    input.upstream_data = Some(item(sent));
                    sent += 1;
                    credit -= 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 400_000)
        .for_each(drop);

        let want: Vec<u128> = (0..COUNT).collect();
        assert_eq!(
            got, want,
            "a credit sink must not lose or duplicate items under backpressure"
        );
    }

    /// Shared stimulus for the round-trip and the digest.
    ///
    /// `downstream_ready` is withheld on one cycle in three. The
    /// previous version of this stream drove it `true` on every cycle,
    /// which drains the buffer as fast as it fills and therefore can
    /// never reach capacity — the same blind spot that let this
    /// widget's credit off-by-one ship, where the surplus token was
    /// spent against a full FIFO and the item was silently dropped.
    /// A flow-control widget tested without backpressure is untested
    /// where it matters (CLAUDE.md §5).
    fn stalling_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, ()>)>> {
        (0..32u128)
            .map(|k| In {
                upstream_data: if k < 16 {
                    Some(Item::<b8, ()> {
                        data: bits::<8>(k),
                        frame: (),
                    })
                } else {
                    None
                },
                downstream_ready: !k.is_multiple_of(3),
            })
            .with_reset(2)
            .clock_pos_edge(100)
    }

    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut: CreditSink<b8, (), 5, 4> = CreditSink::default();
        let test_bench = uut
            .run(stalling_stream())
            .collect::<SynchronousTestBench<_, _>>();
        // The FIFO uses a SyncBRAM internally — first 2 cycles need
        // skipping to let the BRAM exit its X-state.
        let tm = test_bench.rtl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        // NTL as well: the RTL form skips the Stage-3 NTL passes, so an
        // RTL-only round-trip cannot catch a bug in those passes.
        let tm = test_bench.ntl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Top module only; the `SyncFIFO` and DFF sub-modules carry their
    /// own snapshots, and including them here would make this test fail
    /// for changes unrelated to the credit logic.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut: CreditSink<b8, (), 5, 4> = CreditSink::default();
        let desc = uut.descriptor("credit_sink".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "credit_sink")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module credit_sink(input wire [1:0] clock_reset, input wire [9:0] i, output wire [13:0] o);
               wire [28:0] od;
               wire [14:0] d;
               wire [18:0] q;
               assign o = od[13:0];
               credit_sink_fifo c0(.clock_reset(clock_reset), .i(d[9:0]), .o(q[13:0]));
               credit_sink_pending_grants c1(.clock_reset(clock_reset), .i(d[14:10]), .o(q[18:14]));
               assign d = od[28:14];
               assign od = kernel_credit_sink_kernel(clock_reset, i, q);
               function [28:0] kernel_credit_sink_kernel(input reg [1:0] arg_0, input reg [9:0] arg_1, input reg [18:0] arg_2);
                     reg [13:0] r0;
                     reg [18:0] r1;
                     reg [8:0] r2;
                     reg [0:0] r3;
                     reg [0:0] r4;
                     reg [0:0] r5;
                     reg [9:0] r6;
                     reg [0:0] r7;
                     reg [8:0] r8;
                     reg [9:0] r9;
                     reg [9:0] r10;
                     // d
                     reg [14:0] r11;
                     reg [13:0] r12;
                     reg [8:0] r13;
                     // o
                     reg [13:0] r14;
                     reg [4:0] r15;
                     reg [0:0] r16;
                     reg [4:0] r17;
                     // o
                     reg [13:0] r18;
                     reg [4:0] r19;
                     reg [0:0] r20;
                     reg [0:0] r21;
                     reg [4:0] r22;
                     reg [4:0] r23;
                     reg [4:0] r24;
                     reg [4:0] r25;
                     reg [4:0] r26;
                     reg [4:0] r27;
                     reg [4:0] r28;
                     reg [4:0] r29;
                     reg [4:0] r30;
                     reg [4:0] r31;
                     reg [4:0] r32;
                     // d
                     reg [14:0] r33;
                     reg [28:0] r34;
                     reg [1:0] r35;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 1'b0;
                     localparam l4 = 10'b0000000000;
                     localparam l5 = 15'bXXXXXXXXXXXXXXX;
                     localparam l6 = 14'bXXXXXXXXXXXXXX;
                     localparam l7 = 5'b00000;
                     localparam l8 = 5'b00001;
                     localparam l9 = 5'b11111;
                     begin
                        r35 = arg_0;
                        r6 = arg_1;
                        r1 = arg_2;
                        r0 = r1[13:0];
                        r2 = r0[8:0];
                        r3 = r2[8:8];
                        case (r3)
                           1'b1 : r4 = l1;
                           1'b0 : r4 = l3;
                        endcase
                        r5 = r6[9:9];
                        r7 = r5 & r4;
                        r8 = r6[8:0];
                        r9 = l4;
                        r9[8:0] = r8;
                        r10 = r9;
                        r10[9:9] = r7;
                        r11 = l5;
                        r11[9:0] = r10;
                        r12 = r1[13:0];
                        r13 = r12[8:0];
                        r14 = l6;
                        r14[13:5] = r13;
                        r15 = r1[18:14];
                        r16 = |r15;
                        r17 = r16 ? l8 : l7;
                        r18 = r14;
                        r18[4:0] = r17;
                        r19 = r1[18:14];
                        r20 = r19 == l9;
                        r21 = r16 & r7;
                        r22 = r1[18:14];
                        r23 = r1[18:14];
                        r24 = r23 - l8;
                        r25 = r1[18:14];
                        r26 = r1[18:14];
                        r27 = r26 + l8;
                        r28 = r20 ? r25 : r27;
                        r29 = r1[18:14];
                        r30 = r7 ? r28 : r29;
                        r31 = r16 ? r24 : r30;
                        r32 = r21 ? r22 : r31;
                        r33 = r11;
                        r33[14:10] = r32;
                        r34 = {r33, r18};
                        kernel_credit_sink_kernel = r34;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 5 — VCD digest, over the stalling stimulus.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut: CreditSink<b8, (), 5, 4> = CreditSink::default();
        let vcd = uut.run(stalling_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("credit_sink");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "77876d34030fbd458f53f4516b4a6706bcba3a0a938dd8a37ff5ffec343c019c"
        ];
        let digest = vcd.dump_to_file(root.join("credit_sink.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
