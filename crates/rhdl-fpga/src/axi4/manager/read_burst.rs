//! Drive one AXI4 read burst: the address, and the beats that come back.
//!
//! The read counterpart of [`super::write_burst::Axi4WriteBurst`], and
//! the other half of Phase 2's manager.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+Axi4ReadBurst+----------+
      |                          |
 ?Addr|                 ar_valid |
+---->| cmd                   ar +----->
      |                          |
 bool |                  r_ready |
+---->| ar_ready                 +----->
+---->| r_valid             data |  ?b<D>
+---->| r                        +----->
+---->| sink_ready          busy |
      |                     done +----->
      +--------------------------+
")]
//!
//!# Internals
#![doc = badascii_doc::badascii!("
  +-+Idle+--+ cmd  +-+Addr+-+ arready +-+Data+-+ rlast
  |         +----->|        +-------->|        +------+
  +----^----+      +--------+         +--------+      |
       |                                              |
       +----------------------------------------------+
")]
//!
//! # Backpressure is forwarded, not absorbed
//!
//! `RREADY` is driven from the consumer's `sink_ready`, so a slow sink
//! stalls the bus rather than silently dropping beats. That is the
//! honest behaviour for a manager with no buffer: absorbing a beat it
//! cannot hand on would mean inventing storage the widget does not
//! have. A consumer that wants elasticity puts an
//! [`crate::rcstream::relay::RCStreamRelay`] or a FIFO behind it.
//!
//! # What this does not do yet
//!
//! One burst in flight, one ID, `Incr` and `Fixed`; `Wrap` is Phase 3
//! and is refused. Read beats bearing an unexpected `RID` are reported
//! on [`Out::id_mismatch`] rather than silently accepted — with one
//! transaction outstanding there is exactly one legal ID, so this is
//! checkable now; per-ID interleaving is Phase 4.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/axi4_read_burst.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/axi4_read_burst.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../../doc/axi4_read_burst_fsm.md")]

use super::super::types::{AxiAddress, AxiReadBeat, AxiResponse, BurstKind, size_fits_bus};
use crate::core::dff;
use rhdl::prelude::*;

/// Where the read sequencer is.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum ReadState {
    /// No burst in flight; a `cmd` is accepted this cycle.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Offering the address, waiting for `ARREADY`.
    #[fsm_state(label = "addr")]
    Addr,
    /// Taking beats until `RLAST`.
    #[fsm_state(label = "data")]
    Data,
}

/// The latched descriptor and the beat counter, bundled.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Extras<const A: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    ar: AxiAddress<A, ID>,
    beat: Bits<8>,
    /// The worst response any beat of this burst carried.
    ///
    /// `RRESP` is per beat, unlike `BRESP`, so a burst can be partly
    /// in error. Reporting only the last beat's status would hide that.
    worst: AxiResponse,
}

/// Inputs to the read-burst manager.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In<const A: usize, const DW: usize, const BYTES: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// A burst to start. Read only while idle.
    pub cmd: Option<AxiAddress<A, ID>>,
    /// `ARREADY` from the subordinate.
    pub ar_ready: bool,
    /// `RVALID` from the subordinate.
    pub r_valid: bool,
    /// The read beat.
    pub r: AxiReadBeat<DW, ID>,
    /// The consumer can take a word this cycle.
    pub sink_ready: bool,
}

/// Outputs from the read-burst manager.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct Out<const A: usize, const DW: usize, const BYTES: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `ARVALID`.
    pub ar_valid: bool,
    /// The address phase.
    pub ar: AxiAddress<A, ID>,
    /// `RREADY`, forwarded from `sink_ready`.
    pub r_ready: bool,
    /// The word just accepted, for the cycle it was accepted.
    pub data: Option<Bits<DW>>,
    /// A burst is in flight.
    pub busy: bool,
    /// The worst `RRESP` of the burst, on the cycle it completes.
    pub done: Option<AxiResponse>,
    /// The command was refused because this manager cannot drive it.
    pub rejected: bool,
    /// A beat arrived bearing an `RID` the outstanding transaction did
    /// not ask for.
    pub id_mismatch: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = ReadState, allow_implicit)]
/// Drives one AXI4 read burst at a time.
///
/// `A` is the address width, `DW` the data width and `ID` the
/// transaction-ID width. `DW` rather than `D` because
/// `#[rhdl(dq_no_prefix)]` generates a type named `D`.
pub struct Axi4ReadBurst<const A: usize, const DW: usize, const BYTES: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    state: dff::DFF<ReadState>,
    extras: dff::DFF<Extras<A, ID>>,
}

