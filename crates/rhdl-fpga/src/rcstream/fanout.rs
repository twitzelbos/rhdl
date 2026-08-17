#![warn(missing_docs)]
//! `RCStreamFanout<T, F, N>` — broadcast one [`RCStream`] to `N` sinks.
//!
//! Every sink receives **every** item. The widget holds each item until
//! all `N` branches have taken it, then accepts the next one.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +--+RCStreamFanout+--+
?Item<T,F>                 | ?Item<T,F>
+---->+ data       data[0] +------>
      |                    | ?Item<T,F>
      |            data[1] +------>
      |               ...  |
 bool |                    | [bool; N]
<-----+ ready      ready[] |<-----+
      +--------------------+
")]
//!
//! # Fan-out is not `tee`
//!
//! [`super::tee`] **splits**: it takes a stream of `(A, B)` and sends
//! the `A`s one way and the `B`s the other. Each output sees a
//! different projection of each item, and each item is delivered once
//! per branch by construction.
//!
//! Fan-out **broadcasts**: every branch sees the same item. That is a
//! materially harder problem, and the reason this is a separate widget
//! rather than a generalisation of `tee`:
//!
//! > Two sinks can go ready on different cycles, and a held item would
//! > otherwise be delivered twice.
//!
//! Concretely — branch 0 accepts on cycle 3, branch 1 is stalled until
//! cycle 7. The item must stay on branch 1's wire for cycles 3..7, and
//! must **not** reappear on branch 0's. A widget that simply presented
//! the held item to everyone until the slowest sink took it would
//! deliver it to branch 0 five times.
//!
//! # Internals
//!
//! Three registers: the held `item`, a `busy` flag, and a `pending`
//! bitmap saying which branches still owe an acceptance.
//!
//! - Accept an item only when `!busy`; latch it and set every `pending`
//!   bit.
//! - Present the item to branch `b` only while `pending[b]` — that is
//!   the per-branch delivery state, and it is what stops a fast branch
//!   seeing the item again while a slow one catches up.
//! - Clear `pending[b]` on the cycle branch `b` asserts `ready`.
//! - When the last bit clears, drop `busy` and take the next item.
//!
//! # Throughput
//!
//! One item per `max(branch delay) + 1` cycles: the widget is idle for
//! one cycle between items, because `ready` is driven from the
//! registered `busy` rather than combinationally from the incoming
//! `ready[]`.
//!
//! That is deliberate. Deriving `ready` from `i.ready[]` would create a
//! combinational input-to-output path and break
//! [`no_combinatorial_paths`](crate::circuit::drc::no_combinatorial_paths),
//! which every `rcstream` combinator carries so that it remains a valid
//! relay-insertion point under the Carloni LID theorem. Put an
//! [`super::relay::RCStreamRelay`] on the input if the gap matters;
//! per the theorem that costs latency, not throughput.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_fanout.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_fanout.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::rcstream::bus::Item;

/// Broadcast one `RCStream` to `N` sinks, holding each item until every
/// branch has taken it.
///
/// `T` is the payload type, `F` the framing marker, `N` the number of
/// branches.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamFanout<T: Digital, F: Digital, const N: usize> {
    /// The item currently being broadcast.
    item: dff::DFF<Item<T, F>>,
    /// True while an item is in flight to at least one branch.
    busy: dff::DFF<bool>,
    /// `pending[b]` is true while branch `b` has not yet accepted the
    /// held item. This is the per-branch delivery state that keeps a
    /// fast branch from being handed the same item twice.
    pending: dff::DFF<[bool; N]>,
}

impl<T: Digital, F: Digital, const N: usize> Default for RCStreamFanout<T, F, N> {
    fn default() -> Self {
        Self {
            item: dff::DFF::new(Item::<T, F>::dont_care()),
            busy: dff::DFF::new(false),
            pending: dff::DFF::new([false; N]),
        }
    }
}

/// Inputs for [`RCStreamFanout`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<T: Digital, F: Digital, const N: usize> {
    /// Item offered by the upstream source.
    pub data: Option<Item<T, F>>,
    /// `ready[b]` is branch `b`'s ready signal.
    pub ready: [bool; N],
}

