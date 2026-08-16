//! Does relay insertion stay behaviour-preserving *across a clock-domain
//! crossing*?
//!
//! `rcstream_relay_insertion.rs` established that inserting
//! `RCStreamRelay`s on an `RCStream` connection changes only latency.
//! Every bit of that argument is **single-domain**: Carloni's
//! latency-insensitive-design theorem is stated for one clock.
//!
//! RCStream Phase 4 nonetheless intends to treat *every* bus boundary as
//! a cut point needing "no hazard analysis, no functional verification".
//! If a design contains an [`RCStreamCdc`], that promise quietly spans a
//! clock crossing — and whether it survives one is a question, not a
//! corollary. This file answers it empirically before a pipeliner is
//! built on the assumption.
//!
//! Structurally the answer *should* be yes: you cannot insert a relay
//! "inside" a crossing. Every insertion point is on one side or the
//! other, entirely within a single domain, so per-domain LID ought to
//! apply and the async FIFO in the middle is unchanged. These tests
//! check that the composition actually behaves that way rather than
//! assuming the decomposition is valid.

use rhdl::prelude::*;
use rhdl_fpga::rcstream::{bus::Item, cdc::RCStreamCdc, relay::RCStreamRelay, RCStream};

/// `W`-domain relays → `RCStreamCdc` → `R`-domain relays.
///
/// `NW` relays before the crossing (in the write domain), `NR` after it
/// (in the read domain). Both are ordinary single-domain insertions;
/// the crossing itself is untouched.
#[derive(Clone, Circuit, CircuitDQ)]
pub struct CdcPipe<const NW: usize, const NR: usize> {
    w_relays: [Adapter<RCStreamRelay<b8, bool>, Red>; NW],
    cdc: RCStreamCdc<b8, bool, Red, Blue, 4>,
    r_relays: [Adapter<RCStreamRelay<b8, bool>, Blue>; NR],
}

