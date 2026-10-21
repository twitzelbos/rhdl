#![warn(missing_docs)]
//! `IqSplit` — one [`Iq`] stream into a [`Real`] stream and an [`Imag`]
//! stream.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
      +-+IqSplit+------------+
      |                      |
+---->+ stream          real +----->
      |                      |
      |                 imag +----->
      +----------------------+
")]
//!
//! # Pure rewiring, zero latency
//!
//! An [`Iq<W>`](Iq) is two `SignedBits<W>` laid end to end; a
//! [`Real<W>`](Real) and an [`Imag<W>`](Imag) are one each. So this
//! widget contains no logic at all — it renames bits. It is
//! combinational and adds **nothing** to the scheduler's latency
//! arithmetic.
//!
//! The value is entirely in the type system: without split and combine
//! the sample types are decorative, because routing a complex stream
//! into a widget that wants a real one would not be expressible, and
//! the `Real × Iq` instantiation of a mixer could never be reached from
//! an `Iq` source.
//!
//! # Validity is propagated, not invented
//!
//! Both outputs are valid exactly when the input is. Splitting cannot
//! create data, and an implementation that emitted `Some` on an idle
//! cycle would be inventing samples.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/iq_split.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/iq_split.md")]

use rhdl::prelude::*;

use crate::dsp::iq::{Imag, Iq, Real};
use crate::rcstream::bus::{Item, RCStream};

/// Splits an `Iq` stream into its real and imaginary parts.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct IqSplit<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// Carries the framing type; see the note on `PhantomData` in
    /// [`super::constant::RCStreamConstant`].
    marker: crate::core::constant::Constant<F>,
}

impl<const W: usize, F: Digital> Default for IqSplit<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    fn default() -> Self {
        Self {
            marker: crate::core::constant::Constant::new(F::dont_care()),
        }
    }
}

/// Inputs to [`IqSplit`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The complex stream.
    pub stream: Option<Item<Iq<W>, F>>,
    /// Ready from the real-side consumer.
    pub real_ready: bool,
    /// Ready from the imaginary-side consumer.
    pub imag_ready: bool,
}

/// Outputs from [`IqSplit`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<const W: usize, F: Digital>
where
    rhdl::bits::W<W>: BitWidth,
{
    /// The real part.
    pub real: RCStream<Real<W>, F>,
    /// The imaginary part.
    pub imag: RCStream<Imag<W>, F>,
    /// Ready toward the upstream source: both consumers must be ready,
    /// since one item becomes two and neither can be held back
    /// independently without buffering.
    pub ready: bool,
}

impl<const W: usize, F: Digital> SynchronousIO for IqSplit<W, F>
where
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<W, F>;
    type O = Out<W, F>;
    type Kernel = iq_split_kernel<W, F>;
}

