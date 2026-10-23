//! In-flight transaction IDs: a direct-mapped table that stalls rather
//! than aliases.
//!
//! `AxID` is what makes AXI4 an interconnect protocol rather than a
//! point link, and this is the structure `axi4-architecture.md` §5 says
//! tracks it.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
      +-+Axi4IdTable+------------+
      |                          |
 ?bID |                    admit |  Admit
+---->| alloc                    +------->
      |                allocated |  bool
 ?bID |                          +------->
+---->| retire       outstanding |  b4
      |                          +------->
      +--------------------------+
")]
//!
//! # Why a collision stalls instead of aliasing
//!
//! The table is direct-mapped on the **low bits** of the ID, so two IDs
//! differing only above that are candidates for the same slot. §5
//! records the decision: a collision **stalls the address phase**. The
//! alternative is accepting a transaction whose response may then be
//! routed to the wrong port, which in an interconnect is a
//! data-corruption bug. Stalling costs throughput and shows up in a
//! waveform; aliasing is silent.
//!
//! This is the same *structure* as `roce::qp::table` — direct-mapped,
//! keyed on low bits — and deliberately the **opposite policy**. That
//! table aliases, and documents it, because a queue-pair context is
//! configuration the host controls: if it collides, the host chose two
//! numbers it should not have. An AXI4 ID arrives from whatever master
//! is attached, so the table cannot assume anyone arranged for it to
//! fit.
//!
//! # Same ID twice is not a collision
//!
//! AXI4 permits several outstanding transactions with the *same* ID;
//! they must simply complete in order. So a slot holds an ID and a
//! **count**, and a second transaction with that ID increments it. Only
//! a *different* ID landing on an occupied slot is a collision. Reading
//! §5's "a collision stalls" as "one transaction per slot" would refuse
//! perfectly legal traffic.
//!
//! # The three outcomes are distinguishable, on purpose
//!
//! §8's Phase 4 gate asks for "a full ID table stalling rather than
//! aliasing — asserted as a *distinguishable* outcome, not merely as
//! 'it still works'". A single `ready` bit could not tell a collision
//! from a saturated counter, and a test that only checked "it still
//! works" would pass whether the table stalled or silently aliased. So
//! [`Admit`] names which of the three happened.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/axi4_id_table.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/axi4_id_table.md")]

use crate::core::dff;
use rhdl::prelude::*;

/// What an allocation request would do.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum Admit {
    /// The slot is free, or already holds this ID with room to spare.
    #[default]
    Ready,
    /// The slot holds a **different** ID. Stalling here is what stops
    /// a response being routed to the wrong port.
    Collision,
    /// The slot holds this ID, but its counter is at maximum.
    Saturated,
}

/// One slot: the ID occupying it and how many of its transactions are
/// outstanding.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Entry<const ID: usize>
where
    rhdl::bits::W<ID>: BitWidth,
{
    /// The ID that owns this slot while `count > 0`.
    pub id: Bits<ID>,
    /// Outstanding transactions for that ID. Zero means free.
    pub count: Bits<4>,
}

/// Inputs to the ID table.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In<const ID: usize>
where
    rhdl::bits::W<ID>: BitWidth,
{
    /// An ID to claim a slot for. Accepted only when `admit` is
    /// [`Admit::Ready`].
    pub alloc: Option<Bits<ID>>,
    /// An ID whose transaction has completed.
    pub retire: Option<Bits<ID>>,
}

/// Outputs from the ID table.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct Out {
    /// What `alloc` would do this cycle — and *why*, not just whether.
    pub admit: Admit,
    /// `alloc` was accepted this cycle.
    pub allocated: bool,
    /// Outstanding count for the slot `alloc` addresses, after this
    /// cycle's updates.
    pub outstanding: Bits<4>,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// A direct-mapped table of in-flight transaction IDs.
///
/// `ID` is the transaction-ID width, `IDXW` the number of low ID bits
/// used as the index, and `DEPTH` must be `1 << IDXW` — see
/// [`Self::check_widths`]. `DEPTH` is a separate parameter because
/// `[Entry; 1 << IDXW]` needs `generic_const_exprs`, the same
/// constraint that makes `AxiWriteBeat` carry `STRB`.
pub struct Axi4IdTable<const ID: usize, const IDXW: usize, const DEPTH: usize>
where
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<IDXW>: BitWidth,
{
    entries: dff::DFF<[Entry<ID>; DEPTH]>,
}

