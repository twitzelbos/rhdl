//! Integer divider (unsigned, shift-subtract)
//!
//! Multi-cycle unsigned integer divider using the textbook
//! restoring shift-subtract algorithm.  Computes quotient and
//! remainder for `N`-bit dividend ÷ `N`-bit divisor in `N` clock
//! cycles after `start` is asserted.
//!
//! The Rust `/` and `%` operators on `Bits<N>` do not synthesize in
//! `#[kernel]` — instantiate this widget any time you need
//! programmable division (baud-rate generation, fixed-point math,
//! address scaling).
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+Divider+-----+
     |                   |
B<N> |                   | B<N>
+--->| dividend  quotient+--->
B<N> |                   | B<N>
+--->| divisor remainder +--->
bool |                   | bool
+--->| start         busy+--->
     |                   |
     +-------------------+
")]
//!
//!# Internals
//!
//! Five registers hold the working state:
//!
//! - `rem` — partial remainder, shifted left by one and merged with
//!   the next dividend bit each cycle.
//! - `quot` — partial quotient, shifted left and OR'd with `1` when
//!   the divisor fits.
//! - `dividend_remaining` — shift register feeding `rem`'s LSB; bits
//!   are consumed MSB-first.
//! - `divisor_reg` — divisor latched at `start` time.
//! - `counter` — counts down from `N` to `0`; `busy = (counter != 0)`.
//!
//! The algorithm avoids `N+1`-bit arithmetic by tracking the would-be
//! carry bit (`rem`'s old MSB before the left shift) separately.  The
//! comparison `(carry || new_rem) >= divisor` reduces to
//! `carry == 1 || new_rem >= divisor`, and the subtraction is
//! performed in plain `N`-bit wrapping arithmetic (correct in both
//! the carry-set and carry-clear cases).
//!
//!# Behavior
//!
//! - When `busy == false`: asserting `start` latches `dividend` and
//!   `divisor`, zeros the working registers, and loads the cycle
//!   counter with `N`.  `start` is ignored if `busy == true`.
//! - For the next `N` cycles, `busy == true` and the result is
//!   computed one bit per cycle (MSB of the quotient first).
//! - When the counter reaches `0`, `busy` drops and the result
//!   appears on the `quotient` and `remainder` outputs.  The result
//!   is held until the next `start`.
//! - Divide-by-zero is *not* trapped: with `divisor = 0` the algorithm
//!   produces `quotient = 2^N - 1` (all ones) and
//!   `remainder = dividend`.  Callers that care should gate `start`
//!   on `divisor != 0`.
//!
//!# Parameters
//!
//! - `N` — width of dividend, divisor, quotient, and remainder
//! - `W` — width of the cycle counter, satisfying `2^W > N`
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/divider.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/divider.md")]
use rhdl::prelude::*;

use super::dff;

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
/// Unsigned integer divider core.
pub struct Divider<const N: usize, const W: usize>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    rem: dff::DFF<Bits<N>>,
    quot: dff::DFF<Bits<N>>,
    dividend_remaining: dff::DFF<Bits<N>>,
    divisor_reg: dff::DFF<Bits<N>>,
    counter: dff::DFF<Bits<W>>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [Divider].
pub struct In<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    /// The dividend.  Latched at the cycle when `start` is asserted.
    pub dividend: Bits<N>,
    /// The divisor.  Latched at the cycle when `start` is asserted.
    pub divisor: Bits<N>,
    /// Strobe to start a new division.  Ignored while `busy` is high.
    pub start: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [Divider].
pub struct Out<const N: usize>
where
    rhdl::bits::W<N>: BitWidth,
{
    /// The quotient.  Valid when `busy == false`.
    pub quotient: Bits<N>,
    /// The remainder.  Valid when `busy == false`.
    pub remainder: Bits<N>,
    /// High while a division is in progress.
    pub busy: bool,
}

impl<const N: usize, const W: usize> SynchronousIO for Divider<N, W>
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    type I = In<N>;
    type O = Out<N>;
    type Kernel = divider<N, W>;
}