impl<const NW: usize, const NR: usize> Default for CdcPipe<NW, NR> {
    fn default() -> Self {
        Self {
            w_relays: std::array::from_fn(|_| Adapter::new(RCStreamRelay::<b8, bool>::default())),
            cdc: RCStreamCdc::default(),
            r_relays: std::array::from_fn(|_| Adapter::new(RCStreamRelay::<b8, bool>::default())),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
pub struct In {
    pub data: Signal<Option<Item<b8, bool>>, Red>,
    pub ready: Signal<bool, Blue>,
    pub cr_w: Signal<ClockReset, Red>,
    pub cr_r: Signal<ClockReset, Blue>,
}

#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
pub struct Out {
    pub ready: Signal<bool, Red>,
    pub data: Signal<Option<Item<b8, bool>>, Blue>,
}

impl<const NW: usize, const NR: usize> CircuitIO for CdcPipe<NW, NR> {
    type I = In;
    type O = Out;
    type Kernel = cdc_pipe_kernel<NW, NR>;
}

#[kernel]
#[doc(hidden)]
// Kernels index arrays explicitly; iterator adapters are not in the
// `#[kernel]` subset.
#[allow(clippy::needless_range_loop)]
pub fn cdc_pipe_kernel<const NW: usize, const NR: usize>(
    i: In,
    q: CdcPipeQ<NW, NR>,
) -> (Out, CdcPipeD<NW, NR>) {
    let mut d = CdcPipeD::<NW, NR>::dont_care();

    // --- write-domain relay chain -------------------------------
    // Each relay's data comes from its predecessor (relay 0 from the
    // external input); each relay's ready comes from its successor
    // (the last one from the crossing).
    let mut w_data = [None; NW];
    w_data[0] = i.data.val();
    for k in 1..NW {
        w_data[k] = q.w_relays[k - 1].val().data;
    }
    let mut w_ready = [false; NW];
    for k in 0..(NW - 1) {
        w_ready[k] = q.w_relays[k + 1].val().ready;
    }
    w_ready[NW - 1] = q.cdc.ready.val();
    for k in 0..NW {
        d.w_relays[k].clock_reset = i.cr_w;
        d.w_relays[k].input = signal(RCStream::<b8, bool> {
            data: w_data[k],
            ready: w_ready[k],
        });
    }

    // --- the crossing --------------------------------------------
    d.cdc.cr_w = i.cr_w;
    d.cdc.cr_r = i.cr_r;
    d.cdc.data = signal(q.w_relays[NW - 1].val().data);
    d.cdc.ready = signal(q.r_relays[0].val().ready);

    // --- read-domain relay chain ---------------------------------
    let mut r_data = [None; NR];
    r_data[0] = q.cdc.data.val();
    for k in 1..NR {
        r_data[k] = q.r_relays[k - 1].val().data;
    }
    let mut r_ready = [false; NR];
    for k in 0..(NR - 1) {
        r_ready[k] = q.r_relays[k + 1].val().ready;
    }
    r_ready[NR - 1] = i.ready.val();
    for k in 0..NR {
        d.r_relays[k].clock_reset = i.cr_r;
        d.r_relays[k].input = signal(RCStream::<b8, bool> {
            data: r_data[k],
            ready: r_ready[k],
        });
    }

    let o = Out {
        ready: signal(q.w_relays[0].val().ready),
        data: signal(q.r_relays[NR - 1].val().data),
    };
    (o, d)
}

/// Drive the pipe with a fixed source and a deterministic sink, and
/// return the delivered sequence.
fn delivered<const NW: usize, const NR: usize>(count: u128) -> Vec<(u128, bool)> {
    let uut = CdcPipe::<NW, NR>::default();
    let mut next_to_send: u128 = 0;
    let mut got: Vec<(u128, bool)> = Vec::new();
    let mut phase: u32 = 0;

    let samples = run_async_red_blue(
        &uut,
        // Red / write domain: offer an item whenever the pipe has room.
        |output, input| {
            if next_to_send < count && output.ready.val() {
                input.data = signal(Some(Item::<b8, bool> {
                    data: b8(next_to_send % 256),
                    frame: next_to_send % 8 == 7,
                }));
                next_to_send += 1;
            } else {
                input.data = signal(None);
            }
        },
        // Blue / read domain: accept on 2 of every 3 cycles.
        |output, input| {
            phase = phase.wrapping_add(1);
            let want = !phase.is_multiple_of(3);
            input.ready = signal(false);
            if want && output.data.val().is_some() {
                input.ready = signal(true);
                let it = output.data.val().unwrap();
                got.push((it.data.raw(), it.frame));
            }
        },
        50,
        78,
        |red, blue, input| {
            input.cr_w = red;
            input.cr_r = blue;
        },
    );
    samples.take_while(|t| t.time < 250_000).for_each(drop);
    got
}

/// **The question this file exists to answer.** Relays inserted before
/// and after a clock-domain crossing must not change what comes out.
///
/// If any configuration diverged, RCStream Phase 4 could not treat bus
/// boundaries as universally safe cut points in a multi-domain design,
/// and the plan would need a domain-aware caveat.
#[test]
fn insertion_around_a_clock_crossing_preserves_the_sequence() {
    const COUNT: u128 = 24;
    let want: Vec<(u128, bool)> = (0..COUNT).map(|k| (k % 256, k % 8 == 7)).collect();

    // (write-side relays, read-side relays)
    assert_eq!(delivered::<1, 1>(COUNT), want, "1 before, 1 after");
    assert_eq!(delivered::<2, 1>(COUNT), want, "2 before, 1 after");
    assert_eq!(delivered::<1, 2>(COUNT), want, "1 before, 2 after");
    assert_eq!(delivered::<3, 3>(COUNT), want, "3 before, 3 after");
    assert_eq!(delivered::<4, 2>(COUNT), want, "4 before, 2 after");
}
