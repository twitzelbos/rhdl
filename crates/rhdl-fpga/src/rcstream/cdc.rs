#![warn(missing_docs)]
//! Clock-domain crossing for [`RCStream`] connections.
//!
//! [`RCStreamCdc`] moves [`Item<T, F>`]s from a source in write domain
//! `W` to a sink in read domain `R`, presenting a normal `RCStream`
//! ready/valid interface on both sides.  It is the `rcstream`
//! counterpart to [`crate::fifo::asynchronous::AsyncFIFO`], which it
//! wraps.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +---+RCStreamCdc+---------------------+
 ?T   |                  +                  |  ?T
+---->| data       W     |     R       data +---->
      |         domain  <+>  domain         |
<-----+ ready            |            ready |<----+
      |                  +                  |
<-----+ overflow             underflow      +---->
      |                                     |
+---->| cr_w                           cr_r |<----+
      |                                     |
      +-------------------------------------+
")]
//!
//!# Why this widget exists
//!
//! [`RCStream`] carries no clock-domain information in the
//! single-domain ([`Synchronous`]) family — the framework fans out one
//! `ClockReset` to every sub-circuit, so "the domain" is implicit and
//! uniform.  The moment a design has two clocks, moving a stream
//! between them needs a real CDC structure: a dual-clock FIFO with
//! gray-coded pointer synchronization.  This widget is that structure,
//! wearing an `RCStream` interface.
//!
//!# Internals
//!
//! A single [`AsyncFIFO<Item<T, F>, W, R, N>`] with ready/valid glue on
//! each side.  The FIFO holds `2^N - 1` items.
//!
#![doc = badascii_doc::badascii!("
   data  +--------+ !full            +------------+          +--------+ data
  +----->| accept |----------------->|            |          | vld    |----->
         | gate   |                  |  AsyncFIFO |          | gate   |
  <------| ready  |<--- !full -------|  W  ->  R  |--------->|        |<-----
   ready +--------+                  +------------+  data    +--------+ ready
                                                                 |
                                        next = ready & is_some <-+
")]
//!
//!# The two handshake hazards
//!
//! Both sides need care; neither is a naive wire-up.
//!
//! **Write side — the FIFO overflow trap.**  A conforming `RCStream`
//! source MAY assert `data = Some(item)` on a cycle when `ready` is
//! false: per the bus contract in [`super::bus`], `data.is_some()`
//! must **not** depend combinationally on `ready`.  The source simply
//! holds the item until a cycle where both are true.  A raw FIFO,
//! however, treats any `Some` on its data port as a write, and writing
//! while full is an overflow.  So the write must be *gated*:
//!
//! ```text
//! accept = if !full { data } else { None }
//! ```
//!
//! This is the same hazard [`crate::stream::stream_to_fifo`] documents
//! ("a FIFO cannot be interfaced to a stream by simply setting
//! `ready = !full`").  That widget solves it with a two-element skid
//! buffer because it is also minimizing resources; here a plain gate
//! is sufficient and costs nothing, and it keeps `rcstream`
//! independent of the [`crate::stream`] module's `Ready<T>` type.
//!
//! **Read side — the underflow trap.**  Asserting the FIFO's `next`
//! when it is empty underflows.  So the read is gated on the data
//! actually being present:
//!
//! ```text
//! next = ready && data.is_some()
//! ```
//!
//!# Contract compliance
//!
//! The bus contract requires that `data.is_some()` must not depend
//! combinationally on `ready`.  Both outputs satisfy it:
//!
//! - `o.data` (R domain) comes from the FIFO's registered read port,
//!   which is a function of the FIFO's state only — `i.ready` is not
//!   in its cone.
//! - `o.ready` (W domain) is `!full`, and `full` is a registered
//!   output of the FIFO's write logic — `i.data` is not in its cone.
//!
//! So there is no combinational path from either input to either
//! output, in either domain.
//!
//!# Sizing
//!
//! `N` is the FIFO's address width; the crossing holds `2^N - 1`
//! items.  Because the gray-coded pointer synchronizers make `full`
//! *pessimistic* in `W` and `empty` pessimistic in `R` (each lags the
//! other domain by the synchronizer depth), a crossing sized too
//! tightly will throttle throughput even when the average rates match.
//! Size `N` so the FIFO can absorb the synchronizer round-trip — a
//! depth of at least 8 items (`N >= 4`) is a sane floor for
//! same-order-of-magnitude clocks.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/rcstream_cdc.rs")]
//!```
//!
//! The trace below demonstrates the result.  Note `ready` in the `Red`
//! domain dropping as the FIFO fills under the sink's backpressure, and
//! items continuing to emerge in order in `Blue`.
#![doc = include_str!("../../doc/rcstream_cdc.md")]

