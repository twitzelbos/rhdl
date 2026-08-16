//! Relay insertion is behaviour-preserving — the property the whole
//! `rcstream` bus rests on.
//!
//! `stream-bus-architecture.md` §13 requires this and it had never been
//! tested.  Four separate places in the source assert that inserting an
//! [`RCStreamRelay`] anywhere on an `RCStream` connection changes only
//! latency, never behaviour; until now that was Carloni's theorem taken
//! on faith rather than a checked property of *this* implementation.
//!
//! It is also the premise the auto-pipeliner will rely on: RCStream
//! Phase 4 treats every bus boundary as a cut point requiring "no hazard
//! analysis, no functional verification".  That claim needs to be true
//! before a pipeliner is built on top of it, not after.
//!
//! The tests here drive a chain of `N` relays, and a
//! `map -> relays -> filter` pipeline, for a range of `N`, and assert
//! the *delivered item sequence* is identical every time.  Latency
//! shifts; the sequence must not.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::rcstream::{
    bus::Item, filter::RCStreamFilter, map::RCStreamMap, relay::RCStreamRelay, RCStream,
};

/// A chain of `N` relay stations on one `RCStream` connection.
///
/// Data flows forward through the chain; `ready` propagates backward.
/// `N >= 1`.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
struct Chain<const N: usize> {
    relays: [RCStreamRelay<b8, bool>; N],
}

impl<const N: usize> Default for Chain<N> {
    fn default() -> Self {
        Self {
            relays: std::array::from_fn(|_| RCStreamRelay::<b8, bool>::default()),
        }
    }
}

impl<const N: usize> SynchronousIO for Chain<N> {
    type I = RCStream<b8, bool>;
    type O = RCStream<b8, bool>;
    type Kernel = chain_kernel<N>;
}

#[kernel]
#[doc(hidden)]
pub fn chain_kernel<const N: usize>(
    _cr: ClockReset,
    i: RCStream<b8, bool>,
    q: Q<N>,
) -> (RCStream<b8, bool>, D<N>) {
    let mut d = D::<N>::dont_care();

    // Forward: upstream data into relay 0, then relay k-1 -> relay k.
    d.relays[0].data = i.data;
    for k in 1..N {
        d.relays[k].data = q.relays[k - 1].data;
    }

    // Backward: downstream ready into the last relay, then relay k+1's
    // ready-to-upstream becomes relay k's ready-from-downstream.
    d.relays[N - 1].ready = i.ready;
    for k in 0..(N - 1) {
        d.relays[k].ready = q.relays[k + 1].ready;
    }

    let o = RCStream::<b8, bool> {
        data: q.relays[N - 1].data,
        ready: q.relays[0].ready,
    };
    (o, d)
}

/// Drive any `RCStream<b8, bool>` -> `RCStream<b8, bool>` widget with a
/// fixed source and a deterministic backpressuring sink, and return the
/// sequence of items actually delivered.
///
/// The source offers `count` items and only advances when the widget
/// signalled room, so nothing is lost at the source.  The sink accepts
/// on 2 of every 3 cycles.  Both are deterministic: the returned
/// sequence is a pure function of the widget.
fn delivered_sequence<U>(uut: &U, count: u128, cycles: u64) -> Vec<(u128, bool)>
where
    U: Synchronous<I = RCStream<b8, bool>, O = RCStream<b8, bool>>,
{
    let mut to_send: u128 = 0;
    let mut got: Vec<(u128, bool)> = Vec::new();
    let mut need_reset = true;
    let mut phase: u32 = 0;

    uut.run_fn(
        |output| {
            if need_reset {
                need_reset = false;
                return Some(ResetOrData::Reset);
            }
            phase = phase.wrapping_add(1);
            let sink_ready = !phase.is_multiple_of(3);
            if let Some(it) = output.data {
                if sink_ready {
                    got.push((it.data.raw(), it.frame));
                }
            }
            let mut input = RCStream::<b8, bool> {
                data: None,
                ready: sink_ready,
            };
            if to_send < count && output.ready {
                input.data = Some(Item::<b8, bool> {
                    data: b8(to_send % 256),
                    frame: to_send % 8 == 7,
                });
                to_send += 1;
            }
            Some(ResetOrData::Data(input))
        },
        100,
    )
    .take_while(|t| t.time < cycles)
    .for_each(drop);

    got
}

/// The source sequence every configuration must reproduce exactly.
fn expected(count: u128) -> Vec<(u128, bool)> {
    (0..count).map(|k| (k % 256, k % 8 == 7)).collect()
}

