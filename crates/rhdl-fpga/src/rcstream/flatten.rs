#![warn(missing_docs)]
//! Expand a stream of arrays into a stream of their elements.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+RCStreamFlatten+--+
 ?[T;N]                    |  ?T
+---->| data          data +---->
      |                    |
<-----+ ready        ready |<----+
      |                    |
      +--------------------+
")]
//!
//!# Framing: one item becomes many, so the marker has to be split
//!
//! This is the first combinator where framing cannot simply ride along.
//! One input `Item<[T; N], F>` becomes `N` output items, and the single
//! marker `F` described the *group*, not any one element.
//!
//! Three answers were possible: attach `F` to every element (wrong — a
//! TLAST-equivalent would fire `N` times), attach it only to the last
//! (discards it for the other `N-1`), or keep it and say where the group
//! ends. This widget takes the third:
//!
//! ```text
//! RCStream<[T; N], F>  ->  RCStream<T, (F, bool)>
//! ```
//!
//! Every element carries the group's original `F` **plus** a `bool` that
//! is true only on the last element of the group. Nothing is invented
//! and nothing is discarded; a consumer that wants a single end-of-frame
//! marker writes `map(|(f, last)| f && last)` under a rule it chose.
//! This is the same principle [`super::zip`] applies when it carries
//! `(F, G)` rather than picking one side.
//!
//!# Internals
//!
//! An [`RCStreamRelay`] holding the current array, plus an index
//! register. The array is consumed from the buffer only once its last
//! element has been handed downstream, so no element can be lost to
//! backpressure mid-group.
//!
//!# Sizing
//!
//! `N` is the array length; `M` is the width of the element index and
//! must satisfy `2^M > N - 1`. This mirrors
//! [`crate::stream::flatten`]'s `(M, N)` convention.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_flatten.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/rcstream_flatten.md")]

use rhdl::prelude::*;

use crate::core::dff;

use super::bus::{Item, RCStream};
use super::relay::RCStreamRelay;

/// Expand a stream of `[T; N]` arrays into a stream of `T` elements.
///
/// Output framing is `(F, bool)`: the group's original marker, plus a
/// last-element-of-group flag. See module docs for why.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct RCStreamFlatten<T: Digital, F: Digital, const M: usize, const N: usize>
where
    rhdl::bits::W<M>: BitWidth,
{
    /// Skid buffer holding the array currently being expanded.
    input: RCStreamRelay<[T; N], F>,
    /// Index of the next element to emit.
    idx: dff::DFF<Bits<M>>,
}

impl<T: Digital, F: Digital, const M: usize, const N: usize> Default for RCStreamFlatten<T, F, M, N>
where
    rhdl::bits::W<M>: BitWidth,
{
    fn default() -> Self {
        Self {
            input: RCStreamRelay::default(),
            idx: dff::DFF::new(bits::<M>(0)),
        }
    }
}

impl<T: Digital, F: Digital, const M: usize, const N: usize> SynchronousIO
    for RCStreamFlatten<T, F, M, N>