use rhdl::prelude::*;

use crate::fifo::asynchronous::AsyncFIFO;

use super::bus::Item;

/// A clock-domain crossing for an [`super::bus::RCStream`] connection.
///
/// `T` is the payload type, `F` the framing-marker type, `W` the write
/// (source) clock domain, `R` the read (sink) clock domain, and `N` the
/// FIFO address width — the crossing holds `2^N - 1` items.
///
/// [`Default`] requires `T: Default` and `F: Default`, inherited from
/// [`AsyncFIFO`]'s own contract.
#[derive(Clone, Circuit, CircuitDQ, Default)]
pub struct RCStreamCdc<T: Digital, F: Digital, W: Domain, R: Domain, const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    /// The dual-clock FIFO carrying `Item<T, F>` from `W` to `R`.
    fifo: AsyncFIFO<Item<T, F>, W, R, N>,
}

#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
/// Inputs to the [`RCStreamCdc`].
///
/// Note that `data` and `ready` are in *different* domains — this is
/// exactly why the bundled [`super::bus::AsyncRCStream`] type cannot
/// express a crossing's ports.
pub struct In<T: Digital, F: Digital, W: Domain, R: Domain> {
    /// `W` domain: data flowing in from the upstream source.  `None`
    /// = idle, `Some(item)` = the source is presenting an item.
    pub data: Signal<Option<Item<T, F>>, W>,
    /// `R` domain: ready flowing in from the downstream sink.  `true`
    /// = the sink will accept an item this cycle.
    pub ready: Signal<bool, R>,
    /// The clock and reset for the `W` domain.
    pub cr_w: Signal<ClockReset, W>,
    /// The clock and reset for the `R` domain.
    pub cr_r: Signal<ClockReset, R>,
}

#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
/// Outputs from the [`RCStreamCdc`].
pub struct Out<T: Digital, F: Digital, W: Domain, R: Domain> {
    /// `W` domain: ready flowing out to the upstream source.  `true`
    /// = the crossing has room and will accept an item this cycle.
    pub ready: Signal<bool, W>,
    /// `R` domain: data flowing out to the downstream sink.  `None` =
    /// idle, `Some(item)` = an item is available this cycle.
    pub data: Signal<Option<Item<T, F>>, R>,
    /// `W` domain: sticky overflow flag from the underlying FIFO.
    /// Should never assert — the write gate prevents it.  Exposed so a
    /// misbehaving source (one that violates the hold-until-ready
    /// contract) is observable rather than silent.
    pub overflow: Signal<bool, W>,
    /// `R` domain: sticky underflow flag from the underlying FIFO.
    /// Should never assert — the read gate prevents it.
    pub underflow: Signal<bool, R>,
}

impl<T: Digital, F: Digital, W: Domain, R: Domain, const N: usize> CircuitIO
    for RCStreamCdc<T, F, W, R, N>
where
    rhdl::bits::W<N>: BitWidth,
{
    type I = In<T, F, W, R>;
    type O = Out<T, F, W, R>;
    type Kernel = rcstream_cdc_kernel<T, F, W, R, N>;
}

