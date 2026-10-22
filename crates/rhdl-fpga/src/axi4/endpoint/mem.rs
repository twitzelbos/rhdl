//! A memory-backed AXI4 subordinate: writes land in RAM, reads come back.
//!
//! The other half of Phase 2's loopback. Serves one write burst and one
//! read burst, single-ID, `Incr` and `Fixed`.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+Axi4MemEndpoint+--------+
      |                          |
 bool |                 aw_ready |
+---->| aw_valid         w_ready +----->
+---->| aw                b_valid|
+---->| w_valid                 b+----->
+---->| w                        |
+---->| b_ready         ar_ready |
+---->| ar_valid         r_valid +----->
+---->| ar                      r|
+---->| r_ready                  +----->
      +--------------------------+
")]
//!
//! # Strobed writes are read-modify-write, and that is not a detail
//!
//! `WSTRB` makes a write byte-granular, but a RAM of `DW`-bit words has
//! no byte enables: writing a word would clobber the lanes the manager
//! did *not* select. A narrow or partially-strobed burst would then
//! corrupt neighbouring bytes, and Phase 2's gate is precisely a
//! byte-exact comparison, so this cannot be papered over.
//!
//! So each strobed beat merges: the old word is read, the selected
//! lanes are replaced, the result is written back. The cost is normally
//! a cycle per beat — except that the burst's addresses are known as
//! soon as AW is accepted, so the read port runs **one beat ahead** of
//! the data phase and the merge needs no extra cycle. One beat per
//! cycle, byte-exact.
//!
//! An FPGA would instead use `DW/8` byte-wide RAMs with per-lane write
//! enables, which is what a vendor block RAM's byte-enable pins are.
//! That is the better hardware and it needs an array of sub-circuits;
//! the read-ahead merge is what is expressible today without one, and
//! it is byte-exact, which is the property the gate tests.
//!
//! # Why the read path yields to the write path
//!
//! [`crate::core::ram::synchronous::SyncBRAM`] has one read port, and
//! the read-modify-write merge needs it. So while a write burst is in
//! flight the read path stalls — `ARREADY` stays low. With one burst of
//! each kind outstanding, which is all Phase 2 allows, this costs
//! nothing and is simpler than arbitrating. Concurrent read and write
//! bursts want the byte-wide-RAM shape above, which frees the read port
//! entirely.
//!
//! # Why this widget does not take `#[derive(FsmWidget)]`
//!
//! It has **two** concurrent sequencers — one per direction — and the
//! derive takes a single `state_field`. Tagging one would produce a
//! diagram that silently omitted the other half of the widget, which is
//! worse than no diagram: CLAUDE.md §12.14 wants the diagram because it
//! makes structural review possible at a glance, and a half-diagram
//! defeats that. The two sequencers are drawn below instead, and the
//! single-FSM managers in [`crate::axi4::manager`] do take the derive.
#![doc = badascii_doc::badascii!("
  write:  +-+Idle+-+ awvalid +-+Data+-+ wlast  +-+Resp+-+
          |        +-------->|        +------->|        +--+
          +---^----+         +--------+        +--------+  |
              +------------------ bready ------------------+

  read:   +-+Idle+-+ arvalid +-+Fetch+-+ ram  +-+Hold+-+
          |        +-------->|         +----->|        +--+
          +---^----+         +----^----+      +--------+  |
              |                   +-- !rlast --+          |
              +---------------- rlast ---------------------+
")]
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/axi4_mem_endpoint.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/axi4_mem_endpoint.md")]

use super::super::types::{
    AxiAddress, AxiReadBeat, AxiResponse, AxiWriteBeat, AxiWriteResponse, beat_address, burst_beats,
};
use crate::core::dff;
use crate::core::ram::synchronous::{self, Write};
use rhdl::prelude::*;

/// The write sequencer's state.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum WrState {
    /// Accepting an address.
    #[default]
    Idle,
    /// Taking beats, merging each into RAM.
    Data,
    /// Holding `BVALID` until the manager takes it.
    Resp,
}

/// The read sequencer's state.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum RdState {
    /// Accepting an address.
    #[default]
    Idle,
    /// The RAM read for this beat is in flight.
    Fetch,
    /// Holding `RVALID` until the manager takes it.
    Hold,
}

