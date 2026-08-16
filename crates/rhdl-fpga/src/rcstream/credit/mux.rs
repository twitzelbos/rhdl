#![warn(missing_docs)]
//! Aggregate `N` credit-based sources into one [`RCStream`].
//!
//! §11 gives credit-based flow control two motivations. The first —
//! breaking a long combinational `ready` path — is served by
//! [`super::source::CreditSource`] / [`super::sink::CreditSink`]. This
//! widget serves the second, which the design plan calls *the classical
//! use case*: **one sink receiving from many sources, where
//! reverse-direction arbitration would be expensive.**
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!("
        +----+CreditMux+-----+
 ?T x N |                    |  ?T
+------>| data          data +---->
        |                    |
<-------+ credit       ready |<----+
  x N   |                    |
        +--------------------+
")]
//!
//!# Why per-source credit pools
//!
//! Each source gets its **own** [`super::sink::CreditSink`], and
//! therefore its own buffer and its own credit pool. The alternative —
//! one shared buffer with credit carved out of it — was rejected:
//! sources would then compete for a common pool, and a fast or
//! misbehaving source could consume the whole thing and starve the
//! others. Independent pools mean a source can only ever exhaust its
//! own credit, which is exactly the *virtual channel* property §11
//! lists as a credit use case.
//!
//! The cost is `N` buffers instead of one. That is the honest price of
//! non-interference, and it is why this widget is a composition of
//! existing parts rather than a monolith.
//!
//!# Arbitration: round-robin, not priority
//!
//! The selector starts each search one past the source it last served,
//! so a source that has just been served goes to the back of the queue.
//!
//! Strict priority was rejected. With priority, a source that always has
//! data starves every lower-ranked source **indefinitely** — and an
//! aggregator's whole job is to merge streams, so permanently dropping
//! one is a silent failure of purpose rather than a tunable policy. If a
//! design genuinely wants priority, it wants a different widget and
//! should say so in its type.
//!
//! Round-robin here is *work-conserving*: the search skips idle sources
//! rather than waiting on them, so an idle channel costs nothing.
//!
//!# Sizing
//!
//! `N` sources, each with a `2^FIFO_N` buffer. `M` is the width of the
//! round-robin pointer and must satisfy `2^M > N - 1`. `CREDIT_W` and
//! `FIFO_N` carry [`super::sink::CreditSink`]'s own constraints —
//! including `FIFO_N >= 2`.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/credit_mux.rs")]
//!```
//!
//! The trace below shows three sources merging: the credit wires move
//! independently, and the arbiter interleaves rather than favouring one.
#![doc = include_str!("../../../doc/credit_mux.md")]

use rhdl::prelude::*;

use crate::core::dff;

use super::sink::CreditSink;
use crate::rcstream::bus::Item;

/// Aggregate `N` credit-based sources into a single [`RCStream`].
///
/// Each source has an independent credit pool and buffer; the outputs
/// are merged by a round-robin arbiter. See module docs for both
/// choices.
#[derive(Clone, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct CreditMux<
    T: Digital,
    F: Digital,
    const CREDIT_W: usize,
    const FIFO_N: usize,
    const M: usize,
    const N: usize,
> where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
    rhdl::bits::W<M>: BitWidth,
{
    /// One credit sink per source: independent buffer, independent pool.
    sinks: [CreditSink<T, F, CREDIT_W, FIFO_N>; N],
    /// Round-robin pointer — the source to start the next search from.
    rr: dff::DFF<Bits<M>>,
}

impl<
        T: Digital,
        F: Digital,
        const CREDIT_W: usize,
        const FIFO_N: usize,
        const M: usize,
        const N: usize,
    > Default for CreditMux<T, F, CREDIT_W, FIFO_N, M, N>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
    rhdl::bits::W<M>: BitWidth,
{
    fn default() -> Self {
        Self {
            sinks: std::array::from_fn(|_| CreditSink::default()),
            rr: dff::DFF::new(bits::<M>(0)),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to the [`CreditMux`].
pub struct In<T: Digital, F: Digital, const N: usize> {
    /// Data flowing in from each of the `N` credit sources.
    pub data: [Option<Item<T, F>>; N],
    /// Ready flowing in from the single downstream sink.
    pub ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from the [`CreditMux`].
pub struct Out<T: Digital, F: Digital, const CREDIT_W: usize, const N: usize>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
{
    /// Credit grant flowing back to each source. Independent pools, so
    /// a grant to one says nothing about any other.
    pub credit_grant: [Bits<CREDIT_W>; N],
    /// The merged data stream.
    pub data: Option<Item<T, F>>,
}

impl<
        T: Digital,
        F: Digital,
        const CREDIT_W: usize,
        const FIFO_N: usize,
        const M: usize,
        const N: usize,
    > SynchronousIO for CreditMux<T, F, CREDIT_W, FIFO_N, M, N>
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
    rhdl::bits::W<M>: BitWidth,
{
    type I = In<T, F, N>;
    type O = Out<T, F, CREDIT_W, N>;
    type Kernel = credit_mux_kernel<T, F, CREDIT_W, FIFO_N, M, N>;
}

#[kernel(allow_weak_partial)]
#[doc(hidden)]
#[allow(clippy::type_complexity)]
// Kernels index arrays explicitly; iterator adapters are not in the
// `#[kernel]` subset, so the range-loop form is required here.
#[allow(clippy::needless_range_loop)]
pub fn credit_mux_kernel<
    T: Digital,
    F: Digital,
    const CREDIT_W: usize,
    const FIFO_N: usize,
    const M: usize,
    const N: usize,
>(
    _cr: ClockReset,
    i: In<T, F, N>,
    q: Q<T, F, CREDIT_W, FIFO_N, M, N>,
) -> (Out<T, F, CREDIT_W, N>, D<T, F, CREDIT_W, FIFO_N, M, N>)
where
    rhdl::bits::W<CREDIT_W>: BitWidth,
    rhdl::bits::W<FIFO_N>: BitWidth,
    rhdl::bits::W<M>: BitWidth,
{
    let mut d = D::<T, F, CREDIT_W, FIFO_N, M, N>::dont_care();

    // Feed each source into its own sink, and surface that sink's grant.
    let mut grants = [bits::<CREDIT_W>(0); N];
    let mut has = [false; N];
    for k in 0..N {
        d.sinks[k].upstream_data = i.data[k];
        // Deassert by default; the winner is enabled below.
        d.sinks[k].downstream_ready = false;
        grants[k] = q.sinks[k].credit_grant;
        has[k] = match q.sinks[k].downstream_data {
            Some(_) => true,
            None => false,
        };
    }

    // Round-robin search: scan from `rr` upward, then wrap.  Two passes
    // rather than a modulo, which the kernel subset does not accept.
    let mut sel = bits::<M>(0);
    let mut found = false;
    for k in 0..N {
        let kb = bits::<M>(k as u128);
        if !found && kb >= q.rr && has[k] {
            sel = kb;
            found = true;
        }
    }
    for k in 0..N {
        let kb = bits::<M>(k as u128);
        if !found && kb < q.rr && has[k] {
            sel = kb;
            found = true;
        }
    }

    // Present the winner's item.
    let o_data = if found {
        q.sinks[sel].downstream_data
    } else {
        None
    };

    // A transfer happens when we have a winner and the sink takes it.
    let transfer = found && i.ready;
    if transfer {
        d.sinks[sel].downstream_ready = true;
    }
    // Move past the source just served, so it goes to the back.
    d.rr = if transfer {
        if sel == bits::<M>(N as u128 - 1) {
            bits::<M>(0)
        } else {
            sel + 1
        }
    } else {
        q.rr
    };

    let o = Out::<T, F, CREDIT_W, N> {
        credit_grant: grants,
        data: o_data,
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhdl::core::sim::ResetOrData;

    type Mux = CreditMux<b8, (), 5, 2, 2, 3>;

    fn item(v: u128) -> Item<b8, ()> {
        Item::<b8, ()> {
            data: bits::<8>(v),
            frame: (),
        }
    }

    fn idle_in() -> In<b8, (), 3> {
        In::<b8, (), 3> {
            data: [None; 3],
            ready: true,
        }
    }

    /// Build a `Q` where the given sinks are presenting items.
    fn q_with(present: [Option<u128>; 3], rr: u128) -> Q<b8, (), 5, 2, 2, 3> {
        let mk = |v: Option<u128>| crate::rcstream::credit::sink::Out::<b8, (), 5> {
            credit_grant: bits::<5>(0),
            downstream_data: v.map(item),
        };
        Q::<b8, (), 5, 2, 2, 3> {
            sinks: [mk(present[0]), mk(present[1]), mk(present[2])],
            rr: bits::<2>(rr),
        }
    }

    /// Tier 1 — with only one source holding data, it wins regardless of
    /// where the round-robin pointer happens to be.
    #[test]
    fn single_active_source_wins_from_any_pointer() {
        for rr in 0..3u128 {
            let q = q_with([None, Some(0x22), None], rr);
            let (o, d) =
                credit_mux_kernel::<b8, (), 5, 2, 2, 3>(ClockReset::dont_care(), idle_in(), q);
            assert_eq!(
                o.data.unwrap().data.raw(),
                0x22,
                "the only active source must win (rr={rr})"
            );
            assert!(d.sinks[1].downstream_ready, "and be the one advanced");
        }
    }

    /// Tier 1 — the pointer breaks ties: with every source active, the
    /// one at `rr` is served.
    #[test]
    fn pointer_selects_among_contending_sources() {
        for rr in 0..3u128 {
            let q = q_with([Some(0xA0), Some(0xA1), Some(0xA2)], rr);
            let (o, _d) =
                credit_mux_kernel::<b8, (), 5, 2, 2, 3>(ClockReset::dont_care(), idle_in(), q);
            assert_eq!(
                o.data.unwrap().data.raw(),
                0xA0 + rr,
                "source at the pointer wins"
            );
        }
    }

    /// Tier 1 — **the anti-starvation property.** After serving a source
    /// the pointer moves past it, so the same source cannot win twice in
    /// a row while others are waiting.
    #[test]
    fn pointer_advances_past_the_served_source() {
        let q = q_with([Some(0xA0), Some(0xA1), Some(0xA2)], 0);
        let (_o, d) =
            credit_mux_kernel::<b8, (), 5, 2, 2, 3>(ClockReset::dont_care(), idle_in(), q);
        assert_eq!(d.rr.raw(), 1, "pointer moves to the next source");
    }

    /// Tier 1 — the search wraps: with the pointer past the only active
    /// source, it is still found.
    #[test]
    fn search_wraps_around() {
        let q = q_with([Some(0x33), None, None], 2);
        let (o, d) = credit_mux_kernel::<b8, (), 5, 2, 2, 3>(ClockReset::dont_care(), idle_in(), q);
        assert_eq!(
            o.data.unwrap().data.raw(),
            0x33,
            "wrapped search finds source 0"
        );
        assert!(d.sinks[0].downstream_ready);
    }

    /// Tier 1 — backpressure: a winner is presented but not consumed, and
    /// the pointer does not move.
    #[test]
    fn backpressure_holds_the_winner_and_the_pointer() {
        let q = q_with([Some(0xA0), Some(0xA1), None], 0);
        let i = In::<b8, (), 3> {
            data: [None; 3],
            ready: false,
        };
        let (o, d) = credit_mux_kernel::<b8, (), 5, 2, 2, 3>(ClockReset::dont_care(), i, q);
        assert!(o.data.is_some(), "the winner stays presented");
        assert!(
            !d.sinks[0].downstream_ready,
            "but is not consumed while the sink is stalled"
        );
        assert_eq!(d.rr.raw(), 0, "and the pointer does not move");
    }

    /// Tier 1 — no source active means no output and no advance.
    #[test]
    fn idle_sources_produce_nothing() {
        let q = q_with([None, None, None], 1);
        let (o, d) = credit_mux_kernel::<b8, (), 5, 2, 2, 3>(ClockReset::dont_care(), idle_in(), q);
        assert!(o.data.is_none());
        assert_eq!(d.rr.raw(), 1);
        for k in 0..3 {
            assert!(!d.sinks[k].downstream_ready);
        }
    }

    /// Tier 1 — every source's grant is surfaced on its own wire; pools
    /// are independent.
    #[test]
    fn grants_are_per_source() {
        let mut q = q_with([None, None, None], 0);
        q.sinks[0].credit_grant = bits::<5>(1);
        q.sinks[2].credit_grant = bits::<5>(3);
        let (o, _d) =
            credit_mux_kernel::<b8, (), 5, 2, 2, 3>(ClockReset::dont_care(), idle_in(), q);
        assert_eq!(o.credit_grant[0].raw(), 1);
        assert_eq!(o.credit_grant[1].raw(), 0);
        assert_eq!(o.credit_grant[2].raw(), 3);
    }

    /// LID requirement.
    #[test]
    fn test_no_combinatorial_paths() -> miette::Result<()> {
        let uut = Mux::default();
        drc::no_combinatorial_paths(&uut)?;
        Ok(())
    }

    /// Tier 2 — closed loop with three sources feeding continuously.
    /// Every item from every source must arrive, and the round-robin
    /// must keep the three roughly balanced rather than starving any.
    #[test]
    fn all_sources_drain_and_none_starves() {
        const PER_SRC: u128 = 12;
        let uut = Mux::default();
        let mut sent = [0u128; 3];
        // Each source keeps a proper credit COUNTER: grants accumulate,
        // and a send decrements.  Gating on the instantaneous grant
        // instead silently overruns the sink's buffer and drops items —
        // which is exactly what `CreditSource` exists to prevent.
        let mut credit = [0u128; 3];
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
                let mut input = In::<b8, (), 3> {
                    data: [None; 3],
                    ready: true,
                };
                // Each source offers while it holds credit.  Source k
                // emits values k*100 + n so origin is recoverable.
                for k in 0..3usize {
                    credit[k] += output.credit_grant[k].raw();
                    if sent[k] < PER_SRC && credit[k] > 0 {
                        input.data[k] = Some(item((k as u128) * 100 + sent[k]));
                        sent[k] += 1;
                        credit[k] -= 1;
                    }
                }
                Some(ResetOrData::Data(input))
            },
            100,
        )
        .take_while(|t| t.time < 400_000)
        .for_each(drop);

        // Every source's items arrive, in that source's own order.
        for k in 0..3u128 {
            let mine: Vec<u128> = got
                .iter()
                .copied()
                .filter(|v| *v / 100 == k)
                .map(|v| v % 100)
                .collect();
            let want: Vec<u128> = (0..PER_SRC).collect();
            assert_eq!(mine, want, "source {k} must deliver all items in order");
        }
    }

    /// Tier 3 — HDL emission snapshot.
    #[test]
    fn hdl_emission_snapshot() -> miette::Result<()> {
        let uut = Mux::default();
        let desc = uut.descriptor("credit_mux".into())?;
        let hdl = desc.hdl()?;
        let top = hdl
            .modules
            .modules
            .iter()
            .find(|m| m.name == "credit_mux")
            .expect("top module must be emitted");
        let expect = expect_test::expect![[r#"
            module credit_mux(input wire [1:0] clock_reset, input wire [27:0] i, output wire [23:0] o);
               wire [55:0] od;
               wire [31:0] d;
               wire [43:0] q;
               assign o = od[23:0];
               credit_mux_sinks c0(.clock_reset(clock_reset), .i(d[29:0]), .o(q[41:0]));
               credit_mux_rr c1(.clock_reset(clock_reset), .i(d[31:30]), .o(q[43:42]));
               assign d = od[55:24];
               assign od = kernel_credit_mux_kernel(clock_reset, i, q);
               function [55:0] kernel_credit_mux_kernel(input reg [1:0] arg_0, input reg [27:0] arg_1, input reg [43:0] arg_2);
                     reg [26:0] r0;
                     reg [27:0] r1;
                     reg [8:0] r2;
                     // d
                     reg [31:0] r3;
                     // d
                     reg [31:0] r4;
                     reg [41:0] r5;
                     reg [43:0] r6;
                     reg [13:0] r7;
                     reg [4:0] r8;
                     // grants
                     reg [14:0] r9;
                     reg [41:0] r10;
                     reg [13:0] r11;
                     reg [8:0] r12;
                     reg [0:0] r13;
                     reg [0:0] r14;
                     // has
                     reg [2:0] r15;
                     reg [26:0] r16;
                     reg [8:0] r17;
                     // d
                     reg [31:0] r18;
                     // d
                     reg [31:0] r19;
                     reg [41:0] r20;
                     reg [13:0] r21;
                     reg [4:0] r22;
                     // grants
                     reg [14:0] r23;
                     reg [41:0] r24;
                     reg [13:0] r25;
                     reg [8:0] r26;
                     reg [0:0] r27;
                     reg [0:0] r28;
                     // has
                     reg [2:0] r29;
                     reg [26:0] r30;
                     reg [8:0] r31;
                     // d
                     reg [31:0] r32;
                     // d
                     reg [31:0] r33;
                     reg [41:0] r34;
                     reg [13:0] r35;
                     reg [4:0] r36;
                     // grants
                     reg [14:0] r37;
                     reg [41:0] r38;
                     reg [13:0] r39;
                     reg [8:0] r40;
                     reg [0:0] r41;
                     reg [0:0] r42;
                     // has
                     reg [2:0] r43;
                     reg [1:0] r44;
                     reg [0:0] r45;
                     reg [0:0] r46;
                     reg [0:0] r47;
                     reg [0:0] r48;
                     // found
                     reg [0:0] r49;
                     // sel
                     reg [1:0] r50;
                     reg [0:0] r51;
                     reg [1:0] r52;
                     reg [0:0] r53;
                     reg [0:0] r54;
                     reg [0:0] r55;
                     reg [0:0] r56;
                     // found
                     reg [0:0] r57;
                     // sel
                     reg [1:0] r58;
                     reg [0:0] r59;
                     reg [1:0] r60;
                     reg [0:0] r61;
                     reg [0:0] r62;
                     reg [0:0] r63;
                     reg [0:0] r64;
                     // found
                     reg [0:0] r65;
                     // sel
                     reg [1:0] r66;
                     reg [0:0] r67;
                     reg [1:0] r68;
                     reg [0:0] r69;
                     reg [0:0] r70;
                     reg [0:0] r71;
                     reg [0:0] r72;
                     // found
                     reg [0:0] r73;
                     // sel
                     reg [1:0] r74;
                     reg [0:0] r75;
                     reg [1:0] r76;
                     reg [0:0] r77;
                     reg [0:0] r78;
                     reg [0:0] r79;
                     reg [0:0] r80;
                     // found
                     reg [0:0] r81;
                     // sel
                     reg [1:0] r82;
                     reg [0:0] r83;
                     reg [1:0] r84;
                     reg [0:0] r85;
                     reg [0:0] r86;
                     reg [0:0] r87;
                     reg [0:0] r88;
                     // found
                     reg [0:0] r89;
                     // sel
                     reg [1:0] r90;
                     reg [41:0] r91;
                     reg [13:0] r92;
                     reg [13:0] r93;
                     reg [13:0] r94;
                     reg [13:0] r95;
                     reg [8:0] r96;
                     reg [8:0] r97;
                     reg [0:0] r98;
                     reg [0:0] r99;
                     // d
                     reg [31:0] r100;
                     reg [31:0] r101;
                     reg [31:0] r102;
                     reg [31:0] r103;
                     // d
                     reg [31:0] r104;
                     reg [0:0] r105;
                     reg [1:0] r106;
                     reg [1:0] r107;
                     reg [1:0] r108;
                     reg [1:0] r109;
                     // d
                     reg [31:0] r110;
                     reg [23:0] r111;
                     reg [23:0] r112;
                     reg [55:0] r113;
                     reg [1:0] r114;
                     localparam l0 = 32'bXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX;
                     localparam l1 = 1'b0;
                     localparam l2 = 15'b000000000000000;
                     localparam l3 = 1'b1;
                     localparam l4 = 1'b1;
                     localparam l5 = 1'b0;
                     localparam l6 = 1'b0;
                     localparam l7 = 3'b000;
                     localparam l8 = 1'b0;
                     localparam l9 = 1'b1;
                     localparam l10 = 1'b1;
                     localparam l11 = 1'b0;
                     localparam l12 = 1'b0;
                     localparam l13 = 1'b0;
                     localparam l14 = 1'b1;
                     localparam l15 = 1'b1;
                     localparam l16 = 1'b0;
                     localparam l17 = 1'b0;
                     localparam l18 = 2'b00;
                     localparam l19 = 1'b1;
                     localparam l20 = 1'b1;
                     localparam l21 = 1'b0;
                     localparam l22 = 2'b00;
                     localparam l23 = 2'b01;
                     localparam l24 = 1'b1;
                     localparam l25 = 2'b10;
                     localparam l26 = 1'b1;
                     localparam l27 = 2'b00;
                     localparam l28 = 1'b1;
                     localparam l29 = 2'b01;
                     localparam l30 = 1'b1;
                     localparam l31 = 2'b10;
                     localparam l32 = 1'b1;
                     localparam l33 = 2'b00;
                     localparam l34 = 2'b01;
                     localparam l35 = 2'b10;
                     localparam l36 = 9'b000000000;
                     localparam l37 = 2'b00;
                     localparam l38 = 2'b01;
                     localparam l39 = 2'b10;
                     localparam l40 = 1'b1;
                     localparam l41 = 2'b10;
                     localparam l42 = 2'b01;
                     localparam l43 = 2'b00;
                     localparam l44 = 24'b000000000000000000000000;
                     begin
                        r114 = arg_0;
                        r1 = arg_1;
                        r6 = arg_2;
                        r0 = r1[26:0];
                        r2 = r0[8:0];
                        r3 = l0;
                        r3[8:0] = r2;
                        r4 = r3;
                        r4[9:9] = l1;
                        r5 = r6[41:0];
                        r7 = r5[13:0];
                        r8 = r7[4:0];
                        r9 = l2;
                        r9[4:0] = r8;
                        r10 = r6[41:0];
                        r11 = r10[13:0];
                        r12 = r11[13:5];
                        r13 = r12[8:8];
                        case (r13)
                           1'b1 : r14 = l4;
                           1'b0 : r14 = l6;
                        endcase
                        r15 = l7;
                        r15[0:0] = r14;
                        r16 = r1[26:0];
                        r17 = r16[17:9];
                        r18 = r4;
                        r18[18:10] = r17;
                        r19 = r18;
                        r19[19:19] = l8;
                        r20 = r6[41:0];
                        r21 = r20[27:14];
                        r22 = r21[4:0];
                        r23 = r9;
                        r23[9:5] = r22;
                        r24 = r6[41:0];
                        r25 = r24[27:14];
                        r26 = r25[13:5];
                        r27 = r26[8:8];
                        case (r27)
                           1'b1 : r28 = l10;
                           1'b0 : r28 = l12;
                        endcase
                        r29 = r15;
                        r29[1:1] = r28;
                        r30 = r1[26:0];
                        r31 = r30[26:18];
                        r32 = r19;
                        r32[28:20] = r31;
                        r33 = r32;
                        r33[29:29] = l13;
                        r34 = r6[41:0];
                        r35 = r34[41:28];
                        r36 = r35[4:0];
                        r37 = r23;
                        r37[14:10] = r36;
                        r38 = r6[41:0];
                        r39 = r38[41:28];
                        r40 = r39[13:5];
                        r41 = r40[8:8];
                        case (r41)
                           1'b1 : r42 = l15;
                           1'b0 : r42 = l17;
                        endcase
                        r43 = r29;
                        r43[2:2] = r42;
                        r44 = r6[43:42];
                        r45 = l18 >= r44;
                        r46 = l19 & r45;
                        r47 = r43[0:0];
                        r48 = r46 & r47;
                        r49 = r48 ? l20 : l21;
                        r50 = r48 ? l18 : l22;
                        r51 = ~r49;
                        r52 = r6[43:42];
                        r53 = l23 >= r52;
                        r54 = r51 & r53;
                        r55 = r43[1:1];
                        r56 = r54 & r55;
                        r57 = r56 ? l24 : r49;
                        r58 = r56 ? l23 : r50;
                        r59 = ~r57;
                        r60 = r6[43:42];
                        r61 = l25 >= r60;
                        r62 = r59 & r61;
                        r63 = r43[2:2];
                        r64 = r62 & r63;
                        r65 = r64 ? l26 : r57;
                        r66 = r64 ? l25 : r58;
                        r67 = ~r65;
                        r68 = r6[43:42];
                        r69 = l27 < r68;
                        r70 = r67 & r69;
                        r71 = r43[0:0];
                        r72 = r70 & r71;
                        r73 = r72 ? l28 : r65;
                        r74 = r72 ? l27 : r66;
                        r75 = ~r73;
                        r76 = r6[43:42];
                        r77 = l29 < r76;
                        r78 = r75 & r77;
                        r79 = r43[1:1];
                        r80 = r78 & r79;
                        r81 = r80 ? l30 : r73;
                        r82 = r80 ? l29 : r74;
                        r83 = ~r81;
                        r84 = r6[43:42];
                        r85 = l31 < r84;
                        r86 = r83 & r85;
                        r87 = r43[2:2];
                        r88 = r86 & r87;
                        r89 = r88 ? l32 : r81;
                        r90 = r88 ? l31 : r82;
                        r91 = r6[41:0];
                        r92 = r91[13:0];
                        r93 = r91[27:14];
                        r94 = r91[41:28];
                        case (r90)
                           2'b00 : r95 = r92;
                           2'b01 : r95 = r93;
                           2'b10 : r95 = r94;
                        endcase
                        r96 = r95[13:5];
                        r97 = r89 ? r96 : l36;
                        r98 = r1[27:27];
                        r99 = r89 & r98;
                        r101 = r33;
                        r101[9:9] = l40;
                        r102 = r33;
                        r102[19:19] = l40;
                        r103 = r33;
                        r103[29:29] = l40;
                        case (r90)
                           2'b00 : r100 = r101;
                           2'b01 : r100 = r102;
                           2'b10 : r100 = r103;
                        endcase
                        r104 = r99 ? r100 : r33;
                        r105 = r90 == l41;
                        r106 = r90 + l42;
                        r107 = r105 ? l43 : r106;
                        r108 = r6[43:42];
                        r109 = r99 ? r107 : r108;
                        r110 = r104;
                        r110[31:30] = r109;
                        r111 = l44;
                        r111[14:0] = r37;
                        r112 = r111;
                        r112[23:15] = r97;
                        r113 = {r110, r112};
                        kernel_credit_mux_kernel = r113;
                     end
               endfunction
            endmodule"#]];
        expect.assert_eq(&top.pretty());
        Ok(())
    }

    fn open_loop() -> impl Iterator<Item = TimedSample<(ClockReset, In<b8, (), 3>)>> {
        (0..24u128)
            .map(|k| In::<b8, (), 3> {
                data: [
                    if k % 2 == 0 { Some(item(k)) } else { None },
                    if k % 3 == 0 {
                        Some(item(100 + k))
                    } else {
                        None
                    },
                    if k % 5 == 0 {
                        Some(item(200 + k))
                    } else {
                        None
                    },
                ],
                ready: k % 4 != 0,
            })
            .with_reset(1)
            .clock_pos_edge(100)
    }

    /// Tier 4 — `iverilog` round-trip, RTL and NTL.
    #[test]
    fn iverilog_round_trip() -> miette::Result<()> {
        let uut = Mux::default();
        let tb = uut.run(open_loop()).collect::<SynchronousTestBench<_, _>>();
        // Each CreditSink holds a SyncFIFO (BRAM) and a non-zero-reset
        // grant counter.  Verilog's `initial` block sets those at time 0
        // while the Rust simulator starts from dont_care; they agree
        // after the first edge.  Same documented `.skip(2)` the sink's
        // own round-trip uses.
        let opts = TestBenchOptions::default().skip(2);
        tb.rtl(&uut, &opts)?.run_iverilog()?;
        tb.ntl(&uut, &opts)?.run_iverilog()?;
        Ok(())
    }

    /// Tier 5 — VCD digest.
    #[test]
    fn trace_digest() -> miette::Result<()> {
        let uut = Mux::default();
        let vcd = uut.run(open_loop()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("credit_mux");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect_test::expect![
            "28ca467aea4bca193a7b2733f4d9258c5fe9a0abd75aea6d36f2b04952135acc"
        ];
        let digest = vcd.dump_to_file(root.join("credit_mux.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
