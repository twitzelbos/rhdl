//! A `MUXF7` primitive, as a widget.
//!
//! The dedicated F7 multiplexer: a 2:1 mux built from hard logic rather
//! than a LUT. Purely combinational, which is what makes it interesting
//! here — put two of these in a ring and, before connectivity was
//! declared, nothing in the compiler noticed.
//!
//! # Why this is a widget and not a `Driver`
//!
//! The Xilinx primitives already in this crate — `IBUFDS`, the
//! open-collector pattern — are [`Driver`](rhdl::core::circuit::fixture::Driver)s:
//! they sit at the pin boundary, emit an instantiation plus constraints,
//! and are assembled into the fixture after the circuit tree is complete.
//! A driver has no descriptor and no reachability matrix, so it cannot
//! declare connectivity and no analysis sees it.
//!
//! `MUXF7` carries *data*, inside the design. So it is a circuit, it has
//! a descriptor, and it declares what it does — which is the whole point
//! of `black-box-connectivity.md`.
//!
//! # It emits an equivalent, not an instantiation — and that is a finding
//!
//! The obvious body for this widget is `MUXF7 inst(.O(o), .I0(...), ...)`,
//! leaving the definition to Xilinx's `unisims`. **RHDL cannot do that
//! today.** `Descriptor::hdl()` calls `ModuleList::checked()`, which runs
//! `iverilog -t null` over the emitted text, and iverilog rejects an
//! instantiation of a module it has never seen: `Unknown module type:
//! MUXF7`. The descriptor cannot even be built.
//!
//! So every black box in the tree *defines* its Verilog rather than
//! instantiating someone else's — `core::dff` writes an `always` block, it
//! does not instantiate `FDRE`. There is no way to say "this module is
//! defined elsewhere", which is the same root cause as Vivado IP cores
//! having no circuit-level representation (`black-box-connectivity.md`
//! §1.3), and it reaches further than that section suggested: not just IP
//! cores, but any external module at all.
//!
//! This widget therefore emits a behavioural equivalent of the mux. What
//! it demonstrates is the part that *does* work: connectivity resolved
//! from a checked-in library rather than written out by hand. Getting the
//! hard primitive itself needs the external-module capability, which is
//! Phase 4's business.

use quote::format_ident;
use rhdl::core::{
    ScopedName,
    circuit::{blackbox_decl::BlackBoxDecl, descriptor::SyncKind},
};
use rhdl::prelude::*;
use syn::parse_quote;

use crate::primitives::xilinx;

/// Inputs to [MuxF7].
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct In {
    /// Selected when `s` is false. The primitive's `I0`.
    pub i0: bool,
    /// Selected when `s` is true. The primitive's `I1`.
    pub i1: bool,
    /// The select. Combinational, like the data inputs — a mux has a path
    /// from its select to its output.
    pub s: bool,
}

/// The dedicated F7 2:1 multiplexer.
#[derive(PartialEq, Debug, Clone, Default)]
pub struct MuxF7;

impl SynchronousIO for MuxF7 {
    type I = In;
    type O = bool;
    type Kernel = NoSynchronousKernel<ClockReset, In, (), (bool, ())>;
}

impl SynchronousDQ for MuxF7 {
    type D = ();
    type Q = ();
}

impl Synchronous for MuxF7 {
    type S = ();

    fn init(&self) -> Self::S {}

    fn sim(&self, _clock_reset: ClockReset, input: Self::I, _state: &mut Self::S) -> Self::O {
        if input.s { input.i1 } else { input.i0 }
    }

    fn descriptor(&self, scoped_name: ScopedName) -> Result<Descriptor<SyncKind>, RHDLError> {
        let name = scoped_name.to_string();
        Descriptor::<SyncKind> {
            combinational_reachability: Default::default(),
            name: scoped_name,
            input_kind: <Self::I as Digital>::static_kind(),
            output_kind: <Self::O as Digital>::static_kind(),
            d_kind: Kind::Empty,
            q_kind: Kind::Empty,
            kernel: None,
            netlist: None,
            hdl: Some(self.hdl(&name)?),
            _phantom: std::marker::PhantomData,
        }
        // Resolved from the library rather than written out here: the
        // primitive's connectivity is a property of the primitive, and
        // belongs with the other things known about it.
        .with_netlist_black_box(Self::declaration().resolve(&[
            ("I0", Path::default().field("i0")),
            ("I1", Path::default().field("i1")),
            ("S", Path::default().field("s")),
            // The output is the whole of `O`, which is a bare `bool`, so
            // its path is empty.
            ("O", Path::default()),
        ])?)
    }
}