impl<const A: usize, const DW: usize, const BYTES: usize, const ID: usize> SynchronousIO
    for Axi4ReadBurst<A, DW, BYTES, ID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    type I = In<A, DW, BYTES, ID>;
    type O = Out<A, DW, BYTES, ID>;
    type Kernel = read_burst<A, DW, BYTES, ID>;
}

#[kernel]
/// The more severe of two responses.
///
/// Ordered `Okay < ExOkay < SlvErr < DecErr`: an error anywhere in the
/// burst outranks a success, so a partly-failed read reports the
/// failure rather than the last beat's status.
pub fn worse(a: AxiResponse, b: AxiResponse) -> AxiResponse {
    let rank_a = match a {
        AxiResponse::Okay => bits::<2>(0),
        AxiResponse::ExOkay => bits::<2>(1),
        AxiResponse::SlvErr => bits::<2>(2),
        AxiResponse::DecErr => bits::<2>(3),
    };
    let rank_b = match b {
        AxiResponse::Okay => bits::<2>(0),
        AxiResponse::ExOkay => bits::<2>(1),
        AxiResponse::SlvErr => bits::<2>(2),
        AxiResponse::DecErr => bits::<2>(3),
    };
    if rank_a >= rank_b { a } else { b }
}

#[kernel]
/// Can this manager drive the requested burst?
pub fn drivable<const A: usize, const ID: usize>(a: AxiAddress<A, ID>, bus_bytes: Bits<9>) -> bool
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let kind_ok = match a.burst {
        BurstKind::Incr => true,
        BurstKind::Fixed => true,
        BurstKind::Wrap => false,
    };
    kind_ok && size_fits_bus(a.size, bus_bytes)
}

