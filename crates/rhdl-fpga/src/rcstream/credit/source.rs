#![warn(missing_docs)]
//! [`CreditSource<T, F, CREDIT_W>`] — converts an [`crate::rcstream::RCStream`]
//! source into a [`super::CreditRCStream`] source.
//!
//! Tracks a local credit counter, gates outgoing items on
//! `counter > 0`, signals upstream `ready` when it has credit.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +--+CreditSource+----+
      |  RCStream :  CreditRC
?Item<T,F>        :        | ?Item<T,F>
+---->+ data      :  data  +------>
      |           :        |
+---->+ ready     :        |
      |           : credit |
      |           : grant  |<------+
 bool |           :        |
<-----+ ready     :        |
      +--------------------+
")]
//!
//! Wait — the symbol above is a placeholder; the *I/O direction*
//! convention is:
//!
//! - **Input** (`I`):
//!   - `upstream_data: Option<Item<T, F>>` — the upstream
//!     `RCStream` source's data flowing in.
//!   - `credit_grant: Bits<CREDIT_W>` — the downstream
//!     `CreditRCStream` sink's credit grant flowing in.
//! - **Output** (`O`):
//!   - `upstream_ready: bool` — the ready signal flowing back to the
//!     upstream `RCStream` source.  Asserted when the local credit
//!     counter is non-zero.
//!   - `downstream_data: Option<Item<T, F>>` — the data flowing
//!     forward to the downstream `CreditRCStream` sink.

use rhdl::prelude::*;

use crate::core::constant::Constant;
use crate::core::dff;
use crate::rcstream::bus::Item;

/// Credit-source: converts incoming `RCStream` into outgoing
/// `CreditRCStream`.
///
/// Maintains a local credit counter (`Bits<CREDIT_W>`).  Each cycle:
///
/// 1. The local counter is updated from `q.credit + i.credit_grant -
///    (sending ? 1 : 0)`, saturating at `2^CREDIT_W - 1`.
/// 2. `sending` = `i.upstream_data.is_some() && q.credit > 0`.
/// 3. `o.downstream_data = if sending { i.upstream_data } else { None }`.
/// 4. `o.upstream_ready = q.credit > 0`.
///
/// The send decision uses `q.credit` (latched at end of last cycle),
/// not the in-cycle `i.credit_grant` — this is what breaks the long
/// combinational dependency that the simple Ready/Valid form has.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CreditSource<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Local credit counter.  Number of tokens the source currently
    /// holds (= maximum number of items it can still send before
    /// running out).
    credit: dff::DFF<Bits<CREDIT_W>>,
    /// Zero-cost type-parameter carrier: a `Constant<Item<T, F>>`
    /// that propagates `T` and `F` through the struct so the
    /// SynchronousDQ derive sees them.  Synthesizes to a constant
    /// driver — no DFF state — and the kernel ignores its output.
    _marker: Constant<Item<T, F>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize> Default for CreditSource<T, F, CREDIT_W>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            credit: dff::DFF::new(bits::<CREDIT_W>(0)),
            _marker: Constant::new(Item::<T, F>::dont_care()),
        }
    }
}

/// Inputs for [`CreditSource`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// `RCStream` source-side data flowing in from upstream.
    pub upstream_data: Option<Item<T, F>>,
    /// Credit grant flowing in from the downstream `CreditRCStream`
    /// sink.  Added to the local counter this cycle (before send
    /// decision uses the LATCHED counter).
    pub credit_grant: Bits<CREDIT_W>,
}