/// Outputs from [`RCStreamFanout`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<T: Digital, F: Digital, const N: usize> {
    /// `data[b]` is what branch `b` is being offered this cycle.
    pub data: [Option<Item<T, F>>; N],
    /// Ready back to the upstream source: true only when no item is in
    /// flight.
    pub ready: bool,
}

impl<T: Digital, F: Digital, const N: usize> SynchronousIO for RCStreamFanout<T, F, N> {
    type I = In<T, F, N>;
    type O = Out<T, F, N>;
    type Kernel = fanout_kernel<T, F, N>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn fanout_kernel<T: Digital, F: Digital, const N: usize>(
    _cr: ClockReset,
    i: In<T, F, N>,
    q: Q<T, F, N>,
) -> (Out<T, F, N>, D<T, F, N>) {
    let mut d = D::<T, F, N>::dont_care();
    let mut o = Out::<T, F, N>::dont_care();

    // Present the held item only to branches that still owe an
    // acceptance.  A branch that has already taken it sees `None`,
    // which is what stops a duplicate delivery while a slower branch
    // catches up.
    for b in 0..N {
        o.data[b] = if q.busy && q.pending[b] {
            Some(q.item)
        } else {
            None
        };
    }

    // Retire the branches that accepted this cycle.
    let mut next_pending = q.pending;
    for b in 0..N {
        if q.busy && q.pending[b] && i.ready[b] {
            next_pending[b] = false;
        }
    }
    let mut any_outstanding = false;
    for b in 0..N {
        if next_pending[b] {
            any_outstanding = true;
        }
    }

    // `ready` is driven from the REGISTERED `busy`, never from
    // `i.ready[]`, so there is no combinational input-to-output path
    // and the widget stays a valid relay-insertion point.
    o.ready = !q.busy;

    let mut d_item = q.item;
    let mut d_busy = q.busy;
    let mut d_pending = next_pending;

    if !q.busy {
        // Idle: take a new item if one is offered.
        if let Some(it) = i.data {
            d_item = it;
            d_busy = true;
            for b in 0..N {
                d_pending[b] = true;
            }
        }
    } else if !any_outstanding {
        // The last branch just took it.
        d_busy = false;
    }

    d.item = d_item;
    d.busy = d_busy;
    d.pending = d_pending;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::ResetOrData;

    fn item(v: u128) -> Item<b8, ()> {
        Item::<b8, ()> {
            data: bits::<8>(v),
            frame: (),
        }
    }

    #[test]
    fn default_construction() {
        let _a: RCStreamFanout<b8, (), 2> = RCStreamFanout::default();
        let _b: RCStreamFanout<b16, bool, 3> = RCStreamFanout::default();
        let _c: RCStreamFanout<b32, b4, 4> = RCStreamFanout::default();
    }

    #[test]
    fn no_combinatorial_paths() -> miette::Result<()> {
        let uut: RCStreamFanout<b8, (), 3> = RCStreamFanout::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Idle: nothing held, so nothing offered and the source may send.
    #[test]
    fn idle_offers_nothing_and_accepts() {
        let q = Q::<b8, (), 2> {
            item: item(0),
            busy: false,
            pending: [false; 2],
        };
        let i = In::<b8, (), 2> {
            data: None,
            ready: [true; 2],
        };
        let (o, d) = fanout_kernel::<b8, (), 2>(ClockReset::dont_care(), i, q);
        assert!(o.data[0].is_none());
        assert!(o.data[1].is_none());
        assert!(o.ready, "idle must accept");
        assert!(!d.busy);
    }

    /// Accepting an item arms every branch.
    #[test]
    fn accepting_arms_all_branches() {
        let q = Q::<b8, (), 3> {
            item: item(0),
            busy: false,
            pending: [false; 3],
        };
        let i = In::<b8, (), 3> {
            data: Some(item(0xAB)),
            ready: [false; 3],
        };
        let (_o, d) = fanout_kernel::<b8, (), 3>(ClockReset::dont_care(), i, q);
        assert!(d.busy);
        assert_eq!(d.item.data.raw(), 0xAB);
        assert_eq!(d.pending, [true; 3], "every branch owes an acceptance");
    }

    /// **The reason this widget exists.** A branch that has taken the
    /// item is offered `None` while a slower branch still holds it —
    /// otherwise the fast branch would receive it repeatedly.
    #[test]
    fn a_retired_branch_is_not_offered_the_item_again() {
        // Branch 0 has already accepted; branch 1 has not.
        let q = Q::<b8, (), 2> {
            item: item(0x42),
            busy: true,
            pending: [false, true],
        };
        let i = In::<b8, (), 2> {
            data: None,
            ready: [true, false],
        };
        let (o, d) = fanout_kernel::<b8, (), 2>(ClockReset::dont_care(), i, q);
        assert!(
            o.data[0].is_none(),
            "a branch that already took the item must not see it again"
        );
        assert_eq!(
            o.data[1].map(|it| it.data.raw()),
            Some(0x42),
            "the outstanding branch must still be offered it"
        );
        assert!(d.busy, "still busy while branch 1 owes an acceptance");
        assert!(!o.ready, "must not accept a new item mid-broadcast");
    }

    /// The last acceptance releases the widget.
    #[test]
    fn the_last_acceptance_clears_busy() {
        let q = Q::<b8, (), 2> {
            item: item(0x42),
            busy: true,
            pending: [false, true],
        };
        let i = In::<b8, (), 2> {
            data: None,
            ready: [false, true], // branch 1 takes it now
        };
        let (_o, d) = fanout_kernel::<b8, (), 2>(ClockReset::dont_care(), i, q);
        assert!(!d.busy, "no branches outstanding -> release");
        assert_eq!(d.pending, [false; 2]);
    }

    /// Tier 2 — every branch receives every item, in order, when the
    /// branches drain at **different, coprime rates**.
    ///
    /// Equal rates would let all branches retire together on every item
    /// and never exercise the hold — the case the widget exists for.
    #[test]
    fn all_branches_receive_every_item_at_different_rates() {
        const COUNT: u128 = 12;
        let uut: RCStreamFanout<b8, (), 3> = RCStreamFanout::default();
        let mut to_send: u128 = 0;
        let mut got: [Vec<u128>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut need_reset = true;
        let mut phase: u128 = 0;

        uut.run_fn(
            |o| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase += 1;
                // Coprime cadences: 1-in-2, 1-in-3, 1-in-5.
                let ready = [
                    phase.is_multiple_of(2),
                    phase.is_multiple_of(3),
                    phase.is_multiple_of(5),
                ];
                for b in 0..3 {
                    if ready[b] {
                        if let Some(it) = o.data[b] {
                            got[b].push(it.data.raw());
                        }
                    }
                }
                let mut input = In::<b8, (), 3> { data: None, ready };
                if to_send < COUNT && o.ready {
                    input.data = Some(item(to_send));
                    to_send += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 400_000)
        .for_each(drop);

        let want: Vec<u128> = (0..COUNT).collect();
        assert_eq!(to_send, COUNT, "the source must not stall forever");
        for b in 0..3 {
            assert_eq!(
                got[b], want,
                "branch {b} must receive every item exactly once, in order"
            );
        }
    }

    fn bench_stream() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, (), 3>)>> {
        (0..36u128)
            .map(|k| In::<b8, (), 3> {
                data: if k.is_multiple_of(4) {
                    None
                } else {
                    Some(item(k % 256))
                },
                ready: [
                    k.is_multiple_of(2),
                    k.is_multiple_of(3),
                    k.is_multiple_of(5),
                ],
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot (top module only).
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut: RCStreamFanout<b8, (), 3> = RCStreamFanout::default();
        let desc = uut.descriptor("rcstream_fanout".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_fanout")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_fanout(input wire [1:0] clock_reset, input wire [11:0] i, output wire [27:0] o);
               wire [39:0] od;
               wire [11:0] d;
               wire [11:0] q;
               assign o = od[27:0];
               rcstream_fanout_item c0(.clock_reset(clock_reset), .i(d[7:0]), .o(q[7:0]));
               rcstream_fanout_busy c1(.clock_reset(clock_reset), .i(d[8:8]), .o(q[8:8]));
               rcstream_fanout_pending c2(.clock_reset(clock_reset), .i(d[11:9]), .o(q[11:9]));
               assign d = od[39:28];
               assign od = kernel_fanout_kernel(clock_reset, i, q);
               function [39:0] kernel_fanout_kernel(input reg [1:0] arg_0, input reg [11:0] arg_1, input reg [11:0] arg_2);
                     reg [0:0] r0;
                     reg [11:0] r1;
                     reg [2:0] r2;
                     reg [0:0] r3;
                     reg [0:0] r4;
                     reg [7:0] r5;
                     reg [8:0] r6;
                     reg [7:0] r7;
                     reg [8:0] r8;
                     // o
                     reg [27:0] r9;
                     reg [0:0] r10;
                     reg [2:0] r11;
                     reg [0:0] r12;
                     reg [0:0] r13;
                     reg [7:0] r14;
                     reg [8:0] r15;
                     reg [7:0] r16;
                     reg [8:0] r17;
                     // o
                     reg [27:0] r18;
                     reg [0:0] r19;
                     reg [2:0] r20;
                     reg [0:0] r21;
                     reg [0:0] r22;
                     reg [7:0] r23;
                     reg [8:0] r24;
                     reg [7:0] r25;
                     reg [8:0] r26;
                     // o
                     reg [27:0] r27;
                     reg [2:0] r28;
                     reg [0:0] r29;
                     reg [2:0] r30;
                     reg [0:0] r31;
                     reg [0:0] r32;
                     reg [2:0] r33;
                     reg [11:0] r34;
                     reg [0:0] r35;
                     reg [0:0] r36;
                     // next_pending
                     reg [2:0] r37;
                     // next_pending
                     reg [2:0] r38;
                     reg [0:0] r39;
                     reg [2:0] r40;
                     reg [0:0] r41;
                     reg [0:0] r42;
                     reg [2:0] r43;
                     reg [0:0] r44;
                     reg [0:0] r45;
                     // next_pending
                     reg [2:0] r46;
                     // next_pending
                     reg [2:0] r47;
                     reg [0:0] r48;
                     reg [2:0] r49;
                     reg [0:0] r50;
                     reg [0:0] r51;
                     reg [2:0] r52;
                     reg [0:0] r53;
                     reg [0:0] r54;
                     // next_pending
                     reg [2:0] r55;
                     // next_pending
                     reg [2:0] r56;
                     reg [0:0] r57;
                     // any_outstanding
                     reg [0:0] r58;
                     reg [0:0] r59;
                     // any_outstanding
                     reg [0:0] r60;
                     reg [0:0] r61;
                     // any_outstanding
                     reg [0:0] r62;
                     reg [0:0] r63;
                     reg [0:0] r64;
                     // o
                     reg [27:0] r65;
                     reg [7:0] r66;
                     reg [0:0] r67;
                     reg [0:0] r68;
                     reg [0:0] r69;
                     reg [8:0] r70;
                     reg [0:0] r71;
                     reg [7:0] r72;
                     // d_pending
                     reg [2:0] r73;
                     // d_pending
                     reg [2:0] r74;
                     // d_pending
                     reg [2:0] r75;
                     // d_busy
                     reg [0:0] r76;
                     // d_item
                     reg [7:0] r77;
                     // d_pending
                     reg [2:0] r78;
                     reg [0:0] r79;
                     // d_busy
                     reg [0:0] r80;
                     // d_busy
                     reg [0:0] r81;
                     // d_item
                     reg [7:0] r82;
                     // d_pending
                     reg [2:0] r83;
                     // d
                     reg [11:0] r84;
                     // d
                     reg [11:0] r85;
                     // d
                     reg [11:0] r86;
                     reg [39:0] r87;
                     reg [1:0] r88;
                     localparam l0 = 1'b1;
                     localparam l1 = 9'b000000000;
                     localparam l2 = 28'bXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l3 = 1'b1;
                     localparam l4 = 9'b000000000;
                     localparam l5 = 1'b1;
                     localparam l6 = 9'b000000000;
                     localparam l7 = 1'b0;
                     localparam l8 = 1'b0;
                     localparam l9 = 1'b0;
                     localparam l10 = 1'b1;
                     localparam l11 = 1'b0;
                     localparam l12 = 1'b1;
                     localparam l13 = 1'b1;
                     localparam l14 = 1'b1;
                     localparam l15 = 1'b1;
                     localparam l16 = 1'b1;
                     localparam l17 = 1'b1;
                     localparam l18 = 1'b1;
                     localparam l19 = 1'b0;
                     localparam l20 = 12'bXXXXXXXXXXXX;
                     begin
                        r88 = arg_0;
                        r34 = arg_1;
                        r1 = arg_2;
                        r0 = r1[8:8];
                        r2 = r1[11:9];
                        r3 = r2[0:0];
                        r4 = r0 & r3;
                        r5 = r1[7:0];
                        r7 = r5[7:0];
                        r6 = {l0, r7};
                        r8 = r4 ? r6 : l1;
                        r9 = l2;
                        r9[8:0] = r8;
                        r10 = r1[8:8];
                        r11 = r1[11:9];
                        r12 = r11[1:1];
                        r13 = r10 & r12;
                        r14 = r1[7:0];
                        r16 = r14[7:0];
                        r15 = {l3, r16};
                        r17 = r13 ? r15 : l4;
                        r18 = r9;
                        r18[17:9] = r17;
                        r19 = r1[8:8];
                        r20 = r1[11:9];
                        r21 = r20[2:2];
                        r22 = r19 & r21;
                        r23 = r1[7:0];
                        r25 = r23[7:0];
                        r24 = {l5, r25};
                        r26 = r22 ? r24 : l6;
                        r27 = r18;
                        r27[26:18] = r26;
                        r28 = r1[11:9];
                        r29 = r1[8:8];
                        r30 = r1[11:9];
                        r31 = r30[0:0];
                        r32 = r29 & r31;
                        r33 = r34[11:9];
                        r35 = r33[0:0];
                        r36 = r32 & r35;
                        r37 = r28;
                        r37[0:0] = l7;
                        r38 = r36 ? r37 : r28;
                        r39 = r1[8:8];
                        r40 = r1[11:9];
                        r41 = r40[1:1];
                        r42 = r39 & r41;
                        r43 = r34[11:9];
                        r44 = r43[1:1];
                        r45 = r42 & r44;
                        r46 = r38;
                        r46[1:1] = l8;
                        r47 = r45 ? r46 : r38;
                        r48 = r1[8:8];
                        r49 = r1[11:9];
                        r50 = r49[2:2];
                        r51 = r48 & r50;
                        r52 = r34[11:9];
                        r53 = r52[2:2];
                        r54 = r51 & r53;
                        r55 = r47;
                        r55[2:2] = l9;
                        r56 = r54 ? r55 : r47;
                        r57 = r56[0:0];
                        r58 = r57 ? l10 : l11;
                        r59 = r56[1:1];
                        r60 = r59 ? l12 : r58;
                        r61 = r56[2:2];
                        r62 = r61 ? l13 : r60;
                        r63 = r1[8:8];
                        r64 = ~r63;
                        r65 = r27;
                        r65[27:27] = r64;
                        r66 = r1[7:0];
                        r67 = r1[8:8];
                        r68 = r1[8:8];
                        r69 = ~r68;
                        r70 = r34[8:0];
                        r71 = r70[8:8];
                        r72 = r70[7:0];
                        r73 = r56;
                        r73[0:0] = l14;
                        r74 = r73;
                        r74[1:1] = l15;
                        r75 = r74;
                        r75[2:2] = l16;
                        case (r71)
                           1'b1 : r76 = l18;
                           default : r76 = r67;
                        endcase
                        case (r71)
                           1'b1 : r77 = r72;
                           default : r77 = r66;
                        endcase
                        case (r71)
                           1'b1 : r78 = r75;
                           default : r78 = r56;
                        endcase
                        r79 = ~r62;
                        r80 = r79 ? l19 : r67;
                        r81 = r69 ? r76 : r80;
                        r82 = r69 ? r77 : r66;
                        r83 = r69 ? r78 : r56;
                        r84 = l20;
                        r84[7:0] = r82;
                        r85 = r84;
                        r85[8:8] = r81;
                        r86 = r85;
                        r86[11:9] = r83;
                        r87 = {r86, r65};
                        kernel_fanout_kernel = r87;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — iverilog round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut: RCStreamFanout<b8, (), 3> = RCStreamFanout::default();
        let tb = uut
            .run(bench_stream())
            .collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut: RCStreamFanout<b8, (), 3> = RCStreamFanout::default();
        let vcd = uut.run(bench_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_fanout");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "05c4d9b189917fe266e711e41af2e4ff652a87bf780f89ca03dfe9c6b8c6ba20"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_fanout.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
