//! Test Sink From Function
//!
//!# Purpose
//!
//! For testing stream processes, it's often handy to have a
//! sink for a stream that can be generated from a closure without
//! worrying that something that is synthesizable.  
//!
use rhdl::{
    core::{ScopedName, SyncKind},
    prelude::*,
};

use crate::stream::{Ready, ready};

/// What a [`SinkFromFn`] closure is told each cycle.
///
/// Two distinct facts, deliberately not conflated:
///
/// - `offered` — what the upstream is **presenting**. Decide `ready`
///   from this. A sink may legitimately withhold `ready` when nothing
///   is offered (AXI permits READY to depend on VALID), and a widget
///   that consumes an item, declines to emit it, and then waits for a
///   downstream that has no reason to respond will **deadlock** against
///   such a sink. That is a real bug shape — it shipped in
///   `stream::filter` — and it is unreachable unless the sink can see
///   what is on the wire.
/// - `accepted` — what this sink actually **took** last cycle. Observe,
///   count, and check sequences against this. It is `Some` exactly once
///   per transferred item, so it is safe to pop an expected-value
///   iterator here; `offered` is *not*, because a stalled item is
///   presented repeatedly.
///
/// Using the wrong field is the easy mistake: gate on `offered`,
/// observe on `accepted`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SinkView<T: Digital> {
    /// What the upstream is presenting. Use for readiness decisions.
    pub offered: Option<T>,
    /// What this sink took last cycle. Use for observation.
    pub accepted: Option<T>,
}

#[derive(Clone)]
/// The [SinkFromFn] core
///
/// This is the core to include in your design if you want to  
/// use a closure or other general Rust function to assess the
/// correctness of the stream output.  It can also control the
/// backpressure to the stream, by returning a boolean that
/// is converted into the `ready` input.  
pub struct SinkFromFn<T: Digital> {
    consumer: std::sync::Arc<std::sync::Mutex<dyn FnMut(SinkView<T>) -> bool>>,
    /// Optional *combinational* readiness policy.
    ///
    /// When present, `ready` is computed from the offer **in the same
    /// cycle** rather than being registered from the previous one, and
    /// `consumer` is used only to observe accepted items.
    ///
    /// This matters more than it sounds. With the default registered
    /// ready there is a one-cycle lag between "nothing is offered" and
    /// "ready drops" — and that lag is exactly enough slack for a widget
    /// which waits on `ready` to consume an item it never presented.
    /// `stream::filter`'s deadlock is invisible to a registered
    /// data-gated sink and visible to a combinational one. If you are
    /// testing a widget that can absorb items, you want this.
    ready_fn: Option<std::sync::Arc<dyn Fn(Option<T>) -> bool>>,
}

impl<T: Digital> SinkFromFn<T> {
    /// Create a new [SinkFromFn] object from the given function
    ///
    /// The function is `fn(SinkView<T>) -> bool`. The return value is
    /// **not** an acceptance flag for anything in the argument — it is
    /// the `ready` signal presented to the stage upstream for the coming
    /// cycle. See [`SinkView`] for which of its two fields to use for
    /// which purpose: gate on `offered`, observe on `accepted`.
    pub fn new<S: FnMut(SinkView<T>) -> bool + 'static>(consumer: S) -> Self {
        Self {
            consumer: std::sync::Arc::new(std::sync::Mutex::new(consumer)),
            ready_fn: None,
        }
    }

    /// Create a [SinkFromFn] whose `ready` is **combinational** on the
    /// current offer.
    ///
    /// `ready_fn` must be pure — it is evaluated whenever the simulator
    /// asks for the output, possibly several times per cycle. `observe`
    /// is called exactly once per clock, with the item actually
    /// transferred (`None` when nothing was).
    ///
    /// Use this for any widget that can absorb or drop items. The
    /// default [`Self::new`] registers `ready`, and that one-cycle lag
    /// masks deadlocks of the `stream::filter` kind.
    pub fn new_combinational(
        ready_fn: impl Fn(Option<T>) -> bool + 'static,
        mut observe: impl FnMut(Option<T>) + 'static,
    ) -> Self {
        Self {
            consumer: std::sync::Arc::new(std::sync::Mutex::new(move |v: SinkView<T>| {
                observe(v.accepted);
                // Unused when `ready_fn` is present.
                true
            })),
            ready_fn: Some(std::sync::Arc::new(ready_fn)),
        }
    }
}

