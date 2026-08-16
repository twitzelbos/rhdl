#![warn(missing_docs)]
//! Gather a stream of elements into a stream of fixed-size arrays.
//!
//! The inverse of [`super::flatten`].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+RCStreamChunked+--+
 ?T   |                    | ?[T;N]
+---->| data          data +---->
      |                    |
<-----+ ready        ready |<----+
      |                    |
      +--------------------+
")]
//!
//!# Framing: N markers arrive, one item leaves
//!
//! The mirror of [`super::flatten`]'s problem. `N` input items each
//! carry an `F`, and they become a single output item — so which `F`
//! does the chunk get?
//!
//! Picking one (first, or last) silently discards `N-1` markers, and
//! there is no principled basis for the choice: if `F` is a
//! TLAST-equivalent, the marker could land on *any* element of the
//! chunk and it matters which. So this widget keeps all of them:
//!
//! ```text
//! RCStream<T, F>  ->  RCStream<[T; N], [F; N]>
//! ```
//!
//! The chunk's framing is the array of its elements' markers, positionally
//! aligned with the payload array. A consumer that wants a single flag
//! reduces it explicitly — `map(|fs| fs.iter().any(..))` in spirit —
//! under a rule it chose rather than one this widget imposed. Same
//! principle as [`super::zip`] carrying `(F, G)` and [`super::flatten`]
//! carrying `(F, bool)`.
//!
//! The round trip is lossless — but be precise about what that means.
//! `chunked` then `flatten` returns every payload in its original order
//! and every marker, and the `k`-th element of a group carries a marker
//! array whose `k`-th entry is that element's own original marker. The
//! association is therefore recoverable **by position**, which a
//! consumer must count to exploit: `flatten` signals only
//! last-of-group, not the running index. Checked end-to-end in
//! `tests/rcstream_chunk_flatten_roundtrip.rs`.
//!
//!# One-cycle bubble per chunk
//!
//! A completed chunk occupies the output register until the sink takes
//! it, and no new element is accepted during that cycle. Sustained
//! throughput is therefore `N/(N+1)` elements per cycle rather than 1.
//! For a deeper pipeline this could be removed with a second chunk
//! buffer; that is deliberately not done here, since the cost falls with
//! `N` and the extra register is real area.
//!
//!# Sizing
//!
//! `N` is the chunk length; `M` is the width of the fill index and must
//! satisfy `2^M > N - 1`.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_chunked.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_chunked.md")]

use rhdl::prelude::*;

use crate::core::dff;

use super::bus::{Item, RCStream};
use super::relay::RCStreamRelay;

/// Gather `N` consecutive items into one `[T; N]` chunk.
///
/// Output framing is `[F; N]` — every element's marker, positionally
/// aligned with the payload. See module docs for why nothing is
/// discarded.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamChunked<T: Digital, F: Digital, const M: usize, const N: usize>
where
    rhdl::bits::W<M>: BitWidth,
{
    /// Skid buffer on the element input.
    input: RCStreamRelay<T, F>,
    /// Payload accumulator.
    acc_data: dff::DFF<[T; N]>,
    /// Framing accumulator, positionally aligned with `acc_data`.
    acc_frame: dff::DFF<[F; N]>,
    /// Index of the next slot to fill.
    idx: dff::DFF<Bits<M>>,
    /// A completed chunk is waiting to be taken.
    full: dff::DFF<bool>,
}

impl<T: Digital, F: Digital, const M: usize, const N: usize> Default for RCStreamChunked<T, F, M, N>
where
    rhdl::bits::W<M>: BitWidth,
{
    fn default() -> Self {
        Self {
            input: RCStreamRelay::default(),
            acc_data: dff::DFF::new([T::dont_care(); N]),
            acc_frame: dff::DFF::new([F::dont_care(); N]),
            idx: dff::DFF::new(bits::<M>(0)),
            full: dff::DFF::new(false),
        }
    }
}

impl<T: Digital, F: Digital, const M: usize, const N: usize> SynchronousIO
    for RCStreamChunked<T, F, M, N>
