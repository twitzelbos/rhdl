//! Design Rule Checking for Circuits
//!
//! Eventually this module will contain various design rule checking functions to apply to RHDL circuits.
//! For now, it contains a single check for combinatorial paths in synchronous circuits.
//!
//! A combinatorial path is a path from an input to an output that does not terminate on any flip flops or
//! other black box components.  Some specifications discourage or even forbid combinatorial paths, as they can
//! lead to timing issues.  This module provides a function to check for such paths and report them.
//!
//! See the [book] for an example of how to use it.
use crate::{
    Synchronous,
    ast::SourcePool,
    circuit::scoped_name::ScopedName,
    ntl::{
        graph::{GraphMode, WriteSource, make_net_graph},
        spec::Wire,
    },
};
use miette::{Diagnostic, SourceSpan};
use petgraph::algo::DfsSpace;
use std::collections::hash_map::RandomState;
use thiserror::Error;

/// Diagnostic for combinatorial paths in synchronous circuits.
#[derive(Debug, Error)]
#[error("RHDL Combinatorial Path")]
pub struct CombinatorialPath {
    src: SourcePool,
    elements: Vec<SourceSpan>,
}

impl Diagnostic for CombinatorialPath {
    fn severity(&self) -> Option<miette::Severity> {
        Some(miette::Severity::Error)
    }
    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.src)
    }
    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(
            "This is a combinatorial pathway between an input and an output",
        ))
    }
    fn labels<'a>(
        &'a self,
    ) -> Option<Box<dyn std::iter::Iterator<Item = miette::LabeledSpan> + 'a>> {
        Some(Box::new(self.elements.iter().map(|span| {
            miette::LabeledSpan::new_primary_with_span(None, *span)
        })))
    }
}

/// Check that the given synchronous circuit has no combinatorial paths from inputs to outputs.
///
/// This function analyzes the netlist of the circuit and looks for paths from inputs to outputs
/// that do not pass through any flip flops or black box components.  If such paths are found,
/// it returns an error with a diagnostic that includes the source locations of the elements
/// involved in the path.
///
pub fn no_combinatorial_paths<T: Synchronous>(uut: &T) -> miette::Result<()> {
    let descriptor = uut.descriptor(ScopedName::top())?;
    // The verdict comes from the reachability matrix, which was computed
    // when the descriptor was built. The netlist walk below runs only to
    // say *where* the path is, because the matrix records which fields
    // are connected and not which opcodes connected them.
    //
    // So the clean case -- overwhelmingly the common one, since this is
    // asserted as a property in dozens of widget tests -- no longer
    // builds a graph over the flattened netlist and searches it.
    if !descriptor.combinational_reachability.has_feedthrough() {
        return Ok(());
    }
    Err(locate_combinatorial_path(&descriptor))
}

/// Walk the flattened netlist to find a concrete input-to-output path and
/// report it with source spans.
///
/// This is the pre-matrix implementation of the whole check, kept because
/// the diagnostic it produces is the user-facing contract -- there is a
/// committed expectation file for it -- and the matrix cannot reproduce
/// it. Field-level connectivity does not say which opcodes formed the
/// connection, and spans live on opcodes.
///
/// Also used by tests to cross-check the matrix: two implementations that
/// agree by different routes is the only evidence that replacing one with
/// the other changed nothing.
pub(crate) fn locate_combinatorial_path(
    descriptor: &crate::circuit::descriptor::Descriptor<crate::circuit::descriptor::SyncKind>,
) -> miette::Report {
    let Ok(ntl) = descriptor.netlist() else {
        // No netlist to walk. The matrix already said there is a path, so
        // report it without the spans rather than claiming there is none.
        return miette::Report::new(CombinatorialPath {
            src: SourcePool::default(),
            elements: Vec::new(),
        });
    };
    let dep = make_net_graph(ntl, GraphMode::Synchronous);
    let input_node = dep.input_node;
    let mut space = DfsSpace::new(&dep.graph);
    let code = &ntl.code;
    for output in ntl.outputs.iter().copied().flat_map(Wire::reg) {
        let source = dep.reg_map[&output];
        match source {
            WriteSource::ClockReset => {}
            WriteSource::Input => {
                return miette::Report::new(CombinatorialPath {
                    src: code.source(),
                    elements: Vec::new(),
                });
            }
            WriteSource::OpCode(ndx) => {
                let op_node = dep.op_nodes[ndx];
                if petgraph::algo::has_path_connecting(
                    &dep.graph,
                    input_node,
                    op_node,
                    Some(&mut space),
                ) {
                    // `min_intermediate_nodes` of 0, and no `unwrap`.
                    //
                    // It was 1, which cannot match a *direct* edge from
                    // the inputs to the writing op -- and then `unwrap`
                    // panicked on the empty iterator. That was
                    // unreachable while every path had an op in the
                    // middle, and a black box declaring a combinational
                    // path is exactly the direct case. An empty path
                    // yields a diagnostic with no spans, which is worse
                    // than one with spans and far better than a crash.
                    let path = petgraph::algo::all_simple_paths::<Vec<_>, _, RandomState>(
                        &dep.graph, input_node, op_node, 0, None,
                    )
                    .next()
                    .unwrap_or_default();
                    let elements = path
                        .iter()
                        .map(|ix| dep.graph[*ix])
                        .filter_map(|ws| match ws {
                            WriteSource::OpCode(ndx) => Some(ndx),
                            _ => None,
                        })
                        .flat_map(|x| ntl.ops[x].loc)
                        .map(|loc| SourceSpan::from(code.span(loc)))
                        .collect();
                    return miette::Report::new(CombinatorialPath {
                        src: code.source(),
                        elements,
                    });
                }
            }
        }
    }
    // The matrix found a path the netlist walk did not. That is a bug in
    // one of them, and reporting the path without spans is better than
    // reporting success.
    miette::Report::new(CombinatorialPath {
        src: code.source(),
        elements: Vec::new(),
    })
}

/// Does the flattened netlist contain an input-to-output path?
///
/// The matrix-free answer. Public so that tests outside this crate can
/// check the matrix against something other than itself: this is the
/// implementation [`no_combinatorial_paths`] used before it queried the
/// matrix, and the two agreeing across a corpus is the evidence that the
/// change was inert.
pub fn feedthrough_by_netlist_walk<T: Synchronous>(uut: &T) -> miette::Result<bool> {
    let descriptor = uut.descriptor(ScopedName::top())?;
    let ntl = descriptor.netlist()?;
    let dep = make_net_graph(ntl, GraphMode::Synchronous);
    let mut space = DfsSpace::new(&dep.graph);
    for output in ntl.outputs.iter().copied().flat_map(Wire::reg) {
        match dep.reg_map[&output] {
            WriteSource::ClockReset => {}
            WriteSource::Input => return Ok(true),
            WriteSource::OpCode(ndx) => {
                if petgraph::algo::has_path_connecting(
                    &dep.graph,
                    dep.input_node,
                    dep.op_nodes[ndx],
                    Some(&mut space),
                ) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
