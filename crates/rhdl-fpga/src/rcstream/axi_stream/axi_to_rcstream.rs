#![warn(missing_docs)]
//! AXI4-Stream master input → [`super::super::RCStream<T, F>`] source.
//!
//! Wraps an AXI4-Stream master input as an `RCStream<T, F>` source.
//! Same pattern as [`crate::axi4lite::stream::axi_to_rhdl::Axi2Rhdl`]
//! but generic over the framing parameter `F` and producing a typed
//! `RCStream<T, F>` instead of `StreamIO<T, S>`.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +----+AxiToRCStream+--+
      |  AXI       :  RCStream
  T   |            :        | ?Item<T,F>
+---->| tdata      :  data  +------>
  F   |            :        |
+---->| tuser      :        |
+---->| tvalid     :        |
      |            :        | bool
<-----+ tready     :  ready |<-----+
      +---------------------+
")]
//!
//! # Internal details
//!
//! Packs `(tdata, tuser, tvalid)` into `Option<Item<T, F>>` via
//! `is_valid → Some(Item { data: tdata, frame: tuser })`.  Forwards
//! `ready` directly.  A [`crate::lid::carloni::Carloni`] skid-buffer
//! on the input isolates the AXI bus from any combinatorial paths in
//! the downstream RCStream-side logic.

use rhdl::prelude::*;

use crate::lid::carloni::{self, Carloni};
use crate::rcstream::bus::{Item, RCStream};

/// AXI4-Stream → RCStream translation widget.
///
/// `T` is the payload type (carried on TDATA).  `F` is the framing
/// marker type (carried on TUSER).  Use `F = ()` for streams without
/// framing.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct AxiToRCStream<T: Digital, F: Digital> {
    /// Carloni skid-buffer on the AXI input side, parameterized over
    /// `Item<T, F>` so it carries both payload + framing through.
    inbuf: Carloni<Item<T, F>>,
}

impl<T: Digital, F: Digital> Default for AxiToRCStream<T, F> {
    fn default() -> Self {
        Self {
            inbuf: Carloni::<Item<T, F>>::default(),
        }
    }
}

/// Inputs for [`AxiToRCStream`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<T: Digital, F: Digital> {
    /// AXI4-Stream `TDATA`: the payload.
    pub tdata: T,
    /// AXI4-Stream `TUSER`: the framing marker.  For `F = ()` this
    /// field is the unit value (zero wire bits).
    pub tuser: F,
    /// AXI4-Stream `TVALID`: 1 when `tdata`/`tuser` are valid this
    /// cycle.
    pub tvalid: bool,
    /// `RCStream` sink-side `ready`: 1 when the downstream is ready
    /// to accept the next item.
    pub ready: bool,
}

/// Outputs from [`AxiToRCStream`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<T: Digital, F: Digital> {
    /// `RCStream` source-side `data`: `Some(item)` when an item is
    /// being delivered this cycle, `None` otherwise.
    pub data: Option<Item<T, F>>,
    /// AXI4-Stream `TREADY`: 1 when this widget is ready to accept
    /// the next AXI item.
    pub tready: bool,
}

impl<T: Digital, F: Digital> SynchronousIO for AxiToRCStream<T, F> {
    type I = In<T, F>;
    type O = Out<T, F>;
    type Kernel = axi_to_rcstream_kernel<T, F>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn axi_to_rcstream_kernel<T: Digital, F: Digital>(
    _cr: ClockReset,
    i: In<T, F>,
    q: Q<T, F>,
) -> (Out<T, F>, D<T, F>) {
    let mut d = D::<T, F>::dont_care();
    let mut o = Out::<T, F>::dont_care();

    // Drive Carloni's input from the AXI side.
    d.inbuf.data_in.data = i.tdata;
    d.inbuf.data_in.frame = i.tuser;
    d.inbuf.void_in = !i.tvalid;
    d.inbuf.stop_in = !i.ready;

    // Compose RCStream output from Carloni's output.  Pack the buffered
    // (data, void) pair into Option<Item<T, F>>.
    o.data = if q.inbuf.void_out {
        None
    } else {
        Some(q.inbuf.data_out)
    };
    o.tready = !q.inbuf.stop_out;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _u: AxiToRCStream<b8, ()> = AxiToRCStream::default();
        let _u2: AxiToRCStream<b16, bool> = AxiToRCStream::default();
        let _u3: AxiToRCStream<b32, b8> = AxiToRCStream::default();
    }