/// Everything the two sequencers latch, bundled.
///
/// One `Digital` struct in one `DFF` rather than a dozen siblings, per
/// CLAUDE.md §3.1.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Extras<const A: usize, const DW: usize, const ID: usize, const N: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    /// The write burst in flight.
    aw: AxiAddress<A, ID>,
    /// Beats of it already merged.
    w_beat: Bits<8>,
    /// The read burst in flight.
    ar: AxiAddress<A, ID>,
    /// Beats of it already returned.
    r_beat: Bits<8>,
    /// The word fetched for the beat being held.
    r_word: Bits<DW>,
    /// The word last written, and where — the merge's forwarding path.
    ///
    /// `SyncBRAM` samples its read on the clock's low phase, *before*
    /// applying that edge's write, so a read presented in one cycle
    /// returns the content as of **before** that cycle's write. The
    /// read-ahead therefore hands the merge a stale word whenever two
    /// consecutive beats land in the same word — which is every narrow
    /// burst — and each beat would clobber the lanes the one before it
    /// had just set. Forwarding the last written word closes that.
    last_addr: Bits<N>,
    last_word: Bits<DW>,
    last_valid: bool,
}

/// Inputs to the endpoint — both directions of all five channels.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In<const A: usize, const DW: usize, const STRB: usize, const ID: usize>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `AWVALID`.
    pub aw_valid: bool,
    /// The write address phase.
    pub aw: AxiAddress<A, ID>,
    /// `WVALID`.
    pub w_valid: bool,
    /// The write beat.
    pub w: AxiWriteBeat<DW, STRB>,
    /// `BREADY`.
    pub b_ready: bool,
    /// `ARVALID`.
    pub ar_valid: bool,
    /// The read address phase.
    pub ar: AxiAddress<A, ID>,
    /// `RREADY`.
    pub r_ready: bool,
}

/// Outputs from the endpoint.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct Out<const DW: usize, const ID: usize>
where
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
{
    /// `AWREADY`.
    pub aw_ready: bool,
    /// `WREADY`.
    pub w_ready: bool,
    /// `BVALID`.
    pub b_valid: bool,
    /// The write response.
    pub b: AxiWriteResponse<ID>,
    /// `ARREADY`. Low while a write burst holds the RAM's read port.
    pub ar_ready: bool,
    /// `RVALID`.
    pub r_valid: bool,
    /// The read beat.
    pub r: AxiReadBeat<DW, ID>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// A memory-backed AXI4 subordinate.
///
/// `A` is the address width, `DW` the data width, `STRB = DW / 8`,
/// `SHIFT = log2(STRB)` and `ID` the transaction-ID width; `N`
/// addresses `2^N` words of `DW` bits.
///
/// `STRB` and `SHIFT` are separate parameters rather than derived
/// because neither `Bits<{ DW / 8 }>` nor a `log2` is available: const
/// expressions in generic position need `generic_const_exprs`, and
/// division and bit-scanning are not kernel operators.
/// [`Self::check_widths`] is what ties the three together, so a
/// mismatched instantiation fails at compile time rather than
/// addressing the wrong word.
pub struct Axi4MemEndpoint<
    const A: usize,
    const DW: usize,
    const STRB: usize,
    const SHIFT: usize,
    const ID: usize,
    const N: usize,
> where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    wr: dff::DFF<WrState>,
    rd: dff::DFF<RdState>,
    extras: dff::DFF<Extras<A, DW, ID, N>>,
    mem: synchronous::SyncBRAM<Bits<DW>, N>,
}

impl<
    const A: usize,
    const DW: usize,
    const STRB: usize,
    const SHIFT: usize,
    const ID: usize,
    const N: usize,
> Axi4MemEndpoint<A, DW, STRB, SHIFT, ID, N>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    /// The invariants the type system cannot state: `STRB == DW / 8`
    /// and `SHIFT == log2(STRB)`.
    ///
    /// Force them with
    /// `const _: () = Axi4MemEndpoint::<32, 32, 4, 2, 4, 8>::check_widths();`.
    pub const fn check_widths() {
        assert!(STRB * 8 == DW, "STRB must be DW / 8");
        assert!(1 << SHIFT == STRB, "SHIFT must be log2(STRB)");
    }
}

