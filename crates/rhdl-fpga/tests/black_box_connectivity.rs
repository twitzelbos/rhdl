//! A *combinational* black box is now visible to the path and loop checks.
//!
//! Before connectivity was declared, `ntl::graph::make_net_graph` skipped
//! every `BlackBox` op when adding edges. That is what made a `DFF` break
//! a combinational path, and it was an assumption about the black boxes
//! that happened to exist rather than a property any of them stated — so
//! a black box that genuinely did feed through was invisible to
//! `no_combinatorial_paths` and to the composition-level cycle detector
//! alike.
//!
//! These tests are the ones that failed before the change. Everything
//! else about it is a migration whose acceptance criterion is that
//! nothing moves.

use quote::format_ident;
use rhdl::core::circuit::descriptor::SyncKind;
use rhdl::prelude::*;
use syn::parse_quote;

/// An inverter with no Rust body: its Verilog is `assign o = ~i;` and the
/// compiler cannot see inside it.
///
/// `MODE` chooses what it *claims*, so one module covers every variant:
///
/// - `0` — `None`, claiming to register what it carries. The lie, and
///   what the old behaviour assumed of everything.
/// - `1` — `Paths`, the truth for this module.
/// - `2` — `Opaque`, which claims nothing and must therefore be assumed
///   to connect everything. The variant Phase 2 makes the default, so it
///   needs exercising before it becomes one.
#[derive(Clone, Debug, Default)]
pub struct BlackInverter<const MODE: u8>;

impl<const MODE: u8> SynchronousIO for BlackInverter<MODE> {
    type I = bool;
    type O = bool;
    type Kernel = NoSynchronousKernel<ClockReset, bool, (), (bool, ())>;
}

impl<const MODE: u8> SynchronousDQ for BlackInverter<MODE> {
    type D = ();
    type Q = ();
}

impl<const MODE: u8> Synchronous for BlackInverter<MODE> {
    type S = ();

    fn init(&self) -> Self::S {}

    fn sim(&self, _cr: ClockReset, input: Self::I, _state: &mut Self::S) -> Self::O {
        !input
    }

    fn descriptor(&self, scoped_name: ScopedName) -> Result<Descriptor<SyncKind>, RHDLError> {
        let name = scoped_name.to_string();
        let module_name = format_ident!("{}", name);
        let module: vlog::ModuleDef = parse_quote! {
            module #module_name(
                input wire [1:0] clock_reset,
                input wire [0:0] i,
                output wire [0:0] o
            );
                assign o = ~i;
            endmodule
        };
        Descriptor::<SyncKind> {
            combinational_reachability: Default::default(),
            name: scoped_name,
            input_kind: Self::I::static_kind(),
            output_kind: Self::O::static_kind(),
            d_kind: Kind::Empty,
            q_kind: Kind::Empty,
            kernel: None,
            hdl: Some(HDLDescriptor {
                name,
                modules: module.into(),
            }),
            netlist: None,
            _phantom: std::marker::PhantomData,
        }
        .with_netlist_black_box(match MODE {
            0 => BlackBoxConnectivity::None,
            1 => BlackBoxConnectivity::Paths(vec![(Path::default(), Path::default())]),
            _ => BlackBoxConnectivity::Opaque,
        })
    }
}

/// The Verilog every black box in this file instantiates.
fn inverter_hdl(name: &str) -> HDLDescriptor {
    let module_name = format_ident!("{}", name);
    let module: vlog::ModuleDef = parse_quote! {
        module #module_name(
            input wire [1:0] clock_reset,
            input wire [0:0] i,
            output wire [0:0] o
        );
            assign o = ~i;
        endmodule
    };
    HDLDescriptor {
        name: name.into(),
        modules: module.into(),
    }
}

/// The same Verilog, for the asynchronous widgets (no clock port).
fn inverter_hdl_async(name: &str) -> HDLDescriptor {
    let module_name = format_ident!("{}", name);
    let module: vlog::ModuleDef = parse_quote! {
        module #module_name(
            input wire [0:0] i,
            output wire [0:0] o
        );
            assign o = ~i;
        endmodule
    };
    HDLDescriptor {
        name: name.into(),
        modules: module.into(),
    }
}