where
    rhdl::bits::W<M>: BitWidth,
{
    type I = RCStream<[T; N], F>;
    type O = RCStream<T, (F, bool)>;
    type Kernel = flatten_kernel<T, F, M, N>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn flatten_kernel<T: Digital, F: Digital, const M: usize, const N: usize>(
    _cr: ClockReset,
    i: RCStream<[T; N], F>,
    q: Q<T, F, M, N>,
) -> (RCStream<T, (F, bool)>, D<T, F, M, N>)
where
    rhdl::bits::W<M>: BitWidth,
{
    let mut d = D::<T, F, M, N>::dont_care();
    d.input.data = i.data;

    let (have, group) = match q.input.data {
        Some(it) => (true, it),
        None => (false, Item::<[T; N], F>::dont_care()),
    };

    let last = q.idx == bits::<M>(N as u128 - 1);
    // Runtime index into the buffered array.
    let elem = group.data[q.idx];

    let o_data = if have {
        Some(Item::<T, (F, bool)> {
            data: elem,
            frame: (group.frame, last),
        })
    } else {
        None
    };

    // An element transfers when we are presenting one and the sink takes it.
    let advance = have && i.ready;
    d.idx = if advance {
        if last {
            bits::<M>(0)
        } else {
            q.idx + 1
        }
    } else {
        q.idx
    };
    // Release the array only once its final element has gone, so
    // backpressure mid-group cannot drop the remainder.
    d.input.ready = advance && last;

    let o = RCStream::<T, (F, bool)> {
        data: o_data,
        ready: q.input.ready,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::ResetOrData;

    type Flat = RCStreamFlatten<b8, bool, 3, 4>;

    fn group(base: u128, frame: bool) -> Item<[b8; 4], bool> {
        Item::<[b8; 4], bool> {
            data: [
                bits::<8>(base),
                bits::<8>(base + 1),
                bits::<8>(base + 2),
                bits::<8>(base + 3),
            ],
            frame,
        }
    }

    fn q_with(data: Option<Item<[b8; 4], bool>>, idx: u128, ready: bool) -> Q<b8, bool, 3, 4> {
        Q::<b8, bool, 3, 4> {
            input: RCStream::<[b8; 4], bool> { data, ready },
            idx: bits::<3>(idx),
        }
    }

    /// Tier 1 — the element at the current index is emitted, carrying the
    /// group's marker and `last = false` when it is not the final one.
    #[test]
    fn emits_indexed_element_with_group_marker() {
        let q = q_with(Some(group(0x10, true)), 1, true);
        let i = RCStream::<[b8; 4], bool> {
            data: None,
            ready: false,
        };
        let (o, _d) = flatten_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        match o.data {
            Some(it) => {
                assert_eq!(it.data.raw(), 0x11, "element at index 1");
                assert!(it.frame.0, "group marker rides along");
                assert!(!it.frame.1, "index 1 of 4 is not the last");
            }
            None => panic!("expected an element"),
        }
    }

    /// Tier 1 — the final element of a group is flagged, and only then is
    /// the array released from the buffer.
    #[test]
    fn last_element_is_flagged_and_releases_the_group() {
        let q = q_with(Some(group(0x10, false)), 3, true);
        let i = RCStream::<[b8; 4], bool> {
            data: None,
            ready: true,
        };
        let (o, d) = flatten_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        let it = o.data.unwrap();
        assert_eq!(it.data.raw(), 0x13);
        assert!(it.frame.1, "index 3 of 4 is the last");
        assert!(
            d.input.ready,
            "the array is released once its last element goes"
        );
        assert_eq!(d.idx.raw(), 0, "index wraps for the next group");
    }

    /// Tier 1 — mid-group, the array must be **held**: releasing it early
    /// would drop the remaining elements.
    #[test]
    fn group_is_held_until_its_last_element() {
        let q = q_with(Some(group(0x10, false)), 1, true);
        let i = RCStream::<[b8; 4], bool> {
            data: None,
            ready: true,
        };
        let (_o, d) = flatten_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert!(!d.input.ready, "must not release the array mid-group");
        assert_eq!(d.idx.raw(), 2, "index advances instead");
    }

    /// Tier 1 — backpressure freezes the index; no element is skipped.
    #[test]
    fn backpressure_freezes_the_index() {
        let q = q_with(Some(group(0x10, false)), 2, true);
        let i = RCStream::<[b8; 4], bool> {
            data: None,
            ready: false,
        };
        let (o, d) = flatten_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_some(), "the element stays presented");
        assert_eq!(d.idx.raw(), 2, "index must not advance while stalled");
        assert!(!d.input.ready);
    }

    /// Tier 1 — an empty buffer emits nothing and advances nothing.
    #[test]
    fn empty_buffer_is_inert() {
        let q = q_with(None, 0, true);
        let i = RCStream::<[b8; 4], bool> {
            data: None,
            ready: true,
        };
        let (o, d) = flatten_kernel::<b8, bool, 3, 4>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_none());
        assert!(!d.input.ready);
        assert_eq!(d.idx.raw(), 0);
    }

    /// LID requirement.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Flat::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed loop: every group expands to exactly its `N`
    /// elements, in order, with the last-of-group flag on exactly the
    /// last one.
    #[test]
    fn stream_expands_every_group_in_order() {
        const GROUPS: u128 = 8;
        let uut = Flat::default();
        let mut sent: u128 = 0;
        let mut got: Vec<(u128, bool, bool)> = Vec::new();
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
                        got.push((it.data.raw(), it.frame.0, it.frame.1));
                    }
                }
                let mut input = RCStream::<[b8; 4], bool> {
                    data: None,
                    ready: sink_ready,
                };
                if sent < GROUPS && output.ready {
                    input.data = Some(group(sent * 4, sent.is_multiple_of(2)));
                    sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 200_000)
        .for_each(drop);

        let want: Vec<(u128, bool, bool)> = (0..GROUPS)
            .flat_map(|g| (0..4u128).map(move |e| (g * 4 + e, g % 2 == 0, e == 3)))
            .collect();
        assert_eq!(got, want, "each group must expand to N ordered elements");
    }

    fn open_loop() -> impl Iterator<Item = TimedSample<(ClockReset, RCStream<[b8; 4], bool>)>> {
        (0..12u128)
            .map(|k| RCStream::<[b8; 4], bool> {
                data: Some(group(k * 4, k % 2 == 0)),
                ready: k % 3 != 0,
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 3 — HDL emission snapshot.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Flat::default();
        let desc = uut.descriptor("rcstream_flatten".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_flatten")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module rcstream_flatten(input wire [1:0] clock_reset, input wire [34:0] i, output wire [11:0] o);
               wire [49:0] od;
               wire [37:0] d;
               wire [37:0] q;
               assign o = od[11:0];
               rcstream_flatten_input c0(.clock_reset(clock_reset), .i(d[34:0]), .o(q[34:0]));
               rcstream_flatten_idx c1(.clock_reset(clock_reset), .i(d[37:35]), .o(q[37:35]));
               assign d = od[49:12];
               assign od = kernel_flatten_kernel(clock_reset, i, q);
               function [49:0] kernel_flatten_kernel(input reg [1:0] arg_0, input reg [34:0] arg_1, input reg [37:0] arg_2);
                     reg [33:0] r0;
                     reg [34:0] r1;
                     // d
                     reg [37:0] r2;
                     reg [34:0] r3;
                     reg [37:0] r4;
                     reg [33:0] r5;
                     reg [0:0] r6;
                     reg [32:0] r7;
                     reg [33:0] r8;
                     reg [33:0] r9;
                     reg [0:0] r10;
                     reg [32:0] r11;
                     reg [2:0] r12;
                     reg [0:0] r13;
                     reg [31:0] r14;
                     reg [2:0] r15;
                     reg [7:0] r16;
                     reg [7:0] r17;
                     reg [7:0] r18;
                     reg [7:0] r19;
                     reg [7:0] r20;
                     reg [0:0] r21;
                     reg [1:0] r22;
                     reg [9:0] r23;
                     reg [9:0] r24;
                     reg [10:0] r25;
                     reg [9:0] r26;
                     reg [10:0] r27;
                     reg [0:0] r28;
                     reg [0:0] r29;
                     reg [2:0] r30;
                     reg [2:0] r31;
                     reg [2:0] r32;
                     reg [2:0] r33;
                     reg [2:0] r34;
                     // d
                     reg [37:0] r35;
                     reg [0:0] r36;
                     // d
                     reg [37:0] r37;
                     reg [34:0] r38;
                     reg [0:0] r39;
                     reg [11:0] r40;
                     reg [11:0] r41;
                     reg [49:0] r42;
                     reg [1:0] r43;
                     localparam l0 = 38'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b0;
                     localparam l4 = 34'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX0;
                     localparam l5 = 3'b011;
                     localparam l6 = 3'b000;
                     localparam l7 = 3'b001;
                     localparam l8 = 3'b010;
                     localparam l9 = 3'b011;
                     localparam l10 = 10'b0000000000;
                     localparam l11 = 1'b1;
                     localparam l12 = 11'b00000000000;
                     localparam l13 = 3'b001;
                     localparam l14 = 3'b000;
                     localparam l15 = 12'b000000000000;
                     begin
                        r43 = arg_0;
                        r1 = arg_1;
                        r4 = arg_2;
                        r0 = r1[33:0];
                        r2 = l0;
                        r2[33:0] = r0;
                        r3 = r4[34:0];
                        r5 = r3[33:0];
                        r6 = r5[33:33];
                        r7 = r5[32:0];
                        r8 = {r7, l1};
                        case (r6)
                           1'b1 : r9 = r8;
                           1'b0 : r9 = l4;
                        endcase
                        r10 = r9[0:0];
                        r11 = r9[33:1];
                        r12 = r4[37:35];
                        r13 = r12 == l5;
                        r14 = r11[31:0];
                        r15 = r4[37:35];
                        r16 = r14[7:0];
                        r17 = r14[15:8];
                        r18 = r14[23:16];
                        r19 = r14[31:24];
                        case (r15)
                           3'b000 : r20 = r16;
                           3'b001 : r20 = r17;
                           3'b010 : r20 = r18;
                           3'b011 : r20 = r19;
                        endcase
                        r21 = r11[32:32];
                        r22 = {r13, r21};
                        r23 = l10;
                        r23[7:0] = r20;
                        r24 = r23;
                        r24[9:8] = r22;
                        r26 = r24[9:0];
                        r25 = {l11, r26};
                        r27 = r10 ? r25 : l12;
                        r28 = r1[34:34];
                        r29 = r10 & r28;
                        r30 = r4[37:35];
                        r31 = r30 + l13;
                        r32 = r13 ? l14 : r31;
                        r33 = r4[37:35];
                        r34 = r29 ? r32 : r33;
                        r35 = r2;
                        r35[37:35] = r34;
                        r36 = r29 & r13;
                        r37 = r35;
                        r37[34:34] = r36;
                        r38 = r4[34:0];
                        r39 = r38[34:34];
                        r40 = l15;
                        r40[10:0] = r27;
                        r41 = r40;
                        r41[11:11] = r39;
                        r42 = {r37, r41};
                        kernel_flatten_kernel = r42;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 4 — `iverilog` round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = Flat::default();
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = Flat::default();
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_flatten");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "d14c5c1b07cacab95a7bba4b5c11cca30b51d6cfc90f7e5087db934b09b78cf6"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_flatten.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
