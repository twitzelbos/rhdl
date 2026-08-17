#![warn(missing_docs)]
//! [`CreditSource<T, F, CREDIT_W>`] — converts an [`crate::rcstream::RCStream`]
//! source into a [`super::CreditRCStream`] source.
//!
//! Tracks a local credit counter, gates outgoing items on
//! `counter > 0`, signals upstream `ready` when it has credit.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +--+CreditSource+----+
      |  RCStream :  CreditRC
?Item<T,F>        :        | ?Item<T,F>
+---->+ data      :  data  +------>
      |           :        |
+---->+ ready     :        |
      |           : credit |
      |           : grant  |<------+
 bool |           :        |
<-----+ ready     :        |
      +--------------------+
")]
//!
//! Wait — the symbol above is a placeholder; the *I/O direction*
//! convention is:
//!
//! - **Input** (`I`):
//!   - `upstream_data: Option<Item<T, F>>` — the upstream
//!     `RCStream` source's data flowing in.
//!   - `credit_grant: Bits<CREDIT_W>` — the downstream
//!     `CreditRCStream` sink's credit grant flowing in.
//! - **Output** (`O`):
//!   - `upstream_ready: bool` — the ready signal flowing back to the
//!     upstream `RCStream` source.  Asserted when the local credit
//!     counter is non-zero.
//!   - `downstream_data: Option<Item<T, F>>` — the data flowing
//!     forward to the downstream `CreditRCStream` sink.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/credit_source.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/credit_source.md")]

use rhdl::prelude::*;

use crate::core::constant::Constant;
use crate::core::dff;
use crate::rcstream::bus::Item;

/// Credit-source: converts incoming `RCStream` into outgoing
/// `CreditRCStream`.
///
/// Maintains a local credit counter (`Bits<CREDIT_W>`).  Each cycle:
///
/// 1. The local counter is updated from `q.credit + i.credit_grant -
///    (sending ? 1 : 0)`, saturating at `2^CREDIT_W - 1`.
/// 2. `sending` = `i.upstream_data.is_some() && q.credit > 0`.
/// 3. `o.downstream_data = if sending { i.upstream_data } else { None }`.
/// 4. `o.upstream_ready = q.credit > 0`.
///
/// The send decision uses `q.credit` (latched at end of last cycle),
/// not the in-cycle `i.credit_grant` — this is what breaks the long
/// combinational dependency that the simple Ready/Valid form has.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CreditSource<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Local credit counter.  Number of tokens the source currently
    /// holds (= maximum number of items it can still send before
    /// running out).
    credit: dff::DFF<Bits<CREDIT_W>>,
    /// Zero-cost type-parameter carrier: a `Constant<Item<T, F>>`
    /// that propagates `T` and `F` through the struct so the
    /// SynchronousDQ derive sees them.  Synthesizes to a constant
    /// driver — no DFF state — and the kernel ignores its output.
    _marker: Constant<Item<T, F>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize> Default for CreditSource<T, F, CREDIT_W>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            credit: dff::DFF::new(bits::<CREDIT_W>(0)),
            _marker: Constant::new(Item::<T, F>::dont_care()),
        }
    }
}

/// Inputs for [`CreditSource`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// `RCStream` source-side data flowing in from upstream.
    pub upstream_data: Option<Item<T, F>>,
    /// Credit grant flowing in from the downstream `CreditRCStream`
    /// sink.  Added to the local counter this cycle (before send
    /// decision uses the LATCHED counter).
    pub credit_grant: Bits<CREDIT_W>,
}

