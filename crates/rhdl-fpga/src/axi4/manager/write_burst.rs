//! Drive one AXI4 write burst: the address, the beats, and the response.
//!
//! Takes a burst descriptor and a stream of data words and produces
//! conformant AW/W/B traffic, one burst at a time. This is the manager
//! half of Phase 2's loopback.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+Axi4WriteBurst+---------+
      |                          |
 ?Addr|                 aw_valid |
+---->| cmd                   aw +----->
      |                          |
 ?b<D>|                  w_valid |
+---->| data                   w +----->
      |                          |
 bool |                  b_ready |
+---->| aw_ready                 +----->
+---->| w_ready             busy |
+---->| b_valid       data_taken |
+---->| b                   done +----->
      +--------------------------+
")]
//!
//!# Internals
//!
//! A four-state sequencer over two registers: the state, and a bundle
//! holding the latched descriptor and the beat counter. Per CLAUDE.md
//! §3.1 the FSM enum keeps its own `DFF` — the `FsmWidget` extractor
//! finds transitions by matching `Index(q, [.state])`, and burying the
//! enum inside the bundle would hide them.
#![doc = badascii_doc::badascii!("
  +-+Idle+--+ cmd  +-+Addr+-+ awready +-+Data+-+ wlast  +-+Resp+-+
  |         +----->|        +-------->|        +------->|        |
  +---------+      +--------+         +--------+        +---+----+
       ^                                                    |
       +------------------- bvalid -------------------------+
")]
//!
//! # Why the address phase finishes before the data phase starts
//!
//! AXI4 permits AW and W to overlap, and a throughput-minded manager
//! would issue them together. This one does not, deliberately: a W beat
//! that arrives before its AW cannot be position-checked by
//! [`crate::axi4::check::monitor::Axi4WriteMonitor`], which counts such
//! beats on `unchecked_beats` rather than guessing which burst they
//! belong to. Overlapping the phases would therefore make the Phase 2
//! gate weaker — the monitor would fall silent precisely because it had
//! stopped checking. Overlap belongs with the outstanding-transaction
//! table in Phase 4, which is what makes those beats placeable.
//!
//! # What this does not do yet
//!
//! One burst in flight, one ID, `Incr` and `Fixed`. `Wrap` is Phase 3
//! and is rejected here rather than mis-driven — see
//! [`Out::rejected`]. Unaligned start addresses are Phase 3 too: this
//! manager drives full strobes for the lanes its size selects and does
//! not reduce the first beat.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/axi4_write_burst.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/axi4_write_burst.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../../doc/axi4_write_burst_fsm.md")]

use super::super::types::{
    AxiAddress, AxiResponse, AxiWriteBeat, AxiWriteResponse, BurstKind, beat_address, burst_beats,
    bytes_per_beat, size_fits_bus,
};
use crate::core::dff;
use rhdl::prelude::*;

/// Where the burst sequencer is.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum BurstState {
    /// No burst in flight; a `cmd` is accepted this cycle.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Offering the address, waiting for `AWREADY`.
    #[fsm_state(label = "addr")]
    Addr,
    /// Offering data beats, waiting for `WREADY` on each.
    #[fsm_state(label = "data")]
    Data,
    /// Waiting for `BVALID`.
    #[fsm_state(label = "resp")]
    Resp,
}

/// The latched descriptor and the beat counter, bundled.
///
/// One `Digital` struct in one `DFF` rather than sibling `DFF`s, per
/// CLAUDE.md §3.1.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Extras<const A: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// The burst being driven.
    aw: AxiAddress<A, ID>,
    /// Beats already accepted.
    beat: Bits<8>,
}

/// Inputs to the write-burst manager.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In<const A: usize, const DW: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// A burst to start. Read only while idle; ignored otherwise.
    pub cmd: Option<AxiAddress<A, ID>>,
    /// The next word to write. Consumed one per accepted beat, and only
    /// when `data_taken` is reported.
    pub data: Option<Bits<DW>>,
    /// `AWREADY` from the subordinate.
    pub aw_ready: bool,
    /// `WREADY` from the subordinate.
    pub w_ready: bool,
    /// `BVALID` from the subordinate.
    pub b_valid: bool,
    /// The response payload.
    pub b: AxiWriteResponse<ID>,
}