/// A combinational black box is reported as a feedthrough.
///
/// This is the check that could not fire before: the DRC took its verdict
/// from a matrix that was empty for every black box, and before that from
/// a graph that skipped them.
#[test]
fn a_combinational_black_box_is_a_feedthrough() {
    let honest = BlackInverter::<1>::default();
    assert!(
        rhdl::core::circuit::drc::no_combinatorial_paths(&honest).is_err(),
        "`assign o = ~i;` is a combinational path from input to output"
    );
}

/// And one that declares itself registered is not.
///
/// The same Verilog, the same module, a different declaration. Which is
/// the point: the compiler is taking the module's word, and the word is
/// now written down where a reviewer can see it.
#[test]
fn a_black_box_declaring_no_feedthrough_is_not_one() {
    let claims_registered = BlackInverter::<0>::default();
    assert!(
        rhdl::core::circuit::drc::no_combinatorial_paths(&claims_registered).is_ok(),
        "a black box declaring `None` must be treated as breaking the path"
    );
}

/// Two combinational black boxes in a ring are a combinational loop.
mod ring {
    use super::BlackInverter;
    use rhdl::prelude::*;

    #[derive(Clone, Debug, Synchronous, Default)]
    pub struct U<const MODE: u8> {
        pub left: BlackInverter<MODE>,
        pub right: BlackInverter<MODE>,
    }

    #[derive(PartialEq, Default, Clone, Copy, Digital)]
    pub struct D {
        pub left: bool,
        pub right: bool,
    }
    #[derive(PartialEq, Default, Clone, Copy, Digital)]
    pub struct Q {
        pub left: bool,
        pub right: bool,
    }

    impl<const MODE: u8> SynchronousIO for U<MODE> {
        type I = bool;
        type O = bool;
        type Kernel = ring_kernel;
    }
    impl<const MODE: u8> SynchronousDQ for U<MODE> {
        type D = D;
        type Q = Q;
    }

    #[kernel]
    pub fn ring_kernel(_cr: ClockReset, _i: bool, q: Q) -> (bool, D) {
        let mut d = D::default();
        d.left = q.right;
        d.right = q.left;
        (q.left, d)
    }
}

/// The loop the compiler used to insist did not exist.
///
/// Two black boxes, each passing its input to its output, wired into a
/// ring with no register anywhere on it. Before connectivity was
/// declared, both contributed an empty matrix, the cycle detector saw no
/// edges through them, and this design built without complaint.
#[test]
fn a_ring_of_combinational_black_boxes_is_a_cycle() {
    let uut = ring::U::<1>::default();
    match uut.descriptor(ScopedName::top()) {
        Ok(_) => panic!("a ring of combinational black boxes is a combinational loop"),
        Err(RHDLError::CombinationalCycle(cycle)) => {
            assert_eq!(cycle.widget_count(), 2, "walk was {}", cycle.walk());
        }
        Err(other) => panic!("expected a combinational cycle, got: {other}"),
    }
}

/// And the same ring built from black boxes that declare `None` is fine.
///
/// Which is what makes the test above meaningful rather than a statement
/// that rings are rejected.
#[test]
fn a_ring_of_registered_black_boxes_is_not_a_cycle() {
    let uut = ring::U::<0>::default();
    assert!(
        uut.descriptor(ScopedName::top()).is_ok(),
        "black boxes declaring `None` break the ring, as a register would"
    );
}

/// A black box fed by kernel logic rather than straight from the inputs.
mod behind_logic {
    use super::BlackInverter;
    use rhdl::prelude::*;

    #[derive(Clone, Debug, Synchronous, Default)]
    pub struct U<const MODE: u8> {
        pub inv: BlackInverter<MODE>,
    }

    #[derive(PartialEq, Default, Clone, Copy, Digital)]
    pub struct D {
        pub inv: bool,
    }
    #[derive(PartialEq, Default, Clone, Copy, Digital)]
    pub struct Q {
        pub inv: bool,
    }

    impl<const MODE: u8> SynchronousIO for U<MODE> {
        type I = bool;
        type O = bool;
        type Kernel = behind_kernel;
    }
    impl<const MODE: u8> SynchronousDQ for U<MODE> {
        type D = D;
        type Q = Q;
    }

    // The input reaches the black box only *through* a kernel operation,
    // and the black box's output reaches the output the same way. So the
    // feedthrough exists only if the edge from that operation into the
    // black box is added.
    #[kernel]
    pub fn behind_kernel(_cr: ClockReset, i: bool, q: Q) -> (bool, D) {
        let mut d = D::default();
        d.inv = !i;
        (!q.inv, d)
    }
}