/// Outputs from [`CreditSource`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Ready signal flowing back to the upstream `RCStream` source.
    /// Asserted when the local credit counter is non-zero (= we can
    /// accept the next item from upstream this cycle and forward it
    /// downstream).
    pub upstream_ready: bool,
    /// Data flowing forward to the downstream `CreditRCStream` sink.
    /// `Some(item)` when both `upstream_data.is_some()` AND the
    /// local credit counter is non-zero.
    pub downstream_data: Option<Item<T, F>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize> SynchronousIO for CreditSource<T, F, CREDIT_W>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    type I = In<T, F, CREDIT_W>;
    type O = Out<T, F, CREDIT_W>;
    type Kernel = credit_source_kernel<T, F, CREDIT_W>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn credit_source_kernel<T: Digital, F: Digital, const CREDIT_W: usize>(
    _cr: ClockReset,
    i: In<T, F, CREDIT_W>,
    q: Q<T, F, CREDIT_W>,
) -> (Out<T, F, CREDIT_W>, D<T, F, CREDIT_W>)
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    let mut d = D::<T, F, CREDIT_W>::dont_care();
    let mut o = Out::<T, F, CREDIT_W>::dont_care();

    // Send decision uses the LATCHED counter (q.credit), not any
    // in-cycle combinational input.  This is what breaks the long
    // sink→source combinational dependency.
    let have_credit: bool = q.credit != bits::<CREDIT_W>(0);
    let upstream_has_item: bool = match i.upstream_data {
        Some(_) => true,
        None => false,
    };
    let sending: bool = have_credit && upstream_has_item;

    // Outputs.
    o.upstream_ready = have_credit;
    o.downstream_data = if sending { i.upstream_data } else { None };

    // Update the credit counter.  add credit_grant, subtract 1 if
    // sending.  Saturating-add: if grant + counter would exceed
    // 2^CREDIT_W - 1, cap at the max value.  (For typical CREDIT_W=4
    // and grant=1 the saturation almost never fires; we add it for
    // robustness when the sink bursts grants after a long stall.)
    let add: Bits<CREDIT_W> = i.credit_grant;
    let sub: Bits<CREDIT_W> = if sending {
        bits::<CREDIT_W>(1)
    } else {
        bits::<CREDIT_W>(0)
    };
    // Compute next counter, saturating at 2^CREDIT_W - 1.  The
    // intermediate may overflow CREDIT_W bits; use a one-wider type
    // for the add, then clamp.
    let raw_next: Bits<CREDIT_W> = q.credit + add - sub;
    // Saturation: if add caused the counter to wrap (raw_next <
    // q.credit despite add > 0 and no subtract), clamp to all-ones.
    let max: Bits<CREDIT_W> = !bits::<CREDIT_W>(0);
    let saturated: bool = (add > sub) && (raw_next < q.credit);
    d.credit = if saturated { max } else { raw_next };

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _u: CreditSource<b8, (), 4> = CreditSource::default();
        let _u2: CreditSource<b16, bool, 8> = CreditSource::default();
        let _u3: CreditSource<b32, b8, 6> = CreditSource::default();
    }

    /// With zero credit and an upstream item, source does NOT send.
    #[test]
    fn kernel_no_credit_does_not_send() {
        let cr = ClockReset::dont_care();
        let item = Item::<b8, ()> {
            data: bits::<8>(0xAB),
            frame: (),
        };
        let i = In::<b8, (), 4> {
            upstream_data: Some(item),
            credit_grant: bits::<4>(0),
        };
        let q = Q::<b8, (), 4> {
            credit: bits::<4>(0),
            _marker: Item::<b8, ()>::dont_care(),
        };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        assert!(o.downstream_data.is_none(), "no credit → no send");
        assert!(!o.upstream_ready, "no credit → not ready upstream");
        // Counter stays 0 (no grant, no send).
        assert_eq!(d.credit.raw(), 0);
    }

    /// With credit and an upstream item, source sends and decrements.
    #[test]
    fn kernel_with_credit_sends_and_decrements() {
        let cr = ClockReset::dont_care();
        let item = Item::<b8, ()> {
            data: bits::<8>(0x42),
            frame: (),
        };
        let i = In::<b8, (), 4> {
            upstream_data: Some(item),
            credit_grant: bits::<4>(0),
        };
        let q = Q::<b8, (), 4> {
            credit: bits::<4>(3),
            _marker: Item::<b8, ()>::dont_care(),
        };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        match o.downstream_data {
            Some(it) => assert_eq!(it.data.raw(), 0x42),
            None => panic!("expected Some(item) when credit > 0"),
        }
        assert!(o.upstream_ready);
        // Counter decrements: 3 - 1 = 2.
        assert_eq!(d.credit.raw(), 2);
    }

    /// Credit grant accumulates into the counter even without sending.
    #[test]
    fn kernel_credit_grant_accumulates() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, (), 4> {
            upstream_data: None,
            credit_grant: bits::<4>(2),
        };
        let q = Q::<b8, (), 4> {
            credit: bits::<4>(3),
            _marker: Item::<b8, ()>::dont_care(),
        };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        assert!(o.downstream_data.is_none());
        assert!(o.upstream_ready);
        // Counter increments: 3 + 2 = 5, no decrement.
        assert_eq!(d.credit.raw(), 5);
    }

    /// Credit grant + send: net change.
    #[test]
    fn kernel_grant_and_send_simultaneously() {
        let cr = ClockReset::dont_care();
        let item = Item::<b8, ()> {
            data: bits::<8>(0x55),
            frame: (),
        };
        let i = In::<b8, (), 4> {
            upstream_data: Some(item),
            credit_grant: bits::<4>(2),
        };
        let q = Q::<b8, (), 4> {
            credit: bits::<4>(1),
            _marker: Item::<b8, ()>::dont_care(),
        };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        // Has credit (q.credit = 1 > 0), so sends.
        assert!(o.downstream_data.is_some());
        // Counter: 1 + 2 - 1 = 2.
        assert_eq!(d.credit.raw(), 2);
    }

    /// Saturation: adding too much credit caps at all-ones.
    #[test]
    fn kernel_credit_saturates_at_max() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, (), 4> {
            upstream_data: None,
            credit_grant: bits::<4>(0xF),
        };
        // Counter at max already.
        let q = Q::<b8, (), 4> {
            credit: bits::<4>(0xF),
            _marker: Item::<b8, ()>::dont_care(),
        };
        let (_o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        // 0xF + 0xF would wrap to 0xE; saturate to 0xF.
        assert_eq!(d.credit.raw(), 0xF);
    }

    /// Tier 2 — **the source's core invariant, under credit starvation.**
    ///
    /// A credit source must never send more items than it has been
    /// granted, at any point in time.  Violate that and the sink's
    /// buffer overruns and items are lost silently — which is exactly
    /// the failure mode that hid in `CreditSink`.
    ///
    /// The grant stream here deliberately dries up for long stretches,
    /// so the source spends its whole allowance and must then stall.
    /// The five pre-existing kernel tests all hold the counter at a
    /// fixed value and never exercise running out under load.
    #[test]
    fn source_never_sends_more_than_it_was_granted() {
        use rhdl::core::sim::ResetOrData;
        let uut = CreditSource::<b8, (), 5>::default();
        let mut granted: u128 = 0;
        let mut sent: u128 = 0;
        let mut need_reset = true;
        let mut phase: u32 = 0;
        let mut violated = false;
        let mut stalled_at_least_once = false;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                if output.downstream_data.is_some() {
                    sent += 1;
                }
                // THE invariant: never ahead of the grants issued.
                if sent > granted {
                    violated = true;
                }
                if !output.upstream_ready {
                    stalled_at_least_once = true;
                }
                // Grant in bursts with long dry spells between them, so
                // the source genuinely runs out.
                let grant = if phase % 20 < 3 { 1u128 } else { 0 };
                granted += grant;
                Some(ResetOrData::Data(In::<b8, (), 5> {
                    // Always offering, so the only limit is credit.
                    upstream_data: Some(Item::<b8, ()> {
                        data: b8(sent % 256),
                        frame: (),
                    }),
                    credit_grant: bits::<5>(grant),
                }))
            },
            100,
        )
        .take_while(|t| t.time < 300_000)
        .for_each(drop);

        assert!(
            !violated,
            "source sent {sent} items having been granted only {granted} — \
             it must never run ahead of its credit"
        );
        assert!(
            stalled_at_least_once,
            "the test never starved the source, so it proved nothing"
        );
        assert!(
            sent > 0,
            "and it must actually send when it does have credit"
        );
    }

    /// Stimulus for the Tier-5 digest.
    ///
    /// Grants credit only on one cycle in four, so the source runs its
    /// counter to zero and has to stop sending. Credit starvation is
    /// this widget's form of backpressure — a stream that grants every
    /// cycle keeps the counter saturated and never exercises the
    /// can't-send path, which is the path the widget exists to
    /// implement.
    fn starving_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, (), 4>)>> {
        (0..32u128)
            .map(|k| In {
                upstream_data: Some(Item::<b8, ()> {
                    data: bits::<8>(k),
                    frame: (),
                }),
                credit_grant: bits::<4>(u128::from(k.is_multiple_of(4))),
            })
            .with_reset(2)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Top module only; the DFF and `Constant` sub-modules are covered
    /// by their own snapshots.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut: CreditSource<b8, (), 4> = CreditSource::default();
        let desc = uut.descriptor("credit_source".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "credit_source")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module credit_source(input wire [1:0] clock_reset, input wire [12:0] i, output wire [9:0] o);
               wire [13:0] od;
               wire [3:0] d;
               wire [11:0] q;
               assign o = od[9:0];
               credit_source_credit c0(.clock_reset(clock_reset), .i(d[3:0]), .o(q[3:0]));
               credit_source__marker c1(.clock_reset(clock_reset), .o(q[11:4]));
               assign d = od[13:10];
               assign od = kernel_credit_source_kernel(clock_reset, i, q);
               function [13:0] kernel_credit_source_kernel(input reg [1:0] arg_0, input reg [12:0] arg_1, input reg [11:0] arg_2);
                     reg [3:0] r0;
                     reg [11:0] r1;
                     reg [0:0] r2;
                     reg [8:0] r3;
                     reg [12:0] r4;
                     reg [0:0] r5;
                     reg [0:0] r6;
                     reg [0:0] r7;
                     // o
                     reg [9:0] r8;
                     reg [8:0] r9;
                     reg [8:0] r10;
                     // o
                     reg [9:0] r11;
                     reg [3:0] r12;
                     reg [3:0] r13;
                     reg [3:0] r14;
                     reg [3:0] r15;
                     reg [3:0] r16;
                     reg [0:0] r17;
                     reg [3:0] r18;
                     reg [0:0] r19;
                     reg [0:0] r20;
                     reg [3:0] r21;
                     // d
                     reg [3:0] r22;
                     reg [13:0] r23;
                     reg [1:0] r24;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 1'b0;
                     localparam l4 = 10'bXXXXXXXXXX;
                     localparam l5 = 9'b000000000;
                     localparam l6 = 4'b0001;
                     localparam l7 = 4'b0000;
                     localparam l8 = 4'b1111;
                     localparam l9 = 4'bXXXX;
                     begin
                        r24 = arg_0;
                        r4 = arg_1;
                        r1 = arg_2;
                        r0 = r1[3:0];
                        r2 = |r0;
                        r3 = r4[8:0];
                        r5 = r3[8:8];
                        case (r5)
                           1'b1 : r6 = l1;
                           1'b0 : r6 = l3;
                        endcase
                        r7 = r2 & r6;
                        r8 = l4;
                        r8[0:0] = r2;
                        r9 = r4[8:0];
                        r10 = r7 ? r9 : l5;
                        r11 = r8;
                        r11[9:1] = r10;
                        r12 = r4[12:9];
                        r13 = r7 ? l6 : l7;
                        r14 = r1[3:0];
                        r15 = r14 + r12;
                        r16 = r15 - r13;
                        r17 = r12 > r13;
                        r18 = r1[3:0];
                        r19 = r16 < r18;
                        r20 = r17 & r19;
                        r21 = r20 ? l8 : r16;
                        r22 = l9;
                        r22[3:0] = r21;
                        r23 = {r22, r11};
                        kernel_credit_source_kernel = r23;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 5 — VCD digest, over the credit-starving stimulus.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut: CreditSource<b8, (), 4> = CreditSource::default();
        let vcd = uut.run(starving_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("credit_source");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "5231c351c1a5549aa8466e4b4a65c9fb227879290a62dd0018cf00cd8975f805"
        ];
        let digest = vcd.dump_to_file(root.join("credit_source.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// iverilog round-trip: a simple use case.  Drive 16 items
    /// through with always-ready credit grant; expect items to
    /// appear on the downstream after a 1-cycle DFF delay.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut: CreditSource<b8, (), 4> = CreditSource::default();
        let inputs: Vec<In<b8, (), 4>> = (0..16)
            .map(|k| {
                let it = Item::<b8, ()> {
                    data: bits::<8>(k as u128),
                    frame: (),
                };
                In {
                    upstream_data: Some(it),
                    credit_grant: bits::<4>(1), // grant 1 per cycle
                }
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }
}
