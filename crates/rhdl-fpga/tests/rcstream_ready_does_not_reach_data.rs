//! `data` must never depend combinationally on `ready`.
//!
//! This is the one rule `stream-bus-architecture.md` §2 singles out as
//! the thing everybody gets wrong:
//!
//! > The AMBA spec says TREADY *may* depend combinationally on TVALID
//! > but TVALID *must not* depend on TREADY. Most third-party IP gets
//! > this wrong at least once.
//!
//! RCStream inherits the hazard, because its `data` and `ready` travel
//! in opposite directions on the same bus struct: a widget's `i.ready`
//! is the *downstream's* ready arriving backwards, and its `o.data` is
//! the forward signal leaving. A combinational path from the first to
//! the second is a backward-to-forward dependency, and composing two
//! such widgets closes a loop with no register in it.
//!
//! Nothing checked it. Every other guarantee the bus makes is tested by
//! simulation, and this one cannot be: a violating widget simulates
//! perfectly well on its own and only fails when it meets another one,
//! or when timing closure fails on real silicon. It is a *structural*
//! property, and the combinational-reachability matrix added to
//! `Descriptor` makes it decidable without running anything.
//!
//! # What this would have caught
//!
//! §2 lists the AXI4-Stream failure as "the most common AXI4-Stream
//! timing-closure problem", and §8's whole argument for relay insertion
//! being free rests on the bus not having such paths. A widget with one
//! would make `RCStreamRelay` insertion *not* behaviour-preserving in
//! the composition that exposed it — so this test defends the premise
//! of `tests/rcstream_relay_insertion.rs` rather than duplicating it.

use rhdl::prelude::*;
use rhdl_fpga::rcstream::bus::RCStream;

/// Does this input path carry flow control *backwards*, from sink to
/// source?
///
/// Two spellings, because the bus has two flow-control styles: the
/// ready/valid variant signals with `ready`, and the credit variant
/// with `credit_grant`. Both travel the same direction and create the
/// same hazard, so both are classified the same way — which is why the
/// property below is stated as "backward input to forward output"
/// rather than in terms of `ready` alone.
fn is_backward_input(path: &str) -> bool {
    path.contains("ready") || path.contains("credit_grant")
}

/// Does this output path carry data *forwards*, from source to sink?
fn is_forward_output(path: &str) -> bool {
    path.contains("data")
}

/// Every `(backward input, forward output)` pair with a combinational
/// path between them. Empty is the only acceptable answer.
fn backward_to_forward<T: Synchronous>(uut: &T) -> Vec<String> {
    let desc = uut.descriptor(ScopedName::top()).expect("descriptor");
    let m = &desc.combinational_reachability;
    assert!(
        m.is_known(),
        "the matrix must be known for this check to mean anything"
    );
    let mut bad = Vec::new();
    for (ri, ip) in m.inputs.iter().enumerate() {
        let inp = format!("{ip:?}");
        if !is_backward_input(&inp) {
            continue;
        }
        for (ci, op) in m.outputs.iter().enumerate() {
            let outp = format!("{op:?}");
            if !is_forward_output(&outp) {
                continue;
            }
            if m.i_to_o.get(ri, ci) {
                bad.push(format!("{inp} -> {outp}"));
            }
        }
    }
    bad
}

/// How many backward inputs and forward outputs a widget has.
///
/// Used to refuse a vacuous pass: with none of either,
/// [`backward_to_forward`] returns an empty list because there was
/// nothing to compare, not because the property holds.
fn port_counts<T: Synchronous>(uut: &T) -> (usize, usize) {
    let desc = uut.descriptor(ScopedName::top()).expect("descriptor");
    let m = &desc.combinational_reachability;
    let r = m
        .inputs
        .iter()
        .filter(|p| is_backward_input(&format!("{p:?}")))
        .count();
    let d = m
        .outputs
        .iter()
        .filter(|p| is_forward_output(&format!("{p:?}")))
        .count();
    (r, d)
}

/// Assert the property *and* that there was something to assert it
/// about.
fn ports<T: Synchronous>(uut: &T) -> String {
    let desc = uut.descriptor(ScopedName::top()).expect("descriptor");
    let m = &desc.combinational_reachability;
    format!(
        "inputs: {:?}\n  outputs: {:?}",
        m.inputs
            .iter()
            .map(|p| format!("{p:?}"))
            .collect::<Vec<_>>(),
        m.outputs
            .iter()
            .map(|p| format!("{p:?}"))
            .collect::<Vec<_>>()
    )
}

