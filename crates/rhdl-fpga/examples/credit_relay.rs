// A credit-based link with two pipeline relays inserted:
//
//     CreditSource -> relay -> relay -> CreditSink
//
// Watch the forward `data` path and the reverse `credit_grant` path
// each pick up one cycle of delay per stage.  The item sequence is
// unaffected; only the credit round trip gets longer, which is why the
// sink's pool has to be sized for it.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::{core::sim::ResetOrData, prelude::*};
use rhdl_fpga::{
    doc::write_svg_as_markdown,
    rcstream::{
        bus::Item,
        credit::{relay::CreditRCStreamRelay, sink::CreditSink, source::CreditSource},
    },
};

const CW: usize = 5;
const FIFO_N: usize = 3;

#[derive(Clone, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
struct Link {
    source: CreditSource<b8, (), CW>,
    r0: CreditRCStreamRelay<b8, (), CW>,
    r1: CreditRCStreamRelay<b8, (), CW>,
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

impl SynchronousIO for Link {
    type I = LinkIn;
    type O = LinkOut;
    type Kernel = link_kernel;
}

#[kernel]
pub fn link_kernel(_cr: ClockReset, i: LinkIn, q: Q) -> (LinkOut, D) {
    let mut d = D::dont_care();
    // Forward: source -> r0 -> r1 -> sink
    d.source.upstream_data = i.data;
    d.r0.data = q.source.downstream_data;
    d.r1.data = q.r0.data;
    d.sink.upstream_data = q.r1.data;
    // Reverse credit: sink -> r1 -> r0 -> source
    d.r1.credit_grant = q.sink.credit_grant;
    d.r0.credit_grant = q.r1.credit_grant;
    d.source.credit_grant = q.r0.credit_grant;
    // Downstream RCStream face
    d.sink.downstream_ready = i.ready;
    let o = LinkOut {
        ready: q.source.upstream_ready,
        data: q.sink.downstream_data,
    };
    (o, d)
}

fn main() -> Result<(), RHDLError> {
    let uut = Link::default();
    let mut to_send: u128 = 0;
    let mut need_reset = true;
    let mut phase: u32 = 0;

    let vcd = uut
        .run_fn(
            |output| {
                if need_reset {
                    need_reset = false;
                    return Some(ResetOrData::Reset);
                }
                phase = phase.wrapping_add(1);
                let mut input = LinkIn {
                    data: None,
                    ready: !phase.is_multiple_of(4),
                };
                if output.ready {
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
        .take_while(|t| t.time < 1500)
        .collect::<SvgFile>();

    write_svg_as_markdown(
        vcd,
        "credit_relay.md",
        SvgOptions::default().with_io_filter(),
    )?;
    Ok(())
}