/// The *netlist graph* finds the path when the black box is fed by an
/// operation, not only when it is fed straight from the primary inputs.
///
/// This is the one branch in the graph builder that resolves a register
/// to the operation writing it, and it needs asserting through
/// `feedthrough_by_netlist_walk` rather than `no_combinatorial_paths`.
/// The latter takes its verdict from the reachability matrix, which is
/// computed by an entirely different route — so it reports this path
/// whether the graph builder is right or wrong, and cannot witness the
/// difference.
///
/// The other tests here cannot witness it either: a standalone black box
/// is fed from the primary inputs, which is the other branch, and the
/// ring goes through the composition-level matrix rather than the graph.
///
/// Verified by reintroducing the fault — the helper returning `None` for
/// an operation source — and watching this assertion fail. A test for a
/// fixed bug is worth nothing until it has been seen to fail.
#[test]
fn the_netlist_graph_follows_a_black_box_fed_by_an_operation() {
    use rhdl::core::circuit::drc::feedthrough_by_netlist_walk;
    let uut = behind_logic::U::<1>::default();
    assert!(
        feedthrough_by_netlist_walk(&uut).expect("netlist walk"),
        "i -> !i -> black box -> !q -> o is a combinational path, and the \
         graph has to follow the edge into the black box to see it"
    );
}

/// And the same shape with the black box declaring `None` is clean by
/// both routes, so the test above is not merely observing that nested
/// widgets fail.
#[test]
fn the_same_shape_with_a_registered_black_box_is_clean() {
    use rhdl::core::circuit::drc::feedthrough_by_netlist_walk;
    let uut = behind_logic::U::<0>::default();
    assert!(
        !feedthrough_by_netlist_walk(&uut).expect("netlist walk"),
        "a black box declaring `None` breaks the path even mid-logic"
    );
    assert!(
        rhdl::core::circuit::drc::no_combinatorial_paths(&uut).is_ok(),
        "and the matrix agrees"
    );
}

/// The two routes agree on a combinational black box behind logic.
///
/// The corpus cross-check in `reachability_corpus.rs` compares the matrix
/// against the netlist walk over a spread of widgets, and is what caught
/// the optimised-versus-raw-NTL discrepancy earlier. It contains no
/// combinational black box, because until now none could exist — so this
/// extends the same comparison to the case that motivated the feature.
#[test]
fn both_routes_agree_on_a_black_box_behind_logic() {
    use rhdl::core::circuit::drc::feedthrough_by_netlist_walk;
    for connected in [true, false] {
        let (matrix, walk) = if connected {
            let uut = behind_logic::U::<1>::default();
            (
                uut.descriptor(ScopedName::top())
                    .expect("descriptor")
                    .combinational_reachability
                    .has_feedthrough(),
                feedthrough_by_netlist_walk(&uut).expect("walk"),
            )
        } else {
            let uut = behind_logic::U::<0>::default();
            (
                uut.descriptor(ScopedName::top())
                    .expect("descriptor")
                    .combinational_reachability
                    .has_feedthrough(),
                feedthrough_by_netlist_walk(&uut).expect("walk"),
            )
        };
        assert_eq!(
            matrix, walk,
            "connected={connected}: matrix says {matrix}, netlist walk says {walk}"
        );
    }
}

/// `Opaque` connects everything, by both routes.
///
/// This is the variant Phase 2 makes the *default* for a module that has
/// not declared, so it needs exercising before it becomes one — up to
/// now nothing in the tree or the tests constructed it, which made
/// `to_matrix`'s and `connect_all`'s handling of it dead code.
#[test]
fn an_opaque_black_box_connects_everything() {
    use rhdl::core::circuit::drc::feedthrough_by_netlist_walk;
    let uut = BlackInverter::<2>::default();
    let m = uut
        .descriptor(ScopedName::top())
        .expect("descriptor")
        .combinational_reachability;
    assert!(m.has_feedthrough(), "Opaque must connect input to output");
    // Every cell, not merely one: the point of the variant is that it
    // claims nothing, so it has to concede everything.
    for r in 0..m.i_to_o.rows() {
        for c in 0..m.i_to_o.cols() {
            assert!(m.i_to_o.get(r, c), "Opaque left ({r},{c}) unconnected");
        }
    }
    assert!(
        feedthrough_by_netlist_walk(&uut).expect("walk"),
        "the netlist graph must follow an opaque black box too"
    );
}