impl<
    const A: usize,
    const DW: usize,
    const STRB: usize,
    const SHIFT: usize,
    const ID: usize,
    const N: usize,
> SynchronousIO for Axi4MemEndpoint<A, DW, STRB, SHIFT, ID, N>
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    type I = In<A, DW, STRB, ID>;
    type O = Out<DW, ID>;
    type Kernel = mem_endpoint<A, DW, STRB, SHIFT, ID, N>;
}

#[kernel]
/// Expand a byte strobe into a bit mask over the data word.
///
/// One lane per `WSTRB` bit, eight bits each. Written as a bounded loop
/// over `STRB` because that is what the kernel accepts, and it unrolls.
pub fn strobe_mask<const DW: usize, const STRB: usize>(strb: Bits<STRB>) -> Bits<DW>
where
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
{
    let mut mask = bits::<DW>(0);
    for lane in 0..STRB {
        let sel = (strb >> (lane as u128)) & bits::<STRB>(1);
        let byte = if sel == bits::<STRB>(1) {
            bits::<DW>(0xFF)
        } else {
            bits::<DW>(0)
        };
        mask |= byte << ((8 * lane) as u128);
    }
    mask
}

#[kernel]
/// Kernel for [Axi4MemEndpoint].
pub fn mem_endpoint<
    const A: usize,
    const DW: usize,
    const STRB: usize,
    const SHIFT: usize,
    const ID: usize,
    const N: usize,