/// **The core property.**  A chain of `N` relays delivers exactly the
/// source sequence — no drops, no duplicates, no reordering — for every
/// depth from 1 to 8.
///
/// This is what "insert a relay anywhere, it only costs latency" means
/// operationally.  If any depth diverged, every claim about RCStream
/// being safely pipelinable would be false.
#[test]
fn relay_chain_preserves_the_item_sequence_at_every_depth() {
    const COUNT: u128 = 40;
    let want = expected(COUNT);

    macro_rules! check_depth {
        ($($n:literal),*) => {$({
            let uut = Chain::<$n>::default();
            let got = delivered_sequence(&uut, COUNT, 200_000);
            assert_eq!(
                got, want,
                "a chain of {} relay(s) changed the delivered sequence; \
                 relay insertion must only affect latency",
                $n
            );
        })*};
    }
    check_depth!(1, 2, 3, 4, 5, 6, 7, 8);
}

/// Relay depth must not cost throughput either — the LID claim is
/// "one cycle of latency, throughput unchanged".  All depths deliver
/// the same number of items within the same simulated window.
#[test]
fn relay_depth_does_not_reduce_throughput() {
    const COUNT: u128 = 60;
    // A window long enough for the shallowest chain to finish, so any
    // throughput loss at depth shows up as a short sequence.
    const WINDOW: u64 = 40_000;

    let shallow = delivered_sequence(&Chain::<1>::default(), COUNT, WINDOW).len();
    for (depth, got) in [
        (
            2,
            delivered_sequence(&Chain::<2>::default(), COUNT, WINDOW).len(),
        ),
        (
            4,
            delivered_sequence(&Chain::<4>::default(), COUNT, WINDOW).len(),
        ),
        (
            8,
            delivered_sequence(&Chain::<8>::default(), COUNT, WINDOW).len(),
        ),
    ] {
        assert!(
            got + depth >= shallow,
            "depth {depth} delivered {got} items vs {shallow} at depth 1 in the \
             same window — relay insertion must not cost throughput beyond its \
             one-cycle-per-stage fill"
        );
    }
}

/// A pipeline with real widgets on both sides of the inserted relays:
/// `map -> N relays -> filter`.  The observable output must be
/// independent of `N`.
///
/// This is the §13 property as stated — insertion *on a connection
/// between widgets*, not just a bare chain.
mod pipeline {
    use super::*;

    #[kernel]
    fn double(_cr: ClockReset, t: b8) -> b8 {
        (t << 1) & b8(0xFF)
    }

    #[kernel]
    fn keep_even(_cr: ClockReset, it: Item<b8, bool>) -> bool {
        it.frame || ((it.data & b8(2)) == b8(0))
    }

    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    struct Pipe<const N: usize> {
        map: RCStreamMap<b8, bool, b8>,
        chain: Chain<N>,
        filter: RCStreamFilter<b8, bool>,
    }

    impl<const N: usize> Pipe<N> {
        fn try_new() -> Result<Self, RHDLError> {
            Ok(Self {
                map: RCStreamMap::try_new::<double>()?,
                chain: Chain::<N>::default(),
                filter: RCStreamFilter::try_new::<keep_even>()?,
            })
        }
    }

    impl<const N: usize> SynchronousIO for Pipe<N> {
        type I = RCStream<b8, bool>;
        type O = RCStream<b8, bool>;
        type Kernel = pipe_kernel<N>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn pipe_kernel<const N: usize>(
        _cr: ClockReset,
        i: RCStream<b8, bool>,
        q: Q<N>,
    ) -> (RCStream<b8, bool>, D<N>) {
        let mut d = D::<N>::dont_care();
        // Forward data: in -> map -> chain -> filter -> out
        d.map.data = i.data;
        d.chain.data = q.map.data;
        d.filter.data = q.chain.data;
        // Backward ready: out -> filter -> chain -> map -> in
        d.filter.ready = i.ready;
        d.chain.ready = q.filter.ready;
        d.map.ready = q.chain.ready;
        let o = RCStream::<b8, bool> {
            data: q.filter.data,
            ready: q.map.ready,
        };
        (o, d)
    }

