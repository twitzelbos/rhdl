//! Relay insertion on **one output branch of a diverging widget**.
//!
//! `tests/rcstream_relay_insertion.rs` establishes the bus's central
//! property — insert a relay anywhere, only latency changes — and
//! covers the hard *converging* case: a `zip` with one input delayed,
//! where getting it wrong shears the pairing.
//!
//! This is the mirror case, and it is the one where the claim is least
//! obviously true. A diverging widget must hand the same item to every
//! branch, and `fanout` carries per-branch "already delivered" state to
//! avoid handing it to one branch twice. Put a relay on one branch and
//! that branch's ready now arrives later than the other's, so the state
//! is exercised in a way a symmetric test never reaches. The failure
//! modes are worse than shearing, too: a duplicate item, or a deadlock
//! where the delayed branch's backpressure stops the undelayed one for
//! ever.
//!
//! The assertion is that **every branch receives the whole sequence,
//! in order, exactly once, at every insertion depth** — and that it
//! finishes at all, which is what rules out the deadlock.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::rcstream::bus::{Item, RCStream};
use rhdl_fpga::rcstream::fanout::{In as FanoutIn, RCStreamFanout};
use rhdl_fpga::rcstream::relay::RCStreamRelay;

/// The shared interface of both fanout fixtures: one upstream data
/// signal and one ready per branch. At the top level so both modules
/// below can name it.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct In {
    pub data: Option<Item<b8, bool>>,
    pub a_ready: bool,
    pub b_ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct Out {
    pub ready: bool,
    pub a_data: Option<Item<b8, bool>>,
    pub b_data: Option<Item<b8, bool>>,
}

/// Each widget lives in its own module: `#[rhdl(dq_no_prefix)]`
/// generates unprefixed `Q` and `D` types, so two widgets in one module
/// collide on them.
mod chain {
    use super::*;

    /// A chain of `N` relay stations on one `RCStream` connection.
    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    pub struct Chain<const N: usize> {
        stages: [RCStreamRelay<b8, bool>; N],
    }

    impl<const N: usize> Default for Chain<N> {
        fn default() -> Self {
            Self {
                stages: std::array::from_fn(|_| RCStreamRelay::default()),
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
        // Data forward through the chain, ready backward.
        for k in 0..N {
            d.stages[k].data = if k == 0 { i.data } else { q.stages[k - 1].data };
            d.stages[k].ready = if k == N - 1 {
                i.ready
            } else {
                q.stages[k + 1].ready
            };
        }
        let o = RCStream::<b8, bool> {
            data: q.stages[N - 1].data,
            ready: q.stages[0].ready,
        };
        (o, d)
    }
}

mod delayed {
    use super::chain::Chain;
    use super::*;

    /// `fanout` with a chain of `N` relays on branch 0 only.
    #[derive(Clone, Synchronous, SynchronousDQ)]
    #[rhdl(dq_no_prefix)]
    pub struct FanoutDelayed<const N: usize> {
        fanout: RCStreamFanout<b8, bool, 2>,
        chain: Chain<N>,
    }

    impl<const N: usize> Default for FanoutDelayed<N> {
        fn default() -> Self {
            Self {
                fanout: RCStreamFanout::default(),
                chain: Chain::<N>::default(),
            }
        }
    }

    impl<const N: usize> SynchronousIO for FanoutDelayed<N> {
        type I = In;
        type O = Out;
        type Kernel = fanout_delayed_kernel<N>;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn fanout_delayed_kernel<const N: usize>(_cr: ClockReset, i: In, q: Q<N>) -> (Out, D<N>) {
        let mut d = D::<N>::dont_care();

        let mut fi = FanoutIn::<b8, bool, 2>::dont_care();
        fi.data = i.data;
        // Branch 0 goes through the chain, so its ready comes from the
        // chain's upstream face. Branch 1 is wired straight to its sink.
        fi.ready = [q.chain.ready, i.b_ready];
        d.fanout = fi;

        d.chain.data = q.fanout.data[0];
        d.chain.ready = i.a_ready;

        let mut o = Out::dont_care();
        o.ready = q.fanout.ready;
        o.a_data = q.chain.data;
        o.b_data = q.fanout.data[1];
        (o, d)
    }
}

mod bare {
    use super::*;