impl MuxF7 {
    /// The library entry this widget wraps.
    pub fn declaration() -> &'static BlackBoxDecl {
        &xilinx::MUXF7
    }

    fn hdl(&self, name: &str) -> Result<HDLDescriptor, RHDLError> {
        let module_name = format_ident!("{}", name);
        let input_width: vlog::BitRange = (0..<In as Digital>::static_kind().bits()).into();
        // Bit positions are read from the type rather than assumed: the
        // field order of `In` decides them, and a reordering would
        // otherwise silently rewire the mux.
        let i = In::dont_care();
        let bit = |path| -> Result<syn::Index, RHDLError> {
            let (range, _) = bit_range(<In as Digital>::static_kind(), &path)?;
            Ok(syn::Index::from(range.start))
        };
        let i0 = bit(path!(i.i0))?;
        let i1 = bit(path!(i.i1))?;
        let s = bit(path!(i.s))?;
        // A behavioural equivalent rather than `MUXF7 inst(...)`, because
        // an instantiation of an undefined module fails
        // `ModuleList::checked()` -- see the module documentation. The
        // synchronous family hands every widget a `clock_reset` port;
        // this logic has no clock, so the port is accepted and ignored.
        let module: vlog::ModuleDef = parse_quote! {
            module #module_name(
                input wire [1:0] clock_reset,
                input wire [#input_width] i,
                output wire [0:0] o
            );
                assign o = i[#s] ? i[#i1] : i[#i0];
            endmodule
        };
        Ok(HDLDescriptor {
            name: name.into(),
            modules: module.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widget declares the primitive's real connectivity.
    ///
    /// Not merely "some feedthrough": every input reaches the output,
    /// including the select, and the declaration came from the library
    /// rather than from this file.
    #[test]
    fn every_input_reaches_the_output() -> miette::Result<()> {
        let d = MuxF7.descriptor(ScopedName::top())?;
        let m = &d.combinational_reachability;
        assert!(m.is_known(), "the declaration was resolved");
        assert_eq!(m.inputs.len(), 3, "i0, i1 and s");
        assert_eq!(m.outputs.len(), 1);
        for row in 0..m.i_to_o.rows() {
            assert!(
                m.i_to_o.get(row, 0),
                "input {:?} must reach the output",
                m.inputs[row]
            );
        }
        Ok(())
    }

    /// And the DRC agrees, by its own route.
    #[test]
    fn the_drc_reports_a_feedthrough() {
        assert!(
            rhdl::core::circuit::drc::no_combinatorial_paths(&MuxF7).is_err(),
            "a combinational mux is a path from input to output"
        );
    }

    /// The emitted Verilog wires the bits the field order dictates.
    ///
    /// A behavioural equivalent rather than a `MUXF7` instantiation, for
    /// the reason in the module documentation: RHDL cannot emit an
    /// instantiation of a module it does not define, because
    /// `ModuleList::checked()` runs iverilog over the result.
    #[test]
    fn the_emitted_verilog_matches_the_field_order() -> miette::Result<()> {
        let d = MuxF7.descriptor(ScopedName::top())?;
        let hdl = d.hdl()?.modules.pretty();
        let expect = expect_test::expect![[r#"
            module top(input wire [1:0] clock_reset, input wire [2:0] i, output wire [0:0] o);
               assign o = i[2] ? i[1] : i[0];
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    /// The simulation model matches what the mux is supposed to do.
    ///
    /// Cheap, and the only functional check available: the emitted Verilog
    /// cannot be simulated without the vendor library, so `sim` is the
    /// only executable description of this widget RHDL has.
    #[test]
    fn the_model_muxes() {
        for (i0, i1, s) in [
            (false, true, false),
            (false, true, true),
            (true, false, false),
            (true, false, true),
        ] {
            let out = MuxF7.sim(ClockReset::dont_care(), In { i0, i1, s }, &mut ());
            assert_eq!(out, if s { i1 } else { i0 }, "i0={i0} i1={i1} s={s}");
        }
    }
}
