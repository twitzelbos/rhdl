#![warn(missing_docs)]
//! `IqCombine` — a [`Real`] stream and an [`Imag`] stream into one
//! [`Iq`] stream.
//!
//! The inverse of [`super::split::IqSplit`], and pure rewiring for the
//! same reason: combinational, zero latency, no logic.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+IqCombine+----------+
      |                      |
+---->+ real          stream +----->
      |                      |
+---->+ imag         starved +----->
      +----------------------+
")]
//!
//! # Both sides must present data
//!
//! A complex sample needs both halves, so an item is emitted only when
//! both inputs are valid. A cycle where exactly one side has data sets
//! `starved`.
//!
//! Reported rather than buffered, for the same reason as
//! [`crate::dsp::mixer::ComplexRealMixer`]: holding one side to wait
//! for the other is an elastic buffer with data-dependent occupancy,
//! which makes the path's latency data-dependent and breaks the
//! scheduler's arithmetic. In the timed domain both sides are
//! isochronous, so a one-sided cycle is a design error rather than a
//! condition to handle.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/iq_combine.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/iq_combine.md")]

use rhdl::prelude::*;

use crate::dsp::iq::{Imag, Iq, Real};
use crate::rcstream::bus::{Item, RCStream};

/// Combines real and imaginary streams into one `Iq` stream.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct IqCombine<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// Carries the framing type.
    marker: crate::core::constant::Constant<F>,
}

impl<const W: usize, F: Digital> Default for IqCombine<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    fn default() -> Self {
        Self {
            marker: crate::core::constant::Constant::new(F::dont_care()),
        }
    }
}

/// Inputs to [`IqCombine`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The real part.
    pub real: Option<Item<Real<W>, F>>,
    /// The imaginary part.
    pub imag: Option<Item<Imag<W>, F>>,
    /// Ready from the downstream consumer.
    pub downstream_ready: bool,
}

/// Outputs from [`IqCombine`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The combined complex stream.
    pub stream: RCStream<Iq<W>, F>,
    /// Exactly one side presented data on some cycle.
    pub starved: bool,
    /// **The two sides presented data whose framing disagreed.**
    ///
    /// The type system requires both inputs to carry the same framing
    /// *type*; it cannot require the same *value*. Two paths that were
    /// split from one stream should carry identical frames, so a
    /// disagreement means they have drifted — a dropped item on one
    /// side, or two decimators that fell out of step. That is a fault
    /// in the composition, not a condition to paper over, so it is
    /// reported rather than resolved by silently preferring one side.
    pub frame_mismatch: bool,
}

impl<const W: usize, F: Digital> SynchronousIO for IqCombine<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<W, F>;
    type O = Out<W, F>;
    type Kernel = iq_combine_kernel<W, F>;
}

#[kernel]
#[doc(hidden)]
pub fn iq_combine_kernel<const W: usize, F: Digital>(
    _cr: ClockReset,
    i: In<W, F>,
    q: Q<W, F>,
) -> (Out<W, F>, D<W, F>)
where
    rhdl::bits::W<W>: BitWidth,
{
    let mut d = D::<W, F>::dont_care();
    d.marker = ();

    // Presence is tracked with plain `bool`s, and the two framing
    // values are compared inside *nested* `if let`s so that neither
    // ever leaves the scope that binds it.
    //
    // This shape is not a preference. Hoisting a generic `F` as
    // `let mut frame = F::dont_care()` and filling it in a branch
    // trips an RHDL partial-initialisation error -- the original
    // version of this kernel carried a comment saying so, and the
    // first attempt at frame comparison did it anyway and broke the
    // build. Nesting keeps `F` inside its binding and needs no
    // placeholder value at all.
    let mut have_re = false;
    if let Some(_item) = i.real {
        have_re = true;
    }
    let mut have_im = false;
    if let Some(_item) = i.imag {
        have_im = true;
    }

    let mut out_data = None;
    let mut frame_mismatch = false;
    if let Some(re_item) = i.real {
        if let Some(im_item) = i.imag {
            // The two frames must agree. Both sides came from one
            // stream, so a disagreement is drift, not a choice to be
            // made -- see `Out::frame_mismatch`. The real side's frame
            // is carried so the item is still well formed, and the
            // mismatch is reported alongside it.
            if re_item.frame != im_item.frame {
                frame_mismatch = true;
            }
            out_data = Some(Item::<Iq<W>, F> {
                data: Iq::<W> {
                    re: re_item.data.v,
                    im: im_item.data.v,
                },
                frame: re_item.frame,
            });
        }
    }
    // Exactly one side presented data.
    let starved = have_re != have_im;

    let o = Out::<W, F> {
        stream: RCStream::<Iq<W>, F> {
            data: out_data,
            ready: i.downstream_ready,
        },
        starved,
        frame_mismatch,
    };
    let _ = q;
    (o, d)
}

