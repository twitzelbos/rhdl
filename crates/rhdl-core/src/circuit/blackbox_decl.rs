//! Declarations for modules RHDL did not author.
//!
//! A [`BlackBoxDecl`] describes one external module: its ports, and which
//! of its inputs combinationally reach which of its outputs. It is the
//! *data* form of that description — generated from a checked-in library
//! file rather than written in Rust — and it exists so that a vendor
//! primitive library can be shipped, reviewed and diffed as data.
//!
//! # Why this is not just `BlackBoxConnectivity`
//!
//! [`crate::circuit::reachability::BlackBoxConnectivity`] is what a
//! widget's descriptor needs: pairs of [`Path`]s into that widget's `I`
//! and `O` types. A library cannot speak in those terms — it describes a
//! Verilog module, whose ports have names, and it has no idea which Rust
//! type some future widget will wrap it in.
//!
//! So a declaration names ports, and [`BlackBoxDecl::resolve`] turns port
//! names into field paths at the one place that knows both: the widget
//! that wraps the module and therefore chooses which field goes to which
//! port.
//!
//! # What is checked, and where
//!
//! `resolve` catches the two mistakes it can see: a data port with no
//! mapping, and a mapping naming a port the module does not have. The
//! third — a mapped path that does not exist in the widget's own types —
//! is caught downstream by `BlackBoxConnectivity::to_paths`, which has
//! the kinds to check against and reports
//! [`crate::RHDLError::BlackBoxPortNotFound`]. Duplicating that check
//! here would mean passing the kinds in for no gain.
//!
//! See `black-box-connectivity.md`.

use crate::{RHDLError, circuit::reachability::BlackBoxConnectivity, types::path::Path};

/// What a port carries.
///
/// Only [`PortRole::Clock`] and [`PortRole::Reset`] need naming, and only
/// because the reachability analysis excludes them: a reset reaches every
/// output by construction, so an edge from it says nothing. Everything
/// else is data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortRole {
    /// Ordinary signal, subject to connectivity.
    Data,
    /// A clock. Excluded from the analysis, and needs no mapping.
    Clock,
    /// A reset. Excluded from the analysis, and needs no mapping.
    Reset,
}

/// Which way a port faces.
///
/// Deliberately not `rhdl_vlog::Direction`: this is the *declared*
/// direction from a library file, and keeping it separate means the
/// library format does not move when the Verilog AST does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortDir {
    /// Into the module.
    Input,
    /// Out of the module.
    Output,
    /// Bidirectional. Not supported by connectivity declarations — see
    /// [`BlackBoxDecl::resolve`].
    Inout,
}

/// One port of an external module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortDecl {
    /// The port's name in the Verilog module.
    pub name: &'static str,
    /// Which way it faces.
    pub dir: PortDir,
    /// Width in bits, as declared.
    pub width: usize,
    /// What it carries.
    pub role: PortRole,
}

/// Declared connectivity, in terms of port names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectivityDecl {
    /// No input reaches any output: the module registers what it carries.
    None,
    /// Every input reaches every output. What a module that has not been
    /// analysed must be assumed to do.
    Opaque,
    /// Exactly these `(input port, output port)` pairs.
    Paths(&'static [(&'static str, &'static str)]),
}

/// One external module, as described by a library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlackBoxDecl {
    /// The Verilog module name to instantiate.
    pub module: &'static str,
    /// Its ports, in declaration order.
    pub ports: &'static [PortDecl],
    /// Which inputs reach which outputs.
    pub connectivity: ConnectivityDecl,
    /// Anything a reader needs to know that the machine cannot check —
    /// surfaced in diagnostics, so it reaches the person who hits a path
    /// through this module rather than sitting in a file they will not
    /// open.
    pub note: Option<&'static str>,
}

impl BlackBoxDecl {
    /// The declaration's data ports, which are the ones needing mappings.
    pub fn data_ports(&self) -> impl Iterator<Item = &PortDecl> {
        self.ports.iter().filter(|p| p.role == PortRole::Data)
    }

    /// Turn port names into field paths for a widget wrapping this module.
    ///
    /// `mapping` pairs each of the module's data ports with the path in
    /// the widget's `I` or `O` that drives or reads it. Clock and reset
    /// ports are not mapped: they are excluded from the analysis, so a
    /// path for them would be ignored, and requiring one would be
    /// ceremony.
    pub fn resolve(&self, mapping: &[(&str, Path)]) -> Result<BlackBoxConnectivity, RHDLError> {
        // A mapping for a port the module does not have. Almost always a
        // typo, and silently ignoring it loses an edge.
        for (name, _) in mapping {
            if !self.ports.iter().any(|p| p.name == *name) {
                return Err(RHDLError::BlackBoxPortUnknown {
                    module: self.module.into(),
                    port: (*name).into(),
                });
            }
        }
        // A data port with no mapping. The widget has to say where every
        // signal goes, or a declared path cannot be translated.
        for port in self.data_ports() {
            if !mapping.iter().any(|(name, _)| *name == port.name) {
                return Err(RHDLError::BlackBoxPortUnmapped {
                    module: self.module.into(),
                    port: port.name.into(),
                });
            }
            if port.dir == PortDir::Inout {
                return Err(RHDLError::BlackBoxPortIsInout {
                    module: self.module.into(),
                    port: port.name.into(),
                });
            }
        }

        let path_of = |name: &str| -> Option<Path> {
            mapping
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, p)| p.clone())
        };
        Ok(match self.connectivity {
            ConnectivityDecl::None => BlackBoxConnectivity::None,
            ConnectivityDecl::Opaque => BlackBoxConnectivity::Opaque,
            ConnectivityDecl::Paths(pairs) => {
                let mut out = Vec::with_capacity(pairs.len());
                for (from, to) in pairs {
                    // Unwrap-free: both names were checked to exist above,
                    // and every data port was checked to have a mapping.
                    let (Some(from), Some(to)) = (path_of(from), path_of(to)) else {
                        return Err(RHDLError::BlackBoxPortUnmapped {
                            module: self.module.into(),
                            port: format!("{from} or {to}"),
                        });
                    };
                    out.push((from, to));
                }
                BlackBoxConnectivity::Paths(out)
            }
        })
    }
}
