//! Circuit Descriptors (run time descriptions of circuits)
//!
//! Most of RHDL's compilation and optimization of your circuit designs
//! happen at run time, not when your code is first compiled by Rust.
//!
//! A key part of this is the [Descriptor] type, which describes
//! the interface and implementation of a circuit at run time.
//! It includes information about the input and output kinds,
//! the internal feedback types, the compiled kernel object,
//! and optionally the netlist and HDL description of the circuit.
//!
//! It also provides a way to iterate over the subcircuits of a
//! circuit, allowing for a run time iteration over the (heterogeneous)
//! hierarchy of circuits that make up a design.
//!
//! You can obtain a [Descriptor] for a circuit by calling the
//! `descriptor` method on the circuit, which is part of the
//! [Circuit](crate::Circuit) trait, or part of the [Synchronous](crate::Synchronous) trait.
//!
//! Note also that the [Descriptor] type is generic over a marker type
//! that indicates whether the circuit is asynchronous or synchronous.
//! This allows for type-safe handling of descriptors for different
//! kinds of circuits.
use std::marker::PhantomData;

use crate::{
    HDLDescriptor, Kind, RHDLError,
    circuit::{reachability::ReachabilityMatrix, scoped_name::ScopedName},
    ntl, rtl,
};

/// Marker type for asynchronous circuits.
pub struct AsyncKind;
/// Marker type for synchronous circuits.
pub struct SyncKind;

/// Run time description of a circuit.
#[derive(Debug)]
pub struct Descriptor<T> {
    /// The scoped name of the circuit.
    pub name: ScopedName,
    /// The kind of the input type.
    pub input_kind: Kind,
    /// The kind of the output type.
    pub output_kind: Kind,
    /// The kind of the internal feedback type to the inputs of the children.
    pub d_kind: Kind,
    /// The kind of the internal feedback type from the outputs of the children.
    pub q_kind: Kind,
    /// The compiled kernel object.
    pub kernel: Option<rtl::Object>,
    /// The netlist representation of the circuit, if available.
    pub netlist: Option<ntl::Object>,
    /// The HDL (Verilog) description of the circuit, if available.
    pub hdl: Option<HDLDescriptor>,
    /// Which inputs combinationally reach which outputs.
    ///
    /// See [`ReachabilityMatrix`]. Defaults to the empty matrix, which
    /// reads as "nothing reaches anything" — correct for a widget that
    /// registers everything, and the value a black box keeps.
    pub combinational_reachability: ReachabilityMatrix,
    /// Phantom data for the marker type.
    pub _phantom: PhantomData<T>,
}

impl<T> Descriptor<T> {
    /// Get a reference to the HDL (Verilog) description of the circuit, if available.
    pub fn hdl(&self) -> Result<&HDLDescriptor, RHDLError> {
        let hdl = self.hdl.as_ref().ok_or(RHDLError::HDLNotAvailable {
            name: self.name.to_string(),
        })?;
        hdl.modules.checked()?;
        Ok(hdl)
    }
    /// A correctly-shaped matrix in which nothing reaches anything.
    ///
    /// Every black box in the tree today registers what it carries --
    /// `DFF`, the RAMs, the CDC primitives -- so no feedthrough is the
    /// right answer as well as the safe-looking one. It is not safe in
    /// general: a combinational black box would need its feedthrough
    /// declared. See [`crate::circuit::reachability`].
    fn empty_reachability(&self) -> ReachabilityMatrix {
        ReachabilityMatrix::none(self.input_kind, self.output_kind, self.d_kind, self.q_kind)
    }

    /// Get a reference to the netlist representation of the circuit, if available.
    pub fn netlist(&self) -> Result<&ntl::Object, RHDLError> {
        self.netlist.as_ref().ok_or(RHDLError::NetlistNotAvailable {
            name: self.name.to_string(),
        })
    }
}

impl Descriptor<AsyncKind> {
    /// Create a black box (asynchronous) netlist for this descriptor.
    pub fn with_netlist_black_box(mut self) -> Result<Descriptor<AsyncKind>, RHDLError> {
        self.netlist = Some(ntl::builder::circuit_black_box(&self)?);
        self.combinational_reachability = self.empty_reachability();
        Ok(self)
    }
}

impl Descriptor<SyncKind> {
    /// Create a black box (synchronous) netlist for this descriptor.
    pub fn with_netlist_black_box(mut self) -> Result<Descriptor<SyncKind>, RHDLError> {
        self.netlist = Some(ntl::builder::synchronous_black_box(&self)?);
        self.combinational_reachability = self.empty_reachability();
        Ok(self)
    }
}
