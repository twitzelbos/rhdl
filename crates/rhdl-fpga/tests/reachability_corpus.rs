//! Does the reachability matrix agree with reality on real widgets?
//!
//! The matrix is new and nothing depends on it yet, so the only way to
//! know it is right is to check it against something that was already
//! true. Two such things exist: `no_combinatorial_paths`, which answers
//! the same question for a whole design by a completely different route
//! (flatten to a netlist, ask once), and the widgets themselves, whose
//! shapes we know by construction -- a `DFF` cannot feed through, and a
//! combinational map must.

use rhdl::prelude::*;
use rhdl_fpga::core::{counter, dff};

fn feeds_through<T: Synchronous>(uut: &T) -> bool {
    uut.descriptor(ScopedName::top())
        .expect("descriptor")
        .combinational_reachability
        .has_feedthrough()
}

/// A register cannot feed through. This is the whole reason a `DFF`
/// exists, and if the matrix got this wrong every sequential design
/// would look like a combinational loop.
#[test]
fn a_dff_does_not_feed_through() {
    assert!(!feeds_through(&dff::DFF::<b8>::default()));
}

/// A counter's output is its register's output, so nothing reaches it
/// from the input combinationally either -- even though the input does
/// influence the register.
#[test]
fn a_counter_does_not_feed_through() {
    assert!(!feeds_through(&counter::Counter::<4>::default()));
}

/// And the matrix agrees with the existing check, which reaches the same
/// verdict by flattening the design instead of composing per-widget
/// matrices. Two independent routes to one answer.
#[test]
fn the_matrix_agrees_with_the_existing_drc() {
    let counter = counter::Counter::<4>::default();
    let drc_says_clean = rhdl::core::circuit::drc::no_combinatorial_paths(&counter).is_ok();
    assert_eq!(drc_says_clean, !feeds_through(&counter));

    let dff = dff::DFF::<b8>::default();
    let drc_says_clean = rhdl::core::circuit::drc::no_combinatorial_paths(&dff).is_ok();
    assert_eq!(drc_says_clean, !feeds_through(&dff));
}

/// A widget that *does* have a combinational path must say so.
///
/// Without this the suite would pass with a matrix that is all-false
/// everywhere, which is exactly the failure mode a set of "no
/// feedthrough" assertions cannot catch. `faulty_reducer` exists in the
/// tree precisely because it has the path, and the existing DRC test
/// asserts that it is reported -- so the two checks should agree on the
/// interesting case as well as the boring ones.
#[test]
fn a_widget_with_a_real_combinational_path_is_reported() {
    // Same shape as `tests/faulty_reducer.rs`, which the existing DRC
    // test uses as its positive case.
    let uut = faulty::U::<4, 2>::default();
    let drc_says_clean = rhdl::core::circuit::drc::no_combinatorial_paths(&uut).is_ok();
    assert!(
        !drc_says_clean,
        "premise: this widget has a combinational path"
    );
    assert!(
        feeds_through(&uut),
        "the DRC finds a combinational path here and the matrix does not"
    );
}

/// The matrix is shaped from the widget's own types, so its row and
/// column counts must match the leaf-field counts of `I` and `O`.
#[test]
fn the_matrix_is_shaped_like_the_widget() {
    let d = faulty::U::<4, 2>::default()
        .descriptor(ScopedName::top())
        .expect("descriptor");
    let m = &d.combinational_reachability;
    assert_eq!(m.i_to_o.rows(), m.inputs.len());
    assert_eq!(m.i_to_o.cols(), m.outputs.len());
    assert!(!m.inputs.is_empty());
    assert!(!m.outputs.is_empty());
}

#[path = "faulty_reducer.rs"]
mod faulty;

/// All four relations, on a widget whose shape we know exactly.
///
/// `Counter<N>` is one `bool` input, one `Bits<N>` output, and a single
/// `DFF` child. So every relation has a known answer:
///
/// - `i_to_o` false — the output is the register's output.
/// - `i_to_d` true — `enable` decides what the register loads.
/// - `q_to_o` true — the output *is* the register's output.
/// - `q_to_d` true — the next count is computed from the current one.
///
/// A matrix that got any of these backwards would still pass a
/// feedthrough-only assertion, which is why this checks all four. The
/// `q_to_d` entry is the one Phase 3 depends on: it is the channel
/// through which two children of one parent can form a loop.
#[test]
fn all_four_relations_are_right_for_a_counter() {
    let d = counter::Counter::<4>::default()
        .descriptor(ScopedName::top())
        .expect("descriptor");
    let m = &d.combinational_reachability;
    assert!(!m.i_to_o.any(), "a counter has no feedthrough");
    assert!(
        m.i_to_d.any(),
        "`enable` must reach the register's input, or the counter cannot count"
    );
    assert!(
        m.q_to_o.any(),
        "the output is the register's output, so q must reach o"
    );
    assert!(
        m.q_to_d.any(),
        "the next count is computed from the current one"
    );
}

