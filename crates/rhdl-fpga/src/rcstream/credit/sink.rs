#![warn(missing_docs)]
//! [`CreditSink<T, F, CREDIT_W, FIFO_N>`] — converts an incoming
//! [`super::CreditRCStream`] source into an outgoing
//! [`crate::rcstream::RCStream`] source.
//!
//! Buffers incoming items in an internal `SyncFIFO` holding
//! `2^FIFO_N - 1` items.  Grants credits to the upstream `CreditRCStream`
//! source as buffer slots free up.
//!
//! # Schematic symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
      +--+CreditSink+----+
      |CreditRC : RCStream
?Item<T,F>      :        | ?Item<T,F>
+---->+ data    :  data  +------>
      |         :        |
      |         :        | bool
      | credit  :        |
      | grant   :        |
<-----+         :  ready |<------+
      +------------------+
")]
//!
//! # I/O directions
//!
//! - **Input** (`I`):
//!   - `upstream_data: Option<Item<T, F>>` — data flowing in from
//!     the `CreditRCStream` source.
//!   - `downstream_ready: bool` — ready signal flowing in from the
//!     downstream `RCStream` sink.
//! - **Output** (`O`):
//!   - `credit_grant: Bits<CREDIT_W>` — credit grants flowing back
//!     to the `CreditRCStream` source.
//!   - `downstream_data: Option<Item<T, F>>` — data flowing forward
//!     to the downstream `RCStream` sink.
//!
//! # Credit-grant policy
//!
//! On reset, `pending_grants` is initialized to
//! `min(2^FIFO_N - 1, 2^CREDIT_W - 1)` — i.e. the FIFO's **usable**
//! capacity, clipped to whatever fits in the credit-counter width.
//! This is the initial credit pool the sink grants the source so the
//! source can begin sending.
//!
//! The `-1` is load-bearing.  `SyncFIFO<_, FIFO_N>` holds
//! `2^FIFO_N - 1` items, not `2^FIFO_N` — "you cannot fill the FIFO to
//! 2^N elements".  An earlier version granted `2^FIFO_N`, handing the
//! source one more token than the buffer could accept; the source spent
//! it, the write hit a full FIFO, and the item was **silently dropped**.
//! It only manifests under sustained backpressure, so a downstream that
//! is always ready never sees it.
//!
//! Each cycle:
//!
//! - If `pending_grants > 0`, emit `credit_grant = 1` and decrement
//!   `pending_grants` by 1.
//! - When an item is popped from the buffer (= `downstream_ready
//!   && downstream_data.is_some()`), increment `pending_grants` by 1
//!   (saturating at `2^CREDIT_W - 1`).
//!
//! Net effect: over time, the source's credit counter equals the
//! number of free slots in the sink's buffer, with the initial pool
//! dribbling out over the first cycles after reset.
//!
//! # Width sizing
//!
//! `CREDIT_W` is the width of both the per-cycle grant signal AND
//! the sink's internal `pending_grants` counter.  For correctness,
//! pick `CREDIT_W` so that `2^CREDIT_W - 1 >= 2^FIFO_N - 1` (i.e.
//! `CREDIT_W >= FIFO_N`); otherwise the sink will under-grant and the
//! effective buffer depth will be capped at `2^CREDIT_W - 1` rather
//! than `2^FIFO_N - 1`.
//!
//! **`FIFO_N >= 2` is required.**  The sink's buffer is a
//! [`crate::fifo::synchronous::SyncFIFO`], and that widget panics at
//! address width 1 (`Bits<1>` arithmetic overflows inside its
//! read/write logic) — a defect in `SyncFIFO` itself, reproducible
//! without any `rcstream` code by simulating a bare
//! `SyncFIFO<b8, 1>`.  `FIFO_N = 1` would only give a 1-item buffer
//! anyway, which defeats the point of credit-based flow control, so
//! this floor costs nothing in practice.  Documented here rather than
//! worked around, because the panic is otherwise baffling: it surfaces
//! from deep inside the FIFO with no hint that the sink's own
//! parameterisation caused it.

use rhdl::prelude::*;

use crate::core::dff;
use crate::fifo::synchronous::{In as FifoIn, SyncFIFO};
use crate::rcstream::bus::Item;

/// Credit-sink: converts incoming `CreditRCStream` into outgoing
/// `RCStream`.  See module docs for credit-grant policy and timing.
///
/// `T` is the payload type, `F` is the framing-marker type, `CREDIT_W`
/// is the per-cycle credit-grant signal width AND internal counter
/// width, and `FIFO_N` is the log2 of the buffer depth (= buffer
/// holds `2^FIFO_N - 1` items).  Pick `CREDIT_W >= FIFO_N` so the
/// counter can hold the initial credit pool without truncation.
#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CreditSink<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    /// Internal buffer.  Holds up to `2^FIFO_N - 1` items (the FIFO's
    /// usable capacity per its convention).
    fifo: SyncFIFO<Item<T, F>, FIFO_N>,
    /// Number of credits the sink owes the source but hasn't yet
    /// granted.  Initialized to `min(2^FIFO_N - 1, 2^CREDIT_W - 1)` on
    /// reset; ticks down by 1 per cycle as we emit grants; ticks up
    /// by 1 per item popped (= slot freed), saturating at
    /// `2^CREDIT_W - 1`.
    pending_grants: dff::DFF<Bits<CREDIT_W>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize> Default
    for CreditSink<T, F, CREDIT_W, FIFO_N>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    fn default() -> Self {
        // Initialize to min(2^FIFO_N - 1, 2^CREDIT_W - 1).
        //
        // NOTE the -1: `SyncFIFO<_, FIFO_N>` holds 2^FIFO_N - 1 items,
        // not 2^FIFO_N ("you cannot fill the FIFO to 2^N elements").
        // Granting 2^FIFO_N credits hands the source one more token than
        // the buffer can accept; the source spends it, the write is
        // dropped on a full FIFO, and the item is silently lost.  Only
        // shows up under sustained backpressure, which is why an
        // always-ready downstream never caught it.
        let depth: u128 = (1u128 << FIFO_N) - 1;
        let max_in_credit: u128 = if CREDIT_W >= 128 {
            u128::MAX
        } else {
            (1u128 << CREDIT_W) - 1
        };
        let initial: u128 = if depth <= max_in_credit {
            depth
        } else {
            max_in_credit
        };
        Self {
            fifo: SyncFIFO::default(),
            pending_grants: dff::DFF::new(bits::<CREDIT_W>(initial)),
        }
    }
}