    /// `fanout` with nothing inserted — the baseline every depth must match.
    #[derive(Clone, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct FanoutBare {
        fanout: RCStreamFanout<b8, bool, 2>,
    }

    impl SynchronousIO for FanoutBare {
        type I = In;
        type O = Out;
        type Kernel = fanout_bare_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn fanout_bare_kernel(_cr: ClockReset, i: In, q: Q) -> (Out, D) {
        let mut d = D::dont_care();
        let mut fi = FanoutIn::<b8, bool, 2>::dont_care();
        fi.data = i.data;
        fi.ready = [i.a_ready, i.b_ready];
        d.fanout = fi;

        let mut o = Out::dont_care();
        o.ready = q.fanout.ready;
        o.a_data = q.fanout.data[0];
        o.b_data = q.fanout.data[1];
        (o, d)
    }
}

use bare::FanoutBare;
use delayed::FanoutDelayed;

/// What each branch received.
#[derive(Debug, PartialEq)]
struct Delivered {
    a: Vec<(u128, bool)>,
    b: Vec<(u128, bool)>,
    /// The cycle branch A's first item was taken on. Used only to prove
    /// the inserted relays are really in the path -- see
    /// `depth_delays_branch_a_without_changing_what_it_delivers`.
    a_first: Option<u32>,
}

/// Drive `uut` with a source offering `count` items and two sinks with
/// *different* backpressure, and collect what each branch received.
///
/// The two sinks accept on coprime cycle counts on purpose. A symmetric
/// pair would advance the fanout's per-branch delivered state in
/// lockstep and never exercise the case it exists for; 1-in-2 against
/// 2-in-3 produces every combination of the two readys.
///
/// Closed-loop in the same shape as `rcstream_relay_insertion.rs`: the
/// source only offers an item when the widget said it had room, so
/// nothing is lost before the fanout, and an item is recorded on the
/// cycle its sink asserts ready -- which is when the held item is
/// actually taken.
///
/// `time_limit` is in simulation *time*, not cycles: the clock period
/// here is 100, so a hundred cycles is 10_000. Getting that wrong is
/// how this harness first reported the undelayed baseline delivering
/// seven items of twenty-four.
fn delivered<U>(uut: &U, count: u128, time_limit: u64) -> Delivered
where
    U: Synchronous<I = In, O = Out>,
{
    let mut to_send: u128 = 0;
    let mut got = Delivered {
        a: Vec::new(),
        b: Vec::new(),
        a_first: None,
    };
    let mut need_reset = true;
    let mut phase: u32 = 0;

    uut.run_fn(
        |output| {
            if need_reset {
                need_reset = false;
                return Some(ResetOrData::Reset);
            }
            phase = phase.wrapping_add(1);
            let a_ready = phase.is_multiple_of(2);
            let b_ready = !phase.is_multiple_of(3);

            if let Some(it) = output.a_data {
                if a_ready {
                    if got.a_first.is_none() {
                        got.a_first = Some(phase);
                    }
                    got.a.push((it.data.raw(), it.frame));
                }
            }
            if let Some(it) = output.b_data {
                if b_ready {
                    got.b.push((it.data.raw(), it.frame));
                }
            }

            let mut input = In {
                data: None,
                a_ready,
                b_ready,
            };
            if to_send < count && output.ready {
                input.data = Some(Item::<b8, bool> {
                    data: b8(to_send % 256),
                    frame: to_send + 1 == count,
                });
                to_send += 1;
            }
            Some(ResetOrData::Data(input))
        },
        100,
    )
    .take_while(|t| t.time < time_limit)
    .for_each(drop);

    got
}

fn expected(count: u128) -> Vec<(u128, bool)> {
    (0..count).map(|k| (k % 256, k + 1 == count)).collect()
}

/// **The property.** Every branch receives the whole sequence, in
/// order, exactly once — at every insertion depth, and identically to
/// the undelayed baseline.
#[test]
fn a_relay_on_one_branch_does_not_disturb_either_branch() {
    const COUNT: u128 = 24;
    // Simulation time, not cycles -- the period is 100.
    const TIME: u64 = 200_000;
    let want = expected(COUNT);

    let base = delivered(&FanoutBare::default(), COUNT, TIME);
    assert_eq!(base.a, want, "baseline: branch A");
    assert_eq!(base.b, want, "baseline: branch B");

    macro_rules! at_depth {
        ($($n:literal),*) => {$({
            let got = delivered(&FanoutDelayed::<$n>::default(), COUNT, TIME);
            assert_eq!(
                got.a, want,
                "depth {}: branch A (the delayed one) must receive the \
                 whole sequence in order, exactly once",
                $n
            );
            assert_eq!(
                got.b, want,
                "depth {}: branch B must be unaffected by a relay on A",
                $n
            );
        })*};
    }
    at_depth!(1, 2, 3, 4, 8);
}

/// **No duplicates, stated separately.** The per-branch delivered state
/// exists precisely to stop an item being handed to one branch twice
/// when the two branches become ready on different cycles — which is
/// what inserting a relay on one of them guarantees. Equality with the
/// expected sequence already implies it; this says so by name, so a
/// failure reads as "duplicated" rather than "sequence differs".
#[test]
fn no_branch_receives_an_item_twice() {
    const COUNT: u128 = 24;
    let got = delivered(&FanoutDelayed::<3>::default(), COUNT, 200_000);
    for (label, seq) in [("A", &got.a), ("B", &got.b)] {
        let mut sorted: Vec<u128> = seq.iter().map(|(v, _)| *v).collect();
        let len = sorted.len();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            len,
            "branch {label} received a duplicate: {seq:?}"
        );
        assert_eq!(len, COUNT as usize, "branch {label} is short");
    }
}

/// **It terminates.** The deadlock this topology could produce is the
/// delayed branch's backpressure stopping the undelayed one for ever,
/// and a test that only compared sequences would report that as "short"
/// rather than as the hang it is. Asserting completion inside a bounded
/// window names it.
#[test]
fn a_relay_on_one_branch_does_not_deadlock() {
    const COUNT: u128 = 16;
    // Generous but finite, in simulation time: 16 items through four
    // relays with sinks accepting on half and two thirds of cycles
    // needs nothing like this long.
    const TIME: u64 = 200_000;
    let got = delivered(&FanoutDelayed::<4>::default(), COUNT, TIME);
    assert_eq!(
        got.a.len(),
        COUNT as usize,
        "branch A did not drain within {TIME} of simulation time -- the \
         delayed branch is stuck"
    );
    assert_eq!(
        got.b.len(),
        COUNT as usize,
        "branch B did not drain within {TIME} of simulation time -- the \
         delayed branch's backpressure is blocking the undelayed one"
    );
}

/// **Proof the relays are actually in the path.**
///
/// Every assertion above is of the form "the sequence is unchanged",
/// and all of them would pass just as well if the inserted chain were
/// somehow bypassed — which is how an insertion-invariance test rots
/// into a tautology. Latency is the observable that *must* change: a
/// deeper chain delays branch A's first delivery, while the sequence it
/// eventually delivers stays identical.
#[test]
fn depth_delays_branch_a_without_changing_what_it_delivers() {
    const COUNT: u128 = 24;
    const TIME: u64 = 200_000;
    let want = expected(COUNT);

    let shallow = delivered(&FanoutDelayed::<1>::default(), COUNT, TIME);
    let deep = delivered(&FanoutDelayed::<8>::default(), COUNT, TIME);

    let s_first = shallow.a_first.expect("depth 1 delivered something");
    let d_first = deep.a_first.expect("depth 8 delivered something");
    assert!(
        d_first > s_first,
        "eight relays must delay branch A's first delivery relative to \
         one (got cycle {d_first} vs {s_first}). If these are equal the \
         chain is not in the path and every other assertion in this \
         file is vacuous."
    );

    // And the thing that must not change, did not.
    assert_eq!(shallow.a, want, "depth 1 sequence");
    assert_eq!(deep.a, want, "depth 8 sequence");
    assert_eq!(
        shallow.b, deep.b,
        "branch B is downstream of no relay, so its delivery must be \
         identical at both depths"
    );
}
