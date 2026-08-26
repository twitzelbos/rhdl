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
/// `CONNECTED` chooses what it *claims*, so the same module can be tested
/// as an honest combinational box and as one that lies about being
/// registered. The lie is what the old behaviour was, for everything.
#[derive(Clone, Debug, Default)]
pub struct BlackInverter<const CONNECTED: bool>;

impl<const CONNECTED: bool> SynchronousIO for BlackInverter<CONNECTED> {
    type I = bool;
    type O = bool;
    type Kernel = NoSynchronousKernel<ClockReset, bool, (), (bool, ())>;
}

impl<const CONNECTED: bool> SynchronousDQ for BlackInverter<CONNECTED> {
    type D = ();
    type Q = ();
}

impl<const CONNECTED: bool> Synchronous for BlackInverter<CONNECTED> {
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
        .with_netlist_black_box(if CONNECTED {
            BlackBoxConnectivity::Paths(vec![(Path::default(), Path::default())])
        } else {
            BlackBoxConnectivity::None
        })
    }
}

/// A combinational black box is reported as a feedthrough.
///
/// This is the check that could not fire before: the DRC took its verdict
/// from a matrix that was empty for every black box, and before that from
/// a graph that skipped them.
#[test]
fn a_combinational_black_box_is_a_feedthrough() {
    let honest = BlackInverter::<true>::default();
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
    let claims_registered = BlackInverter::<false>::default();
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
    pub struct U<const CONNECTED: bool> {
        pub left: BlackInverter<CONNECTED>,
        pub right: BlackInverter<CONNECTED>,
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

    impl<const CONNECTED: bool> SynchronousIO for U<CONNECTED> {
        type I = bool;
        type O = bool;
        type Kernel = ring_kernel;
    }
    impl<const CONNECTED: bool> SynchronousDQ for U<CONNECTED> {
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
    let uut = ring::U::<true>::default();
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
    let uut = ring::U::<false>::default();
    assert!(
        uut.descriptor(ScopedName::top()).is_ok(),
        "black boxes declaring `None` break the ring, as a register would"
    );
}