>(
    cr: ClockReset,
    i: In<A, DW, STRB, ID>,
    q: Q<A, DW, STRB, SHIFT, ID, N>,
) -> (Out<DW, ID>, D<A, DW, STRB, SHIFT, ID, N>)
where
    rhdl::bits::W<A>: BitWidth,
    rhdl::bits::W<DW>: BitWidth,
    rhdl::bits::W<STRB>: BitWidth,
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<N>: BitWidth,
{
    let s = q.extras;
    let mut next = s;
    let mut wr = q.wr;
    let mut rd = q.rd;

    let mut o = Out::<DW, ID>::dont_care();
    o.aw_ready = false;
    o.w_ready = false;
    o.b_valid = false;
    o.b = AxiWriteResponse::<ID>::dont_care();
    o.b.id = s.aw.id;
    o.b.resp = AxiResponse::Okay;
    o.ar_ready = false;
    o.r_valid = false;
    o.r = AxiReadBeat::<DW, ID>::dont_care();
    o.r.id = s.ar.id;
    o.r.data = s.r_word;
    o.r.resp = AxiResponse::Okay;
    o.r.last = false;

    // Word index of a byte address on this bus.
    let w_addr = beat_address::<A>(s.aw.addr, s.aw.size, s.aw.len, s.aw.burst, s.w_beat);
    let w_word = (w_addr >> (SHIFT as u128)).resize::<N>();

    // Both directions need the RAM's single read port -- the writer for
    // its read-modify-write merge, the reader for its data -- so only
    // one burst is admitted at a time. With one burst of each kind
    // outstanding, which is all Phase 2 allows, this costs nothing;
    // concurrency wants the byte-wide-RAM shape described above, which
    // frees the read port entirely.
    let wr_idle = match q.wr {
        WrState::Idle => true,
        _ => false,
    };
    let rd_idle = match q.rd {
        RdState::Idle => true,
        _ => false,
    };
    let both_idle = wr_idle && rd_idle;

    // ---- write path
    let mut write = Write::<Bits<DW>, N>::dont_care();
    write.addr = w_word;
    write.value = bits::<DW>(0);
    write.enable = false;

    match q.wr {
        WrState::Idle => {
            o.aw_ready = both_idle;
            if i.aw_valid && both_idle {
                next.aw = i.aw;
                next.w_beat = bits::<8>(0);
                // The forwarding path is per burst: a stale entry from
                // the previous burst could otherwise shadow a word this
                // one has not written.
                next.last_valid = false;
                wr = WrState::Data;
            }
        }
        WrState::Data => {
            o.w_ready = true;
            if i.w_valid {
                // Merge: keep the lanes WSTRB did not select. The base
                // is the forwarded word when this beat lands where the
                // last one did, because the RAM's read port cannot have
                // seen that write yet.
                let mask = strobe_mask::<DW, STRB>(i.w.strb);
                let base = if s.last_valid && s.last_addr == w_word {
                    s.last_word
                } else {
                    q.mem
                };
                let merged = (base & !mask) | (i.w.data & mask);
                write.addr = w_word;
                write.value = merged;
                write.enable = true;
                next.last_addr = w_word;
                next.last_word = merged;
                next.last_valid = true;
                if i.w.last {
                    wr = WrState::Resp;
                } else {
                    next.w_beat = s.w_beat + bits::<8>(1);
                }
            }
        }
        WrState::Resp => {
            o.b_valid = true;
            if i.b_ready {
                wr = WrState::Idle;
            }
        }
    }

    // ---- read path, which yields the RAM's read port to the writer
    match q.rd {
        RdState::Idle => {
            o.ar_ready = both_idle;
            if i.ar_valid && both_idle {
                next.ar = i.ar;
                next.r_beat = bits::<8>(0);
                rd = RdState::Fetch;
            }
        }
        RdState::Fetch => {
            // The address was presented last cycle; the word is here.
            next.r_word = q.mem;
            rd = RdState::Hold;
        }
        RdState::Hold => {
            o.r_valid = true;
            o.r.data = s.r_word;
            o.r.last = s.r_beat == burst_beats(s.ar.len).resize::<8>() - bits::<8>(1);
            if i.r_ready {
                if o.r.last {
                    rd = RdState::Idle;
                } else {
                    next.r_beat = s.r_beat + bits::<8>(1);
                    rd = RdState::Fetch;
                }
            }
        }
    }

    // The address presented to the RAM is the one the NEXT cycle needs,
    // so it is derived from the post-transition state -- `next`, not
    // `s`. Deriving it from the registered state made the acceptance
    // cycle present the *previous* descriptor's address, which shifted
    // every read beat by one and left the first word of every burst
    // reading as zero.
    let nw_addr = beat_address::<A>(
        next.aw.addr,
        next.aw.size,
        next.aw.len,
        next.aw.burst,
        next.w_beat,
    );
    let nr_addr = beat_address::<A>(
        next.ar.addr,
        next.ar.size,
        next.ar.len,
        next.ar.burst,
        next.r_beat,
    );
    // The writer holds the port whenever it will merge next cycle.
    let writer_holds = match wr {
        WrState::Data => true,
        _ => false,
    };

    let mut d = D::<A, DW, STRB, SHIFT, ID, N>::dont_care();
    d.wr = wr;
    d.rd = rd;
    d.extras = next;
    d.mem.write = write;
    d.mem.read_addr = if writer_holds {
        (nw_addr >> (SHIFT as u128)).resize::<N>()
    } else {
        (nr_addr >> (SHIFT as u128)).resize::<N>()
    };

    if cr.reset.any() {
        d.wr = WrState::Idle;
        d.rd = RdState::Idle;
        d.extras = Extras::<A, DW, ID, N>::default();
        o.aw_ready = false;
        o.w_ready = false;
        o.b_valid = false;
        o.ar_ready = false;
        o.r_valid = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::super::super::types::BurstKind;
    use super::*;
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    type Uut = Axi4MemEndpoint<32, 32, 4, 2, 4, 10>;
    const _: () = Uut::check_widths();

    fn run_cr() -> ClockReset {
        clock_reset(clock(true), reset(false))
    }

    fn aw(len: u128, size: u128) -> AxiAddress<32, 4> {
        AxiAddress::<32, 4> {
            id: bits(1),
            addr: bits(0x40),
            len: bits(len),
            size: bits(size),
            burst: BurstKind::Incr,
            ..Default::default()
        }
    }

    fn q(
        wr: WrState,
        rd: RdState,
        extras: Extras<32, 32, 4, 10>,
        mem: u128,
    ) -> Q<32, 32, 4, 2, 4, 10> {
        Q::<32, 32, 4, 2, 4, 10> {
            wr,
            rd,
            extras,
            mem: bits(mem),
        }
    }

    /// Tier 1: reset idles both sequencers and asserts nothing.
    #[test]
    fn reset_idles_both_directions() {
        let cr = clock_reset(clock(true), reset(true));
        let busy = Extras::<32, 32, 4, 10> {
            aw: aw(3, 2),
            w_beat: bits(2),
            ..Default::default()
        };
        let (o, d) = mem_endpoint::<32, 32, 4, 2, 4, 10>(
            cr,
            In::default(),
            q(WrState::Data, RdState::Hold, busy, 0),
        );
        assert!(!o.aw_ready && !o.w_ready && !o.b_valid && !o.ar_ready && !o.r_valid);
        assert_eq!(d.wr, WrState::Idle);
        assert_eq!(d.rd, RdState::Idle);
        assert_eq!(d.extras, Extras::<32, 32, 4, 10>::default());
    }

    /// Tier 1: `strobe_mask` expands a byte strobe to a bit mask.
    #[test]
    fn strobe_mask_expands_lane_by_lane() {
        assert_eq!(strobe_mask::<32, 4>(bits(0b0000)).raw(), 0x0000_0000);
        assert_eq!(strobe_mask::<32, 4>(bits(0b0001)).raw(), 0x0000_00FF);
        assert_eq!(strobe_mask::<32, 4>(bits(0b0100)).raw(), 0x00FF_0000);
        assert_eq!(strobe_mask::<32, 4>(bits(0b1010)).raw(), 0xFF00_FF00);
        assert_eq!(strobe_mask::<32, 4>(bits(0b1111)).raw(), 0xFFFF_FFFF);
    }

    /// Tier 1: the merge keeps the lanes `WSTRB` did not select.
    ///
    /// This is the property Phase 2's byte-exact gate rests on: a
    /// narrow beat must not clobber its neighbours.
    #[test]
    fn the_merge_preserves_unselected_lanes() {
        let i = In::<32, 32, 4, 4> {
            w_valid: true,
            w: AxiWriteBeat {
                data: bits(0xAABB_CCDD),
                strb: bits(0b0010),
                last: false,
            },
            ..Default::default()
        };
        let ex = Extras::<32, 32, 4, 10> {
            aw: aw(3, 2),
            ..Default::default()
        };
        let (_o, d) = mem_endpoint::<32, 32, 4, 2, 4, 10>(
            run_cr(),
            i,
            q(WrState::Data, RdState::Idle, ex, 0x1111_1111),
        );
        // Only lane 1 is replaced; the rest keep the old word.
        assert_eq!(d.mem.write.value.raw(), 0x1111_CC11);
        assert!(d.mem.write.enable);
    }

    /// Tier 1: the merge forwards the last written word when this beat
    /// lands where the last one did.
    ///
    /// `SyncBRAM` samples its read before applying that edge's write,
    /// so without this every narrow burst would clobber its own lanes.
    /// The byte-exact gate caught exactly that.
    #[test]
    fn the_merge_forwards_a_same_word_write() {
        let i = In::<32, 32, 4, 4> {
            w_valid: true,
            w: AxiWriteBeat {
                data: bits(0x0000_00EE),
                strb: bits(0b0001),
                last: false,
            },
            ..Default::default()
        };
        let ex = Extras::<32, 32, 4, 10> {
            aw: aw(3, 0),
            w_beat: bits(0),
            last_addr: bits(0x10), // 0x40 >> 2
            last_word: bits(0xDDDD_DD00),
            last_valid: true,
            ..Default::default()
        };
        let (_o, d) = mem_endpoint::<32, 32, 4, 2, 4, 10>(
            run_cr(),
            i,
            // The RAM would hand back a stale zero here; forwarding wins.
            q(WrState::Data, RdState::Idle, ex, 0x0000_0000),
        );
        assert_eq!(
            d.mem.write.value.raw(),
            0xDDDD_DDEE,
            "the forwarded word, not the RAM's stale copy"
        );
    }

    /// Tier 1: the two directions are serialized — both need the RAM's
    /// single read port.
    #[test]
    fn a_read_is_not_admitted_while_a_write_is_in_flight() {
        let ex = Extras::<32, 32, 4, 10> {
            aw: aw(3, 2),
            ..Default::default()
        };
        let i = In::<32, 32, 4, 4> {
            ar_valid: true,
            ar: aw(3, 2),
            ..Default::default()
        };
        let (o, d) = mem_endpoint::<32, 32, 4, 2, 4, 10>(
            run_cr(),
            i,
            q(WrState::Data, RdState::Idle, ex, 0),
        );
        assert!(!o.ar_ready, "ARREADY must stay low while writing");
        assert_eq!(d.rd, RdState::Idle, "and the read must not start");
    }

    /// Tier 2: a write burst followed by a read burst returns the bytes.
    ///
    /// The exhaustive version of this is
    /// `tests/axi4_phase2_loopback.rs`, which is Phase 2's gate; this
    /// is the in-widget smoke test.
    #[test]
    fn a_write_then_a_read_returns_the_same_words() {
        let desc = aw(3, 2);
        let words = [0x1111_1111u128, 0x2222_2222, 0x3333_3333, 0x4444_4444];
        let mut stim: Vec<In<32, 32, 4, 4>> = Vec::new();
        stim.push(In {
            aw_valid: true,
            aw: desc,
            ..Default::default()
        });
        for (k, w) in words.iter().enumerate() {
            stim.push(In {
                w_valid: true,
                w: AxiWriteBeat {
                    data: bits(*w),
                    strb: bits(0b1111),
                    last: k == 3,
                },
                ..Default::default()
            });
        }
        stim.push(In {
            b_ready: true,
            ..Default::default()
        });
        stim.push(In {
            ar_valid: true,
            ar: desc,
            ..Default::default()
        });
        for _ in 0..12 {
            stim.push(In {
                r_ready: true,
                ..Default::default()
            });
        }
        let uut = Uut::default();
        let input = stim.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output)
            .collect::<Vec<_>>();
        let got: Vec<u128> = out
            .iter()
            .filter(|o| o.r_valid)
            .map(|o| o.r.data.raw())
            .collect();
        // Beats are held until taken, so the same word can appear twice;
        // what matters is the sequence of distinct words in order.
        let mut seq: Vec<u128> = Vec::new();
        for w in got {
            if seq.last() != Some(&w) {
                seq.push(w);
            }
        }
        assert_eq!(seq, words.to_vec());
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
            module top(input wire [1:0] clock_reset, input wire [171:0] i, output wire [49:0] o);
            module top_wr(input wire [1:0] clock_reset, input wire [1:0] i, output reg [1:0] o);
            module top_rd(input wire [1:0] clock_reset, input wire [1:0] i, output reg [1:0] o);
            module top_extras(input wire [1:0] clock_reset, input wire [220:0] i, output reg [220:0] o);
            module top_mem(input wire [1:0] clock_reset, input wire [52:0] i, output reg [31:0] o);"#]].assert_eq(&headers);
        expect!["852"].assert_eq(&hdl.lines().count().to_string());
        Ok(())
    }

    fn smoke() -> Vec<In<32, 32, 4, 4>> {
        let desc = aw(3, 2);
        let mut stim: Vec<In<32, 32, 4, 4>> = vec![In {
            aw_valid: true,
            aw: desc,
            ..Default::default()
        }];
        for k in 0..4 {
            stim.push(In {
                w_valid: true,
                w: AxiWriteBeat {
                    data: bits(0xC0 + k),
                    strb: bits(0b1111),
                    last: k == 3,
                },
                ..Default::default()
            });
        }
        stim.push(In {
            b_ready: true,
            ..Default::default()
        });
        stim
    }

    /// Tier 4: the ground truth, through both the RTL and NTL paths.
    #[test]
    fn test_widget_hdl_works() -> miette::Result<()> {
        let uut = Uut::default();
        let input = smoke().into_iter().with_reset(1).clock_pos_edge(100);
        let tb = uut.run(input).collect::<SynchronousTestBench<_, _>>();
        tb.rtl(&uut, &Default::default())?.run_iverilog()?;
        tb.ntl(&uut, &Default::default())?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5: a digest of the waveform.
    #[test]
    fn test_widget_trace() -> miette::Result<()> {
        let uut = Uut::default();
        let input = smoke().into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(input).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("axi4_mem_endpoint");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["faa57f229af0c8eae3b4e6cee9bf9e86a6e5dbf360068aa250104e2fb600f507"];
        let digest = vcd.dump_to_file(root.join("endpoint.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