fn check<T: Synchronous>(label: &str, uut: &T) {
    let (readys, datas) = port_counts(uut);
    assert!(
        readys > 0,
        "{label}: no backward (`ready` / `credit_grant`) input path \
         found, so this check would be vacuous. Either the port naming \
         changed, or this widget genuinely has no backward input -- in \
         which case it belongs in the exclusion list below, with a \
         reason.\n  {}",
        ports(uut)
    );
    assert!(
        datas > 0,
        "{label}: no `data` output path found, so this check would be \
         vacuous.\n  {}",
        ports(uut)
    );
    let bad = backward_to_forward(uut);
    assert!(
        bad.is_empty(),
        "{label}: a forward output depends combinationally on a backward \
         input, which is the AXI4-Stream mistake \
         stream-bus-architecture.md §2 names. Two such widgets composed \
         close a combinational loop with no register in it.\n  {}",
        bad.join("\n  ")
    );
}

// ------------------------------------------------- the positive control

/// A widget that deliberately gets it wrong: its forward `data` is
/// gated by the backward `ready`.
///
/// Without this, every assertion in this file could be passing because
/// the checker never finds anything -- which is exactly how a
/// structural test rots. This is the counterexample that proves the
/// checker works.
mod wrong {
    use rhdl::prelude::*;
    use rhdl_fpga::rcstream::bus::RCStream;

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct ReadyGatesData {}

    impl SynchronousIO for ReadyGatesData {
        type I = RCStream<b8, ()>;
        type O = RCStream<b8, ()>;
        type Kernel = ready_gates_data;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn ready_gates_data(_cr: ClockReset, i: RCStream<b8, ()>, _q: Q) -> (RCStream<b8, ()>, D) {
        let d = D::dont_care();
        // The mistake, in one line: the forward data is only presented
        // when the *downstream* says it is ready.
        let out = RCStream::<b8, ()> {
            data: if i.ready { i.data } else { None },
            ready: i.ready,
        };
        (out, d)
    }
}

/// **The checker catches a widget that does it wrong.**
#[test]
fn the_check_is_not_vacuous() {
    let uut = wrong::ReadyGatesData::default();
    let (readys, datas) = port_counts(&uut);
    assert!(readys > 0 && datas > 0, "ports found: {readys} / {datas}");
    let bad = backward_to_forward(&uut);
    assert!(
        !bad.is_empty(),
        "a widget whose `o.data` is gated by `i.ready` must be reported; \
         if this fails, every other assertion in this file is worthless"
    );
}

// ------------------------------------------- the library, widget by widget

/// Trivial synthesizable functions, so the function-carrying
/// combinators can be instantiated. What the function *computes* is
/// irrelevant here -- this is a structural check on the handshake
/// wiring, which cannot depend on it.
mod funcs {
    use rhdl::prelude::*;
    use rhdl_fpga::rcstream::bus::Item;

    #[kernel]
    #[doc(hidden)]
    pub fn identity(_cr: ClockReset, x: b8) -> b8 {
        x
    }

    #[kernel]
    #[doc(hidden)]
    pub fn accept_all(_cr: ClockReset, _it: Item<b8, bool>) -> bool {
        true
    }

