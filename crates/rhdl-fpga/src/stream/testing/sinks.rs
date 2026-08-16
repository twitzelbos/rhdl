//! Sink behaviours for stream tests.
//!
//! These are for **closed-loop `run_fn` tests**, where the closure is
//! handed the widget's *current output* and returns the input for the
//! next cycle. They are deliberately NOT for
//! [`super::single_stage::single_stage`] — see the harness note below,
//! which is itself the reason a whole class of bug went unfound.
//!
//! *Which* readiness policy a test picks decides what it can find, and
//! the conventional choice has historically been the one that finds
//! least.
//!
//! # Why this module exists
//!
//! `stream::filter` and `stream::filter_map` shipped with a deadlock:
//! a rejected item produced `data = None` downstream, and the widget
//! waited for the sink's `ready` before discarding it. Against a sink
//! that withholds `ready` when there is nothing to take — which the AXI
//! Ready/Valid contract explicitly permits — the rejected item was never
//! consumed and the stream stopped forever.
//!
//! It survived review and a full test suite because the tests used a
//! sink whose readiness was **random and independent of the data**:
//!
//! ```rust,ignore
//! let consume = move |data: Option<b4>| {
//!     if let Some(d) = data { assert!(d.raw() & 1 == 0); }
//!     rand::random::<f64>() > 0.2      // stalls often — but never *because* of absent data
//! };
//! ```
//!
//! That sink stalls constantly and still cannot find the bug. The
//! failure needs `ready` to be withheld *correlated with* data absence,
//! which random stalling never does.
//!
//! # Choosing a sink
//!
//! - [`data_gated`] — the adversarial-but-conforming one. Use it for
//!   any widget that can **drop** or **absorb** items. This is the sink
//!   that finds the bug above.
//! - [`periodic`] — deterministic backpressure at a fixed rate. Use it
//!   for throughput and ordering checks where you want stalls without
//!   correlating them to data.
//! - [`always_ready`] — no backpressure. Necessary for some baselines,
//!   never sufficient on its own for a flow-control widget.
//!
//! All three are deterministic, unlike [`super::utils::stalling`],
//! so tests built on them are reproducible per CLAUDE.md §12 rule 10.
//!
//! # Harness note: `SinkFromFn` cannot express a data-gated sink
//!
//! [`super::sink_from_fn::SinkFromFn`] — which
//! [`super::single_stage::single_stage`] uses — calls its closure with
//!
//! ```rust,ignore
//! (consumer)(if !me.ready { None } else { me.latched_value })
//! ```
//!
//! so the `Option<T>` argument is an **acceptance report** ("here is the
//! item you took"), not an offer ("here is what is available"). A sink
//! built on it therefore *cannot see what is being presented*, and
//! gating readiness on that argument self-deadlocks at once: return
//! `false` once and the argument is `None` forever after.
//!
//! That is not a detail — it is why `stream::filter`'s deadlock was
//! invisible to the shared harness. The only readiness policies
//! `SinkFromFn` can express are ones uncorrelated with data presence,
//! which is precisely the family that cannot find the bug. Tests
//! needing a data-gated sink must drive the widget with `run_fn`
//! directly, as the regression tests in `stream::filter` and
//! `stream::filter_map` do.
//!
//! Teaching `SinkFromFn` to pass the offered value (or adding a sibling
//! that does) would make this class findable through the shared
//! fixture. That is a change to established harness semantics and is
//! left as a follow-up rather than smuggled in here.

use super::sink_from_fn::SinkView;
use rhdl::prelude::Digital;

/// A sink that asserts `ready` **only when it can see data**.
///
/// This is legal under the Ready/Valid contract `stream` implements —
/// AXI permits READY to depend on VALID — and it is the shape that
/// catches a widget which consumes an item, decides not to emit it, and
/// then waits for a downstream that has no reason to respond.
///
/// Reach for this first when testing anything that filters, drops,
/// merges, or otherwise absorbs items.
///
/// `observe` is called with each item actually accepted, in order.
///
/// ```rust,ignore
/// let mut got = Vec::new();
/// let sink = data_gated(|t: b4| got.push(t.raw()));
/// ```
pub fn data_gated<T: Digital>(mut observe: impl FnMut(T)) -> impl FnMut(SinkView<T>) -> bool {
    move |v| {
        // Observe what we actually took...
        if let Some(t) = v.accepted {
            observe(t);
        }
        // ...and gate readiness on what is being offered.  Withholding
        // ready when nothing is on the wire is what exposes a widget
        // that waits for us to absorb an item it never showed us.
        v.offered.is_some()
    }
}

/// A sink that accepts on one cycle in `period`, deterministically.
///
/// Use for ordering and throughput checks where you want real
/// backpressure without tying it to data presence. `period == 1` means
/// always ready.
///
/// `observe` is called with each item actually accepted, in order.
pub fn periodic<T: Digital>(
    period: u32,
    mut observe: impl FnMut(T),
) -> impl FnMut(SinkView<T>) -> bool {
    let mut phase: u32 = 0;
    move |v| {
        if let Some(t) = v.accepted {
            observe(t);
        }
        phase = phase.wrapping_add(1);
        period <= 1 || phase.is_multiple_of(period)
    }
}