impl<T: Digital + std::fmt::Debug> SinkFromFn<T> {
    /// Create a new [SinkFromFn] object from the given iterator
    ///
    /// This constructor will create a sink that expects each item from the
    /// sink to match an item from the generated iterator.  Acceptance is
    /// throttled at roughly `1 - stall_probability`.
    ///
    /// The throttling is **deterministic** — a seeded generator, not
    /// `rand::random`. Several examples build their committed trace
    /// through this constructor, and those traces run as doctests, so a
    /// random draw here made `cargo test` rewrite checked-in artifacts on
    /// every run.
    ///
    /// The seed is derived from `stall_probability`, so two sinks
    /// throttled at different rates decorrelate on their own. Two sinks
    /// at the *same* rate do not — use [`Self::new_from_iter_with_seed`]
    /// there.
    pub fn new_from_iter<S: Iterator<Item = T> + 'static>(iter: S, stall_probability: f32) -> Self {
        let seed = super::utils::seed_for(f64::from(stall_probability));
        Self::new_from_iter_with_seed(iter, stall_probability, seed)
    }

    /// [`Self::new_from_iter`], with the generator seed given explicitly.
    ///
    /// Needed whenever one test builds **two sinks in the same run**.
    /// Sharing a seed makes them stall on identical cycles, and a
    /// request/response pair that stalls in lockstep never exercises the
    /// case where one side is blocked while the other flows — which is
    /// the case worth testing. Differing probabilities are not enough on
    /// their own: drawing from one sequence against two thresholds nests
    /// one sink's ready-set inside the other's rather than making them
    /// independent.
    pub fn new_from_iter_with_seed<S: Iterator<Item = T> + 'static>(
        mut iter: S,
        stall_probability: f32,
        seed: u32,
    ) -> Self {
        let mut det = crate::doc::DetRng::new(seed);
        let func = move |v: SinkView<T>| {
            // Check against ACCEPTED, never offered: a stalled item is
            // presented repeatedly and would pop the iterator more than
            // once.
            if let Some(res) = v.accepted {
                let y = iter.next().unwrap();
                assert_eq!(res, y);
            }
            det.chance(((1.0 - stall_probability) * 100.0) as u32)
        };
        Self::new(func)
    }
}

impl<T> SynchronousIO for SinkFromFn<T>
where
    T: Digital,
{
    // Data signal
    type I = Option<T>;
    // Ready signal
    type O = Ready<T>;
    type Kernel = NoSynchronousKernel<ClockReset, Option<T>, (), (Ready<T>, ())>;
}

impl<T> SynchronousDQ for SinkFromFn<T>
where
    T: Digital,
{
    type D = ();
    type Q = ();
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Init,
    Run,
}

#[derive(Clone, PartialEq)]
#[doc(hidden)]
pub struct SinkFromFnState<T: Digital> {
    state: State,
    latched_value: Option<T>,
    prev_clock: Clock,
    ready: bool,
}

impl<T: Digital> Synchronous for SinkFromFn<T> {
    type S = SinkFromFnState<T>;

    fn init(&self) -> Self::S {
        SinkFromFnState {
            state: State::Init,
            latched_value: None,
            prev_clock: clock(false),
            ready: false,
        }
    }

    fn sim(&self, clock_reset: ClockReset, input: Self::I, me: &mut Self::S) -> Self::O {
        trace_push_path("sink_from_fn");
        trace("input", &input);
        let pos_edge = clock_reset.clock.raw() && !me.prev_clock.raw();
        // With a combinational policy, whether the latched offer was
        // taken is decided by the same function that drives `ready`, so
        // the accept-report stays exact.
        let accepted_now = |me: &SinkFromFnState<T>| match &self.ready_fn {
            Some(f) => match me.latched_value {
                Some(v) if f(Some(v)) => Some(v),
                _ => None,
            },
            None => {
                if me.ready {
                    me.latched_value
                } else {
                    None
                }
            }
        };
        let process = || {
            let mut consumer = self.consumer.lock().unwrap();
            (consumer)(SinkView {
                offered: me.latched_value,
                accepted: accepted_now(me),
            })
        };
        match me.state {
            State::Init => {
                if !clock_reset.reset.any() && clock_reset.clock.raw() {
                    me.ready = process();
                    me.state = State::Run;
                }
            }
            State::Run => {
                if pos_edge {
                    me.ready = process();
                }
            }
        }
        if !clock_reset.clock.raw() {
            me.latched_value = input;
        }
        me.prev_clock = clock_reset.clock;
        // Combinational policies recompute from the live input; the
        // default registers the value decided at the last edge.
        let out = match &self.ready_fn {
            Some(f) => f(input),
            None => me.ready,
        };
        trace("output", &out);
        trace_pop_path();
        ready(out)
    }

    fn descriptor(&self, _name: ScopedName) -> Result<Descriptor<SyncKind>, RHDLError> {
        Err(RHDLError::NotSynthesizable)
    }
}