/// The `DFF` child contributes no edges, and that is what makes the
/// parent's `i_to_o` false despite `i_to_d` and `q_to_o` both being true.
///
/// This is the composition step in miniature: `i -> d` and `q -> o` both
/// hold, so if the child had a feedthrough the parent would have one
/// too. It does not, so the path is broken exactly where the register is.
#[test]
fn the_register_is_where_the_path_breaks() {
    let d = counter::Counter::<4>::default()
        .descriptor(ScopedName::top())
        .expect("descriptor");
    let m = &d.combinational_reachability;
    assert!(m.i_to_d.any() && m.q_to_o.any());
    assert!(
        !m.i_to_o.any(),
        "i->d and q->o both hold, so i->o can only be false because the child broke it"
    );
}

/// The asynchronous path computes a matrix too, and does not merely
/// return the empty default.
///
/// An asynchronous kernel is `fn(i, q)` rather than `fn(clock_reset,
/// i, q)`, so `i` sits at a different port index. Reading the wrong port
/// would silently produce an all-false matrix -- which looks exactly like
/// a widget with no feedthrough, and would be invisible to any test that
/// only asserts the absence of one. `Sync1Bit` is the smallest widget on
/// that path.
#[test]
fn the_asynchronous_path_is_wired_too() {
    use rhdl_fpga::cdc::synchronizer::Sync1Bit;
    let d = Sync1Bit::<Red, Blue>::default()
        .descriptor(ScopedName::top())
        .expect("descriptor");
    let m = &d.combinational_reachability;
    assert!(
        !m.inputs.is_empty() && !m.outputs.is_empty(),
        "the async matrix was not shaped from the widget's types"
    );
    // A synchroniser is registers all the way through, so the useful
    // assertion is that it says so having actually looked.
    assert!(
        !m.has_feedthrough(),
        "a two-flop synchroniser cannot feed through"
    );
}

/// A composite asynchronous widget wires its children up, so at least
/// one of the composition relations must be non-empty.
///
/// Without this, the async test above would pass on a matrix that is
/// empty for the boring reason rather than the interesting one.
#[test]
fn an_async_composite_populates_its_composition_relations() {
    use rhdl_fpga::fifo::asynchronous::AsyncFIFO;
    let d = AsyncFIFO::<b8, Red, Blue, 4>::default()
        .descriptor(ScopedName::top())
        .expect("descriptor");
    let m = &d.combinational_reachability;
    // Not merely non-empty: a childless widget used to report one
    // zero-width `d_path`, which made this assertion vacuous.
    assert!(
        m.d_paths.len() > 1,
        "an AsyncFIFO has several children, got {} d paths",
        m.d_paths.len()
    );
    assert!(
        m.i_to_d.any() || m.q_to_d.any(),
        "something must reach the children's inputs, or the FIFO is inert"
    );
}