/// Outputs from the write-burst manager.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct Out<const A: usize, const DW: usize, const STRB: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `AWVALID`.
    pub aw_valid: bool,
    /// The address phase.
    pub aw: AxiAddress<A, ID>,
    /// `WVALID`.
    pub w_valid: bool,
    /// The data beat, with the strobes its size and address select.
    pub w: AxiWriteBeat<DW, STRB>,
    /// `BREADY`.
    pub b_ready: bool,
    /// A burst is in flight.
    pub busy: bool,
    /// `data` was consumed this cycle.
    pub data_taken: bool,
    /// The response, for exactly the cycle the burst completes.
    pub done: Option<AxiResponse>,
    /// The command was refused because this manager cannot drive it.
    ///
    /// Asserted for the cycle a `cmd` is seen that is not `Incr` or
    /// `Fixed`, or whose `AxSIZE` exceeds the bus. Refusing is the
    /// point: the alternative is driving traffic that the monitors
    /// would then correctly report, which would turn a manager
    /// limitation into a protocol violation.
    pub rejected: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = BurstState, allow_implicit)]
/// Drives one AXI4 write burst at a time.
///
/// `A` is the address width, `DW` the data width, `STRB = DW / 8` and
/// `ID` the transaction-ID width. `STRB` is a separate parameter
/// because `Bits<{ DW / 8 }>` does not compile on stable Rust and
/// division is not a kernel operator; [`AxiWriteBeat::check_widths`]
/// ties the two together.
pub struct Axi4WriteBurst<const A: usize, const DW: usize, const STRB: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    state: dff::DFF<BurstState>,
    extras: dff::DFF<Extras<A, ID>>,
}

impl<const A: usize, const DW: usize, const STRB: usize, const ID: usize> SynchronousIO
    for Axi4WriteBurst<A, DW, STRB, ID>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    type I = In<A, DW, ID>;
    type O = Out<A, DW, STRB, ID>;
    type Kernel = write_burst<A, DW, STRB, ID>;
}

#[kernel]
/// Can this manager drive the requested burst?
///
/// `Wrap` is Phase 3; an oversized `AxSIZE` is a protocol violation
/// whoever asked for it. Both are refused rather than driven.
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
/// The byte lanes a beat at `addr` of `size` bytes occupies.
///
/// Computed from the beat's offset within the bus, which is what makes
/// a narrow transfer's lanes rotate as the address advances.
pub fn lane_strobe<const A: usize, const STRB: usize>(
    addr: Bits<A>,
    size: Bits<3>,
    bus_bytes: Bits<9>,
) -> Bits<STRB>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
{
    // Clamp the operand, not the result: both arms of an `if` evaluate
    // in hardware and in the kernel VM, so an oversized `size` would
    // trip the VM's shift bound on the way past even when guarded.
    // CLAUDE.md §4.
    let fits = size_fits_bus(size, bus_bytes);
    let tb = if fits {
        bytes_per_beat(size)
    } else {
        bits::<9>(1)
    }
    .resize::<33>();
    let ones = (bits::<33>(1) << tb) - bits::<33>(1);
    let bus_mask = bus_bytes.resize::<33>() - bits::<33>(1);
    let lane = addr.resize::<33>() & bus_mask & !(tb - bits::<33>(1));
    (ones << lane).resize::<STRB>()
}