impl<const ID: usize, const IDXW: usize, const DEPTH: usize> Default
    for Axi4IdTable<ID, IDXW, DEPTH>
where
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<IDXW>: BitWidth,
{
    fn default() -> Self {
        // Written out because `std` has no blanket `[T; N]: Default`
        // for a generic `N`, only fixed sizes. Every slot starts with
        // `count == 0`, which is what "free" means here -- there is no
        // separate validity bit, for the same reason `roce::qp::table`
        // has none.
        Self {
            entries: dff::DFF::new([Entry::<ID>::default(); DEPTH]),
        }
    }
}

impl<const ID: usize, const IDXW: usize, const DEPTH: usize> Axi4IdTable<ID, IDXW, DEPTH>
where
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<IDXW>: BitWidth,
{
    /// The invariant the type system cannot state: `DEPTH == 1 << IDXW`.
    ///
    /// Force it with
    /// `const _: () = Axi4IdTable::<4, 2, 4>::check_widths();`.
    pub const fn check_widths() {
        assert!(1 << IDXW == DEPTH, "DEPTH must be 1 << IDXW");
        assert!(IDXW <= ID, "the index cannot be wider than the ID");
    }
}

impl<const ID: usize, const IDXW: usize, const DEPTH: usize> SynchronousIO
    for Axi4IdTable<ID, IDXW, DEPTH>
where
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<IDXW>: BitWidth,
{
    type I = In<ID>;
    type O = Out;
    type Kernel = id_table<ID, IDXW, DEPTH>;
}

#[kernel]
/// The slot an ID maps to: its low `IDXW` bits.
///
/// Truncation is the whole mechanism, and the reason a collision is
/// possible at all.
pub fn slot_of<const ID: usize, const IDXW: usize>(id: Bits<ID>) -> Bits<IDXW>
where
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<IDXW>: BitWidth,
{
    id.resize::<IDXW>()
}