/// Committed expectations for a spread of real widgets.
///
/// The tests above check properties; this one pins the actual numbers, so
/// that a change to the analysis shows up as a diff a reviewer can read
/// rather than as a property that happens to still hold. The rendering is
/// deliberately terse -- shapes and per-relation densities -- because the
/// full matrices for a FIFO run to hundreds of entries and nobody would
/// audit that.
#[test]
fn the_corpus_matrices_are_as_expected() {
    use expect_test::expect;
    use rhdl_fpga::fifo::synchronous::SyncFIFO;

    fn describe<T: Synchronous>(label: &str, uut: &T) -> String {
        let d = uut.descriptor(ScopedName::top()).expect("descriptor");
        let m = &d.combinational_reachability;
        let density = |b: &rhdl::core::circuit::reachability::BitMatrix| {
            let set: usize = (0..b.rows()).map(|r| b.row_iter(r).count()).sum();
            format!("{}x{} set={set}", b.rows(), b.cols())
        };
        format!(
            "{label}\n  i={} o={} d={} q={}\n  i_to_o {}\n  i_to_d {}\n  q_to_o {}\n  q_to_d {}\n",
            m.inputs.len(),
            m.outputs.len(),
            m.d_paths.len(),
            m.q_paths.len(),
            density(&m.i_to_o),
            density(&m.i_to_d),
            density(&m.q_to_o),
            density(&m.q_to_d),
        )
    }

    let mut out = String::new();
    out.push_str(&describe("dff::DFF<b8>", &dff::DFF::<b8>::default()));
    out.push_str(&describe(
        "counter::Counter<4>",
        &counter::Counter::<4>::default(),
    ));
    out.push_str(&describe("SyncFIFO<b8, 4>", &SyncFIFO::<b8, 4>::default()));
    out.push_str(&describe("faulty::U<4, 2>", &faulty::U::<4, 2>::default()));

    let expected = expect![[r#"
        dff::DFF<b8>
          i=1 o=1 d=0 q=0
          i_to_o 1x1 set=0
          i_to_d 1x0 set=0
          q_to_o 0x1 set=0
          q_to_d 0x0 set=0
        counter::Counter<4>
          i=1 o=1 d=1 q=1
          i_to_o 1x1 set=0
          i_to_d 1x1 set=1
          q_to_o 1x1 set=1
          q_to_d 1x1 set=1
        SyncFIFO<b8, 4>
          i=3 o=7 d=8 q=11
          i_to_o 3x7 set=5
          i_to_d 3x8 set=8
          q_to_o 11x7 set=18
          q_to_d 11x8 set=6
        faulty::U<4, 2>
          i=3 o=3 d=2 q=2
          i_to_o 3x3 set=1
          i_to_d 3x2 set=5
          q_to_o 2x3 set=4
          q_to_d 2x2 set=3
    "#]];
    expected.assert_eq(&out);
}

/// Phase 2's acceptance criterion: the matrix and the netlist walk agree
/// on every widget in a spread of the library.
///
/// `no_combinatorial_paths` now takes its verdict from the matrix. That
/// is only safe if the matrix says what the netlist walk said, and the
/// dangerous direction is the quiet one: a widget whose matrix is empty
/// because nobody wired its descriptor builder reads as "no feedthrough"
/// and the check silently passes. Phase 1 left five such builders
/// defaulted -- `function`, `array`, `chain`, `adapter`, `phantom` -- and
/// four of them needed real composition logic before this test could
/// pass.
///
/// The spread is deliberately wide rather than deep: one widget per
/// structural shape, because what is being tested is the *composition*,
/// not any one widget's logic.
#[test]
fn the_matrix_and_the_netlist_walk_agree_across_the_corpus() {
    use rhdl::core::circuit::drc::feedthrough_by_netlist_walk;

    macro_rules! check {
        ($label:expr, $uut:expr) => {{
            let uut = $uut;
            let matrix = feeds_through(&uut);
            let walk = feedthrough_by_netlist_walk(&uut).expect("netlist walk");
            assert_eq!(
                matrix, walk,
                "{}: matrix says {matrix}, netlist walk says {walk}",
                $label
            );
            (($label), matrix)
        }};
    }

    let verdicts = vec![
        check!("dff", dff::DFF::<b8>::default()),
        check!("counter", counter::Counter::<4>::default()),
        check!("faulty_reducer", faulty::U::<4, 2>::default()),
        check!(
            "sync_fifo",
            rhdl_fpga::fifo::synchronous::SyncFIFO::<b8, 4>::default()
        ),
        check!("delay", rhdl_fpga::core::delay::Delay::<b8, 3>::default()),
        check!(
            "constant",
            rhdl_fpga::core::constant::Constant::<b8>::new(bits(3))
        ),
        check!(
            "rcstream_map",
            rhdl_fpga::rcstream::relay::RCStreamRelay::<b8, ()>::default()
        ),
    ];

    // And the corpus must contain at least one of each verdict, or the
    // agreement above is agreement on a constant.
    assert!(
        verdicts.iter().any(|(_, v)| *v),
        "no widget in the corpus feeds through; the test proves nothing"
    );
    assert!(
        verdicts.iter().any(|(_, v)| !*v),
        "every widget feeds through; the test proves nothing"
    );
}
