//! A credit link must not lose items under sustained backpressure.
//!
//! Regression test for a real defect: `CreditSink` initialised its
//! credit pool to `2^FIFO_N`, but `SyncFIFO<_, FIFO_N>` holds only
//! `2^FIFO_N - 1` items. The sink therefore issued one more token than
//! its buffer could accept; the source spent it, the write hit a full
//! FIFO, and the item was **silently dropped**.
//!
//! The bug is invisible with an always-ready downstream, because the
//! buffer drains as fast as it fills and never reaches capacity. It
//! needs a sink that actually stalls — which is why the existing
//! single-pair tests missed it and `CreditMux` (three sinks sharing one
//! output port, so each drains a third of the time) exposed it.
//!
//! The whole point of credit-based flow control is that the source
//! cannot overrun the sink. If this test fails, that guarantee is gone
//! and the loss is silent.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::rcstream::{
    bus::Item,
    credit::{sink::CreditSink, source::CreditSource},
};

const CW: usize = 5;

#[derive(Clone, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
struct Link<const FIFO_N: usize>
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    source: CreditSource<b8, (), CW>,
    sink: CreditSink<b8, (), CW, FIFO_N>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct LinkIn {
    pub data: Option<Item<b8, ()>>,
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct LinkOut {
    pub ready: bool,
    pub data: Option<Item<b8, ()>>,
}

impl<const FIFO_N: usize> SynchronousIO for Link<FIFO_N>
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    type I = LinkIn;
    type O = LinkOut;
    type Kernel = link_kernel<FIFO_N>;
}

#[kernel]
#[doc(hidden)]
pub fn link_kernel<const FIFO_N: usize>(
    _cr: ClockReset,
    i: LinkIn,
    q: Q<FIFO_N>,
) -> (LinkOut, D<FIFO_N>)
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    let mut d = D::<FIFO_N>::dont_care();
    d.source.upstream_data = i.data;
    d.sink.upstream_data = q.source.downstream_data;
    d.source.credit_grant = q.sink.credit_grant;
    d.sink.downstream_ready = i.ready;
    let o = LinkOut {
        ready: q.source.upstream_ready,
        data: q.sink.downstream_data,
    };
    (o, d)
}

/// Drive the link with a source that always has data and a sink that
/// only accepts `1` cycle in `stall_period` — enough backpressure that
/// the buffer genuinely fills.
fn delivered<const FIFO_N: usize>(count: u128, stall_period: u32) -> Vec<u128>
where
    rhdl::bits::W<FIFO_N>: BitWidth,
{
    let uut = Link::<FIFO_N>::default();
    let mut sent: u128 = 0;
    let mut got: Vec<u128> = Vec::new();
    let mut need_reset = true;
    let mut phase: u32 = 0;

    uut.run_fn(
        |output| {
            if need_reset {
                need_reset = false;
                return Some(ResetOrData::Reset);
            }
            phase = phase.wrapping_add(1);
            let sink_ready = phase.is_multiple_of(stall_period);
            if let Some(it) = output.data {
                if sink_ready {
                    got.push(it.data.raw());
                }
            }
            let mut input = LinkIn {
                data: None,
                ready: sink_ready,
            };
            if sent < count && output.ready {
                input.data = Some(Item::<b8, ()> {
                    data: b8(sent % 256),
                    frame: (),
                });
                sent += 1;
            }
            Some(ResetOrData::Data(input))
        },
        100,
    )
    .take_while(|t| t.time < 600_000)
    .for_each(drop);

    got
}

/// Every item must arrive, at every buffer size, under heavy stalling.
#[test]
fn credit_link_loses_nothing_under_backpressure() {
    const COUNT: u128 = 24;
    let want: Vec<u128> = (0..COUNT).collect();
    // Small buffers and hard stalls are where the off-by-one bites.
    assert_eq!(delivered::<2>(COUNT, 4), want, "FIFO_N=2, 1-in-4 sink");
    assert_eq!(delivered::<2>(COUNT, 8), want, "FIFO_N=2, 1-in-8 sink");
    assert_eq!(delivered::<3>(COUNT, 5), want, "FIFO_N=3, 1-in-5 sink");
    assert_eq!(delivered::<4>(COUNT, 6), want, "FIFO_N=4, 1-in-6 sink");
}