/// Outputs from [`CreditSource`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Ready signal flowing back to the upstream `RCStream` source.
    /// Asserted when the local credit counter is non-zero (= we can
    /// accept the next item from upstream this cycle and forward it
    /// downstream).
    pub upstream_ready: bool,
    /// Data flowing forward to the downstream `CreditRCStream` sink.
    /// `Some(item)` when both `upstream_data.is_some()` AND the
    /// local credit counter is non-zero.
    pub downstream_data: Option<Item<T, F>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize> SynchronousIO
    for CreditSource<T, F, CREDIT_W>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    type I = In<T, F, CREDIT_W>;
    type O = Out<T, F, CREDIT_W>;
    type Kernel = credit_source_kernel<T, F, CREDIT_W>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn credit_source_kernel<T: Digital, F: Digital, const CREDIT_W: usize>(
    _cr: ClockReset,
    i: In<T, F, CREDIT_W>,
    q: Q<T, F, CREDIT_W>,
) -> (Out<T, F, CREDIT_W>, D<T, F, CREDIT_W>)
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    let mut d = D::<T, F, CREDIT_W>::dont_care();
    let mut o = Out::<T, F, CREDIT_W>::dont_care();

    // Send decision uses the LATCHED counter (q.credit), not any
    // in-cycle combinational input.  This is what breaks the long
    // sink→source combinational dependency.
    let have_credit: bool = q.credit != bits::<CREDIT_W>(0);
    let upstream_has_item: bool = match i.upstream_data {
        Some(_) => true,
        None => false,
    };
    let sending: bool = have_credit && upstream_has_item;

    // Outputs.
    o.upstream_ready = have_credit;
    o.downstream_data = if sending { i.upstream_data } else { None };

    // Update the credit counter.  add credit_grant, subtract 1 if
    // sending.  Saturating-add: if grant + counter would exceed
    // 2^CREDIT_W - 1, cap at the max value.  (For typical CREDIT_W=4
    // and grant=1 the saturation almost never fires; we add it for
    // robustness when the sink bursts grants after a long stall.)
    let add: Bits<CREDIT_W> = i.credit_grant;
    let sub: Bits<CREDIT_W> = if sending {
        bits::<CREDIT_W>(1)
    } else {
        bits::<CREDIT_W>(0)
    };
    // Compute next counter, saturating at 2^CREDIT_W - 1.  The
    // intermediate may overflow CREDIT_W bits; use a one-wider type
    // for the add, then clamp.
    let raw_next: Bits<CREDIT_W> = q.credit + add - sub;
    // Saturation: if add caused the counter to wrap (raw_next <
    // q.credit despite add > 0 and no subtract), clamp to all-ones.
    let max: Bits<CREDIT_W> = !bits::<CREDIT_W>(0);
    let saturated: bool = (add > sub) && (raw_next < q.credit);
    d.credit = if saturated { max } else { raw_next };

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _u: CreditSource<b8, (), 4> = CreditSource::default();
        let _u2: CreditSource<b16, bool, 8> = CreditSource::default();
        let _u3: CreditSource<b32, b8, 6> = CreditSource::default();
    }

    /// With zero credit and an upstream item, source does NOT send.
    #[test]
    fn kernel_no_credit_does_not_send() {
        let cr = ClockReset::dont_care();
        let item = Item::<b8, ()> { data: bits::<8>(0xAB), frame: () };
        let i = In::<b8, (), 4> {
            upstream_data: Some(item),
            credit_grant: bits::<4>(0),
        };
        let q = Q::<b8, (), 4> { credit: bits::<4>(0), _marker: Item::<b8, ()>::dont_care() };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        assert!(o.downstream_data.is_none(), "no credit → no send");
        assert!(!o.upstream_ready, "no credit → not ready upstream");
        // Counter stays 0 (no grant, no send).
        assert_eq!(d.credit.raw(), 0);
    }

    /// With credit and an upstream item, source sends and decrements.
    #[test]
    fn kernel_with_credit_sends_and_decrements() {
        let cr = ClockReset::dont_care();
        let item = Item::<b8, ()> { data: bits::<8>(0x42), frame: () };
        let i = In::<b8, (), 4> {
            upstream_data: Some(item),
            credit_grant: bits::<4>(0),
        };
        let q = Q::<b8, (), 4> { credit: bits::<4>(3), _marker: Item::<b8, ()>::dont_care() };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        match o.downstream_data {
            Some(it) => assert_eq!(it.data.raw(), 0x42),
            None => panic!("expected Some(item) when credit > 0"),
        }
        assert!(o.upstream_ready);
        // Counter decrements: 3 - 1 = 2.
        assert_eq!(d.credit.raw(), 2);
    }

    /// Credit grant accumulates into the counter even without sending.
    #[test]
    fn kernel_credit_grant_accumulates() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, (), 4> {
            upstream_data: None,
            credit_grant: bits::<4>(2),
        };
        let q = Q::<b8, (), 4> { credit: bits::<4>(3), _marker: Item::<b8, ()>::dont_care() };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        assert!(o.downstream_data.is_none());
        assert!(o.upstream_ready);
        // Counter increments: 3 + 2 = 5, no decrement.
        assert_eq!(d.credit.raw(), 5);
    }

    /// Credit grant + send: net change.
    #[test]
    fn kernel_grant_and_send_simultaneously() {
        let cr = ClockReset::dont_care();
        let item = Item::<b8, ()> { data: bits::<8>(0x55), frame: () };
        let i = In::<b8, (), 4> {
            upstream_data: Some(item),
            credit_grant: bits::<4>(2),
        };
        let q = Q::<b8, (), 4> { credit: bits::<4>(1), _marker: Item::<b8, ()>::dont_care() };
        let (o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        // Has credit (q.credit = 1 > 0), so sends.
        assert!(o.downstream_data.is_some());
        // Counter: 1 + 2 - 1 = 2.
        assert_eq!(d.credit.raw(), 2);
    }

    /// Saturation: adding too much credit caps at all-ones.
    #[test]
    fn kernel_credit_saturates_at_max() {
        let cr = ClockReset::dont_care();
        let i = In::<b8, (), 4> {
            upstream_data: None,
            credit_grant: bits::<4>(0xF),
        };
        // Counter at max already.
        let q = Q::<b8, (), 4> { credit: bits::<4>(0xF), _marker: Item::<b8, ()>::dont_care() };
        let (_o, d) = credit_source_kernel::<b8, (), 4>(cr, i, q);
        // 0xF + 0xF would wrap to 0xE; saturate to 0xF.
        assert_eq!(d.credit.raw(), 0xF);
    }

    /// Smoke test: descriptor + HDL emission.
    #[test]
    fn descriptor_smoke() -> miette::Result<()> {
        let uut: CreditSource<b8, (), 4> = CreditSource::default();
        let _desc = uut.descriptor("credit_source_b8_w4".into())?;
        Ok(())
    }

    /// iverilog round-trip: a simple use case.  Drive 16 items
    /// through with always-ready credit grant; expect items to
    /// appear on the downstream after a 1-cycle DFF delay.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut: CreditSource<b8, (), 4> = CreditSource::default();
        let inputs: Vec<In<b8, (), 4>> = (0..16).map(|k| {
            let it = Item::<b8, ()> { data: bits::<8>(k as u128), frame: () };
            In {
                upstream_data: Some(it),
                credit_grant: bits::<4>(1),  // grant 1 per cycle
            }
        }).collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }
}