where
    rhdl::bits::W<M>: BitWidth,
{
    type I = RCStream<T, F>;
    type O = RCStream<[T; N], [F; N]>;
    type Kernel = chunked_kernel<T, F, M, N>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn chunked_kernel<T: Digital, F: Digital, const M: usize, const N: usize>(
    _cr: ClockReset,
    i: RCStream<T, F>,
    q: Q<T, F, M, N>,
) -> (RCStream<[T; N], [F; N]>, D<T, F, M, N>)
where
    rhdl::bits::W<M>: BitWidth,
{
    let mut d = D::<T, F, M, N>::dont_care();
    d.input.data = i.data;

    let (have, it) = match q.input.data {
        Some(x) => (true, x),
        None => (false, Item::<T, F>::dont_care()),
    };

    // Accept only while no completed chunk is waiting, or the pending
    // chunk would be overwritten.
    let can_accept = !q.full;
    let take = have && can_accept;
    d.input.ready = can_accept;

    // Write the element into its slot.
    let mut nd = q.acc_data;
    let mut nf = q.acc_frame;
    if take {
        nd[q.idx] = it.data;
        nf[q.idx] = it.frame;
    }
    d.acc_data = nd;
    d.acc_frame = nf;

    let last = q.idx == bits::<M>(N as u128 - 1);
    d.idx = if take {
        if last {
            bits::<M>(0)
        } else {
            q.idx + 1
        }
    } else {
        q.idx
    };

    // A chunk completes when the last slot is filled; it clears when the
    // sink takes it.  These cannot coincide: `take` requires `!q.full`.
    let emitted = q.full && i.ready;
    d.full = if take && last {
        true
    } else if emitted {
        false
    } else {
        q.full
    };

    let o = RCStream::<[T; N], [F; N]> {
        data: if q.full {
            Some(Item::<[T; N], [F; N]> {
                data: q.acc_data,
                frame: q.acc_frame,
            })
        } else {
            None
        },
        ready: q.input.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::ResetOrData;

    type Chunk = RCStreamChunked<b8, bool, 3, 4>;

    fn item(v: u128, frame: bool) -> Item<b8, bool> {
        Item::<b8, bool> {
            data: bits::<8>(v),
            frame,
        }
    }

    fn q_with(
        data: Option<Item<b8, bool>>,
        idx: u128,
        full: bool,
        ready: bool,
    ) -> Q<b8, bool, 3, 4> {
        Q::<b8, bool, 3, 4> {
            input: RCStream::<b8, bool> { data, ready },
            acc_data: [bits::<8>(0); 4],
            acc_frame: [false; 4],
            idx: bits::<3>(idx),
            full,
        }
    }

    /// Tier 1 — an accepted element lands in its slot and advances the
    /// index.
    #[test]
    fn element_lands_in_its_slot() {
        let q = q_with(Some(item(0xAB, true)), 2, false, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (_o, d) = chunked_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert_eq!(d.acc_data[2].raw(), 0xAB, "payload goes to slot 2");
        assert!(d.acc_frame[2], "marker goes to the SAME slot");
        assert_eq!(d.idx.raw(), 3, "index advances");
        assert!(!d.full, "not complete until the last slot is filled");
    }

    /// Tier 1 — filling the final slot completes the chunk.
    #[test]
    fn final_slot_completes_the_chunk() {
        let q = q_with(Some(item(0x11, false)), 3, false, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (_o, d) = chunked_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert!(d.full, "chunk is complete");
        assert_eq!(d.idx.raw(), 0, "index wraps for the next chunk");
    }

    /// Tier 1 — a completed chunk is presented, and while it waits no new
    /// element is accepted (it would overwrite the pending chunk).
    #[test]
    fn pending_chunk_blocks_further_input() {
        let q = q_with(Some(item(0x22, false)), 0, true, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: false,
        };
        let (o, d) = chunked_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_some(), "the completed chunk is presented");
        assert!(
            !d.input.ready,
            "input must be held while a chunk is pending, or it is overwritten"
        );
        assert_eq!(d.idx.raw(), 0, "and nothing is accumulated");
    }

    /// Tier 1 — the sink taking the chunk clears it and reopens the input.
    #[test]
    fn taking_the_chunk_clears_it() {
        let q = q_with(None, 0, true, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (_o, d) = chunked_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert!(!d.full, "chunk is released");
    }

    /// Tier 1 — with no pending chunk there is nothing to present.
    #[test]
    fn no_chunk_means_no_output() {
        let q = q_with(None, 1, false, true);
        let i = RCStream::<b8, bool> {
            data: None,
            ready: true,
        };
        let (o, _d) = chunked_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_none());
    }

    /// LID requirement.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Chunk::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed loop: elements gather into chunks in order, and
    /// **every** marker survives in its own slot.
    #[test]
    fn stream_gathers_elements_and_keeps_every_marker() {
        const COUNT: u128 = 24;
        let uut = Chunk::default();
        let mut sent: u128 = 0;
        let mut got: Vec<([u128; 4], [bool; 4])> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let sink_ready = !phase.is_multiple_of(3);
                if let Some(it) = output.data {
                    if sink_ready {
                        got.push((
                            [
                                it.data[0].raw(),
                                it.data[1].raw(),
                                it.data[2].raw(),
                                it.data[3].raw(),
                            ],
                            it.frame,
                        ));
                    }
                }
                let mut input = RCStream::<b8, bool> {
                    data: None,
                    ready: sink_ready,
                };
                if sent < COUNT && output.ready {
                    input.data = Some(item(sent, sent % 3 == 0));
                    sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 300_000)
        .for_each(drop);

        let want: Vec<([u128; 4], [bool; 4])> = (0..COUNT / 4)
            .map(|c| {
                let b = c * 4;
                (
                    [b, b + 1, b + 2, b + 3],
                    [
                        b % 3 == 0,
                        (b + 1) % 3 == 0,
                        (b + 2) % 3 == 0,
                        (b + 3) % 3 == 0,
                    ],
                )
            })
            .collect();
        assert_eq!(
            got, want,
            "chunks must gather in order with every marker kept"
        );
    }

    fn open_loop() -> impl Iterator<Item = TimedSample<(ClockReset, RCStream<b8, bool>)>> {
        (0..24u128)
            .map(|k| RCStream::<b8, bool> {
                data: Some(item(k, k % 3 == 0)),
                ready: k % 4 != 0,
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Chunk::default();
        let desc = uut.descriptor("rcstream_chunked".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_chunked")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_chunked(input wire [1:0] clock_reset, input wire [10:0] i, output wire [37:0] o);
               wire [88:0] od;
               wire [50:0] d;
               wire [50:0] q;
               assign o = od[37:0];
               rcstream_chunked_input c0(.clock_reset(clock_reset), .i(d[10:0]), .o(q[10:0]));
               rcstream_chunked_acc_data c1(.clock_reset(clock_reset), .i(d[42:11]), .o(q[42:11]));
               rcstream_chunked_acc_frame c2(.clock_reset(clock_reset), .i(d[46:43]), .o(q[46:43]));
               rcstream_chunked_idx c3(.clock_reset(clock_reset), .i(d[49:47]), .o(q[49:47]));
               rcstream_chunked_full c4(.clock_reset(clock_reset), .i(d[50:50]), .o(q[50:50]));
               assign d = od[88:38];
               assign od = kernel_chunked_kernel(clock_reset, i, q);
               function [88:0] kernel_chunked_kernel(input reg [1:0] arg_0, input reg [10:0] arg_1, input reg [50:0] arg_2);
                     reg [9:0] r0;
                     reg [10:0] r1;
                     // d
                     reg [50:0] r2;
                     reg [10:0] r3;
                     reg [50:0] r4;
                     reg [9:0] r5;
                     reg [0:0] r6;
                     reg [8:0] r7;
                     reg [9:0] r8;
                     reg [9:0] r9;
                     reg [0:0] r10;
                     reg [8:0] r11;
                     reg [0:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     // d
                     reg [50:0] r15;
                     reg [31:0] r16;
                     reg [3:0] r17;
                     reg [7:0] r18;
                     reg [2:0] r19;
                     // nd
                     reg [31:0] r20;
                     reg [31:0] r21;
                     reg [31:0] r22;
                     reg [31:0] r23;
                     reg [31:0] r24;
                     reg [0:0] r25;
                     reg [2:0] r26;
                     // nf
                     reg [3:0] r27;
                     reg [3:0] r28;
                     reg [3:0] r29;
                     reg [3:0] r30;
                     reg [3:0] r31;
                     // nd
                     reg [31:0] r32;
                     // nf
                     reg [3:0] r33;
                     // d
                     reg [50:0] r34;
                     // d
                     reg [50:0] r35;
                     reg [2:0] r36;
                     reg [0:0] r37;
                     reg [2:0] r38;
                     reg [2:0] r39;
                     reg [2:0] r40;
                     reg [2:0] r41;
                     reg [2:0] r42;
                     // d
                     reg [50:0] r43;
                     reg [0:0] r44;
                     reg [0:0] r45;
                     reg [0:0] r46;
                     reg [0:0] r47;
                     reg [0:0] r48;
                     reg [0:0] r49;
                     reg [0:0] r50;
                     // d
                     reg [50:0] r51;
                     reg [0:0] r52;
                     reg [31:0] r53;
                     reg [3:0] r54;
                     reg [35:0] r55;
                     reg [35:0] r56;
                     reg [36:0] r57;
                     reg [35:0] r58;
                     reg [36:0] r59;
                     reg [10:0] r60;
                     reg [0:0] r61;
                     reg [37:0] r62;
                     reg [37:0] r63;
                     reg [88:0] r64;
                     reg [1:0] r65;
                     localparam l0 = 51'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 10'bXXXXXXXXX0;
                     localparam l5 = 3'b000;
                     localparam l6 = 3'b001;
                     localparam l7 = 3'b010;
                     localparam l8 = 3'b011;
                     localparam l9 = 3'b000;
                     localparam l10 = 3'b001;
                     localparam l11 = 3'b010;
                     localparam l12 = 3'b011;
                     localparam l13 = 3'b011;
                     localparam l14 = 3'b001;
                     localparam l15 = 3'b000;
                     localparam l16 = 1'b0;
                     localparam l17 = 1'b1;
                     localparam l18 = 36'b000000000000000000000000000000000000;
                     localparam l19 = 1'b1;
                     localparam l20 = 37'b0000000000000000000000000000000000000;
                     localparam l21 = 38'b00000000000000000000000000000000000000;
                     begin
                        r65 = arg_0;
                        r1 = arg_1;
                        r4 = arg_2;
                        r0 = r1[9:0];
                        r2 = l0;
                        r2[9:0] = r0;
                        r3 = r4[10:0];
                        r5 = r3[9:0];
                        r6 = r5[9:9];
                        r7 = r5[8:0];
                        r8 = {r7, l1};
                        case (r6)
                           1'b1 : r9 = r8;
                           1'b0 : r9 = l4;
                        endcase
                        r10 = r9[0:0];
                        r11 = r9[9:1];
                        r12 = r4[50:50];
                        r13 = ~r12;
                        r14 = r10 & r13;
                        r15 = r2;
                        r15[10:10] = r13;
                        r16 = r4[42:11];
                        r17 = r4[46:43];
                        r18 = r11[7:0];
                        r19 = r4[49:47];
                        r21 = r16;
                        r21[7:0] = r18;
                        r22 = r16;
                        r22[15:8] = r18;
                        r23 = r16;
                        r23[23:16] = r18;
                        r24 = r16;
                        r24[31:24] = r18;
                        case (r19)
                           3'b000 : r20 = r21;
                           3'b001 : r20 = r22;
                           3'b010 : r20 = r23;
                           3'b011 : r20 = r24;
                        endcase
                        r25 = r11[8:8];
                        r26 = r4[49:47];
                        r28 = r17;
                        r28[0:0] = r25;
                        r29 = r17;
                        r29[1:1] = r25;
                        r30 = r17;
                        r30[2:2] = r25;
                        r31 = r17;
                        r31[3:3] = r25;
                        case (r26)
                           3'b000 : r27 = r28;
                           3'b001 : r27 = r29;
                           3'b010 : r27 = r30;
                           3'b011 : r27 = r31;
                        endcase
                        r32 = r14 ? r20 : r16;
                        r33 = r14 ? r27 : r17;
                        r34 = r15;
                        r34[42:11] = r32;
                        r35 = r34;
                        r35[46:43] = r33;
                        r36 = r4[49:47];
                        r37 = r36 == l13;
                        r38 = r4[49:47];
                        r39 = r38 + l14;
                        r40 = r37 ? l15 : r39;
                        r41 = r4[49:47];
                        r42 = r14 ? r40 : r41;
                        r43 = r35;
                        r43[49:47] = r42;
                        r44 = r4[50:50];
                        r45 = r1[10:10];
                        r46 = r44 & r45;
                        r47 = r14 & r37;
                        r48 = r4[50:50];
                        r49 = r46 ? l16 : r48;
                        r50 = r47 ? l17 : r49;
                        r51 = r43;
                        r51[50:50] = r50;
                        r52 = r4[50:50];
                        r53 = r4[42:11];
                        r54 = r4[46:43];
                        r55 = l18;
                        r55[31:0] = r53;
                        r56 = r55;
                        r56[35:32] = r54;
                        r58 = r56[35:0];
                        r57 = {l19, r58};
                        r59 = r52 ? r57 : l20;
                        r60 = r4[10:0];
                        r61 = r60[10:10];
                        r62 = l21;
                        r62[36:0] = r59;
                        r63 = r62;
                        r63[37:37] = r61;
                        r64 = {r51, r63};
                        kernel_chunked_kernel = r64;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — `iverilog` round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = Chunk::default();
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = Chunk::default();
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_chunked");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "42a3f942a85543329c77c7c110c7507892d096b7ee3232bd73a8bc1e85a43b72"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_chunked.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