#[cfg(test)]
mod frame_alignment_tests {
    use super::*;

    /// Agreeing frames pass through and raise nothing.
    #[test]
    fn aligned_frames_do_not_flag() {
        let uut = IqCombine::<8, bool>::default();
        let seq = vec![In::<8, bool> {
            real: Some(Item::<Real<8>, bool> {
                data: Real::<8> { v: signed::<8>(3) },
                frame: true,
            }),
            imag: Some(Item::<Imag<8>, bool> {
                data: Imag::<8> { v: signed::<8>(-4) },
                frame: true,
            }),
            downstream_ready: true,
        }];
        let out: Vec<(bool, bool)> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .filter_map(|s| {
                s.output
                    .stream
                    .data
                    .map(|it| (it.frame, s.output.frame_mismatch))
            })
            .collect();
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|(f, m)| *f && !*m),
            "aligned frames must pass through unflagged: {out:?}"
        );
    }

    /// **Disagreeing frames are reported, not silently resolved.**
    ///
    /// Before `frame_mismatch` existed, the real side's frame was taken
    /// and the imaginary side's discarded without comment — so two
    /// paths that had drifted produced a confident, wrong answer.
    #[test]
    fn disagreeing_frames_are_flagged() {
        let uut = IqCombine::<8, bool>::default();
        let seq = vec![In::<8, bool> {
            real: Some(Item::<Real<8>, bool> {
                data: Real::<8> { v: signed::<8>(3) },
                frame: true,
            }),
            imag: Some(Item::<Imag<8>, bool> {
                data: Imag::<8> { v: signed::<8>(-4) },
                frame: false,
            }),
            downstream_ready: true,
        }];
        let flagged: Vec<bool> = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output.frame_mismatch)
            .collect();
        assert!(
            flagged.iter().any(|m| *m),
            "a frame disagreement must be reported"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 18;
    type Uut = IqCombine<W, ()>;

    fn both(re: i128, im: i128) -> In<W, ()> {
        In::<W, ()> {
            real: Some(Item::<Real<W>, ()> {
                data: Real::<W> { v: signed::<W>(re) },
                frame: (),
            }),
            imag: Some(Item::<Imag<W>, ()> {
                data: Imag::<W> { v: signed::<W>(im) },
                frame: (),
            }),
            downstream_ready: true,
        }
    }

    fn only_real(re: i128) -> In<W, ()> {
        In::<W, ()> {
            real: Some(Item::<Real<W>, ()> {
                data: Real::<W> { v: signed::<W>(re) },
                frame: (),
            }),
            imag: None,
            downstream_ready: true,
        }
    }

    fn run(seq: Vec<In<W, ()>>) -> Vec<Out<W, ()>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    /// The halves land in the right components.
    #[test]
    fn the_components_are_not_swapped() {
        let out = run(vec![both(4321, -8765); 3]);
        match out[2].stream.data {
            Some(item) => {
                assert_eq!(item.data.re.raw(), 4321);
                assert_eq!(item.data.im.raw(), -8765);
            }
            None => panic!("both inputs were valid, so an item must be emitted"),
        }
    }

    /// One-sided cycles are reported and emit nothing.
    #[test]
    fn a_one_sided_cycle_is_reported() {
        let mut seq = vec![both(10, 20); 2];
        seq.push(only_real(30));
        seq.extend(vec![both(10, 20); 2]);
        let out = run(seq);
        assert!(
            out.iter().any(|o| o.starved),
            "a one-sided cycle was not reported"
        );
        assert!(out.iter().any(|o| !o.starved), "starved is stuck high");
        let starved_at = out.iter().position(|o| o.starved).unwrap();
        assert!(
            out[starved_at].stream.data.is_none(),
            "a half sample must not be emitted as if it were whole"
        );
    }

    /// **Split then combine is the identity.**
    ///
    /// The strongest statement about the pair: whatever routing the
    /// type algebra makes possible, it does not alter the data. A
    /// transposition or a dropped component in *either* widget breaks
    /// this, which a test of one widget alone would not catch.
    #[test]
    fn split_then_combine_is_the_identity() {
        use crate::rcstream::util::split::{In as SplitIn, IqSplit};

        let values: Vec<(i128, i128)> = (0..24i128)
            .map(|k| ((k - 12) * 1100, (12 - k) * 900))
            .collect();

        // Stage 1: split.
        let split = IqSplit::<W, ()>::default();
        let halves: Vec<(i128, i128)> = split
            .run(
                values
                    .iter()
                    .map(|(r, i)| SplitIn::<W, ()> {
                        stream: Some(Item::<Iq<W>, ()> {
                            data: Iq::<W> {
                                re: signed::<W>(*r),
                                im: signed::<W>(*i),
                            },
                            frame: (),
                        }),
                        real_ready: true,
                        imag_ready: true,
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .with_reset(1)
                    .clock_pos_edge(100),
            )
            .synchronous_sample()
            .filter_map(|s| match (s.output.real.data, s.output.imag.data) {
                (Some(r), Some(i)) => Some((r.data.v.raw(), i.data.v.raw())),
                _ => None,
            })
            .collect();

        assert_eq!(halves.len(), values.len(), "the split dropped samples");

        // Stage 2: combine.
        let recombined: Vec<(i128, i128)> =
            run(halves.iter().map(|(r, i)| both(*r, *i)).collect::<Vec<_>>())
                .into_iter()
                .filter_map(|o| o.stream.data.map(|it| (it.data.re.raw(), it.data.im.raw())))
                .collect();

        assert_eq!(
            recombined, values,
            "split followed by combine must return exactly what went in"
        );
    }

    /// Tier 3 — the emitted top module is the contract.
    ///
    /// Of the three `util` widgets this is the one worst to leave
    /// un-snapshotted: it is the only one with behaviour to regress,
    /// namely the framing comparison and `frame_mismatch`.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Uut::default();
        let desc = uut.descriptor("iq_combine".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "iq_combine")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module iq_combine(input wire [1:0] clock_reset, input wire [38:0] i, output wire [39:0] o);
               wire [39:0] od;
               assign o = od[39:0];
               assign od = kernel_iq_combine_kernel(clock_reset, i);
               function [39:0] kernel_iq_combine_kernel(input reg [1:0] arg_0, input reg [38:0] arg_1);
                     reg [18:0] r0;
                     reg [38:0] r1;
                     reg [0:0] r2;
                     // have_re
                     reg [0:0] r3;
                     reg [18:0] r4;
                     reg [0:0] r5;
                     // have_im
                     reg [0:0] r6;
                     reg [18:0] r7;
                     reg [0:0] r8;
                     reg [17:0] r9;
                     reg [18:0] r10;
                     reg [0:0] r11;
                     reg [17:0] r12;
                     reg [0:0] r13;
                     // frame_mismatch
                     reg [0:0] r14;
                     reg [35:0] r15;
                     reg [35:0] r16;
                     reg [35:0] r17;
                     reg [36:0] r18;
                     reg [35:0] r19;
                     // frame_mismatch
                     reg [0:0] r20;
                     // out_data
                     reg [36:0] r21;
                     // frame_mismatch
                     reg [0:0] r22;
                     // out_data
                     reg [36:0] r23;
                     reg [0:0] r24;
                     reg [0:0] r25;
                     reg [37:0] r26;
                     reg [37:0] r27;
                     reg [39:0] r28;
                     reg [39:0] r29;
                     reg [39:0] r30;
                     reg [1:0] r31;
                     localparam l0 = 1'b1;
                     localparam l1 = 1'b1;
                     localparam l2 = 1'b0;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b1;
                     localparam l5 = 1'b0;
                     localparam l6 = 1'b0;
                     localparam l7 = 1'b1;
                     localparam l8 = 1'b0;
                     localparam l9 = 36'b000000000000000000000000000000000000;
                     localparam l10 = 36'b000000000000000000000000000000000000;
                     localparam l11 = 1'b1;
                     localparam l12 = 1'b1;
                     localparam l13 = 37'b0000000000000000000000000000000000000;
                     localparam l14 = 1'b1;
                     localparam l15 = 38'b00000000000000000000000000000000000000;
                     localparam l16 = 40'b0000000000000000000000000000000000000000;
                     begin
                        r31 = arg_0;
                        r1 = arg_1;
                        r0 = r1[18:0];
                        r2 = r0[18:18];
                        case (r2)
                           1'b1 : r3 = l1;
                           default : r3 = l2;
                        endcase
                        r4 = r1[37:19];
                        r5 = r4[18:18];
                        case (r5)
                           1'b1 : r6 = l4;
                           default : r6 = l5;
                        endcase
                        r7 = r1[18:0];
                        r8 = r7[18:18];
                        r9 = r7[17:0];
                        r10 = r1[37:19];
                        r11 = r10[18:18];
                        r12 = r10[17:0];
                        r13 = l6;
                        r14 = r13 ? l7 : l8;
                        r15 = l9;
                        r15[17:0] = r9;
                        r16 = r15;
                        r16[35:18] = r12;
                        r17 = l10;
                        r17[35:0] = r16;
                        r19 = r17[35:0];
                        r18 = {l11, r19};
                        case (r11)
                           1'b1 : r20 = r14;
                           default : r20 = l8;
                        endcase
                        case (r11)
                           1'b1 : r21 = r18;
                           default : r21 = l13;
                        endcase
                        case (r8)
                           1'b1 : r22 = r20;
                           default : r22 = l8;
                        endcase
                        case (r8)
                           1'b1 : r23 = r21;
                           default : r23 = l13;
                        endcase
                        r24 = r3 != r6;
                        r25 = r1[38:38];
                        r26 = l15;
                        r26[36:0] = r23;
                        r27 = r26;
                        r27[37:37] = r25;
                        r28 = l16;
                        r28[37:0] = r27;
                        r29 = r28;
                        r29[38:38] = r24;
                        r30 = r29;
                        r30[39:39] = r22;
                        kernel_iq_combine_kernel = r30;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 5 — a waveform digest, on a stream that exercises the
    /// framing comparison rather than only the datapath.
    ///
    /// `F = SyncMark` here where the rest of this module uses `()`. A
    /// digest taken with unit framing could not detect a change to the
    /// one piece of logic worth protecting.
    #[test]
    fn trace_digest() {
        use crate::dsp::sync::SyncMark;
        let uut = IqCombine::<W, SyncMark>::default();
        let seq: Vec<In<W, SyncMark>> = (0..16i128)
            .map(|k| In::<W, SyncMark> {
                real: Some(Item::<Real<W>, SyncMark> {
                    data: Real::<W> {
                        v: signed::<W>((k - 8) * 800),
                    },
                    frame: SyncMark {
                        sync: k == 4 || k == 9,
                    },
                }),
                imag: Some(Item::<Imag<W>, SyncMark> {
                    data: Imag::<W> {
                        v: signed::<W>((8 - k) * 600),
                    },
                    // Disagrees on k = 9, so the digest covers a flagged
                    // cycle as well as agreeing ones.
                    frame: SyncMark { sync: k == 4 },
                }),
                downstream_ready: true,
            })
            .collect();
        let vcd = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("iq_combine");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "c13f01f11c13840a7a932ee5a76b6e73765073f9b7ebee121a94bd40de3160a6"
        ];
        let digest = vcd.dump_to_file(root.join("iq_combine.vcd")).unwrap();
        expect.assert_eq(&digest);
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_iq_combine_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<W, ()>> = (0..16i128)
            .map(|k| both((k - 8) * 800, (8 - k) * 600))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }
}