    #[kernel]
    #[doc(hidden)]
    pub fn pass_through(_cr: ClockReset, it: Item<b8, bool>) -> Option<Item<b8, bool>> {
        Some(it)
    }
}

#[test]
fn the_single_stream_combinators_are_clean() {
    use rhdl_fpga::rcstream::{filter, filter_map, map, relay};
    check(
        "map",
        &map::RCStreamMap::<b8, bool, b8>::try_new::<funcs::identity>().expect("map"),
    );
    check(
        "filter",
        &filter::RCStreamFilter::<b8, bool>::try_new::<funcs::accept_all>().expect("filter"),
    );
    check(
        "filter_map",
        &filter_map::RCStreamFilterMap::<b8, bool, b8>::try_new::<funcs::pass_through>()
            .expect("filter_map"),
    );
    check("relay", &relay::RCStreamRelay::<b8, bool>::default());
}

#[test]
fn the_multi_port_combinators_are_clean() {
    use rhdl_fpga::rcstream::{fanout, tee, zip};
    check("tee", &tee::RCStreamTee::<b8, bool, b8, bool>::default());
    check("zip", &zip::RCStreamZip::<b8, bool, b8, bool>::default());
    check("fanout", &fanout::RCStreamFanout::<b8, bool, 3>::default());
}

#[test]
fn the_width_changing_combinators_are_clean() {
    use rhdl_fpga::rcstream::{chunked, flatten};
    check(
        "chunked",
        &chunked::RCStreamChunked::<b8, bool, 4, 2>::default(),
    );
    check(
        "flatten",
        &flatten::RCStreamFlatten::<b8, bool, 4, 2>::default(),
    );
}

#[test]
fn the_utility_widgets_are_clean() {
    use rhdl_fpga::rcstream::util::{combine, split};
    check("iq_combine", &combine::IqCombine::<8, bool>::default());
    check("iq_split", &split::IqSplit::<8, bool>::default());
}

/// **`RCStreamConstant` is excluded, and this is why.**
///
/// It has no inputs at all: its own docs explain that a constant source
/// ignores backpressure deliberately, because its samples are not
/// specific and dropping one costs nothing. With no backward input
/// there is no hazard of the kind this file is about — the property is
/// not satisfied, it is inapplicable.
///
/// Stated as an assertion rather than a comment so the exclusion cannot
/// outlive its reason. If the widget ever gains a backward input, this
/// fails and whoever added it has to decide whether the real check
/// applies.
#[test]
fn the_constant_source_has_no_backward_input_to_check() {
    use rhdl_fpga::rcstream::util::constant;
    let uut = constant::RCStreamConstant::<b8, bool>::default();
    let (backward, forward) = port_counts(&uut);
    assert_eq!(
        backward,
        0,
        "RCStreamConstant has gained a backward input; it is no longer \
         exempt from the forward/backward independence check and should \
         be moved into `the_utility_widgets_are_clean`.\n  {}",
        ports(&uut)
    );
    assert!(
        forward > 0,
        "it is still a source, so it must still have a data output"
    );
}

#[test]
fn the_credit_widgets_are_clean() {
    use rhdl_fpga::rcstream::credit::{mux, relay, sink, source};
    check(
        "credit_source",
        &source::CreditSource::<b8, bool, 5>::default(),
    );
    check(
        "credit_sink",
        &sink::CreditSink::<b8, bool, 5, 4>::default(),
    );
    check(
        "credit_mux",
        &mux::CreditMux::<b8, bool, 5, 4, 2, 2>::default(),
    );
    check(
        "credit_relay",
        &relay::CreditRCStreamRelay::<b8, bool, 5>::default(),
    );
}

#[test]
fn the_axi_stream_bridges_are_clean() {
    use rhdl_fpga::rcstream::axi_stream::{axi_to_rcstream, rcstream_to_axi};
    check(
        "axi_to_rcstream",
        &axi_to_rcstream::AxiToRCStream::<b8, bool>::default(),
    );
    check(
        "rcstream_to_axi",
        &rcstream_to_axi::RCStreamToAxi::<b8, bool>::default(),
    );
}

/// A sanity check on the other direction, so that "no path from ready to
/// data" is not mistaken for "no paths at all".
///
/// `ready` is *permitted* to depend on `data` — that is the direction
/// AMBA allows and the one a zero-latency combinational combinator would
/// use. This asserts the matrix is actually populated for a widget we
/// know has forward paths, so a future change that silently emptied it
/// would be caught here rather than making every assertion above pass
/// for free.
#[test]
fn the_matrix_is_populated_and_not_simply_empty() {
    let uut = wrong::ReadyGatesData::default();
    let desc = uut.descriptor(ScopedName::top()).expect("descriptor");
    let m = &desc.combinational_reachability;
    assert!(
        m.i_to_o.any(),
        "this widget is entirely combinational, so its input-to-output \
         matrix cannot be empty"
    );
    let _ = RCStream::<b8, ()> {
        data: None,
        ready: false,
    };
}

/// Prints every widget's port paths. Not an assertion — a way to see
/// what the matrix actually names, which is how the two exclusions
/// below were established.
#[test]
#[ignore = "diagnostic; run with --ignored --nocapture to see port names"]
fn dump_ports() {
    use rhdl_fpga::rcstream::credit::{mux, relay, sink, source};
    use rhdl_fpga::rcstream::util::constant;
    println!(
        "credit_source\n  {}",
        ports(&source::CreditSource::<b8, bool, 5>::default())
    );
    println!(
        "credit_sink\n  {}",
        ports(&sink::CreditSink::<b8, bool, 5, 4>::default())
    );
    println!(
        "credit_mux\n  {}",
        ports(&mux::CreditMux::<b8, bool, 5, 4, 2, 2>::default())
    );
    println!(
        "credit_relay\n  {}",
        ports(&relay::CreditRCStreamRelay::<b8, bool, 5>::default())
    );
    println!(
        "constant\n  {}",
        ports(&constant::RCStreamConstant::<b8, bool>::default())
    );
}