    /// Direct kernel test: an incoming AXI item with tvalid=1 ends up
    /// in Carloni's `data_in`/`void_in=0`.
    #[test]
    fn kernel_forwards_axi_to_carloni() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, ()> {
            tdata: bits::<8>(0xAB),
            tuser: (),
            tvalid: true,
            ready: true,
        };
        let q = Q::<b8, ()> {
            inbuf: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true,
                stop_out: false,
            },
        };
        let (_o, d) = axi_to_rcstream_kernel::<b8, ()>(cr, i, q);
        assert_eq!(d.inbuf.data_in.data.raw(), 0xAB);
        assert_eq!(d.inbuf.void_in, false);
        assert_eq!(d.inbuf.stop_in, false);
    }

    /// Direct kernel test: tvalid=0 produces void_in=1.
    #[test]
    fn kernel_idle_axi_produces_void() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, ()> {
            tdata: bits::<8>(0),
            tuser: (),
            tvalid: false,
            ready: true,
        };
        let q = Q::<b8, ()> {
            inbuf: carloni::Out::<Item<b8, ()>> {
                data_out: Item::<b8, ()>::dont_care(),
                void_out: true,
                stop_out: false,
            },
        };
        let (_o, d) = axi_to_rcstream_kernel::<b8, ()>(cr, i, q);
        assert_eq!(d.inbuf.void_in, true);
    }

    /// Direct kernel test: when Carloni has buffered data, it emerges
    /// on the RCStream output as `data: Some(item)`.
    #[test]
    fn kernel_outputs_buffered_item() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, ()> {
            tdata: bits::<8>(0),
            tuser: (),
            tvalid: false,
            ready: true,
        };
        let held = Item::<b8, ()> {
            data: bits::<8>(0x42),
            frame: (),
        };
        let q = Q::<b8, ()> {
            inbuf: carloni::Out::<Item<b8, ()>> {
                data_out: held,
                void_out: false,
                stop_out: false,
            },
        };
        let (o, _d) = axi_to_rcstream_kernel::<b8, ()>(cr, i, q);
        match o.data {
            Some(it) => assert_eq!(it.data.raw(), 0x42),
            None => panic!("expected Some(item) when void_out=false"),
        }
        assert_eq!(o.tready, true); // !stop_out = !false = true
    }

    /// Framing test: `F = bool` flows TUSER through into Item::frame.
    #[test]
    fn kernel_framing_through_tuser() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, bool> {
            tdata: bits::<8>(0xFF),
            tuser: true, // end-of-frame marker
            tvalid: true,
            ready: true,
        };
        let q = Q::<b8, bool> {
            inbuf: carloni::Out::<Item<b8, bool>> {
                data_out: Item::<b8, bool>::dont_care(),
                void_out: true,
                stop_out: false,
            },
        };
        let (_o, d) = axi_to_rcstream_kernel::<b8, bool>(cr, i, q);
        assert_eq!(d.inbuf.data_in.data.raw(), 0xFF);
        assert_eq!(d.inbuf.data_in.frame, true);
    }

    /// Smoke test: descriptor + HDL emission.
    /// Tier 2 — **backpressure**, which this translator had no coverage
    /// of at all.  Its entire job is bridging two ready/valid
    /// handshakes, so a test that never stalls either side exercises
    /// nothing that matters.
    ///
    /// The AXI master holds TDATA/TVALID until TREADY (per AMBA), and
    /// the RCStream sink accepts only intermittently.  Every item must
    /// arrive exactly once, in order.
    #[test]
    fn axi_to_rcstream_loses_nothing_under_backpressure() {
        use rhdl::core::sim::ResetOrData;
        const COUNT: u128 = 20;
        let uut = AxiToRCStream::<b8, ()>::default();
        let mut to_send: u128 = 0;
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
                // The RCStream sink accepts 1 cycle in 3.
                let ready = phase.is_multiple_of(3);
                if let Some(it) = output.data {
                    if ready {
                        got.push(it.data.raw());
                    }
                }
                // AXI master: hold the current beat until TREADY.
                if to_send < COUNT && output.tready {
                    to_send += 1;
                }
                Some(ResetOrData::Data(In::<b8, ()> {
                    tdata: b8(to_send.min(COUNT - 1) % 256),
                    tuser: (),
                    tvalid: to_send < COUNT,
                    ready,
                }))
            },
            100,
        )
        .take_while(|t| t.time < 400_000)
        .for_each(drop);

        let want: Vec<u128> = (0..COUNT).collect();
        assert_eq!(
            got, want,
            "AXI->RCStream must not drop or duplicate beats when the RCStream sink stalls"
        );
    }

    #[test]
    fn descriptor_smoke() -> miette::Result<()> {
        let uut: AxiToRCStream<b8, ()> = AxiToRCStream::default();
        let _desc = uut.descriptor("axi_to_rcstream_b8".into())?;
        Ok(())
    }

    /// iverilog round-trip: F=().
    #[test]
    fn iverilog_round_trip_f_unit() -> Result<(), RHDLError> {
        let uut: AxiToRCStream<b8, ()> = AxiToRCStream::default();
        let inputs: Vec<In<b8, ()>> = (0..16)
            .map(|k| In {
                tdata: bits::<8>(k as u128),
                tuser: (),
                tvalid: true,
                ready: true,
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

    /// iverilog round-trip: F=bool (TUSER carries end-of-frame).
    #[test]
    fn iverilog_round_trip_f_bool() -> Result<(), RHDLError> {
        let uut: AxiToRCStream<b8, bool> = AxiToRCStream::default();
        let inputs: Vec<In<b8, bool>> = (0..16)
            .map(|k| In {
                tdata: bits::<8>(k as u128),
                tuser: k == 15, // last item carries TUSER=1 (end-of-frame)
                tvalid: true,
                ready: true,
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }
}