    /// Inserting relays between `map` and `filter` must not change what
    /// the pipeline computes.
    #[test]
    fn pipeline_output_is_independent_of_inserted_relay_count() -> Result<(), RHDLError> {
        const COUNT: u128 = 40;

        let baseline = delivered_sequence(&Pipe::<1>::try_new()?, COUNT, 300_000);
        assert!(
            !baseline.is_empty(),
            "the pipeline produced nothing — the test would be vacuous"
        );

        macro_rules! check_depth {
            ($($n:literal),*) => {$({
                let got = delivered_sequence(&Pipe::<$n>::try_new()?, COUNT, 300_000);
                assert_eq!(
                    got, baseline,
                    "inserting {} relays between map and filter changed the \
                     pipeline's output; LID insertion must be transparent",
                    $n
                );
            })*};
        }
        check_depth!(2, 3, 4, 5);
        Ok(())
    }

    /// The pipeline actually computes what the two widgets say it does,
    /// so the invariance test above is anchored to real behaviour rather
    /// than to a uniformly-broken pipeline.
    #[test]
    fn pipeline_computes_the_expected_function() -> Result<(), RHDLError> {
        const COUNT: u128 = 40;
        let got = delivered_sequence(&Pipe::<1>::try_new()?, COUNT, 300_000);
        let want: Vec<(u128, bool)> = (0..COUNT)
            .map(|k| ((k << 1) & 0xFF, k % 8 == 7))
            .filter(|(d, f)| *f || (d & 2) == 0)
            .collect();
        assert_eq!(
            got, want,
            "map -> relays -> filter must compute double-then-keep-even"
        );
        Ok(())
    }
}

/// Multi-port insertion: relays on **one input of a `zip`**.
///
/// This is the case a single in/out pair cannot exercise. Delaying one
/// side of a zip relative to the other changes their *relative* arrival
/// timing, so if the lockstep logic were wrong the pairs would shear —
/// `a[k]` matched with `b[k+1]` or worse. Correct behaviour is that
/// pairing is by index and completely insensitive to how much one side
/// is delayed.
mod zip_ports {
    use super::*;
    use rhdl_fpga::rcstream::zip::{In as ZipIn, RCStreamZip};

    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    struct ZipDelayedA<const N: usize> {
        a_chain: Chain<N>,
        zip: RCStreamZip<b8, bool, b8, bool>,
    }

    impl<const N: usize> Default for ZipDelayedA<N> {
        fn default() -> Self {
            Self {
                a_chain: Chain::<N>::default(),
                zip: RCStreamZip::default(),
            }
        }
    }

    #[derive(PartialEq, Debug, Digital, Clone, Copy)]
    pub struct In {
        pub a_data: Option<Item<b8, bool>>,
        pub b_data: Option<Item<b8, bool>>,
        pub ready: bool,
    }

    #[derive(PartialEq, Debug, Digital, Clone, Copy)]
    pub struct Out {
        pub a_ready: bool,
        pub b_ready: bool,
        pub data: Option<Item<(b8, b8), (bool, bool)>>,
    }

    impl<const N: usize> SynchronousIO for ZipDelayedA<N> {
        type I = In;
        type O = Out;
        type Kernel = zip_delayed_kernel<N>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn zip_delayed_kernel<const N: usize>(_cr: ClockReset, i: In, q: Q<N>) -> (Out, D<N>) {
        let mut d = D::<N>::dont_care();
        // `a` goes through the relay chain; `b` goes straight in.
        d.a_chain.data = i.a_data;
        d.a_chain.ready = q.zip.a_ready;
        d.zip = ZipIn::<b8, bool, b8, bool> {
            a_data: q.a_chain.data,
            b_data: i.b_data,
            ready: i.ready,
        };
        let o = Out {
            a_ready: q.a_chain.ready,
            b_ready: q.zip.b_ready,
            data: q.zip.data,
        };
        (o, d)
    }

    fn pairs<const N: usize>(count: u128) -> Vec<(u128, u128)> {
        let uut = ZipDelayedA::<N>::default();
        let (mut a_sent, mut b_sent) = (0u128, 0u128);
        let mut got: Vec<(u128, u128)> = Vec::new();
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let sink_ready = !phase.is_multiple_of(3);
                if let Some(it) = output.data {
                    if sink_ready {
                        got.push((it.data.0.raw(), it.data.1.raw()));
                    }
                }
                let mut input = In {
                    a_data: None,
                    b_data: None,
                    ready: sink_ready,
                };
                if a_sent < count && output.a_ready {
                    input.a_data = Some(Item::<b8, bool> {
                        data: b8(a_sent % 256),
                        frame: false,
                    });
                    a_sent += 1;
                }
                // `b` offers on a different cadence, so the two sides are
                // already skewed before the relays add more.
                if b_sent < count && output.b_ready && !phase.is_multiple_of(2) {
                    input.b_data = Some(Item::<b8, bool> {
                        data: b8((100 + b_sent) % 256),
                        frame: false,
                    });
                    b_sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 300_000)
        .for_each(drop);
        got
    }

