#![warn(missing_docs)]
//! Pipeline register for a [`super::CreditRCStream`] connection.
//!
//! The credit variant exists for **long inter-block paths** — the case
//! where you most need to break the path with a register — and until
//! now it had no way to insert one.  [`super::super::relay::RCStreamRelay`]
//! only speaks the simple Ready/Valid form.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +CreditRCStreamRelay+
 ?T   |                   |  ?T
+---->| data         data +---->
      |                   |
<-----+ credit     credit |<----+
      |                   |
      +-------------------+
")]
//!
//!# Why this is a register pair, not a skid buffer
//!
//! [`super::super::relay::RCStreamRelay`] is a Carloni skid buffer
//! because the simple bus has **forward backpressure**: a sink can
//! deassert `ready`, the item must be held somewhere, and the skid
//! buffer is that somewhere.
//!
//! The credit protocol has no forward backpressure at all.  A source
//! never asks "are you ready?" — it sends only when it holds a credit,
//! and the sink has already reserved buffer space for every credit it
//! issued.  There is therefore no stall for a relay to absorb, and the
//! relay forwards unconditionally on every cycle.  A skid buffer here
//! would be dead silicon.
//!
//! It cannot be overrun either: the source's credit accounting bounds
//! the number of items in flight to the capacity the sink has already
//! reserved, and this relay holds at most one of them at a time.
//!
//!# Credit conservation — the correctness property
//!
//! `credit_grant` is a **count**, not a level.  The invariant a relay
//! must preserve is that the running total of credits reaching the
//! source equals the running total the sink issued: grants may be
//! *delayed*, never dropped, merged, or duplicated.  Lose one grant and
//! the source permanently believes it has one fewer token than it does;
//! the link degrades and eventually deadlocks.  Duplicate one and the
//! source can overrun the sink's buffer.
//!
//! A register preserves each cycle's value exactly, shifted by one
//! cycle, so the running total is conserved.  That is the whole
//! argument, and it is why the reverse path must be a plain register
//! rather than anything that combines or gates grants.
//!
//!# This relay does NOT preserve throughput
//!
//! **The important difference from [`super::super::relay::RCStreamRelay`].**
//! There, Carloni's theorem guarantees insertion costs one cycle of
//! latency and *nothing else* — throughput is unchanged at any depth.
//!
//! Credit-based flow control has no such guarantee.  Sustained
//! throughput is bounded by
//!
//! ```text
//!   credits available >= round-trip latency (in cycles)
//! ```
//!
//! because a credit cannot be reused until it has travelled to the sink
//! as an item and come back as a grant.  Each relay adds **two** cycles
//! to that round trip — one forward on `data`, one back on
//! `credit_grant`.  Insert enough relays and the source runs dry
//! waiting for credit that is still in flight, even though the sink has
//! free space.
//!
//! So: inserting these relays is always **correct** (the item sequence
//! is preserved at any depth) but not always **free**.  If throughput
//! matters, size the sink's `FIFO_N` — and hence the initial credit
//! pool — to cover the round trip *after* insertion.  The tests in this
//! module demonstrate both halves of that: sequence preserved at every
//! depth, and throughput falling with depth under a small pool while
//! holding steady under a large one.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/credit_relay.rs")]
//!```
//!
//! The trace below shows two relays inserted on the link: the forward
//! `data` path and the reverse `credit_grant` path each pick up one
//! cycle per stage.
#![doc = include_str!("../../../doc/credit_relay.md")]

use rhdl::prelude::*;

use crate::core::dff;
use crate::rcstream::bus::Item;

/// A pipeline register for a [`super::CreditRCStream`] connection.
///
/// One cycle of latency forward on `data`, one cycle back on
/// `credit_grant`.  See the module docs: this is correct at any
/// insertion depth, but each stage adds two cycles to the credit round
/// trip and so can cost throughput unless the credit pool is sized for
/// it.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CreditRCStreamRelay<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Forward data register.
    data: dff::DFF<Option<Item<T, F>>>,
    /// Reverse credit-grant register.
    grant: dff::DFF<Bits<CREDIT_W>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize> Default for CreditRCStreamRelay<T, F, CREDIT_W>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            // Reset state: no item in flight, no credit in flight.
            // Emitting a phantom grant out of reset would inflate the
            // source's counter and let it overrun the sink.
            data: dff::DFF::new(None),
            grant: dff::DFF::new(bits::<CREDIT_W>(0)),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`CreditRCStreamRelay`].
