//! `chunked` then `flatten` is lossless.
//!
//! The two widgets make mirror-image framing decisions — `chunked`
//! carries every element's marker as `[F; N]`, `flatten` carries the
//! group marker plus a last-of-group flag — and the point of both
//! choices was that nothing is discarded. This test checks that claim
//! end-to-end rather than leaving it as an assertion in module docs.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::rcstream::{
    bus::Item, chunked::RCStreamChunked, flatten::RCStreamFlatten, RCStream,
};

#[derive(Clone, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
struct RoundTrip {
    chunk: RCStreamChunked<b8, bool, 3, 4>,
    flat: RCStreamFlatten<b8, [bool; 4], 3, 4>,
}

impl SynchronousIO for RoundTrip {
    type I = RCStream<b8, bool>;
    type O = RCStream<b8, ([bool; 4], bool)>;
    type Kernel = round_trip_kernel;
}

#[kernel]
#[doc(hidden)]
pub fn round_trip_kernel(
    _cr: ClockReset,
    i: RCStream<b8, bool>,
    q: Q,
) -> (RCStream<b8, ([bool; 4], bool)>, D) {
    let mut d = D::dont_care();
    d.chunk.data = i.data;
    d.chunk.ready = q.flat.ready;
    d.flat.data = q.chunk.data;
    d.flat.ready = i.ready;
    let o = RCStream::<b8, ([bool; 4], bool)> {
        data: q.flat.data,
        ready: q.chunk.ready,
    };
    (o, d)
}

/// Every payload returns, in order; and every marker returns, recoverable
/// by position within its group.
#[test]
fn chunk_then_flatten_returns_every_payload_and_every_marker() {
    const COUNT: u128 = 24;
    let uut = RoundTrip::default();
    let mut sent: u128 = 0;
    let mut got: Vec<(u128, [bool; 4], bool)> = Vec::new();
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
                    got.push((it.data.raw(), it.frame.0, it.frame.1));
                }
            }
            let mut input = RCStream::<b8, bool> {
                data: None,
                ready: sink_ready,
            };
            if sent < COUNT && output.ready {
                input.data = Some(Item::<b8, bool> {
                    data: b8(sent % 256),
                    frame: sent % 3 == 0,
                });
                sent += 1;
            }
            Some(ResetOrData::Data(input))
        },
        100,
    )
    .take_while(|t| t.time < 400_000)
    .for_each(drop);

    // Payloads: exact original order, nothing dropped or duplicated.
    let payloads: Vec<u128> = got.iter().map(|(d, _, _)| *d).collect();
    let want_payloads: Vec<u128> = (0..COUNT).collect();
    assert_eq!(
        payloads, want_payloads,
        "every payload must survive the round trip"
    );

    // Markers: the k-th element of each group carries a marker array whose
    // k-th entry is that element's ORIGINAL marker.  This is what
    // "nothing discarded" means concretely — the association is
    // recoverable by position.
    for (n, (payload, frames, last)) in got.iter().enumerate() {
        let k = n % 4;
        assert_eq!(
            frames[k],
            (*payload % 3 == 0),
            "element {payload} (slot {k}) must carry its own original marker"
        );
        assert_eq!(*last, k == 3, "last-of-group flag must mark slot 3 only");
    }
}