#[kernel]
/// Kernel for [Axi4ReadBurst].
pub fn read_burst<const A: usize, const DW: usize, const BYTES: usize, const ID: usize>(
    cr: ClockReset,
    i: In<A, DW, BYTES, ID>,
    q: Q<A, DW, BYTES, ID>,
) -> (Out<A, DW, BYTES, ID>, D<A, DW, BYTES, ID>)
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<BYTES>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    // `BYTES` rather than `DW / 8`: division is not a kernel operator.
    // Same workaround, same reason, as `check::read_monitor`.
    let bus_bytes = bits::<9>(BYTES as u128);
    let s = q.extras;

    let mut o = Out::<A, DW, BYTES, ID>::dont_care();
    o.ar_valid = false;
    o.ar = s.ar;
    o.r_ready = false;
    o.data = None;
    o.busy = true;
    o.done = None;
    o.rejected = false;
    o.id_mismatch = false;

    let mut next_state = q.state;
    let mut next = s;

    match q.state {
        ReadState::Idle => {
            o.busy = false;
            if let Some(c) = i.cmd {
                if drivable::<A, ID>(c, bus_bytes) {
                    next.ar = c;
                    next.beat = bits::<8>(0);
                    next.worst = AxiResponse::Okay;
                    next_state = ReadState::Addr;
                } else {
                    o.rejected = true;
                }
            }
        }
        ReadState::Addr => {
            o.ar_valid = true;
            o.ar = s.ar;
            if i.ar_ready {
                next_state = ReadState::Data;
            }
        }
        ReadState::Data => {
            // Forwarded, not absorbed: no buffer here means a slow sink
            // must stall the bus.
            o.r_ready = i.sink_ready;
            if i.r_valid && i.sink_ready {
                o.data = Some(i.r.data);
                o.id_mismatch = i.r.id != s.ar.id;
                let w = worse(s.worst, i.r.resp);
                next.worst = w;
                if i.r.last {
                    o.done = Some(w);
                    next_state = ReadState::Idle;
                } else {
                    next.beat = s.beat + bits::<8>(1);
                }
            }
        }
    }
    // The burst ends on RLAST, not on a beat count. Ending on the count
    // would make a *missing* RLAST unobservable here, and reporting it
    // is the read monitor's job -- a manager that silently recovered
    // would hide the subordinate's fault.

    let mut d = D::<A, DW, BYTES, ID>::dont_care();
    d.state = next_state;
    d.extras = next;

    if cr.reset.any() {
        d.state = ReadState::Idle;
        d.extras = Extras::<A, ID>::default();
        o.ar_valid = false;
        o.r_ready = false;
        o.data = None;
        o.busy = false;
        o.done = None;
        o.rejected = false;
        o.id_mismatch = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    type Uut = Axi4ReadBurst<32, 32, 4, 4>;

    fn run_cr() -> ClockReset {
        clock_reset(clock(true), reset(false))
    }

    fn cmd(len: u128, size: u128, burst: BurstKind) -> AxiAddress<32, 4> {
        AxiAddress::<32, 4> {
            id: bits(1),
            addr: bits(0x2000),
            len: bits(len),
            size: bits(size),
            burst,
            ..Default::default()
        }
    }

    fn q(
        state: ReadState,
        ar: AxiAddress<32, 4>,
        beat: u128,
        worst: AxiResponse,
    ) -> Q<32, 32, 4, 4> {
        Q::<32, 32, 4, 4> {
            state,
            extras: Extras::<32, 4> {
                ar,
                beat: bits(beat),
                worst,
            },
        }
    }

    fn beat(id: u128, data: u128, resp: AxiResponse, last: bool) -> AxiReadBeat<32, 4> {
        AxiReadBeat::<32, 4> {
            id: bits(id),
            data: bits(data),
            resp,
            last,
        }
    }

    /// Tier 1: reset returns to Idle with nothing asserted.
    #[test]
    fn reset_idles_and_asserts_nothing() {
        let cr = clock_reset(clock(true), reset(true));
        let (o, d) = read_burst::<32, 32, 4, 4>(
            cr,
            In::default(),
            q(
                ReadState::Data,
                cmd(3, 2, BurstKind::Incr),
                2,
                AxiResponse::SlvErr,
            ),
        );
        assert!(!o.ar_valid && !o.r_ready && !o.busy);
        assert_eq!(o.data, None);
        assert_eq!(d.state, ReadState::Idle);
        assert_eq!(d.extras, Extras::<32, 4>::default());
    }

    /// Tier 1: WRAP and an oversized `ARSIZE` are refused in place.
    #[test]
    fn undrivable_commands_are_refused_in_place() {
        for bad in [cmd(3, 2, BurstKind::Wrap), cmd(3, 7, BurstKind::Incr)] {
            let i = In::<32, 32, 4, 4> {
                cmd: Some(bad),
                ..Default::default()
            };
            let (o, d) = read_burst::<32, 32, 4, 4>(
                run_cr(),
                i,
                q(ReadState::Idle, AxiAddress::default(), 0, AxiResponse::Okay),
            );
            assert!(o.rejected, "{bad:?} should be refused");
            assert!(!o.ar_valid);
            assert_eq!(d.state, ReadState::Idle);
        }
    }

    /// Tier 1: `RREADY` is the consumer's readiness, forwarded.
    ///
    /// The manager has no buffer, so absorbing a beat it cannot hand on
    /// would mean inventing storage it does not have.
    #[test]
    fn rready_is_forwarded_from_the_sink() {
        let c = cmd(3, 2, BurstKind::Incr);
        for sink_ready in [false, true] {
            let i = In::<32, 32, 4, 4> {
                r_valid: true,
                r: beat(1, 0xAB, AxiResponse::Okay, false),
                sink_ready,
                ..Default::default()
            };
            let (o, d) = read_burst::<32, 32, 4, 4>(
                run_cr(),
                i,
                q(ReadState::Data, c, 0, AxiResponse::Okay),
            );
            assert_eq!(o.r_ready, sink_ready);
            assert_eq!(o.data.is_some(), sink_ready, "no take without the sink");
            assert_eq!(d.extras.beat.raw(), if sink_ready { 1 } else { 0 });
        }
    }

    /// Tier 1: the burst ends on `RLAST`, reporting the worst `RRESP`.
    ///
    /// `RRESP` is per beat, unlike `BRESP`, so a burst can be partly in
    /// error; reporting only the last beat's status would hide that.
    #[test]
    fn the_worst_response_of_the_burst_is_reported() {
        let c = cmd(1, 2, BurstKind::Incr);
        // A mid-burst error, then an OKAY last beat.
        let i = In::<32, 32, 4, 4> {
            r_valid: true,
            r: beat(1, 0x11, AxiResponse::Okay, true),
            sink_ready: true,
            ..Default::default()
        };
        let (o, d) =
            read_burst::<32, 32, 4, 4>(run_cr(), i, q(ReadState::Data, c, 1, AxiResponse::SlvErr));
        assert_eq!(
            o.done,
            Some(AxiResponse::SlvErr),
            "an error earlier in the burst outranks an OKAY last beat"
        );
        assert_eq!(d.state, ReadState::Idle);
    }

    /// Tier 1: `worse` orders the four responses.
    #[test]
    fn worse_orders_the_responses() {
        use AxiResponse::*;
        assert_eq!(worse(Okay, ExOkay), ExOkay);
        assert_eq!(worse(ExOkay, SlvErr), SlvErr);
        assert_eq!(worse(SlvErr, DecErr), DecErr);
        assert_eq!(worse(DecErr, Okay), DecErr);
        assert_eq!(worse(Okay, Okay), Okay);
    }

    /// Tier 1: a beat bearing the wrong `RID` is reported.
    #[test]
    fn a_beat_with_the_wrong_id_is_reported() {
        let c = cmd(3, 2, BurstKind::Incr);
        let i = In::<32, 32, 4, 4> {
            r_valid: true,
            r: beat(9, 0x11, AxiResponse::Okay, false),
            sink_ready: true,
            ..Default::default()
        };
        let (o, _d) =
            read_burst::<32, 32, 4, 4>(run_cr(), i, q(ReadState::Data, c, 0, AxiResponse::Okay));
        assert!(o.id_mismatch);
        assert!(o.data.is_some(), "the beat is still delivered, and flagged");
    }

    /// Tier 2: a four-beat burst delivers four words and completes once.
    #[test]
    fn a_burst_delivers_every_beat() {
        let c = cmd(3, 2, BurstKind::Incr);
        let stimulus: Vec<In<32, 32, 4, 4>> = (0..12)
            .map(|k| In::<32, 32, 4, 4> {
                cmd: if k == 0 { Some(c) } else { None },
                ar_ready: true,
                r_valid: (2..=5).contains(&k),
                r: beat(1, 0xD0 + k as u128, AxiResponse::Okay, k == 5),
                sink_ready: true,
            })
            .collect();
        let uut = Uut::default();
        let input = stimulus.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output)
            .collect::<Vec<_>>();
        assert_eq!(out.iter().filter(|o| o.data.is_some()).count(), 4);
        assert_eq!(out.iter().filter(|o| o.done.is_some()).count(), 1);
        assert!(!out.iter().any(|o| o.id_mismatch));
    }

    /// Tier 3: the emitted Verilog's interface is the contract.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let headers = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .collect::<Vec<_>>()
            .join("\n");
        expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [107:0] i, output wire [105:0] o);
            module top_state(input wire [1:0] clock_reset, input wire [1:0] i, output reg [1:0] o);
            module top_extras(input wire [1:0] clock_reset, input wire [74:0] i, output reg [74:0] o);"#]].assert_eq(&headers);
        expect!["412"].assert_eq(&hdl.lines().count().to_string());
        Ok(())
    }

    /// Tier 4: the ground truth, through both the RTL and NTL paths.
    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let c = cmd(3, 2, BurstKind::Incr);
        let stimulus: Vec<In<32, 32, 4, 4>> = (0..12)
            .map(|k| In::<32, 32, 4, 4> {
                cmd: if k == 0 { Some(c) } else { None },
                ar_ready: true,
                r_valid: (2..=5).contains(&k),
                r: beat(1, 0xD0 + k as u128, AxiResponse::Okay, k == 5),
                sink_ready: true,
            })
            .collect();
        let uut = Uut::default();
        let input = stimulus.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5: a digest of the waveform.
    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let c = cmd(3, 2, BurstKind::Incr);
        let stimulus: Vec<In<32, 32, 4, 4>> = (0..12)
            .map(|k| In::<32, 32, 4, 4> {
                cmd: if k == 0 { Some(c) } else { None },
                ar_ready: true,
                r_valid: (2..=5).contains(&k),
                r: beat(1, 0xD0 + k as u128, AxiResponse::Okay, k == 5),
                sink_ready: true,
            })
            .collect();
        let uut = Uut::default();
        let input = stimulus.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("axi4_read_burst");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["f95eaad2391157203a87fa96e0d3944d721c35de0ca7aba8ade21314cac90b71"];
        let digest = vcd.dump_to_file(root.join("burst.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// The FSM has exactly the three edges the sequencer can take.
    ///
    /// Pinned as a set rather than a count: the extractor
    /// over-approximates when a condition does not mention the state.
    #[test]
    fn the_fsm_has_exactly_the_expected_inter_state_edges() {
        use std::collections::BTreeSet;
        let transitions =
            rhdl::core::fsm::extract_widget_transitions_strict::<Uut>().expect("extracting");
        const IDLE: usize = 0;
        const ADDR: usize = 1;
        const DATA: usize = 2;
        let edges: BTreeSet<(usize, usize)> = transitions
            .iter()
            .map(|t| (t.source_index, t.target_index))
            .filter(|(a, b)| a != b)
            .collect();
        let expected: BTreeSet<(usize, usize)> = [(IDLE, ADDR), (ADDR, DATA), (DATA, IDLE)]
            .into_iter()
            .collect();
        assert_eq!(edges, expected, "FSM edge set drifted");
    }

    /// The FSM metadata the diagram is generated from.
    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = Uut::fsm_descriptor();
        assert_eq!(desc.widget_name, "Axi4ReadBurst");
        assert_eq!(desc.variants().len(), 3);
        assert_eq!(desc.initial_index(), 0);
    }
}
