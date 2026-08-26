//! Cycles of three, four and five widgets.
//!
//! `logic_loop.rs` covers the two-widget case and pins the rendered
//! diagnostic. These check that the detector finds the cycle at all when
//! it is longer, and — more usefully — that the reported walk names the
//! right widgets in the right order. A detector that says "there is a
//! cycle" without saying where is only marginally better than a
//! compile error.

use rhdl::prelude::*;

/// A combinational widget: whatever goes in comes out, inverted.
mod inv {
    use rhdl::prelude::*;
    #[derive(Clone, Debug, Synchronous, Default)]
    pub struct U;
    impl SynchronousIO for U {
        type I = bool;
        type O = bool;
        type Kernel = inv;
    }
    impl SynchronousDQ for U {
        type D = ();
        type Q = ();
    }
    #[kernel]
    pub fn inv(_cr: ClockReset, i: bool, _q: ()) -> (bool, ()) {
        (!i, ())
    }
}

/// Pull the cycle description out of whatever error `descriptor` gave.
fn cycle_of<T: Synchronous>(uut: &T) -> rhdl::core::circuit::error::CombinationalCycle {
    match uut.descriptor(ScopedName::top()) {
        Ok(_) => panic!("expected a combinational cycle to be reported"),
        Err(RHDLError::CombinationalCycle(c)) => *c,
        Err(other) => panic!("expected a combinational cycle, got: {other}"),
    }
}

macro_rules! ring {
    ($modname:ident, $($field:ident),+ ; $($src:ident),+) => {
        mod $modname {
            use super::inv;
            use rhdl::prelude::*;

            #[derive(Clone, Debug, Synchronous, Default)]
            pub struct U {
                $(pub $field: inv::U),+
            }
            #[derive(PartialEq, Default, Clone, Copy, Digital)]
            pub struct D { $(pub $field: bool),+ }
            #[derive(PartialEq, Default, Clone, Copy, Digital)]
            pub struct Q { $(pub $field: bool),+ }
            impl SynchronousIO for U {
                type I = bool;
                type O = bool;
                type Kernel = ring;
            }
            impl SynchronousDQ for U {
                type D = D;
                type Q = Q;
            }
            // Each widget's input is fed from the previous widget's
            // output, and the first from the last -- a ring with no
            // register anywhere on it.
            #[kernel]
            pub fn ring(_cr: ClockReset, i: bool, q: Q) -> (bool, D) {
                let mut d = D::default();
                if i {
                    $(d.$field = q.$src;)+
                }
                (q.a, d)
            }
        }
    };
}

// a <- c, b <- a, c <- b: a three-widget ring.
ring!(three, a, b, c ; c, a, b);
// a <- d, b <- a, c <- b, d <- c.
ring!(four, a, b, c, d ; d, a, b, c);
// a <- e, b <- a, c <- b, d <- c, e <- d.
ring!(five, a, b, c, d, e ; e, a, b, c, d);

#[test]
fn a_three_widget_cycle_names_all_three() {
    let c = cycle_of(&three::U::default());
    assert_eq!(c.widget_count(), 3, "walk was {}", c.walk());
    // The walk is closed, so it names one more step than there are
    // widgets, and the first and last are the same.
    let walk = c.walk();
    let steps: Vec<&str> = walk.split(" -> ").collect();
    assert_eq!(steps.len(), 4, "walk was {}", c.walk());
    assert_eq!(steps.first(), steps.last());
}

#[test]
fn a_four_widget_cycle_names_all_four() {
    let c = cycle_of(&four::U::default());
    assert_eq!(c.widget_count(), 4, "walk was {}", c.walk());
    assert_eq!(c.walk().split(" -> ").count(), 5, "walk was {}", c.walk());
}

#[test]
fn a_five_widget_cycle_names_all_five() {
    let c = cycle_of(&five::U::default());
    assert_eq!(c.widget_count(), 5, "walk was {}", c.walk());
    assert_eq!(c.walk().split(" -> ").count(), 6, "walk was {}", c.walk());
}

/// Every hop is labelled with a span, and the closing one says so.
///
/// The span is what makes the diagnostic actionable: without it the user
/// is told there is a ring but not which line built it.
#[test]
fn every_hop_is_labelled_and_the_closing_one_is_marked() {
    let c = cycle_of(&four::U::default());
    assert_eq!(
        c.elements.len(),
        4,
        "expected one label per hop, got {:?}",
        c.elements.iter().map(|(l, _)| l).collect::<Vec<_>>()
    );
    let closing = c
        .elements
        .iter()
        .filter(|(l, _)| l.as_deref().is_some_and(|l| l.contains("closes the cycle")))
        .count();
    assert_eq!(closing, 1, "exactly one hop closes the cycle");
}
