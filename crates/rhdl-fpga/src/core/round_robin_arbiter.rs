//! Round-robin arbiter
//!
//! Arbitrates among `N` competing requesters, granting one per cycle.
//! After a grant, the priority pointer advances so that the requester
//! that just won has the *lowest* priority next cycle — the textbook
//! round-robin discipline.  This is the building block behind
//! multi-master AXI fabrics, switch crossbars, and DMA channel
//! schedulers.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-+RoundRobinArbiter+-+
     |                     |
B<N> |                     | Option<B<W>>
+--->| requests      grant +--->
     |                     |
     +---------------------+
")]
//!
//!# Internals
//!
//! Each cycle the arbiter scans the `N` request bits in a rotated
//! order whose first position is `last_granted + 1`.  The first set
//! bit encountered is the new grant.  A small register pair tracks
//! the last grant index and a "valid" flag (so the very first cycle
//! starts the priority pointer at zero, not at an undefined value).
//!
#![doc = badascii_doc::badascii!(r"
                  +-+last_granted+
                  |               |
                  v               |
            +-+rotate+--+         |   +-+priority+
requests +->|by start   |   +---->|   | encode   |
            |           +-->|     +-->| + de-rot +-->grant
            +-----------+   |         +----------+    |
                            |                         |
                            +-------------------------+
")]
//!
//!# Parameters
//!
//! - `N` — number of requesters (must equal `2^W` for the round-robin
//!   wrap arithmetic to be correct)
//! - `W` — bit width of the grant index, satisfying `N = 2^W`
//!
//! Common configurations: `<2, 1>`, `<4, 2>`, `<8, 3>`, `<16, 4>`.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/round_robin_arbiter.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/round_robin_arbiter.md")]
use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
/// Round-robin arbiter core.
///
/// `N` is the number of requesters; `W` is the bit width of the
/// grant index.  `N` must equal `2^W`.
pub struct RoundRobinArbiter<const N: usize, const W: usize>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    last_granted: dff::DFF<Bits<W>>,
    valid: dff::DFF<bool>,
}

impl<const N: usize, const W: usize> Default for RoundRobinArbiter<N, W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    fn default() -> Self {
        Self {
            last_granted: dff::DFF::new(bits(0)),
            valid: dff::DFF::new(false),
        }
    }
}

impl<const N: usize, const W: usize> SynchronousIO for RoundRobinArbiter<N, W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    type I = Bits<N>;
    type O = Option<Bits<W>>;
    type Kernel = round_robin_arbiter<N, W>;
}

