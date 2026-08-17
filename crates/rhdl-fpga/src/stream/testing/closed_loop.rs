#![warn(missing_docs)]
//! Closed-loop driver for `StreamIO`-shaped widgets.
//!
//! Most `stream::*` Tier-2 tests are the same twenty lines of `run_fn`
//! bookkeeping written out by hand: track a reset flag, decide a
//! `ready`, push arrivals into a `Vec`, offer the next item when the
//! widget says it can take one, stop when everything has landed. That
//! boilerplate is where the interesting decisions get made silently,
//! and it is easy to write a version that cannot fail.
//!
//! # What it encodes
//!
//! Three lessons this library learned the hard way, so a new test gets
//! them by default rather than by remembering:
//!
//! - **The sink must be able to stall** ([`Cadence`]). An always-ready
//!   sink exercises every path except the one a flow-control widget
//!   exists for. `rcstream::credit::CreditSink` shipped silently
//!   dropping an item because its only test drove `ready` true on every
//!   cycle.
//! - **[`Cadence::DataGated`] is the adversarial-but-legal default.** A
//!   sink may withhold `ready` until it sees data — AXI permits READY to
//!   depend on VALID — and a widget that consumes an item, declines to
//!   emit it, then waits for a downstream that has no reason to respond
//!   will deadlock. [`crate::stream::filter`] shipped exactly that.
//! - **Assert the whole sequence** ([`Delivered::assert_exactly`]), so
//!   an empty result fails. [`crate::stream::pipe_wrapper`] shipped
//!   completely dead behind assertions that only ran on delivery — a
//!   property of what arrives cannot detect nothing arriving.
//!
//! It also distinguishes *stalled* from *delivered the wrong thing*,
//! which a hand-rolled `got == want` comparison cannot.
//!
//! # Scope
//!
//! Covers widgets shaped `I = StreamIO<T, S>`, `O = StreamIO<S, T>` —
//! [`crate::stream::map`], [`crate::stream::filter`],
//! [`crate::stream::filter_map`], [`crate::stream::stream_buffer`], and
//! also [`crate::stream::chunked`] / [`crate::stream::flatten`], whose
//! `S` is an array type.
//!
//! Not covered, deliberately: [`crate::stream::tee`] and
//! [`crate::stream::zip`] have multiple ports, and
//! [`crate::stream::fifo_to_stream`] / [`crate::stream::stream_to_fifo`]
//! speak the FIFO `next`/`full` protocol rather than Ready/Valid.
//! Forcing those through a fixture that does not fit would test the
//! adapter instead of the widget; they keep driving `run_fn` directly.
//!
//! # Relationship to `rcstream::testing`
//!
//! That module does the same job for the `RCStream` bus. The two are
//! deliberately not shared: the bus types differ (`Ready<S>` versus a
//! bare `bool`, `Item<T, F>` versus a plain payload), and
//! `stream` and `rcstream` are documented as independent — coupling
//! their test harnesses would undo that for the sake of one shared
//! enum. If a third bus ever appears, factoring the cadence policy out
//! becomes worthwhile.

use rhdl::core::sim::ResetOrData;
use rhdl::prelude::*;

use crate::stream::{StreamIO, ready};

/// How the downstream sink offers `ready`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Cadence {
    /// Ready **only when an item is visible on the wire**.
    ///
    /// Legal under the Ready/Valid contract, and the shape that catches
    /// a widget which absorbs an item, emits nothing, and then waits on
    /// a downstream it never showed anything to. Reach for this first
    /// for anything that can drop, filter, or merge.
    DataGated,
    /// Accept on one cycle in `k`, irrespective of what is offered.
    ///
    /// Backpressure uncorrelated with data. `k <= 1` is always ready.
    Periodic(u32),
    /// Never stall. A baseline only — on its own it cannot exercise any
    /// backpressure path.
    AlwaysReady,
}