/// Inputs for [`CreditSink`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In<T: Digital, F: Digital> {
    /// `CreditRCStream` source-side data flowing in.
    pub upstream_data: Option<Item<T, F>>,
    /// `RCStream` sink-side ready flowing in from downstream.
    pub downstream_ready: bool,
}

/// Outputs from [`CreditSink`].
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Out<T: Digital, F: Digital, const CREDIT_W: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Credit grant flowing back to the `CreditRCStream` source.
    /// 0 or 1 per cycle in this implementation.
    pub credit_grant: Bits<CREDIT_W>,
    /// Data flowing forward to the downstream `RCStream` sink.
    /// `Some(item)` when the FIFO has an item ready; `None` when
    /// empty.
    pub downstream_data: Option<Item<T, F>>,
}

impl<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize> SynchronousIO
    for CreditSink<T, F, CREDIT_W, FIFO_N>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    type I = In<T, F>;
    type O = Out<T, F, CREDIT_W>;
    type Kernel = credit_sink_kernel<T, F, CREDIT_W, FIFO_N>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
pub fn credit_sink_kernel<T: Digital, F: Digital, const CREDIT_W: usize, const FIFO_N: usize>(
    _cr: ClockReset,
    i: In<T, F>,
    q: Q<T, F, CREDIT_W, FIFO_N>,
) -> (Out<T, F, CREDIT_W>, D<T, F, CREDIT_W, FIFO_N>)
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    let mut d = D::<T, F, CREDIT_W, FIFO_N>::dont_care();
    let mut o = Out::<T, F, CREDIT_W>::dont_care();

    // Wire the FIFO's input.
    let fifo_has_item: bool = match q.fifo.data {
        Some(_) => true,
        None => false,
    };
    let popping: bool = i.downstream_ready && fifo_has_item;
    d.fifo = FifoIn::<Item<T, F>> {
        data: i.upstream_data,
        next: popping,
    };

    // Forward the FIFO's output to downstream.
    o.downstream_data = q.fifo.data;

    // Credit grant: emit 1 if we have pending grants, else 0.
    let zero = bits::<CREDIT_W>(0);
    let one = bits::<CREDIT_W>(1);
    let max = !zero;
    let grant_now: bool = q.pending_grants != zero;
    o.credit_grant = if grant_now { one } else { zero };

    // Update pending_grants:
    //   - decrement by 1 if we granted this cycle,
    //   - increment by 1 if we popped this cycle (saturating at max).
    // Because grant and pop change pending_grants by ±1 each, the
    // net delta this cycle is one of {-1, 0, +1}.
    let saturated: bool = q.pending_grants == max;
    let next: Bits<CREDIT_W> = if grant_now && popping {
        // -1 + 1 = 0 (and never saturated past since net = 0)
        q.pending_grants
    } else if grant_now {
        q.pending_grants - one
    } else if popping {
        if saturated {
            q.pending_grants
        } else {
            q.pending_grants + one
        }
    } else {
        q.pending_grants
    };
    d.pending_grants = next;

    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_construction() {
        let _u: CreditSink<b8, (), 5, 4> = CreditSink::default();
        let _u2: CreditSink<b16, bool, 8, 6> = CreditSink::default();
    }

    /// Smoke test: descriptor + HDL emission.
    #[test]
    fn descriptor_smoke() -> miette::Result<()> {
        let uut: CreditSink<b8, (), 5, 4> = CreditSink::default();
        let _desc = uut.descriptor("credit_sink_b8_w5_n4".into())?;
        Ok(())
    }

    /// iverilog round-trip: drive items in with downstream always
    /// ready; expect items to propagate through the FIFO and credit
    /// grants to flow back.
    #[test]
    fn iverilog_round_trip() -> Result<(), RHDLError> {
        let uut: CreditSink<b8, (), 5, 4> = CreditSink::default();
        let inputs: Vec<In<b8, ()>> = (0..32)
            .map(|k| {
                let it = Item::<b8, ()> {
                    data: bits::<8>(k as u128),
                    frame: (),
                };
                In {
                    upstream_data: if k < 16 { Some(it) } else { None },
                    downstream_ready: true,
                }
            })
            .collect();
        let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        // The FIFO uses a SyncBRAM internally — first 2 cycles need
        // skipping to let the BRAM exit its X-state.
        let tm = test_bench.rtl(&uut, &TestBenchOptions::default().skip(2))?;
        tm.run_iverilog()?;
        Ok(())
    }
}