#[kernel]
/// Kernel for [RoundRobinArbiter].
pub fn round_robin_arbiter<const N: usize, const W: usize>(
    cr: ClockReset,
    requests: Bits<N>,
    q: Q<N, W>,
) -> (Option<Bits<W>>, D<N, W>)
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    // Start scanning one past the previous winner.  On the very first
    // cycle (before any grant has been issued), start from index 0.
    let start: Bits<W> = if q.valid { q.last_granted + 1 } else { bits(0) };
    let mut winner_idx: Bits<W> = bits(0);
    let mut found = false;
    // Walk all N positions in rotated order; the first set request wins.
    // Bits<W> arithmetic wraps modulo 2^W = N, so `start + offset`
    // gives the de-rotated index directly.
    for i in 0..N {
        let offset: Bits<W> = bits(i as u128);
        let idx: Bits<W> = start + offset;
        let bit_at_idx = (requests >> idx) & bits(1);
        if bit_at_idx != bits(0) && !found {
            winner_idx = idx;
            found = true;
        }
    }
    let mut d = D::<N, W>::dont_care();
    d.last_granted = if found { winner_idx } else { q.last_granted };
    d.valid = found;
    let o = if found { Some(winner_idx) } else { None };
    if cr.reset.any() {
        d.last_granted = bits(0);
        d.valid = false;
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn q4(last: u128, valid: bool) -> Q<4, 2> {
        Q::<4, 2> {
            last_granted: bits(last),
            valid,
        }
    }

    // Tier 1 — direct kernel unit tests

    #[test]
    fn test_no_requests_no_grant() {
        let cr = ClockReset::dont_care();
        let (o, d) = round_robin_arbiter::<4, 2>(cr, bits(0), q4(0, false));
        assert_eq!(o, None);
        assert!(!d.valid);
    }

    #[test]
    fn test_single_request_is_granted() {
        let cr = ClockReset::dont_care();
        for i in 0u128..4 {
            let (o, d) = round_robin_arbiter::<4, 2>(cr, bits(1 << i), q4(0, false));
            assert_eq!(o, Some(bits(i)), "single request bit {i}");
            assert_eq!(d.last_granted, bits(i));
            assert!(d.valid);
        }
    }

    #[test]
    fn test_priority_starts_from_last_plus_one() {
        let cr = ClockReset::dont_care();
        // Last grant was 1; priority starts from 2.  All four requesters
        // are asking, so the winner should be requester 2.
        let (o, _d) = round_robin_arbiter::<4, 2>(cr, bits(0b1111), q4(1, true));
        assert_eq!(o, Some(bits(2)));

        // Last was 2; should wrap to 3.
        let (o, _d) = round_robin_arbiter::<4, 2>(cr, bits(0b1111), q4(2, true));
        assert_eq!(o, Some(bits(3)));

        // Last was 3; should wrap to 0.
        let (o, _d) = round_robin_arbiter::<4, 2>(cr, bits(0b1111), q4(3, true));
        assert_eq!(o, Some(bits(0)));
    }

    #[test]
    fn test_priority_skips_inactive_requests() {
        let cr = ClockReset::dont_care();
        // Last grant was 0; priority starts from 1.  Only requesters 0
        // and 2 are asking.  Should pick 2 (priority order is 1,2,3,0).
        let (o, _d) = round_robin_arbiter::<4, 2>(cr, bits(0b0101), q4(0, true));
        assert_eq!(o, Some(bits(2)));

        // Last was 2; priority starts at 3.  Requesters 0 and 2 ask.
        // Order is 3,0,1,2 — first asker is 0.
        let (o, _d) = round_robin_arbiter::<4, 2>(cr, bits(0b0101), q4(2, true));
        assert_eq!(o, Some(bits(0)));
    }

    #[test]
    fn test_reset_clears_state() {
        let cr = clock_reset(clock(true), reset(true));
        // Even with active grant computation, reset must zero everything.
        let (_o, d) = round_robin_arbiter::<4, 2>(cr, bits(0b1111), q4(2, true));
        assert_eq!(d.last_granted, bits(0));
        assert!(!d.valid);
    }

    // Tier 2 — iterator simulation

    /// All four requesters constantly asking should each receive a grant
    /// in strict 0,1,2,3,0,1,2,3,... rotation.
    #[test]
    fn test_fairness_under_constant_pressure() -> miette::Result<()> {
        let stream = std::iter::repeat_n(bits(0b1111), 32)
            .with_reset(1)
            .clock_pos_edge(100);
        let uut = RoundRobinArbiter::<4, 2>::default();
        let grants = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        // First grant: cycle 0 starts with valid=false, so start=0;
        //   request bit 0 wins.
        // Subsequent: the kernel result is `Some(idx)` directly,
        //   captured each cycle via the kernel output (no DFF in the
        //   output path), so the rotation should be tight.
        let expected = (0..32u128).map(|i| Some(bits(i % 4))).collect::<Vec<_>>();
        assert_eq!(grants, expected);
        Ok(())
    }

    /// When only one requester asks, it always wins regardless of rotation.
    #[test]
    fn test_single_persistent_requester_always_wins() -> miette::Result<()> {
        let stream = std::iter::repeat_n(bits(0b0010), 16)
            .with_reset(1)
            .clock_pos_edge(100);
        let uut = RoundRobinArbiter::<4, 2>::default();
        let grants = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        assert!(grants.iter().all(|g| *g == Some(bits(1))), "{grants:?}");
        Ok(())
    }

    // Tier 3 — HDL emission snapshot length sanity check
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = RoundRobinArbiter::<4, 2>::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["5066"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    #[test]
    fn test_arbiter_hdl_works() -> miette::Result<()> {
        let uut = RoundRobinArbiter::<4, 2>::default();
        // Mix of patterns: all asking, some asking, none asking.
        let inputs: Vec<Bits<4>> = vec![
            bits(0b1111),
            bits(0b1111),
            bits(0b0101),
            bits(0b0000),
            bits(0b1000),
            bits(0b1111),
            bits(0b0010),
            bits(0b0011),
        ];
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_arbiter_trace() -> miette::Result<()> {
        let uut = RoundRobinArbiter::<4, 2>::default();
        let inputs: Vec<Bits<4>> = vec![
            bits(0b1111),
            bits(0b1111),
            bits(0b1111),
            bits(0b1111),
            bits(0b0101),
            bits(0b0101),
            bits(0b0010),
            bits(0b0010),
            bits(0b0000),
            bits(0b1000),
        ];
        let stream = inputs.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("round_robin_arbiter");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["17cf239b4228ff5cc9ea4c1f443aa1d0edad80adf6abcbfbc451bdfc7a37fb03"];
        let digest = vcd
            .dump_to_file(root.join("round_robin_arbiter.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
