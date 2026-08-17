#![warn(missing_docs)]
//! Closed-loop test fixtures for `RCStream` widgets.
//!
//! Every `rcstream` combinator's Tier-2 test was the same twenty lines
//! of `run_fn` bookkeeping — track a reset flag, decide a `ready`,
//! push accepted items into a `Vec`, feed the next item when the widget
//! says it can take one, stop when everything has arrived. Written by
//! hand each time, that boilerplate is where the interesting decisions
//! get made silently, and it is easy to write a version that cannot
//! fail.
//!
//! This module makes those decisions explicit and reusable.
//!
//! # What it encodes
//!
//! Three lessons this library has learned the hard way, baked into the
//! fixture so a new widget gets them by default:
//!
//! - **The sink must be able to stall** ([`Cadence`]). A permissive,
//!   always-ready sink exercises every path except the one a
//!   flow-control widget exists for. `rcstream::credit::CreditSink`
//!   shipped silently dropping an item because its only test drove
//!   `ready` true on every cycle.
//! - **[`Cadence::DataGated`] is the adversarial-but-legal shape.** A
//!   sink may withhold `ready` until it sees data (AXI permits READY to
//!   depend on VALID). A widget that consumes an item, declines to emit
//!   it, and then waits for a downstream that has no reason to respond
//!   will deadlock. `stream::filter` shipped that way.
//! - **Assert the whole sequence, not a property of it.**
//!   [`Delivered::assert_exactly`] compares against the full expected
//!   list, so an empty result fails. A test whose assertions sit inside
//!   `if let Some(..)` passes when the widget delivers nothing —
//!   `stream::pipe_wrapper` shipped completely dead behind one.
//!
//! # Scope
//!
//! Covers widgets shaped `I = RCStream<T, F>`, `O = RCStream<S, F>`:
//! [`super::relay`], [`super::map`], [`super::filter`],
//! [`super::filter_map`]. Widgets with a different I/O shape — the
//! `N`-branch [`super::fanout`], the credit pair, the AXI translators —
//! drive `run_fn` directly, and should: forcing them through a fixture
//! that does not fit would test the adapter rather than the widget.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;

use crate::rcstream::bus::{Item, RCStream};

/// How the downstream sink offers `ready`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Cadence {
    /// Ready **only when an item is visible on the wire**.
    ///
    /// Legal under the Ready/Valid contract, and the shape that catches
    /// a widget which absorbs an item, emits nothing, and then waits for
    /// a downstream that has no reason to respond. Reach for this first
    /// for anything that can drop, filter, or merge.
    DataGated,
    /// Accept on one cycle in `k`, irrespective of what is offered.
    ///
    /// Real backpressure uncorrelated with data — use when the claim
    /// depends on a known rate. `k <= 1` is always ready.
    Periodic(u32),
    /// Never stall.
    ///
    /// A baseline only. On its own it cannot exercise any backpressure
    /// path, which for a flow-control widget is every path that matters.
    AlwaysReady,
}

impl Cadence {
    /// Decide `ready` for this cycle.
    fn ready(self, phase: u32, offered: bool) -> bool {
        match self {
            Cadence::DataGated => offered,
            Cadence::Periodic(k) => k <= 1 || phase.is_multiple_of(k),
            Cadence::AlwaysReady => true,
        }
    }
}

/// What a run delivered, plus the assertions worth making about it.
#[derive(Clone, Debug)]
pub struct Delivered<S: Digital, F: Digital> {
    /// Items the sink accepted, in arrival order.
    pub items: Vec<Item<S, F>>,
    /// How many items the source managed to hand over.
    pub sent: usize,
    /// True if the run hit its cycle budget rather than finishing.
    pub timed_out: bool,
}

impl<S: Digital, F: Digital> Delivered<S, F> {
    /// Assert the run delivered exactly `want`, in order.
    ///
    /// Compares the **whole sequence**: an empty result fails, as does a
    /// duplicated or reordered one. That is the point — a property of
    /// what arrived cannot detect nothing arriving.
    pub fn assert_exactly(&self, want: &[Item<S, F>])
    where
        S: std::fmt::Debug,
        F: std::fmt::Debug,
    {
        assert!(
            !self.timed_out,
            "the run hit its cycle budget with {} of {} items delivered — \
             the widget is stalled, not slow",
            self.items.len(),
            want.len()
        );
        assert_eq!(
            self.items.len(),
            want.len(),
            "delivered {} items, expected {}",
            self.items.len(),
            want.len()
        );
        assert_eq!(&self.items[..], want, "delivered sequence differs");
    }

    /// Assert at least `n` items arrived.
    ///
    /// For runs where the exact set is not predictable (a filter, say)
    /// but "it delivered something" still needs pinning down.
    pub fn assert_at_least(&self, n: usize) {
        assert!(
            self.items.len() >= n,
            "delivered {} items, expected at least {n}",
            self.items.len()
        );
    }
}

