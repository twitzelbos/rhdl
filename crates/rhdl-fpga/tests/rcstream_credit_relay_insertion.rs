//! Credit-relay insertion: correct at any depth, but not free.
//!
//! The companion to `rcstream_relay_insertion.rs`.  That file proves
//! `RCStreamRelay` insertion preserves both the item sequence *and*
//! throughput, which is Carloni's theorem operationalised.
//!
//! The credit variant is different, and the difference matters.  Credit
//! flow control sustains full rate only while
//! `credits >= round-trip latency`, and every inserted relay adds two
//! cycles to that round trip — one forward on `data`, one back on
//! `credit_grant`.  So insertion stays **correct** at any depth but can
//! **cost throughput** when the credit pool is too small to cover the
//! longer loop.
//!
//! These tests pin down both halves of that, so the claim in the
//! `credit::relay` module docs is a checked property rather than an
//! assertion.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::rcstream::{
    bus::Item,
    credit::{relay::CreditRCStreamRelay, sink::CreditSink, source::CreditSource},
};

const CW: usize = 5;

/// `CreditSource -> N credit relays -> CreditSink`, presented as an
/// ordinary `RCStream`-style widget at both ends.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
struct CreditPipe<const N: usize, const FIFO_N: usize>
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    source: CreditSource<b8, (), CW>,
    relays: [CreditRCStreamRelay<b8, (), CW>; N],
    sink: CreditSink<b8, (), CW, FIFO_N>,
}

impl<const N: usize, const FIFO_N: usize> Default for CreditPipe<N, FIFO_N>
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    fn default() -> Self {
        Self {
            source: CreditSource::<b8, (), CW>::default(),
            relays: std::array::from_fn(|_| CreditRCStreamRelay::<b8, (), CW>::default()),
            sink: CreditSink::<b8, (), CW, FIFO_N>::default(),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct PipeIn {
    pub data: Option<Item<b8, ()>>,
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct PipeOut {
    pub ready: bool,
    pub data: Option<Item<b8, ()>>,
}

impl<const N: usize, const FIFO_N: usize> SynchronousIO for CreditPipe<N, FIFO_N>
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    type I = PipeIn;
    type O = PipeOut;
    type Kernel = pipe_kernel<N, FIFO_N>;
}

#[kernel]
#[doc(hidden)]
pub fn pipe_kernel<const N: usize, const FIFO_N: usize>(
    _cr: ClockReset,
    i: PipeIn,
    q: Q<N, FIFO_N>,
) -> (PipeOut, D<N, FIFO_N>)
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    let mut d = D::<N, FIFO_N>::dont_care();

    // Upstream RCStream face into the credit source.
    d.source.upstream_data = i.data;

    // Data travels source -> relay 0 -> .. -> relay N-1 -> sink.
    d.relays[0].data = q.source.downstream_data;
    for k in 1..N {
        d.relays[k].data = q.relays[k - 1].data;
    }
    d.sink.upstream_data = q.relays[N - 1].data;

    // Credit travels the other way: sink -> relay N-1 -> .. -> relay 0
    // -> source.  Every grant must survive the trip intact.
    d.relays[N - 1].credit_grant = q.sink.credit_grant;
    for k in 0..(N - 1) {
        d.relays[k].credit_grant = q.relays[k + 1].credit_grant;
    }
    d.source.credit_grant = q.relays[0].credit_grant;

    // Downstream RCStream face out of the credit sink.
    d.sink.downstream_ready = i.ready;

    let o = PipeOut {
        ready: q.source.upstream_ready,
        data: q.sink.downstream_data,
    };
    (o, d)
}

/// Drive a pipe with an always-offering source and an always-ready
/// sink, and return the items delivered within `cycles`.
///
/// Both ends are maximally permissive so that the only thing limiting
/// the rate is the credit loop itself — which is exactly what we want
/// to measure.
fn run_pipe<const N: usize, const FIFO_N: usize>(count: u128, cycles: u64) -> Vec<u128>
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    let uut = CreditPipe::<N, FIFO_N>::default();
    let mut to_send: u128 = 0;
    let mut got: Vec<u128> = Vec::new();
    let mut need_reset = true;

    uut.run_fn(
        |output| {
            if need_reset {
                need_reset = false;
                return Some(ResetOrData::Reset);
            }
            if let Some(it) = output.data {
                got.push(it.data.raw());
            }
            let mut input = PipeIn {
                data: None,
                ready: true,
            };
            if to_send < count && output.ready {
                input.data = Some(Item::<b8, ()> {
                    data: b8(to_send % 256),
                    frame: (),
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

/// **Correctness.** Inserting credit relays never changes what comes
/// out: every item arrives exactly once, in order, at every depth.
///
/// This is the half that mirrors the simple relay — and the half the
/// credit-conservation argument in the module docs is really about. If
/// a grant were dropped anywhere along the chain the source would run
/// permanently short of credit and this test would come up short.
#[test]
fn credit_relay_insertion_preserves_the_item_sequence() {
    const COUNT: u128 = 32;
    let want: Vec<u128> = (0..COUNT).collect();

    // Generous pool so depth cannot starve the loop; we are testing
    // correctness here, not rate.
    assert_eq!(run_pipe::<1, 4>(COUNT, 400_000), want, "depth 1");
    assert_eq!(run_pipe::<2, 4>(COUNT, 400_000), want, "depth 2");
    assert_eq!(run_pipe::<3, 4>(COUNT, 400_000), want, "depth 3");
    assert_eq!(run_pipe::<4, 4>(COUNT, 400_000), want, "depth 4");
    assert_eq!(run_pipe::<6, 4>(COUNT, 400_000), want, "depth 6");
}

/// **Throughput.** The property that distinguishes this relay from
/// `RCStreamRelay`, and the reason the module docs carry a sizing rule.
///
/// Credit flow control sustains full rate only while
/// `credits >= round-trip latency`, and each relay adds two cycles to
/// that loop.  So with a small pool, depth costs throughput; with a
/// pool large enough to cover the longer loop, it does not.
///
/// Measured at the time of writing, over a 20k-cycle window:
///
/// | pool | depth 1 | depth 6 |
/// |---|---|---|
/// | 4 credits (`FIFO_N=2`) | 131 | 48 |
/// | 16 credits (`FIFO_N=4`) | 195 | 185 |
///
/// The assertions below are deliberately looser than those numbers so
/// they track the *property*, not the exact schedule.  If this test
/// ever fails, the sizing guidance in `credit::relay`'s module docs is
/// wrong and must be rewritten rather than the test relaxed.
#[test]
fn throughput_degrades_with_depth_only_when_the_credit_pool_is_small() {
    const WINDOW: u64 = 20_000;
    const COUNT: u128 = 200;

    let small_shallow = run_pipe::<1, 2>(COUNT, WINDOW).len();
    let small_deep = run_pipe::<6, 2>(COUNT, WINDOW).len();
    let large_shallow = run_pipe::<1, 4>(COUNT, WINDOW).len();
    let large_deep = run_pipe::<6, 4>(COUNT, WINDOW).len();

    assert!(
        small_deep * 2 < small_shallow,
        "with only 4 credits, six relays must cost substantial throughput \
         (depth1={small_shallow}, depth6={small_deep})"
    );
    assert!(
        large_deep * 100 >= large_shallow * 85,
        "with 16 credits the pool covers the longer round trip, so depth \
         should cost little (depth1={large_shallow}, depth6={large_deep})"
    );
    assert!(
        large_deep > small_deep,
        "at equal depth a larger credit pool must sustain a higher rate \
         (small={small_deep}, large={large_deep})"
    );
}