#[kernel]
/// Kernel for [Divider].
pub fn divider<const N: usize, const W: usize>(
    cr: ClockReset,
    i: In<N>,
    q: Q<N, W>,
) -> (Out<N>, D<N, W>)
where
    rhdl::bits::W<N>: BitWidth,
    rhdl::bits::W<W>: BitWidth,
{
    let busy = q.counter != bits(0);
    let mut d = D::<N, W>::dont_care();
    let mut o = Out::<N>::dont_care();
    o.quotient = q.quot;
    o.remainder = q.rem;
    o.busy = busy;

    // Default: hold state.
    d.dividend_remaining = q.dividend_remaining;
    d.divisor_reg = q.divisor_reg;
    d.rem = q.rem;
    d.quot = q.quot;
    d.counter = q.counter;

    if !busy && i.start {
        // Latch operands and arm.
        d.dividend_remaining = i.dividend;
        d.divisor_reg = i.divisor;
        d.rem = bits(0);
        d.quot = bits(0);
        d.counter = bits(N as u128);
    } else if busy {
        // Capture the about-to-be-shifted-off MSB of `rem`; it is the
        // implicit (N+1)-th carry bit of the partial remainder.
        let rem_msb = (q.rem >> ((N - 1) as u128)) & bits(1);
        let dividend_msb = (q.dividend_remaining >> ((N - 1) as u128)) & bits(1);
        let new_rem_low = (q.rem << 1) | dividend_msb;
        // (carry || new_rem_low) >= divisor reduces to:
        //   carry==1 (always >=) OR new_rem_low >= divisor
        let take_subtract = rem_msb != bits(0) || new_rem_low >= q.divisor_reg;
        // Subtraction in plain N-bit wrap arithmetic gives the correct
        // result regardless of whether `carry` was set, because
        //   (2^N + new_rem_low) - divisor mod 2^N
        //     = new_rem_low + (2^N - divisor) mod 2^N
        //     = new_rem_low - divisor in wrapping form.
        let new_rem = if take_subtract {
            new_rem_low - q.divisor_reg
        } else {
            new_rem_low
        };
        let quot_lsb: Bits<N> = if take_subtract { bits(1) } else { bits(0) };
        let new_quot = (q.quot << 1) | quot_lsb;
        d.dividend_remaining = q.dividend_remaining << 1;
        d.rem = new_rem;
        d.quot = new_quot;
        d.counter = q.counter - bits(1);
    }

    if cr.reset.any() {
        d.dividend_remaining = bits(0);
        d.divisor_reg = bits(0);
        d.rem = bits(0);
        d.quot = bits(0);
        d.counter = bits(0);
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    /// Software reference: divide `dividend` by `divisor` over `N` bits.
    fn div_ref(dividend: u128, divisor: u128, n: usize) -> (u128, u128) {
        let mask = if n == 128 { !0u128 } else { (1u128 << n) - 1 };
        let dividend = dividend & mask;
        let divisor = divisor & mask;
        if divisor == 0 {
            (mask, dividend)
        } else {
            (dividend / divisor, dividend % divisor)
        }
    }

    /// Build an input stream that issues a single division and waits
    /// for it to complete.
    fn divide_one(uut: &Divider<8, 4>, dividend: u128, divisor: u128) -> (u128, u128) {
        let mut stream_in: Vec<In<8>> = vec![In {
            dividend: bits(dividend),
            divisor: bits(divisor),
            start: true,
        }];
        for _ in 0..16 {
            stream_in.push(In {
                dividend: bits(0),
                divisor: bits(0),
                start: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .map(|s| s.output)
            .collect::<Vec<_>>();
        // The result appears on the first non-busy cycle after start.
        // Find the first cycle where busy is false AND we've already seen busy go high.
        let mut seen_busy = false;
        for o in &outputs {
            if o.busy {
                seen_busy = true;
            } else if seen_busy {
                return (o.quotient.raw(), o.remainder.raw());
            }
        }
        // Fallback: last sample.
        let last = outputs.last().unwrap();
        (last.quotient.raw(), last.remainder.raw())
    }

    // Tier 2 — iterator simulation against the software reference.

    #[test]
    fn test_simple_divisions() -> miette::Result<()> {
        let uut = Divider::<8, 4>::default();
        let cases = [
            (100u128, 7u128),
            (255, 16),
            (0, 5),
            (200, 1),
            (50, 50),
            (0xFF, 0xFF),
        ];
        for (dvd, dvs) in cases {
            let (q, r) = divide_one(&uut, dvd, dvs);
            let (eq, er) = div_ref(dvd, dvs, 8);
            assert_eq!((q, r), (eq, er), "{dvd} / {dvs}");
        }
        Ok(())
    }

    #[test]
    fn test_divide_by_zero_returns_max_quotient_and_dividend() -> miette::Result<()> {
        let uut = Divider::<8, 4>::default();
        let (q, r) = divide_one(&uut, 42, 0);
        assert_eq!(q, 0xFF);
        assert_eq!(r, 42);
        Ok(())
    }

    #[test]
    fn test_random_sweep_against_reference() -> miette::Result<()> {
        let uut = Divider::<8, 4>::default();
        // Sweep a fixed grid of operands.
        let dividends = [0u128, 1, 7, 100, 128, 200, 255];
        let divisors = [1u128, 2, 3, 7, 13, 16, 100, 255];
        for &d in &dividends {
            for &k in &divisors {
                let (q, r) = divide_one(&uut, d, k);
                let (eq, er) = div_ref(d, k, 8);
                assert_eq!((q, r), (eq, er), "{d} / {k}");
            }
        }
        Ok(())
    }

    // Tier 3 — HDL emission length sanity check
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Divider::<8, 4>::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["8199"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    #[test]
    fn test_divider_hdl_works() -> miette::Result<()> {
        let uut = Divider::<8, 4>::default();
        let mut stream_in: Vec<In<8>> = vec![In {
            dividend: bits(100),
            divisor: bits(7),
            start: true,
        }];
        for _ in 0..16 {
            stream_in.push(In {
                dividend: bits(0),
                divisor: bits(0),
                start: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_divider_trace() -> miette::Result<()> {
        let uut = Divider::<8, 4>::default();
        let mut stream_in: Vec<In<8>> = vec![In {
            dividend: bits(100),
            divisor: bits(7),
            start: true,
        }];
        for _ in 0..12 {
            stream_in.push(In {
                dividend: bits(0),
                divisor: bits(0),
                start: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("divider");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["99a49516fd43126e7f410765a994cdc56b6c7a87c8386f76fc50263da9825c0c"];
        let digest = vcd.dump_to_file(root.join("divider.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