impl Cadence {
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
pub struct Delivered<S: Digital> {
    /// Items the sink accepted, in arrival order.
    pub items: Vec<S>,
    /// How many items the source managed to hand over.
    pub sent: usize,
    /// True if the run hit its cycle budget rather than finishing.
    pub timed_out: bool,
}

impl<S: Digital + std::fmt::Debug> Delivered<S> {
    /// Assert the run delivered exactly `want`, in order.
    ///
    /// Compares the **whole sequence**, so an empty result fails, as
    /// does a duplicated or reordered one.
    pub fn assert_exactly(&self, want: &[S]) {
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
/// the sink accepts per `cadence`. The run ends once everything has been
/// offered and a drain window passes with nothing further arriving, or
/// at `max_cycles` — recorded in [`Delivered::timed_out`] rather than
/// panicking, so the caller reports it with its own context.
pub fn drive<W, T, S>(uut: &W, items: &[T], cadence: Cadence, max_cycles: usize) -> Delivered<S>
where
    W: Synchronous + SynchronousIO<I = StreamIO<T, S>, O = StreamIO<S, T>>,
    T: Digital,
    S: Digital,
{
    let mut sent = 0usize;
    let mut got: Vec<S> = Vec::new();
    let mut need_reset = true;
    let mut phase: u32 = 0;
    let mut cycles = 0usize;
    let mut idle_after_last = 0usize;
    let drain_window = 128usize;

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

            let sink_ready = cadence.ready(phase, o.data.is_some());
            if sink_ready {
                if let Some(v) = o.data {
                    got.push(v);
                    idle_after_last = 0;
                }
            }

            let mut input = StreamIO::<T, S> {
                data: None,
                ready: ready::<S>(sink_ready),
            };
            if sent < items.len() && o.ready.raw {
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

/// Drive `items` through `uut` against a **data-gated** sink and assert
/// every one arrives, in order.
///
/// The common case for an order- and content-preserving widget. Uses
/// [`Cadence::DataGated`] because that is the shape most likely to
/// expose a flow-control bug.
pub fn assert_lossless<W, T>(uut: &W, items: &[T])
where
    W: Synchronous + SynchronousIO<I = StreamIO<T, T>, O = StreamIO<T, T>>,
    T: Digital + std::fmt::Debug,
{
    let out = drive::<W, T, T>(uut, items, Cadence::DataGated, 20_000);
    out.assert_exactly(items);
}

/// [`assert_lossless`], for a widget whose output type differs from its
/// input type.
///
/// `want` is the full expected output sequence, so this also covers
/// widgets that legitimately emit *fewer* items than they consume — a
/// filter's `want` is simply the surviving subsequence. What it will not
/// tolerate is delivering nothing, which is the point.
pub fn assert_lossless_mapped<W, T, S>(uut: &W, items: &[T], want: &[S])
where
    W: Synchronous + SynchronousIO<I = StreamIO<T, S>, O = StreamIO<S, T>>,
    T: Digital,
    S: Digital + std::fmt::Debug,
{
    let out = drive::<W, T, S>(uut, items, Cadence::DataGated, 20_000);
    out.assert_exactly(want);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::stream_buffer::StreamBuffer;

    fn items(n: u128) -> Vec<b4> {
        (0..n).map(|k| b4(k % 16)).collect()
    }

    /// The fixture drives a real widget losslessly.
    #[test]
    fn stream_buffer_is_lossless_through_the_fixture() {
        let uut = StreamBuffer::<b4>::default();
        assert_lossless(&uut, &items(16));
    }

    /// Every cadence must actually deliver — a fixture whose sink never
    /// accepted would make each of these vacuously "lossless".
    #[test]
    fn every_cadence_delivers() {
        for cadence in [
            Cadence::DataGated,
            Cadence::Periodic(3),
            Cadence::AlwaysReady,
        ] {
            let uut = StreamBuffer::<b4>::default();
            let want = items(12);
            let out = drive::<_, b4, b4>(&uut, &want, cadence, 20_000);
            out.assert_exactly(&want);
        }
    }

    /// `Periodic` really throttles: a rate-limited sink takes strictly
    /// longer than an always-ready one. Without this the cadence could
    /// be ignored and every test above would still pass.
    #[test]
    fn periodic_actually_throttles() {
        fn cycles_for(cadence: Cadence) -> usize {
            let uut = StreamBuffer::<b4>::default();
            let want = items(12);
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
                    let r = cadence.ready(phase, o.data.is_some());
                    if r && o.data.is_some() {
                        got += 1;
                    }
                    let mut input = StreamIO::<b4, b4> {
                        data: None,
                        ready: ready::<b4>(r),
                    };
                    if sent < want.len() && o.ready.raw {
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
            "a 1-in-4 sink must take longer than always-ready: {slow} vs {fast}"
        );
    }

    /// `assert_exactly` rejects a short delivery, not just a wrong one —
    /// the failure mode that lets a dead widget pass.
    #[test]
    #[should_panic(expected = "delivered 0 items, expected 4")]
    fn assert_exactly_rejects_an_empty_delivery() {
        let empty = Delivered::<b4> {
            items: Vec::new(),
            sent: 4,
            timed_out: false,
        };
        empty.assert_exactly(&items(4));
    }

    /// A stalled run is reported as a stall, not a content mismatch.
    #[test]
    #[should_panic(expected = "stalled, not slow")]
    fn assert_exactly_reports_a_timeout_distinctly() {
        let stuck = Delivered::<b4> {
            items: Vec::new(),
            sent: 1,
            timed_out: true,
        };
        stuck.assert_exactly(&items(4));
    }
}