#[kernel]
/// Kernel for [Axi4IdTable].
pub fn id_table<const ID: usize, const IDXW: usize, const DEPTH: usize>(
    cr: ClockReset,
    i: In<ID>,
    q: Q<ID, IDXW, DEPTH>,
) -> (Out, D<ID, IDXW, DEPTH>)
where
    rhdl::bits::W<ID>: BitWidth,
    rhdl::bits::W<IDXW>: BitWidth,
{
    let mut next = q.entries;

    let (want, a_id) = match i.alloc {
        Some(id) => (true, id),
        None => (false, bits::<ID>(0)),
    };
    let a_slot = slot_of::<ID, IDXW>(a_id);
    let cur = q.entries[a_slot];

    // Free, mine-with-room, mine-but-full, or someone else's.
    let free = cur.count == bits::<4>(0);
    let mine = cur.id == a_id;
    let admit = if free {
        Admit::Ready
    } else if !mine {
        Admit::Collision
    } else if cur.count == bits::<4>(15) {
        Admit::Saturated
    } else {
        Admit::Ready
    };
    let accept = want
        && match admit {
            Admit::Ready => true,
            _ => false,
        };

    if accept {
        let mut e = cur;
        e.id = a_id;
        e.count = cur.count + bits::<4>(1);
        next[a_slot] = e;
    }

    // A retirement is applied after the allocation, so an alloc and a
    // retire of the same slot in one cycle net out correctly rather
    // than one overwriting the other.
    if let Some(r_id) = i.retire {
        let r_slot = slot_of::<ID, IDXW>(r_id);
        let r_cur = next[r_slot];
        // Only the owner may retire, and only a non-empty slot. A
        // retirement for an ID that does not own the slot is a protocol
        // error upstream; dropping it here is better than corrupting a
        // count that belongs to someone else.
        if r_cur.id == r_id && r_cur.count != bits::<4>(0) {
            let mut e = r_cur;
            e.count = r_cur.count - bits::<4>(1);
            next[r_slot] = e;
        }
    }

    let mut o = Out::dont_care();
    o.admit = admit;
    o.allocated = accept;
    o.outstanding = next[a_slot].count;

    let mut d = D::<ID, IDXW, DEPTH>::dont_care();
    d.entries = next;

    if cr.reset.any() {
        d.entries = [Entry::<ID>::default(); DEPTH];
        o.admit = Admit::Ready;
        o.allocated = false;
        o.outstanding = bits::<4>(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    // Four-bit IDs into four slots: 0x1 and 0x5 share slot 1, which is
    // what makes a collision reachable at all.
    type Uut = Axi4IdTable<4, 2, 4>;
    const _: () = Uut::check_widths();

    fn run_cr() -> ClockReset {
        clock_reset(clock(true), reset(false))
    }

    fn q4(entries: [Entry<4>; 4]) -> Q<4, 2, 4> {
        Q::<4, 2, 4> { entries }
    }

    fn e(id: u128, count: u128) -> Entry<4> {
        Entry::<4> {
            id: bits(id),
            count: bits(count),
        }
    }

    /// Tier 1: the index is the low bits, and that is why collisions
    /// exist.
    #[test]
    fn the_slot_is_the_low_bits_of_the_id() {
        assert_eq!(slot_of::<4, 2>(bits(0b0001)).raw(), 1);
        assert_eq!(slot_of::<4, 2>(bits(0b0101)).raw(), 1, "0x5 shares slot 1");
        assert_eq!(slot_of::<4, 2>(bits(0b1001)).raw(), 1, "and so does 0x9");
        assert_eq!(slot_of::<4, 2>(bits(0b0010)).raw(), 2);
    }

    /// Tier 1: a free slot admits, and the count rises.
    #[test]
    fn a_free_slot_admits() {
        let i = In::<4> {
            alloc: Some(bits(1)),
            retire: None,
        };
        let (o, d) = id_table::<4, 2, 4>(run_cr(), i, q4([e(0, 0); 4]));
        assert_eq!(o.admit, Admit::Ready);
        assert!(o.allocated);
        assert_eq!(o.outstanding.raw(), 1);
        assert_eq!(d.entries[1], e(1, 1));
    }

    /// Tier 1: the **same** ID again is not a collision.
    ///
    /// AXI4 permits several outstanding transactions with one ID; they
    /// must merely complete in order. Reading §5's "a collision stalls"
    /// as "one transaction per slot" would refuse legal traffic.
    #[test]
    fn the_same_id_twice_is_admitted() {
        let i = In::<4> {
            alloc: Some(bits(1)),
            retire: None,
        };
        let mut st = [e(0, 0); 4];
        st[1] = e(1, 2);
        let (o, d) = id_table::<4, 2, 4>(run_cr(), i, q4(st));
        assert_eq!(o.admit, Admit::Ready);
        assert_eq!(d.entries[1], e(1, 3));
    }

    /// **The Phase 4 gate, half one.** A different ID on an occupied
    /// slot stalls, and says *why*.
    ///
    /// §8 asks for the stall to be "asserted as a distinguishable
    /// outcome, not merely as 'it still works'". A bare `ready` bit
    /// could not tell this apart from a saturated counter, and a test
    /// that only checked "it still works" would pass whether the table
    /// stalled or silently aliased ID 5 onto ID 1's slot — which in an
    /// interconnect routes a response to the wrong port.
    #[test]
    fn a_different_id_collides_and_is_named_as_a_collision() {
        let i = In::<4> {
            alloc: Some(bits(5)), // 0x5 -> slot 1, held by 0x1
            retire: None,
        };
        let mut st = [e(0, 0); 4];
        st[1] = e(1, 1);
        let (o, d) = id_table::<4, 2, 4>(run_cr(), i, q4(st));
        assert_eq!(
            o.admit,
            Admit::Collision,
            "distinguishable from Saturated, which is the point"
        );
        assert!(!o.allocated, "and it must not be admitted");
        assert_eq!(d.entries[1], e(1, 1), "nor may it disturb the owner");
    }

    /// **The Phase 4 gate, half two.** A saturated slot stalls, and is
    /// distinguishable from a collision.
    #[test]
    fn a_saturated_slot_is_named_as_saturated() {
        let i = In::<4> {
            alloc: Some(bits(1)),
            retire: None,
        };
        let mut st = [e(0, 0); 4];
        st[1] = e(1, 15);
        let (o, d) = id_table::<4, 2, 4>(run_cr(), i, q4(st));
        assert_eq!(o.admit, Admit::Saturated);
        assert!(!o.allocated);
        assert_eq!(d.entries[1].count.raw(), 15, "and must not wrap to zero");
    }

    /// Tier 1: retiring frees the slot for a different ID.
    #[test]
    fn retiring_frees_the_slot() {
        let mut st = [e(0, 0); 4];
        st[1] = e(1, 1);
        let i = In::<4> {
            alloc: None,
            retire: Some(bits(1)),
        };
        let (_o, d) = id_table::<4, 2, 4>(run_cr(), i, q4(st));
        assert_eq!(d.entries[1].count.raw(), 0, "slot 1 is free again");
    }

    /// Tier 1: only the owner may retire.
    ///
    /// A retirement for an ID that does not hold the slot is a protocol
    /// error upstream. Dropping it is better than decrementing a count
    /// that belongs to someone else, which would free a slot still in
    /// use.
    #[test]
    fn a_foreign_retire_is_ignored() {
        let mut st = [e(0, 0); 4];
        st[1] = e(1, 2);
        let i = In::<4> {
            alloc: None,
            retire: Some(bits(5)), // same slot, different ID
        };
        let (_o, d) = id_table::<4, 2, 4>(run_cr(), i, q4(st));
        assert_eq!(d.entries[1], e(1, 2), "untouched");
    }

    /// Tier 1: an allocate and a retire of the same slot net out.
    #[test]
    fn alloc_and_retire_in_one_cycle_net_out() {
        let mut st = [e(0, 0); 4];
        st[1] = e(1, 2);
        let i = In::<4> {
            alloc: Some(bits(1)),
            retire: Some(bits(1)),
        };
        let (o, d) = id_table::<4, 2, 4>(run_cr(), i, q4(st));
        assert!(o.allocated);
        assert_eq!(d.entries[1].count.raw(), 2, "+1 then -1");
    }

    /// Tier 1: reset clears every slot.
    #[test]
    fn reset_clears_every_slot() {
        let cr = clock_reset(clock(true), reset(true));
        let (o, d) = id_table::<4, 2, 4>(cr, In::default(), q4([e(3, 7); 4]));
        assert_eq!(o.admit, Admit::Ready);
        assert_eq!(d.entries, [Entry::<4>::default(); 4]);
    }

    /// Tier 2: the collision persists until the owner fully retires.
    #[test]
    fn a_collision_clears_only_when_the_owner_is_gone() {
        let alloc = |id: u128| In::<4> {
            alloc: Some(bits(id)),
            retire: None,
        };
        let retire = |id: u128| In::<4> {
            alloc: None,
            retire: Some(bits(id)),
        };
        let stim = vec![
            alloc(1),
            alloc(1),
            alloc(5),
            retire(1),
            alloc(5),
            retire(1),
            alloc(5),
            In::default(),
        ];
        let uut = Uut::default();
        let input = stim.into_iter().with_reset(1).clock_pos_edge(100);
        let out = uut
            .run(input)
            .synchronous_sample()
            .map(|s| s.output)
            .collect::<Vec<_>>();
        let admits: Vec<Admit> = out.iter().map(|o| o.admit).collect();
        // Two accepted allocs for ID 1, two collisions for ID 5 while
        // ID 1 still holds the slot, then ID 5 admitted.
        assert_eq!(
            out.iter().filter(|o| o.allocated).count(),
            3,
            "two for ID 1, one for ID 5 once the slot frees: {admits:?}"
        );
        assert!(
            admits.iter().filter(|a| **a == Admit::Collision).count() >= 2,
            "the collision must persist while the owner is outstanding: {admits:?}"
        );
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
            module top(input wire [1:0] clock_reset, input wire [9:0] i, output wire [6:0] o);
            module top_entries(input wire [1:0] clock_reset, input wire [31:0] i, output reg [31:0] o);"#]].assert_eq(&headers);
        expect!["305"].assert_eq(&hdl.lines().count().to_string());
        Ok(())
    }

    fn smoke() -> Vec<In<4>> {
        vec![
            In {
                alloc: Some(bits(1)),
                retire: None,
            },
            In {
                alloc: Some(bits(5)),
                retire: None,
            },
            In {
                alloc: None,
                retire: Some(bits(1)),
            },
            In {
                alloc: Some(bits(5)),
                retire: None,
            },
            In::default(),
        ]
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
            .join("axi4_id_table");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["02465f602b8cf16dd2919d7f28dfa16141a228456a10ed346f1a239e1aa277ed"];
        let digest = vcd.dump_to_file(root.join("table.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