#[kernel]
/// Kernel for [`RCStreamCdc`].
///
/// The `(O, D)` return tuple is the framework-mandated kernel signature
/// (CLAUDE.md §2), not an incidental type; with five generic parameters
/// it trips `clippy::type_complexity`.  Factoring it behind an alias
/// would obscure the convention every other widget follows, so the lint
/// is suppressed here instead.
#[allow(clippy::type_complexity)]
pub fn rcstream_cdc_kernel<T: Digital, F: Digital, W: Domain, R: Domain, const N: usize>(
    i: In<T, F, W, R>,
    q: RCStreamCdcQ<T, F, W, R, N>,
) -> (Out<T, F, W, R>, RCStreamCdcD<T, F, W, R, N>)
where
    rhdl::bits::W<N>: BitWidth,
{
    let mut d = RCStreamCdcD::<T, F, W, R, N>::dont_care();
    let mut o = Out::<T, F, W, R>::dont_care();

    // Clock the FIFO's two halves.
    d.fifo.cr_w = i.cr_w;
    d.fifo.cr_r = i.cr_r;

    // --- Write side (W domain) ------------------------------------
    // `full` is a registered output of the FIFO's write logic, so
    // nothing here creates a combinational path from `i.data`.
    let full = q.fifo.full.val();
    let ready_w = !full;
    // Gate the write.  A conforming source may present `Some` while
    // we are full; writing it would overflow the FIFO, so we drop it
    // on the floor and rely on the source holding the item (which the
    // bus contract requires) until a cycle where `ready_w` is true.
    let accept = if ready_w { i.data.val() } else { None };
    d.fifo.data = signal(accept);

    // --- Read side (R domain) -------------------------------------
    // The FIFO presents `Some(item)` whenever it is non-empty.
    let read_data = q.fifo.data.val();
    // Gate the read so we never assert `next` on an empty FIFO.
    //
    // The validity test is written inline rather than via
    // `core::option::is_some`.  Calling that generic helper compiles it
    // as a separate, domain-agnostic kernel object, and the clock-domain
    // pass then cannot resolve which domain its result belongs to
    // ("Expression belongs to Unknown clock domain").  Inlining keeps
    // the test in this kernel's RHIF, where inference unifies it with
    // the `R`-domain `q.fifo.data` and `i.ready`.
    let has_data = match read_data {
        Some(_) => true,
        None => false,
    };
    let will_read = i.ready.val() && has_data;
    d.fifo.next = signal(will_read);

    // --- Outputs ---------------------------------------------------
    o.ready = signal(ready_w);
    o.data = signal(read_data);
    o.overflow = q.fifo.overflow;
    o.underflow = q.fifo.underflow;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fifo::asynchronous;

    type TestItem = Item<b8, ()>;

    fn item(v: u128) -> TestItem {
        Item::<b8, ()> {
            data: bits::<8>(v),
            frame: (),
        }
    }

    /// Build a `Q` with the FIFO reporting the given `full` / `data`
    /// state.  The remaining flags are quiescent.
    fn q_with(full: bool, data: Option<TestItem>) -> RCStreamCdcQ<b8, (), Red, Blue, 4> {
        RCStreamCdcQ::<b8, (), Red, Blue, 4> {
            fifo: asynchronous::Out::<TestItem, Red, Blue> {
                data: signal(data),
                almost_empty: signal(false),
                underflow: signal(false),
                full: signal(full),
                almost_full: signal(false),
                overflow: signal(false),
            },
        }
    }

    fn in_with(data: Option<TestItem>, ready: bool) -> In<b8, (), Red, Blue> {
        In::<b8, (), Red, Blue> {
            data: signal(data),
            ready: signal(ready),
            cr_w: signal(ClockReset::dont_care()),
            cr_r: signal(ClockReset::dont_care()),
        }
    }

    /// Tier 1 — the write gate is the whole point of the widget.  When
    /// the FIFO is full, a source presenting `Some` must NOT reach the
    /// FIFO's data port, or it overflows.  The bus contract explicitly
    /// permits the source to present `Some` while `ready` is false.
    #[test]
    fn write_gate_blocks_when_full() {
        let i = in_with(Some(item(0xAB)), false);
        let q = q_with(true, None);
        let (o, d) = rcstream_cdc_kernel::<b8, (), Red, Blue, 4>(i, q);
        assert_eq!(
            d.fifo.data.val(),
            None,
            "a full FIFO must not be written, even though the source presented an item"
        );
        assert!(!o.ready.val(), "ready must be deasserted while full");
    }

    /// Tier 1 — the complementary case: with room, the item passes
    /// through to the FIFO unchanged and `ready` is asserted.
    #[test]
    fn write_passes_when_not_full() {
        let i = in_with(Some(item(0x55)), false);
        let q = q_with(false, None);
        let (o, d) = rcstream_cdc_kernel::<b8, (), Red, Blue, 4>(i, q);
        match d.fifo.data.val() {
            Some(it) => assert_eq!(it.data.raw(), 0x55),
            None => panic!("expected the item to reach the FIFO when not full"),
        }
        assert!(o.ready.val(), "ready must be asserted while not full");
    }

    /// Tier 1 — an idle source drives no write regardless of fullness.
    #[test]
    fn idle_source_drives_no_write() {
        let i = in_with(None, false);
        let q = q_with(false, None);
        let (_o, d) = rcstream_cdc_kernel::<b8, (), Red, Blue, 4>(i, q);
        assert_eq!(d.fifo.data.val(), None);
    }

    /// Tier 1 — the read gate: a transfer happens only when the sink
    /// is ready AND an item is actually present.
    #[test]
    fn read_advances_on_ready_with_data() {
        let i = in_with(None, true);
        let q = q_with(false, Some(item(0x11)));
        let (o, d) = rcstream_cdc_kernel::<b8, (), Red, Blue, 4>(i, q);
        assert!(d.fifo.next.val(), "next must assert on ready + data");
        match o.data.val() {
            Some(it) => assert_eq!(it.data.raw(), 0x11),
            None => panic!("expected the FIFO's item on the output"),
        }
    }

    /// Tier 1 — backpressure: data present but the sink is not ready
    /// means no advance, and the item stays on the output.
    #[test]
    fn read_holds_when_sink_not_ready() {
        let i = in_with(None, false);
        let q = q_with(false, Some(item(0x22)));
        let (o, d) = rcstream_cdc_kernel::<b8, (), Red, Blue, 4>(i, q);
        assert!(
            !d.fifo.next.val(),
            "next must not assert while the sink is not ready"
        );
        assert!(o.data.val().is_some(), "the item must remain presented");
    }

    /// Tier 1 — the underflow guard: `ready` asserted against an empty
    /// FIFO must not advance the read pointer.
    #[test]
    fn read_gate_blocks_when_empty() {
        let i = in_with(None, true);
        let q = q_with(false, None);
        let (o, d) = rcstream_cdc_kernel::<b8, (), Red, Blue, 4>(i, q);
        assert!(
            !d.fifo.next.val(),
            "next must not assert on an empty FIFO, or it underflows"
        );
        assert_eq!(o.data.val(), None);
    }

    /// Tier 1 — the FIFO's sticky error flags are surfaced verbatim, in
    /// their respective domains.
    #[test]
    fn error_flags_are_surfaced() {
        let i = in_with(None, false);
        let mut q = q_with(false, None);
        q.fifo.overflow = signal(true);
        q.fifo.underflow = signal(true);
        let (o, _d) = rcstream_cdc_kernel::<b8, (), Red, Blue, 4>(i, q);
        assert!(o.overflow.val());
        assert!(o.underflow.val());
    }

    /// Tier 2 — the end-to-end crossing property, and the one that
    /// matters most: every item written in `W` emerges in `R` exactly
    /// once, in order, with no drops, duplicates, or reordering.
    ///
    /// The source here is deliberately *aggressive*: it presents
    /// `Some(item)` on **every** cycle, including cycles where the
    /// crossing has deasserted `ready`.  That is legal under the bus
    /// contract (`data.is_some()` must not depend on `ready`) and it is
    /// precisely the case the write gate exists to handle — an ungated
    /// FIFO would overflow here.  The sink applies deterministic
    /// backpressure so the FIFO actually fills.
    ///
    /// Determinism: no RNG (CLAUDE.md §12 rule 10).  The sink's
    /// ready pattern is a fixed period-3 cycle.
    #[test]
    fn items_cross_domains_in_order_without_loss() {
        const COUNT: u128 = 64;
        let uut = RCStreamCdc::<b8, (), Red, Blue, 4>::default();

        // Source state: the item currently being presented.
        let mut next_to_send: u128 = 0;
        // Sink state: the next item we expect to receive.
        let mut expect_next: u128 = 0;
        // Sink backpressure phase counter.
        let mut phase: u32 = 0;
        let mut received: u128 = 0;
        let mut saw_overflow = false;
        let mut saw_underflow = false;

        let samples = run_async_red_blue(
            &uut,
            // W (Red) — the source.  Always presents an item while it
            // still has one, regardless of `ready`.
            |output, input| {
                if next_to_send < COUNT {
                    input.data = signal(Some(item(next_to_send)));
                    // The item presented during the cycle that just
                    // ended was accepted iff the crossing had room.
                    if output.ready.val() {
                        next_to_send += 1;
                    }
                } else {
                    input.data = signal(None);
                }
            },
            // R (Blue) — the sink.  Deterministic backpressure: ready
            // on 2 of every 3 cycles.
            |output, input| {
                phase = phase.wrapping_add(1);
                let want = !phase.is_multiple_of(3);
                input.ready = signal(false);
                if want && output.data.val().is_some() {
                    input.ready = signal(true);
                    let got = output.data.val().unwrap();
                    assert_eq!(
                        got.data.raw(),
                        expect_next,
                        "item arrived out of order: expected {expect_next}, got {}",
                        got.data.raw()
                    );
                    expect_next += 1;
                    received += 1;
                }
            },
            50,
            78,
            |red, blue, input| {
                input.cr_w = red;
                input.cr_r = blue;
            },
        );

        for sample in samples.take_while(|t| t.time < 400_000) {
            if sample.output.overflow.val() {
                saw_overflow = true;
            }
            if sample.output.underflow.val() {
                saw_underflow = true;
            }
        }

        assert!(
            !saw_overflow,
            "the write gate must make overflow unreachable, even with a source \
             that presents an item on every cycle"
        );
        assert!(
            !saw_underflow,
            "the read gate must make underflow unreachable"
        );
        assert_eq!(
            received, COUNT,
            "every item must cross exactly once (got {received} of {COUNT})"
        );
    }

    /// Tier 3 — HDL emission snapshot.
    ///
    /// Snapshots the widget's **own** emitted module: the two gates,
    /// the output wiring, and the port list.  Deliberately scoped to
    /// the top module rather than the whole tree — the sub-modules are
    /// `AsyncFIFO`'s emitted Verilog (BRAM + two gray-code
    /// cross-counters + read/write logic), which belong to that
    /// widget's own snapshot contract, not this one.  Scoping this way
    /// keeps the snapshot a real contract on *this* widget's codegen
    /// while staying stable against unrelated FIFO-internal changes.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = RCStreamCdc::<b8, (), Red, Blue, 4>::default();
        let desc = uut.descriptor("rcstream_cdc".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "rcstream_cdc")
            .expect("the top module must be present in the emitted Verilog");
        let expect = expect_test::expect![[r#"
            module rcstream_cdc(input wire [13:0] i, output wire [11:0] o);
               wire [25:0] od;
               wire [13:0] d;
               wire [13:0] q;
               assign o = od[11:0];
               rcstream_cdc_fifo c0(.i(d[13:0]), .o(q[13:0]));
               assign d = od[25:12];
               assign od = kernel_rcstream_cdc_kernel(i, q);
               function [25:0] kernel_rcstream_cdc_kernel(input reg [13:0] arg_0, input reg [13:0] arg_1);
                     reg [1:0] r0;
                     reg [13:0] r1;
                     // d
                     reg [13:0] r2;
                     reg [1:0] r3;
                     // d
                     reg [13:0] r4;
                     reg [13:0] r5;
                     reg [0:0] r6;
                     reg [0:0] r7;
                     reg [8:0] r8;
                     reg [8:0] r9;
                     // d
                     reg [13:0] r10;
                     reg [8:0] r11;
                     reg [0:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     reg [0:0] r15;
                     // d
                     reg [13:0] r16;
                     // o
                     reg [11:0] r17;
                     // o
                     reg [11:0] r18;
                     reg [0:0] r19;
                     // o
                     reg [11:0] r20;
                     reg [0:0] r21;
                     // o
                     reg [11:0] r22;
                     reg [25:0] r23;
                     localparam l0 = 14'bXXXXXXXXXXXXXX;
                     localparam l1 = 9'b000000000;
                     localparam l2 = 1'b1;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b0;
                     localparam l5 = 1'b0;
                     localparam l6 = 12'bXXXXXXXXXXXX;
                     begin
                        r1 = arg_0;
                        r5 = arg_1;
                        r0 = r1[11:10];
                        r2 = l0;
                        r2[11:10] = r0;
                        r3 = r1[13:12];
                        r4 = r2;
                        r4[13:12] = r3;
                        r6 = r5[11:11];
                        r7 = ~r6;
                        r8 = r1[8:0];
                        r9 = r7 ? r8 : l1;
                        r10 = r4;
                        r10[8:0] = r9;
                        r11 = r5[8:0];
                        r12 = r11[8:8];
                        case (r12)
                           1'b1 : r13 = l3;
                           1'b0 : r13 = l5;
                        endcase
                        r14 = r1[9:9];
                        r15 = r14 & r13;
                        r16 = r10;
                        r16[9:9] = r15;
                        r17 = l6;
                        r17[0:0] = r7;
                        r18 = r17;
                        r18[9:1] = r11;
                        r19 = r5[13:13];
                        r20 = r18;
                        r20[10:10] = r19;
                        r21 = r5[10:10];
                        r22 = r20;
                        r22[11:11] = r21;
                        r23 = {r16, r22};
                        kernel_rcstream_cdc_kernel = r23;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    /// Build the open-loop stimulus used by the `iverilog` round-trip
    /// and the VCD digest.  Open-loop (rather than the closed-loop
    /// harness above) because a testbench needs a concrete, replayable
    /// input sequence.
    fn open_loop_stream() -> impl Iterator<Item = TimedSample<In<b8, (), Red, Blue>>> {
        let write = (0..32u128)
            .map(|x| Some(item(x)))
            .chain(std::iter::repeat(None))
            .take(64)
            .with_reset(1)
            .clock_pos_edge(100);
        let read = std::iter::repeat_n(false, 24)
            .chain(std::iter::repeat(true))
            .take(64)
            .with_reset(1)
            .clock_pos_edge(75);
        write.merge_map(read, |w, r| In {
            data: signal(w.1),
            ready: signal(r.1),
            cr_w: signal(w.0),
            cr_r: signal(r.0),
        })
    }

    /// Tier 4 — `iverilog` round-trip on both the RTL and NTL paths.
    /// The NTL path exercises the Stage-3 optimization passes that RTL
    /// skips.
    #[test]
    fn cdc_iverilog_round_trip() -> miette::Result<()> {
        let uut = RCStreamCdc::<b8, (), Red, Blue, 4>::default();
        let test_bench = uut.run(open_loop_stream()).collect::<TestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &TestBenchOptions::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &TestBenchOptions::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.  Catches ordering/timing changes that pass
    /// the functional tests but signal a regression.
    #[test]
    fn cdc_trace_digest() {
        let uut = RCStreamCdc::<b8, (), Red, Blue, 4>::default();
        let vcd = uut.run(open_loop_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("rcstream_cdc");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "be710e321fd0e5571dc308fbc3b23439c57e55cd9b9bf495b2f052ddaed0467e"
        ];
        let digest = vcd.dump_to_file(root.join("rcstream_cdc.vcd")).unwrap();
        expect.assert_eq(&digest);
    }
}