    #[test]
    fn zip_pairs_by_index_regardless_of_input_delay() {
        const COUNT: u128 = 20;
        let want: Vec<(u128, u128)> = (0..COUNT).map(|k| (k, (100 + k) % 256)).collect();
        for (label, got) in [
            ("1 relay on a", pairs::<1>(COUNT)),
            ("2 relays on a", pairs::<2>(COUNT)),
            ("4 relays on a", pairs::<4>(COUNT)),
            ("7 relays on a", pairs::<7>(COUNT)),
        ] {
            assert_eq!(
                got, want,
                "{label}: zip must pair by index; delaying one input must not shear the pairing"
            );
        }
    }
}

/// Multi-port insertion, the other direction: relays on **one output of
/// a `tee`**.
///
/// The mirror of the zip case. Delaying one branch relative to the other
/// must not let the two outputs drift apart — both halves are still
/// index-aligned, which is what makes a later `zip` able to recombine
/// them.
mod tee_ports {
    use super::*;
    use rhdl_fpga::rcstream::tee::{In as TeeIn, RCStreamTee};

    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    struct TeeDelayedA<const N: usize> {
        tee: RCStreamTee<b8, bool, b8, bool>,
        a_chain: Chain<N>,
    }

    impl<const N: usize> Default for TeeDelayedA<N> {
        fn default() -> Self {
            Self {
                tee: RCStreamTee::default(),
                a_chain: Chain::<N>::default(),
            }
        }
    }

    #[derive(PartialEq, Debug, Digital, Clone, Copy)]
    pub struct In {
        pub data: Option<Item<(b8, b8), (bool, bool)>>,
        pub a_ready: bool,
        pub b_ready: bool,
    }

    #[derive(PartialEq, Debug, Digital, Clone, Copy)]
    pub struct Out {
        pub ready: bool,
        pub a_data: Option<Item<b8, bool>>,
        pub b_data: Option<Item<b8, bool>>,
    }

    impl<const N: usize> SynchronousIO for TeeDelayedA<N> {
        type I = In;
        type O = Out;
        type Kernel = tee_delayed_kernel<N>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn tee_delayed_kernel<const N: usize>(_cr: ClockReset, i: In, q: Q<N>) -> (Out, D<N>) {
        let mut d = D::<N>::dont_care();
        d.tee = TeeIn::<b8, bool, b8, bool> {
            data: i.data,
            a_ready: q.a_chain.ready,
            b_ready: i.b_ready,
        };
        // The `a` branch runs through the relay chain.
        d.a_chain.data = q.tee.a_data;
        d.a_chain.ready = i.a_ready;
        let o = Out {
            ready: q.tee.ready,
            a_data: q.a_chain.data,
            b_data: q.tee.b_data,
        };
        (o, d)
    }

    fn halves<const N: usize>(count: u128) -> (Vec<u128>, Vec<u128>) {
        let uut = TeeDelayedA::<N>::default();
        let mut sent: u128 = 0;
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let mut need_reset = true;
        let mut phase: u32 = 0;

        uut.run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let a_ready = !phase.is_multiple_of(5);
                let b_ready = phase.is_multiple_of(2);
                if let Some(it) = output.a_data {
                    if a_ready {
                        a.push(it.data.raw());
                    }
                }
                if let Some(it) = output.b_data {
                    if b_ready {
                        b.push(it.data.raw());
                    }
                }
                let mut input = In {
                    data: None,
                    a_ready,
                    b_ready,
                };
                if sent < count && output.ready {
                    input.data = Some(Item::<(b8, b8), (bool, bool)> {
                        data: (b8(sent % 256), b8((100 + sent) % 256)),
                        frame: (false, false),
                    });
                    sent += 1;
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 400_000)
        .for_each(drop);
        (a, b)
    }

    #[test]
    fn tee_halves_stay_aligned_regardless_of_output_delay() {
        const COUNT: u128 = 16;
        let want_a: Vec<u128> = (0..COUNT).collect();
        let want_b: Vec<u128> = (0..COUNT).map(|k| (100 + k) % 256).collect();
        for (label, (a, b)) in [
            ("1 relay on a", halves::<1>(COUNT)),
            ("3 relays on a", halves::<3>(COUNT)),
            ("6 relays on a", halves::<6>(COUNT)),
        ] {
            assert_eq!(
                a, want_a,
                "{label}: delayed branch must still be complete and in order"
            );
            assert_eq!(b, want_b, "{label}: undelayed branch must be unaffected");
        }
    }
}