#[kernel]
#[doc(hidden)]
pub fn iq_split_kernel<const W: usize, F: Digital>(
    _cr: ClockReset,
    i: In<W, F>,
    q: Q<W, F>,
) -> (Out<W, F>, D<W, F>)
where
    rhdl::bits::W<W>: BitWidth,
{
    let mut d = D::<W, F>::dont_care();
    d.marker = ();

    let mut real_data = None;
    let mut imag_data = None;

    if let Some(item) = i.stream {
        real_data = Some(Item::<Real<W>, F> {
            data: Real::<W> { v: item.data.re },
            frame: item.frame,
        });
        imag_data = Some(Item::<Imag<W>, F> {
            data: Imag::<W> { v: item.data.im },
            frame: item.frame,
        });
    }

    let o = Out::<W, F> {
        real: RCStream::<Real<W>, F> {
            data: real_data,
            ready: i.real_ready,
        },
        imag: RCStream::<Imag<W>, F> {
            data: imag_data,
            ready: i.imag_ready,
        },
        // One item becomes two, so the source may only advance when
        // both consumers can take theirs.
        ready: i.real_ready && i.imag_ready,
    };
    let _ = q;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 18;
    type Uut = IqSplit<W, ()>;

    fn item(re: i128, im: i128) -> In<W, ()> {
        In::<W, ()> {
            stream: Some(Item::<Iq<W>, ()> {
                data: Iq::<W> {
                    re: signed::<W>(re),
                    im: signed::<W>(im),
                },
                frame: (),
            }),
            real_ready: true,
            imag_ready: true,
        }
    }

    fn idle() -> In<W, ()> {
        In::<W, ()> {
            stream: None,
            real_ready: true,
            imag_ready: true,
        }
    }

    fn run(seq: Vec<In<W, ()>>) -> Vec<Out<W, ()>> {
        let uut = Uut::default();
        uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect()
    }

    /// Each half goes to the right output, and the halves are not
    /// swapped.
    ///
    /// Distinct values per component, so a transposition shows up
    /// rather than cancelling.
    #[test]
    fn the_components_are_not_swapped() {
        let out = run(vec![item(1234, -5678); 3]);
        let s = &out[2];
        match (s.real.data, s.imag.data) {
            (Some(r), Some(i)) => {
                assert_eq!(r.data.v.raw(), 1234, "real side carries the real part");
                assert_eq!(
                    i.data.v.raw(),
                    -5678,
                    "imag side carries the imaginary part"
                );
            }
            _ => panic!("both outputs must be valid when the input is"),
        }
    }

    /// Validity is propagated, not invented: an idle input gives two
    /// idle outputs.
    #[test]
    fn an_idle_input_gives_idle_outputs() {
        let mut seq = vec![item(100, 200); 2];
        seq.extend(vec![idle(); 3]);
        let out = run(seq);
        let tail = out.last().unwrap();
        assert!(
            tail.real.data.is_none() && tail.imag.data.is_none(),
            "splitting cannot create data"
        );
    }

    /// The source may advance only when both consumers can take their
    /// half — one item becomes two.
    #[test]
    fn ready_requires_both_consumers() {
        let uut = Uut::default();
        let mut a = item(1, 2);
        a.imag_ready = false;
        let out: Vec<Out<W, ()>> = uut
            .run(vec![a; 3].into_iter().with_reset(1).clock_pos_edge(100))
            .synchronous_sample()
            .map(|s| s.output)
            .collect();
        assert!(
            !out[2].ready,
            "upstream must not be told to advance while one consumer is stalled"
        );
    }

    /// Tier 3 — the emitted top module is the contract.
    ///
    /// Missing until the RCStream review noticed that this widget, and
    /// its two siblings in `util`, were the only ones in the module
    /// without a codegen snapshot or a VCD digest.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Uut::default();
        let desc = uut.descriptor("iq_split".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "iq_split")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module iq_split(input wire [1:0] clock_reset, input wire [38:0] i, output wire [40:0] o);
               wire [40:0] od;
               assign o = od[40:0];
               assign od = kernel_iq_split_kernel(clock_reset, i);
               function [40:0] kernel_iq_split_kernel(input reg [1:0] arg_0, input reg [38:0] arg_1);
                     reg [36:0] r0;
                     reg [38:0] r1;
                     reg [0:0] r2;
                     reg [35:0] r3;
                     reg signed [17:0] r4;
                     reg [17:0] r5;
                     reg [17:0] r6;
                     reg [18:0] r7;
                     reg [17:0] r8;
                     reg signed [17:0] r9;
                     reg [17:0] r10;
                     reg [17:0] r11;
                     reg [18:0] r12;
                     reg [17:0] r13;
                     // imag_data
                     reg [18:0] r14;
                     // real_data
                     reg [18:0] r15;
                     reg [0:0] r16;
                     reg [19:0] r17;
                     reg [19:0] r18;
                     reg [0:0] r19;
                     reg [19:0] r20;
                     reg [19:0] r21;
                     reg [0:0] r22;
                     reg [0:0] r23;
                     reg [0:0] r24;
                     reg [40:0] r25;
                     reg [40:0] r26;
                     reg [40:0] r27;
                     reg [1:0] r28;
                     localparam l0 = 18'b000000000000000000;
                     localparam l1 = 18'b000000000000000000;
                     localparam l2 = 1'b1;
                     localparam l3 = 18'b000000000000000000;
                     localparam l4 = 18'b000000000000000000;
                     localparam l5 = 1'b1;
                     localparam l6 = 1'b1;
                     localparam l7 = 19'b0000000000000000000;
                     localparam l8 = 19'b0000000000000000000;
                     localparam l9 = 20'b00000000000000000000;
                     localparam l10 = 20'b00000000000000000000;
                     localparam l11 = 41'b00000000000000000000000000000000000000000;
                     begin
                        r28 = arg_0;
                        r1 = arg_1;
                        r0 = r1[36:0];
                        r2 = r0[36:36];
                        r3 = r0[35:0];
                        r4 = r3[17:0];
                        r5 = l0;
                        r5[17:0] = r4;
                        r6 = l1;
                        r6[17:0] = r5;
                        r8 = r6[17:0];
                        r7 = {l2, r8};
                        r9 = r3[35:18];
                        r10 = l3;
                        r10[17:0] = r9;
                        r11 = l4;
                        r11[17:0] = r10;
                        r13 = r11[17:0];
                        r12 = {l5, r13};
                        case (r2)
                           1'b1 : r14 = r12;
                           default : r14 = l7;
                        endcase
                        case (r2)
                           1'b1 : r15 = r7;
                           default : r15 = l8;
                        endcase
                        r16 = r1[37:37];
                        r17 = l9;
                        r17[18:0] = r15;
                        r18 = r17;
                        r18[19:19] = r16;
                        r19 = r1[38:38];
                        r20 = l10;
                        r20[18:0] = r14;
                        r21 = r20;
                        r21[19:19] = r19;
                        r22 = r1[37:37];
                        r23 = r1[38:38];
                        r24 = r22 & r23;
                        r25 = l11;
                        r25[19:0] = r18;
                        r26 = r25;
                        r26[39:20] = r21;
                        r27 = r26;
                        r27[40:40] = r24;
                        kernel_iq_split_kernel = r27;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Tier 5 — a waveform digest, so an ordering change that passes the
    /// functional tests still shows up.
    #[test]
    fn trace_digest() {
        let uut = Uut::default();
        let seq: Vec<In<W, ()>> = (0..16i128)
            .map(|k| item((k - 8) * 900, (8 - k) * 700))
            .collect();
        let vcd = uut
            .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
            .collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("iq_split");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "7d085ac36c86b3b836cc536e31e305b6004c4256a1fc89f51208938b34ba7618"
        ];
        let digest = vcd.dump_to_file(root.join("iq_split.vcd")).unwrap();
        expect.assert_eq(&digest);
    }

    /// Tier 4 — emitted Verilog agrees with the Rust simulation.
    #[test]
    fn test_iq_split_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let seq: Vec<In<W, ()>> = (0..16i128)
            .map(|k| item((k - 8) * 900, (8 - k) * 700))
            .collect();
        let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }
}