/// A sink that is always ready.
///
/// Fine as a baseline, but on its own it cannot exercise any
/// backpressure path — which for a flow-control widget is every path
/// that matters. Pair it with [`data_gated`] or [`periodic`].
pub fn always_ready<T: Digital>(mut observe: impl FnMut(T)) -> impl FnMut(SinkView<T>) -> bool {
    move |v| {
        if let Some(t) = v.accepted {
            observe(t);
        }
        true
    }
}

/// Deterministic source-side stalling: yield `None` on one call in
/// `period`, otherwise pull from `s`.
///
/// The deterministic counterpart to [`super::utils::stalling`], which
/// uses `rand::random` and therefore makes any committed artifact
/// irreproducible.
pub fn stalling_periodic<S>(
    mut s: S,
    period: u32,
) -> impl Iterator<Item = Option<<S as Iterator>::Item>>
where
    S: Iterator,
{
    let mut phase: u32 = 0;
    std::iter::from_fn(move || {
        phase = phase.wrapping_add(1);
        Some(if period > 1 && phase.is_multiple_of(period) {
            None
        } else {
            s.next()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::prelude::*;

    fn view(offered: Option<b4>, accepted: Option<b4>) -> SinkView<b4> {
        SinkView { offered, accepted }
    }

    /// `data_gated` gates readiness on **offered** and observes
    /// **accepted**. Conflating the two is the mistake the `SinkView`
    /// split exists to prevent, so pin both halves.
    #[test]
    fn data_gated_gates_on_offered_and_observes_accepted() {
        let mut seen: Vec<u128> = Vec::new();
        {
            let mut sink = data_gated(|t: b4| seen.push(t.raw()));
            // Nothing offered -> not ready, nothing observed.
            assert!(!sink(view(None, None)), "no offer means not ready");
            // Offered but not yet accepted -> ready, still nothing observed.
            assert!(sink(view(Some(b4(3)), None)), "an offer makes us ready");
            // Now it was accepted: observed exactly once.
            assert!(sink(view(Some(b4(4)), Some(b4(3)))));
            assert!(!sink(view(None, Some(b4(4)))), "offer gone -> not ready");
        }
        assert_eq!(seen, vec![3, 4], "observes accepted items, once each");
    }

    /// A stalled item is *offered* repeatedly but *accepted* once. If a
    /// sink observed offers it would double-count — the exact hazard
    /// that makes `offered` unsafe for sequence checks.
    #[test]
    fn repeated_offers_are_observed_only_once() {
        let mut seen: Vec<u128> = Vec::new();
        {
            let mut sink = periodic(3, |t: b4| seen.push(t.raw()));
            // Same item offered three times, accepted once.
            sink(view(Some(b4(9)), None));
            sink(view(Some(b4(9)), None));
            sink(view(Some(b4(9)), Some(b4(9))));
        }
        assert_eq!(seen, vec![9], "one transfer, one observation");
    }

    /// `periodic` accepts on a fixed cadence irrespective of offers.
    #[test]
    fn periodic_accepts_on_its_cadence() {
        let mut sink = periodic(3, |_t: b4| {});
        assert!(!sink(view(Some(b4(1)), None)));
        assert!(!sink(view(Some(b4(2)), None)));
        assert!(sink(view(Some(b4(3)), None)));
    }

    /// `period <= 1` degenerates to always-ready.
    #[test]
    fn periodic_with_period_one_is_always_ready() {
        let mut sink = periodic(1, |_t: b4| {});
        assert!(sink(view(Some(b4(1)), None)));
        assert!(sink(view(None, None)));
    }

    /// `always_ready` never stalls, even with nothing offered.
    #[test]
    fn always_ready_never_stalls() {
        let mut seen: Vec<u128> = Vec::new();
        {
            let mut sink = always_ready(|t: b4| seen.push(t.raw()));
            assert!(sink(view(None, None)));
            assert!(sink(view(Some(b4(7)), Some(b4(7)))));
        }
        assert_eq!(seen, vec![7]);
    }

    /// `stalling_periodic` inserts a gap on the given cadence and is
    /// reproducible across runs.
    #[test]
    fn stalling_periodic_is_deterministic() {
        let run = || {
            stalling_periodic(0..6u32, 3)
                .take(9)
                .map(|x| x.map(|v| v as i64).unwrap_or(-1))
                .collect::<Vec<_>>()
        };
        let a = run();
        assert_eq!(a, run(), "must be reproducible");
        assert!(a.contains(&-1), "must actually stall");
        let vals: Vec<i64> = a.iter().copied().filter(|v| *v >= 0).collect();
        assert_eq!(vals, vec![0, 1, 2, 3, 4, 5][..vals.len()].to_vec());
    }
}