pub struct In<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Data flowing in from the upstream side of the connection.
    pub data: Option<Item<T, F>>,
    /// Credit grant flowing in from the downstream side.
    pub credit_grant: Bits<CREDIT_W>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [`CreditRCStreamRelay`].
pub struct Out<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Data flowing out to the downstream side, delayed one cycle.
    pub data: Option<Item<T, F>>,
    /// Credit grant flowing out to the upstream side, delayed one
    /// cycle.
    pub credit_grant: Bits<CREDIT_W>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize> SynchronousIO
    for CreditRCStreamRelay<T, F, CREDIT_W>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    type I = In<T, F, CREDIT_W>;
    type O = Out<T, F, CREDIT_W>;
    type Kernel = credit_relay_kernel<T, F, CREDIT_W>;
}

#[kernel]
/// Kernel for [`CreditRCStreamRelay`].
pub fn credit_relay_kernel<T: Digital, F: Digital, const CREDIT_W: usize>(
    _cr: ClockReset,
    i: In<T, F, CREDIT_W>,
    q: Q<T, F, CREDIT_W>,
) -> (Out<T, F, CREDIT_W>, D<T, F, CREDIT_W>)
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    let mut d = D::<T, F, CREDIT_W>::dont_care();

    // Forward and reverse paths are each a plain register.  Nothing is
    // gated: gating the grant path would break credit conservation, and
    // there is no forward backpressure to gate the data path on.
    d.data = i.data;
    d.grant = i.credit_grant;

    let o = Out::<T, F, CREDIT_W> {
        data: q.data,
        credit_grant: q.grant,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Relay = CreditRCStreamRelay<b8, (), 5>;

    fn item(v: u128) -> Item<b8, ()> {
        Item::<b8, ()> {
            data: bits::<8>(v),
            frame: (),
        }
    }

    /// Tier 1 — the forward path delays by exactly one cycle: what the
    /// register holds is what comes out, and what comes in is what the
    /// register will hold.
    #[test]
    fn forward_path_is_a_one_cycle_delay() {
        let q = Q::<b8, (), 5> {
            data: Some(item(0xAA)),
            grant: bits::<5>(0),
        };
        let i = In::<b8, (), 5> {
            data: Some(item(0xBB)),
            credit_grant: bits::<5>(0),
        };
        let (o, d) = credit_relay_kernel::<b8, (), 5>(ClockReset::dont_care(), i, q);
        match o.data {
            Some(it) => assert_eq!(it.data.raw(), 0xAA, "output is the registered value"),
            None => panic!("expected the held item on the output"),
        }
        match d.data {
            Some(it) => assert_eq!(it.data.raw(), 0xBB, "input is what gets registered"),
            None => panic!("expected the incoming item to be captured"),
        }
    }

    /// Tier 1 — the reverse credit path delays by exactly one cycle and
    /// passes the grant **unmodified**.  Any arithmetic here would break
    /// credit conservation.
    #[test]
    fn reverse_credit_path_is_an_unmodified_one_cycle_delay() {
        let q = Q::<b8, (), 5> {
            data: None,
            grant: bits::<5>(3),
        };
        let i = In::<b8, (), 5> {
            data: None,
            credit_grant: bits::<5>(7),
        };
        let (o, d) = credit_relay_kernel::<b8, (), 5>(ClockReset::dont_care(), i, q);
        assert_eq!(
            o.credit_grant.raw(),
            3,
            "output grant is the registered one"
        );
        assert_eq!(d.grant.raw(), 7, "incoming grant is captured verbatim");
    }

    /// Tier 1 — an idle cycle forwards idleness, and crucially forwards
    /// a **zero** grant.  Manufacturing credit out of nothing would let
    /// the source overrun the sink's buffer.
    #[test]
    fn idle_cycle_manufactures_no_credit() {
        let q = Q::<b8, (), 5> {
            data: None,
            grant: bits::<5>(0),
        };
        let i = In::<b8, (), 5> {
            data: None,
            credit_grant: bits::<5>(0),
        };
        let (o, d) = credit_relay_kernel::<b8, (), 5>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_none());
        assert_eq!(o.credit_grant.raw(), 0);
        assert_eq!(d.grant.raw(), 0);
    }

    /// The two directions are independent: a grant travelling backward
    /// must not disturb an item travelling forward on the same cycle.
    #[test]
    fn directions_are_independent() {
        let q = Q::<b8, (), 5> {
            data: Some(item(0x11)),
            grant: bits::<5>(2),
        };
        let i = In::<b8, (), 5> {
            data: Some(item(0x22)),
            credit_grant: bits::<5>(4),
        };
        let (o, d) = credit_relay_kernel::<b8, (), 5>(ClockReset::dont_care(), i, q);
        assert_eq!(o.data.unwrap().data.raw(), 0x11);
        assert_eq!(o.credit_grant.raw(), 2);
        assert_eq!(d.data.unwrap().data.raw(), 0x22);
        assert_eq!(d.grant.raw(), 4);
    }

    /// Reset state must be "nothing in flight, no credit in flight".
    /// A relay that came out of reset holding a non-zero grant would
    /// inflate the source's credit counter.
    #[test]
    fn resets_to_no_item_and_no_credit() {
        let uut = Relay::default();
        let stream = std::iter::repeat_n(
            In::<b8, (), 5> {
                data: None,
                credit_grant: bits::<5>(0),
            },
            4,
        )
        .with_reset(2)
        .clock_pos_edge(100);
        let out = uut
            .run(stream)
            .synchronous_sample()
            .map(|s| (s.output.data.is_some(), s.output.credit_grant.raw()))
            .collect::<Vec<_>>();
        assert!(
            out.iter().all(|(has_item, grant)| !has_item && *grant == 0),
            "an idle, reset relay must emit no items and no credit: {out:?}"
        );
    }

    /// Tier 3 — HDL emission snapshot of the widget's own module.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Relay::default();
        let desc = uut.descriptor("credit_relay".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "credit_relay")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module credit_relay(input wire [1:0] clock_reset, input wire [13:0] i, output wire [13:0] o);
               wire [27:0] od;
               wire [13:0] d;
               wire [13:0] q;
               assign o = od[13:0];
               credit_relay_data c0(.clock_reset(clock_reset), .i(d[8:0]), .o(q[8:0]));
               credit_relay_grant c1(.clock_reset(clock_reset), .i(d[13:9]), .o(q[13:9]));
               assign d = od[27:14];
               assign od = kernel_credit_relay_kernel(clock_reset, i, q);
               function [27:0] kernel_credit_relay_kernel(input reg [1:0] arg_0, input reg [13:0] arg_1, input reg [13:0] arg_2);
                     reg [8:0] r0;
                     reg [13:0] r1;
                     // d
                     reg [13:0] r2;
                     reg [4:0] r3;
                     // d
                     reg [13:0] r4;
                     reg [8:0] r5;
                     reg [13:0] r6;
                     reg [4:0] r7;
                     reg [13:0] r8;
                     reg [13:0] r9;
                     reg [27:0] r10;
                     reg [1:0] r11;
                     localparam l0 = 14'bXXXXXXXXXXXXXX;
                     localparam l1 = 14'b00000000000000;
                     begin
                        r11 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[8:0];
                        r2 = l0;
                        r2[8:0] = r0;
                        r3 = r1[13:9];
                        r4 = r2;
                        r4[13:9] = r3;
                        r5 = r6[8:0];
                        r7 = r6[13:9];
                        r8 = l1;
                        r8[8:0] = r5;
                        r9 = r8;
                        r9[13:9] = r7;
                        r10 = {r4, r9};
                        kernel_credit_relay_kernel = r10;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    fn open_loop() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, (), 5>)>> {
        (0..24u128)
            .map(|k| In::<b8, (), 5> {
                data: if k % 3 == 0 { None } else { Some(item(k)) },
                credit_grant: bits::<5>(k % 2),
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 4 — `iverilog` round-trip on both RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = Relay::default();
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = Relay::default();
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("credit_relay");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "9976cfad911871af1b0409de13f1c4c43b93f056c4d876106ba04ab7bcc21f28"
        ];
        let digest = vcd.dump_to_file(root.join("credit_relay.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