#[kernel]
/// Kernel for [Axi4WriteBurst].
pub fn write_burst<const A: usize, const DW: usize, const STRB: usize, const ID: usize>(
    cr: ClockReset,
    i: In<A, DW, ID>,
    q: Q<A, DW, STRB, ID>,
) -> (Out<A, DW, STRB, ID>, D<A, DW, STRB, ID>)
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    let bus_bytes = bits::<9>(STRB as u128);
    let s = q.extras;

    let mut o = Out::<A, DW, STRB, ID>::dont_care();
    o.aw_valid = false;
    o.aw = s.aw;
    o.w_valid = false;
    o.w = AxiWriteBeat::<DW, STRB>::dont_care();
    o.w.data = bits::<DW>(0);
    o.w.strb = bits::<STRB>(0);
    o.w.last = false;
    o.b_ready = false;
    o.busy = true;
    o.data_taken = false;
    o.done = None;
    o.rejected = false;

    let mut next_state = q.state;
    let mut next = s;

    // The beat currently being offered.
    let addr = beat_address::<A>(s.aw.addr, s.aw.size, s.aw.len, s.aw.burst, s.beat);
    let last_beat = s.beat == burst_beats(s.aw.len).resize::<8>() - bits::<8>(1);
    let have_data = match i.data {
        Some(_d) => true,
        None => false,
    };
    let word = match i.data {
        Some(d) => d,
        None => bits::<DW>(0),
    };

    match q.state {
        BurstState::Idle => {
            o.busy = false;
            if let Some(c) = i.cmd {
                if drivable::<A, ID>(c, bus_bytes) {
                    next.aw = c;
                    next.beat = bits::<8>(0);
                    next_state = BurstState::Addr;
                } else {
                    o.rejected = true;
                }
            }
        }
        BurstState::Addr => {
            o.aw_valid = true;
            o.aw = s.aw;
            if i.aw_ready {
                next_state = BurstState::Data;
            }
        }
        BurstState::Data => {
            // Offer a beat only when there is a word to send; `WVALID`
            // must not be asserted speculatively, because once asserted
            // it may not be withdrawn.
            o.w_valid = have_data;
            o.w.data = word;
            o.w.strb = lane_strobe::<A, STRB>(addr, s.aw.size, bus_bytes);
            o.w.last = last_beat;
            if have_data && i.w_ready {
                o.data_taken = true;
                if last_beat {
                    next_state = BurstState::Resp;
                } else {
                    next.beat = s.beat + bits::<8>(1);
                }
            }
        }
        BurstState::Resp => {
            o.b_ready = true;
            if i.b_valid {
                o.done = Some(i.b.resp);
                next_state = BurstState::Idle;
            }
        }
    }

    let mut d = D::<A, DW, STRB, ID>::dont_care();
    d.state = next_state;
    d.extras = next;

    if cr.reset.any() {
        d.state = BurstState::Idle;
        d.extras = Extras::<A, ID>::default();
        o.aw_valid = false;
        o.w_valid = false;
        o.b_ready = false;
        o.busy = false;
        o.data_taken = false;
        o.done = None;
        o.rejected = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    type Uut = Axi4WriteBurst<32, 32, 4, 4>;
    const BUS: u128 = 4;

    fn cmd(len: u128, size: u128, burst: BurstKind) -> AxiAddress<32, 4> {
        AxiAddress::<32, 4> {
            id: bits(1),
            addr: bits(0x1000),
            len: bits(len),
            size: bits(size),
            burst,
            ..Default::default()
        }
    }

    fn q(state: BurstState, aw: AxiAddress<32, 4>, beat: u128) -> Q<32, 32, 4, 4> {
        Q::<32, 32, 4, 4> {
            state,
            extras: Extras::<32, 4> {
                aw,
                beat: bits(beat),
            },
        }
    }

    /// A clock edge with reset low — the ordinary running case.
    fn run_cr() -> ClockReset {
        clock_reset(clock(true), reset(false))
    }

    /// Tier 1: reset returns to Idle with nothing asserted.
    #[test]
    fn reset_idles_and_asserts_nothing() {
        let cr = clock_reset(clock(true), reset(true));
        let (o, d) = write_burst::<32, 32, 4, 4>(
            cr,
            In::<32, 32, 4>::default(),
            q(BurstState::Resp, cmd(3, 2, BurstKind::Incr), 2),
        );
        assert!(!o.aw_valid && !o.w_valid && !o.b_ready && !o.busy);
        assert_eq!(o.done, None);
        assert_eq!(d.state, BurstState::Idle);
        assert_eq!(d.extras, Extras::<32, 4>::default());
    }

    /// Tier 1: a drivable command is latched and moves to the address
    /// phase.
    #[test]
    fn a_drivable_command_is_latched() {
        let i = In::<32, 32, 4> {
            cmd: Some(cmd(3, 2, BurstKind::Incr)),
            ..Default::default()
        };
        let (o, d) =
            write_burst::<32, 32, 4, 4>(run_cr(), i, q(BurstState::Idle, AxiAddress::default(), 0));
        assert!(!o.rejected);
        assert!(!o.busy, "busy is low on the cycle the command is accepted");
        assert_eq!(d.state, BurstState::Addr);
        assert_eq!(d.extras.aw.len.raw(), 3);
        assert_eq!(d.extras.beat.raw(), 0);
    }

    /// Tier 1: WRAP and an oversized `AxSIZE` are refused, and refusing
    /// does not move the sequencer.
    #[test]
    fn undrivable_commands_are_refused_in_place() {
        for bad in [cmd(3, 2, BurstKind::Wrap), cmd(3, 7, BurstKind::Incr)] {
            let i = In::<32, 32, 4> {
                cmd: Some(bad),
                ..Default::default()
            };
            let (o, d) = write_burst::<32, 32, 4, 4>(
                run_cr(),
                i,
                q(BurstState::Idle, AxiAddress::default(), 0),
            );
            assert!(o.rejected, "{bad:?} should be refused");
            assert!(!o.aw_valid, "a refused command must not drive AWVALID");
            assert_eq!(d.state, BurstState::Idle);
        }
    }

    /// Tier 1: `WVALID` is not asserted speculatively.
    ///
    /// Once asserted it may not be withdrawn, so the data phase must
    /// wait for a word rather than offer and retract — the exact rule
    /// `Fault::WValidDropped` exists to catch.
    #[test]
    fn wvalid_waits_for_data_rather_than_offering_early() {
        let c = cmd(3, 2, BurstKind::Incr);
        let dry = In::<32, 32, 4> {
            w_ready: true,
            data: None,
            ..Default::default()
        };
        let (o, d) = write_burst::<32, 32, 4, 4>(run_cr(), dry, q(BurstState::Data, c, 0));
        assert!(!o.w_valid, "no word, no WVALID");
        assert!(!o.data_taken);
        assert_eq!(d.state, BurstState::Data, "and no progress");
        assert_eq!(d.extras.beat.raw(), 0);
    }

    /// Tier 1: `WLAST` lands on the beat whose index is `AxLEN`, and the
    /// burst then waits for the response.
    #[test]
    fn wlast_lands_on_the_final_beat() {
        let c = cmd(3, 2, BurstKind::Incr);
        let i = In::<32, 32, 4> {
            w_ready: true,
            data: Some(bits(0xAB)),
            ..Default::default()
        };
        for beat in 0..3u128 {
            let (o, d) = write_burst::<32, 32, 4, 4>(run_cr(), i, q(BurstState::Data, c, beat));
            assert!(!o.w.last, "beat {beat} is not last");
            assert_eq!(d.extras.beat.raw(), beat + 1);
            assert_eq!(d.state, BurstState::Data);
        }
        let (o, d) = write_burst::<32, 32, 4, 4>(run_cr(), i, q(BurstState::Data, c, 3));
        assert!(o.w.last, "beat 3 of a len=3 burst is last");
        assert_eq!(d.state, BurstState::Resp);
    }

    /// Tier 1: the response is reported for exactly one cycle.
    #[test]
    fn the_response_is_reported_once() {
        let c = cmd(0, 2, BurstKind::Incr);
        let i = In::<32, 32, 4> {
            b_valid: true,
            b: AxiWriteResponse {
                id: bits(1),
                resp: AxiResponse::SlvErr,
            },
            ..Default::default()
        };
        let (o, d) = write_burst::<32, 32, 4, 4>(run_cr(), i, q(BurstState::Resp, c, 0));
        assert!(o.b_ready);
        assert_eq!(o.done, Some(AxiResponse::SlvErr));
        assert_eq!(d.state, BurstState::Idle);
    }

    /// Tier 1: a narrow transfer's lanes rotate with the address.
    ///
    /// This is the case `axi4-architecture.md` §4 singles out as the one
    /// a loopback between two RHDL widgets cannot catch if both share an
    /// off-by-one, which is why the same traffic also goes past the
    /// hand-written Verilog monitor.
    #[test]
    fn narrow_transfer_lanes_rotate_with_the_address() {
        let bb = bits::<9>(BUS);
        // One byte per beat: lane follows the low address bits.
        for (addr, want) in [
            (0x100u128, 0b0001u128),
            (0x101, 0b0010),
            (0x102, 0b0100),
            (0x103, 0b1000),
        ] {
            let s = lane_strobe::<32, 4>(bits(addr), bits(0), bb);
            assert_eq!(s.raw(), want, "1-byte beat at {addr:#x}");
        }
        // Two bytes per beat: the low or the high half.
        assert_eq!(lane_strobe::<32, 4>(bits(0x100), bits(1), bb).raw(), 0b0011);
        assert_eq!(lane_strobe::<32, 4>(bits(0x102), bits(1), bb).raw(), 0b1100);
        // Full width: every lane.
        assert_eq!(lane_strobe::<32, 4>(bits(0x100), bits(2), bb).raw(), 0b1111);
    }

    /// Tier 2: a burst completes and consumes exactly one word per beat.
    #[test]
    fn a_burst_consumes_one_word_per_beat() -> miette::Result<()> {
        let c = cmd(3, 2, BurstKind::Incr);
        let stimulus: Vec<In<32, 32, 4>> = (0..12)
            .map(|k| In::<32, 32, 4> {
                cmd: if k == 0 { Some(c) } else { None },
                data: Some(bits(0x10 + k as u128)),
                aw_ready: true,
                w_ready: true,
                b_valid: true,
                b: AxiWriteResponse {
                    id: bits(1),
                    resp: AxiResponse::Okay,
                },
            })
            .collect();
        let uut = Uut::default();
        let input = stimulus.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output)
            .collect::<Vec<_>>();
        assert_eq!(out.iter().filter(|o| o.data_taken).count(), 4);
        assert_eq!(out.iter().filter(|o| o.done.is_some()).count(), 1);
        Ok(())
    }

    /// Tier 3: the emitted Verilog's interface is the contract.
    ///
    /// The headers carry the packed-vector widths of `In` and `Out`,
    /// which is the ABI; the body is pinned by line count for the
    /// reason given in `check::monitor`.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Uut::default();
        let hdl = uut.descriptor("top".into())?.hdl()?.modules.pretty();
        let headers = hdl
            .lines()
            .filter(|l| l.starts_with("module "))
            .collect::<Vec<_>>()
            .join("\n");
        let expect = expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [107:0] i, output wire [110:0] o);
            module top_state(input wire [1:0] clock_reset, input wire [1:0] i, output reg [1:0] o);
            module top_extras(input wire [1:0] clock_reset, input wire [72:0] i, output reg [72:0] o);"#]];
        expect.assert_eq(&headers);
        expect!["539"].assert_eq(&hdl.lines().count().to_string());
        Ok(())
    }

    /// Tier 4: the ground truth — emitted Verilog against the Rust
    /// simulation, through both the RTL and the NTL path.
    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let c = cmd(3, 2, BurstKind::Incr);
        let stimulus: Vec<In<32, 32, 4>> = (0..12)
            .map(|k| In::<32, 32, 4> {
                cmd: if k == 0 { Some(c) } else { None },
                data: Some(bits(0x10 + k as u128)),
                aw_ready: true,
                w_ready: true,
                b_valid: k >= 6,
                b: AxiWriteResponse {
                    id: bits(1),
                    resp: AxiResponse::Okay,
                },
            })
            .collect();
        let uut = Uut::default();
        let input = stimulus.into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        let tm = tb.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = tb.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    /// Tier 5: a digest of the waveform.
    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let c = cmd(3, 2, BurstKind::Incr);
        let stimulus: Vec<In<32, 32, 4>> = (0..12)
            .map(|k| In::<32, 32, 4> {
                cmd: if k == 0 { Some(c) } else { None },
                data: Some(bits(0x10 + k as u128)),
                aw_ready: true,
                w_ready: true,
                b_valid: k >= 6,
                b: AxiWriteResponse {
                    id: bits(1),
                    resp: AxiResponse::Okay,
                },
            })
            .collect();
        let uut = Uut::default();
        let input = stimulus.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("axi4_write_burst");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["0f148fdff7898009084a53cf1b47a67d7d9b32c10264a8d1ea2c8d605c6894ec"];
        let digest = vcd.dump_to_file(root.join("burst.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    /// The FSM has exactly the four edges the sequencer can take.
    ///
    /// Pinned as a set rather than a count, because the extractor
    /// over-approximates when a transition's condition does not mention
    /// the state: RoCE's TX FSM once reported all 20 edges on 5 nodes
    /// from an offset-derived `if`-chain, including impossible ones.
    /// A diagram is the load-bearing artifact for FSM-aware review
    /// (CLAUDE.md §12.14), and a diagram showing edges the hardware
    /// cannot take is worse than none.
    #[test]
    fn the_fsm_has_exactly_the_expected_inter_state_edges() {
        use std::collections::BTreeSet;
        let transitions =
            rhdl::core::fsm::extract_widget_transitions_strict::<Uut>().expect("extracting");
        // Declaration order: Idle = 0, Addr = 1, Data = 2, Resp = 3.
        const IDLE: usize = 0;
        const ADDR: usize = 1;
        const DATA: usize = 2;
        const RESP: usize = 3;
        let edges: BTreeSet<(usize, usize)> = transitions
            .iter()
            .map(|t| (t.source_index, t.target_index))
            .filter(|(a, b)| a != b)
            .collect();
        let expected: BTreeSet<(usize, usize)> =
            [(IDLE, ADDR), (ADDR, DATA), (DATA, RESP), (RESP, IDLE)]
                .into_iter()
                .collect();
        assert_eq!(edges, expected, "FSM edge set drifted");
    }

    /// The FSM metadata the diagram is generated from.
    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = Uut::fsm_descriptor();
        assert_eq!(desc.widget_name, "Axi4WriteBurst");
        assert_eq!(desc.variants().len(), 4);
        assert_eq!(desc.initial_index(), 0);
    }
}