/// And an opaque black box in a ring is a cycle.
///
/// The behaviour Phase 2 relies on: a module that has not declared its
/// connectivity cannot be assumed to break a loop.
#[test]
fn a_ring_of_opaque_black_boxes_is_a_cycle() {
    match ring::U::<2>::default().descriptor(ScopedName::top()) {
        Ok(_) => panic!("a ring of opaque black boxes cannot be assumed acyclic"),
        Err(RHDLError::CombinationalCycle(cycle)) => {
            assert_eq!(cycle.widget_count(), 2, "walk was {}", cycle.walk());
        }
        Err(other) => panic!("expected a combinational cycle, got: {other}"),
    }
}

/// A declared path naming a port the widget does not have is an error.
///
/// The reason the declaration is in terms of `Path` rather than a port
/// name string: a typo becomes a diagnostic instead of a silently missing
/// edge, and a missing edge is exactly what this whole mechanism exists
/// to prevent. Without this test the check was never run.
mod bad_port {
    use rhdl::core::circuit::descriptor::SyncKind;
    use rhdl::prelude::*;

    #[derive(Clone, Debug, Default)]
    pub struct U;

    impl SynchronousIO for U {
        type I = bool;
        type O = bool;
        type Kernel = NoSynchronousKernel<ClockReset, bool, (), (bool, ())>;
    }
    impl SynchronousDQ for U {
        type D = ();
        type Q = ();
    }
    impl Synchronous for U {
        type S = ();
        fn init(&self) -> Self::S {}
        fn sim(&self, _cr: ClockReset, i: Self::I, _s: &mut Self::S) -> Self::O {
            i
        }
        fn descriptor(&self, scoped_name: ScopedName) -> Result<Descriptor<SyncKind>, RHDLError> {
            let hdl = super::inverter_hdl(&scoped_name.to_string());
            Descriptor::<SyncKind> {
                combinational_reachability: Default::default(),
                name: scoped_name,
                input_kind: Self::I::static_kind(),
                output_kind: Self::O::static_kind(),
                d_kind: Kind::Empty,
                q_kind: Kind::Empty,
                kernel: None,
                hdl: Some(hdl),
                netlist: None,
                _phantom: std::marker::PhantomData,
            }
            // `I` is a bare `bool`, so it has no field called `nope`.
            .with_netlist_black_box(BlackBoxConnectivity::Paths(vec![(
                Path::default().field("nope"),
                Path::default(),
            )]))
        }
    }
}

#[test]
fn a_declared_path_naming_a_port_that_does_not_exist_is_rejected() {
    let Err(err) = bad_port::U.descriptor(ScopedName::top()) else {
        panic!("a path naming a nonexistent port must not be accepted");
    };
    let text = format!("{err}");
    assert!(
        text.contains("nope") || matches!(err, RHDLError::BlackBoxPortNotFound { .. }),
        "the error should name the offending port, got: {text}"
    );
}

/// The asynchronous route detects a cycle too.
///
/// Every ring above is `Synchronous`. `build_asynchronous_descriptor`
/// reaches the same check through `compute_asynchronous` and
/// `into_matrix`, and the two differ in a way that matters: an
/// asynchronous kernel is `fn(i, q)` rather than `fn(clock_reset, i, q)`,
/// so `i` sits at a different port index. Reading the wrong one yields an
/// all-false matrix, which looks exactly like a design with no
/// feedthrough and would make this cycle invisible.
mod async_ring {
    use rhdl::core::circuit::descriptor::AsyncKind;
    use rhdl::prelude::*;

    /// An asynchronous combinational black box: `assign o = ~i;`.
    #[derive(Clone, Debug, Default)]
    pub struct AsyncInverter<C: Domain> {
        _c: std::marker::PhantomData<C>,
    }