/// Drive `items` through `uut` against a stalling sink and collect what
/// comes out.
///
/// The source offers the next item whenever the widget asserts `ready`;
/// the sink accepts per `cadence`. The run stops once every item has
/// been offered and nothing further arrives for a full drain window, or
/// when `max_cycles` is reached (recorded in
/// [`Delivered::timed_out`] rather than panicking, so the caller can
/// report it with its own context).
pub fn drive<W, T, F, S>(
    uut: &W,
    items: &[Item<T, F>],
    cadence: Cadence,
    max_cycles: usize,
) -> Delivered<S, F>
where
    W: Synchronous + SynchronousIO<I = RCStream<T, F>, O = RCStream<S, F>>,
    T: Digital,
    S: Digital,
    F: Digital,
{
    let mut sent = 0usize;
    let mut got: Vec<Item<S, F>> = Vec::new();
    let mut need_reset = true;
    let mut phase: u32 = 0;
    let mut cycles = 0usize;
    // Once everything has been offered, allow a drain window for items
    // still in flight before declaring the run finished.
    let mut idle_after_last = 0usize;
    let drain_window = 64usize;

    uut.run_fn(
        |o| {
            if need_reset {
                need_reset = false;
                return Some(ResetOrData::Reset);
            }
            cycles += 1;
            if cycles > max_cycles {
                return None;
            }
            phase = phase.wrapping_add(1);

            let offered = o.data.is_some();
            let ready = cadence.ready(phase, offered);
            if ready {
                if let Some(it) = o.data {
                    got.push(it);
                    idle_after_last = 0;
                }
            }

            let mut input = RCStream::<T, F> { data: None, ready };
            if sent < items.len() && o.ready {
                input.data = Some(items[sent]);
                sent += 1;
            }

            if sent == items.len() {
                idle_after_last += 1;
                if idle_after_last > drain_window {
                    return None;
                }
            }
            Some(ResetOrData::Data(input))
        },
        100,
    )
    .for_each(drop);

    Delivered {
        items: got,
        sent,
        timed_out: cycles > max_cycles,
    }
}

/// Drive `items` through `uut` and assert every one arrives, in order.
///
/// The common case for an order- and content-preserving widget: a relay,
/// a buffer, a clock crossing. Uses [`Cadence::DataGated`] by default
/// because that is the shape most likely to expose a flow-control bug.
pub fn assert_lossless<W, T, F>(uut: &W, items: &[Item<T, F>])
where
    W: Synchronous + SynchronousIO<I = RCStream<T, F>, O = RCStream<T, F>>,
    T: Digital + std::fmt::Debug,
    F: Digital + std::fmt::Debug,
{
    let out = drive::<W, T, F, T>(uut, items, Cadence::DataGated, 20_000);
    out.assert_exactly(items);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcstream::relay::RCStreamRelay;

    fn items(n: u128) -> Vec<Item<b8, ()>> {
        (0..n)
            .map(|k| Item::<b8, ()> {
                data: bits::<8>(k % 256),
                frame: (),
            })
            .collect()
    }

    /// The fixture drives a real widget losslessly.
    #[test]
    fn relay_is_lossless_through_the_fixture() {
        let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
        assert_lossless(&uut, &items(24));
    }

    /// Every cadence must actually deliver — a fixture whose sink never
    /// accepts would make each of these vacuously "lossless".
    #[test]
    fn every_cadence_delivers() {
        for cadence in [
            Cadence::DataGated,
            Cadence::Periodic(3),
            Cadence::AlwaysReady,
        ] {
            let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
            let want = items(16);
            let out = drive::<_, b8, (), b8>(&uut, &want, cadence, 20_000);
            out.assert_exactly(&want);
        }
    }

    /// `Periodic` really does stall: a rate-limited sink takes strictly
    /// longer than an always-ready one. Without this the cadence could
    /// be silently ignored and every test above would still pass.
    #[test]
    fn periodic_actually_throttles() {
        fn cycles_for(cadence: Cadence) -> usize {
            let uut: RCStreamRelay<b8, ()> = RCStreamRelay::default();
            let want = items(16);
            let mut n = 0usize;
            let mut need_reset = true;
            let mut phase = 0u32;
            let mut sent = 0usize;
            let mut got = 0usize;
            uut.run_fn(
                |o| {
                    if need_reset {
                        need_reset = false;
                        return Some(ResetOrData::Reset);
                    }
                    n += 1;
                    phase = phase.wrapping_add(1);
                    if n > 20_000 || got == want.len() {
                        return None;
                    }
                    let ready = cadence.ready(phase, o.data.is_some());
                    if ready && o.data.is_some() {
                        got += 1;
                    }
                    let mut input = RCStream::<b8, ()> { data: None, ready };
                    if sent < want.len() && o.ready {
                        input.data = Some(want[sent]);
                        sent += 1;
                    }
                    Some(ResetOrData::Data(input))
                },
                100,
            )
            .for_each(drop);
            n
        }
        let fast = cycles_for(Cadence::AlwaysReady);
        let slow = cycles_for(Cadence::Periodic(4));
        assert!(
            slow > fast,
            "a 1-in-4 sink must take longer than an always-ready one: {slow} vs {fast}"
        );
    }

    /// `assert_exactly` must reject a short delivery, not just a wrong
    /// one — the failure mode that lets a dead widget pass.
    #[test]
    #[should_panic(expected = "delivered 0 items, expected 4")]
    fn assert_exactly_rejects_an_empty_delivery() {
        let empty = Delivered::<b8, ()> {
            items: Vec::new(),
            sent: 4,
            timed_out: false,
        };
        empty.assert_exactly(&items(4));
    }

    /// A stalled run is reported as a stall, not as a content mismatch.
    #[test]
    #[should_panic(expected = "stalled, not slow")]
    fn assert_exactly_reports_a_timeout_distinctly() {
        let stuck = Delivered::<b8, ()> {
            items: Vec::new(),
            sent: 1,
            timed_out: true,
        };
        stuck.assert_exactly(&items(4));
    }
}