    impl<C: Domain> CircuitDQ for AsyncInverter<C> {
        type D = ();
        type Q = ();
    }
    impl<C: Domain> CircuitIO for AsyncInverter<C> {
        type I = Signal<bool, C>;
        type O = Signal<bool, C>;
        type Kernel = NoCircuitKernel<Self::I, (), (Self::O, ())>;
    }
    impl<C: Domain> Circuit for AsyncInverter<C> {
        type S = ();
        fn init(&self) -> Self::S {}
        fn sim(&self, input: Self::I, _state: &mut Self::S) -> Self::O {
            signal(!input.val())
        }
        fn descriptor(&self, scoped_name: ScopedName) -> Result<Descriptor<AsyncKind>, RHDLError> {
            let name = scoped_name.to_string();
            Descriptor::<AsyncKind> {
                combinational_reachability: Default::default(),
                name: scoped_name,
                input_kind: <Self::I as Digital>::static_kind(),
                output_kind: <Self::O as Digital>::static_kind(),
                d_kind: Kind::Empty,
                q_kind: Kind::Empty,
                kernel: None,
                netlist: None,
                hdl: Some(super::inverter_hdl_async(&name)),
                _phantom: std::marker::PhantomData,
            }
            .with_netlist_black_box(BlackBoxConnectivity::Paths(vec![(
                Path::default().signal_value(),
                Path::default().signal_value(),
            )]))
        }
    }

    #[derive(Clone, Debug, Default, Circuit)]
    pub struct U {
        pub left: AsyncInverter<Red>,
        pub right: AsyncInverter<Red>,
    }

    // Hand-written, matching the synchronous ring above: the derive wants
    // to name the child types and this test only needs the shape.
    #[derive(PartialEq, Debug, Digital, Clone, Copy)]
    pub struct D {
        pub left: Signal<bool, Red>,
        pub right: Signal<bool, Red>,
    }
    #[derive(PartialEq, Debug, Digital, Clone, Copy)]
    pub struct Q {
        pub left: Signal<bool, Red>,
        pub right: Signal<bool, Red>,
    }
    impl Timed for D {}
    impl Timed for Q {}

    impl CircuitDQ for U {
        type D = D;
        type Q = Q;
    }

    impl CircuitIO for U {
        type I = Signal<bool, Red>;
        type O = Signal<bool, Red>;
        type Kernel = ring_kernel;
    }

    #[kernel]
    pub fn ring_kernel(_i: Signal<bool, Red>, q: Q) -> (Signal<bool, Red>, D) {
        let mut d = D::dont_care();
        d.left = q.right;
        d.right = q.left;
        (q.left, d)
    }
}

#[test]
fn an_async_ring_of_combinational_black_boxes_is_a_cycle() {
    match async_ring::U::default().descriptor(ScopedName::top()) {
        Ok(_) => panic!("the asynchronous route missed a combinational cycle"),
        Err(RHDLError::CombinationalCycle(cycle)) => {
            assert_eq!(cycle.widget_count(), 2, "walk was {}", cycle.walk());
        }
        Err(other) => panic!("expected a combinational cycle, got: {other}"),
    }
}
/// widget to build a descriptor by hand would have inherited the claim.
mod never_analysed {
    use rhdl::core::circuit::descriptor::SyncKind;
    use rhdl::prelude::*;

    /// Deliberately skips `with_netlist_black_box`, so nothing ever fills
    /// in the matrix.
    #[derive(Clone, Debug, Default)]
    pub struct U;

    impl SynchronousIO for U {
        type I = bool;
        type O = bool;
        type Kernel = NoSynchronousKernel<ClockReset, bool, (), (bool, ())>;
    }
    impl SynchronousDQ for U {
        type D = ();
        type Q = ();
    }
    impl Synchronous for U {
        type S = ();
        fn init(&self) -> Self::S {}
        fn sim(&self, _cr: ClockReset, i: Self::I, _s: &mut Self::S) -> Self::O {
            i
        }
        fn descriptor(&self, scoped_name: ScopedName) -> Result<Descriptor<SyncKind>, RHDLError> {
            let name = scoped_name.to_string();
            // No netlist: the assertion is about the matrix, and building
            // one would mean going through the very helper this widget
            // exists to skip.
            Ok(Descriptor::<SyncKind> {
                combinational_reachability: Default::default(),
                name: scoped_name,
                input_kind: Self::I::static_kind(),
                output_kind: Self::O::static_kind(),
                d_kind: Kind::Empty,
                q_kind: Kind::Empty,
                kernel: None,
                netlist: None,
                hdl: Some(super::inverter_hdl(&name)),
                _phantom: std::marker::PhantomData,
            })
        }
    }
}

#[test]
fn an_uncomputed_matrix_is_treated_as_connecting_everything() {
    let d = never_analysed::U
        .descriptor(ScopedName::top())
        .expect("descriptor");
    let m = &d.combinational_reachability;
    assert!(!m.is_known(), "premise: nothing computed this matrix");
    assert!(
        m.has_feedthrough(),
        "an unknown matrix must concede a feedthrough rather than deny one"
    );
}
