//! Errors raised while finalizing a circuit's descriptor.

use std::fmt::Display;

use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

use crate::{ast::SourcePool, circuit::reachability::CyclePort};

/// A combinational cycle among a widget's children.
///
/// # Why this exists alongside the netlist-level loop error
///
/// [`crate::ntl::error::NetLoopError`] finds the same class of fault by a
/// different route: it flattens the whole design into one netlist and
/// runs a topological sort, so it reports the cycle as a set of opcodes.
/// That is a correct answer to a question the user did not ask. A user
/// wires *widgets* together, and wants to know which widgets they wired
/// into a ring.
///
/// This error is raised earlier — while the parent's descriptor is being
/// built, before its netlist exists — and names the cycle in the terms
/// the parent's source is written in: child instance names and their
/// ports, in the order the signal travels.
///
/// Both remain. The netlist-level pass sees the fully flattened design
/// and so has the stronger guarantee; this one has the better
/// explanation. In practice a cycle in user code is caught here first,
/// and the netlist pass becomes a backstop for anything that escapes.
#[derive(Debug, Error)]
pub struct CombinationalCycle {
    /// Source of the parent widget's kernel, for rendering spans.
    pub src: SourcePool,
    /// The ports forming the cycle, in traversal order. The first and
    /// last entries are the same port, so the walk reads as closed.
    pub ports: Vec<CyclePort>,
    /// Spans in the parent's kernel, labelled with the hop each one
    /// wires. Empty when the kernel carried no location for them.
    pub elements: Vec<(Option<String>, SourceSpan)>,
}

impl CombinationalCycle {
    /// How many distinct child instances the cycle passes through.
    ///
    /// The headline number: "a cycle through 2 widgets" is a more useful
    /// opening than the length of the port walk, which counts each widget
    /// twice — once entering, once leaving.
    pub fn widget_count(&self) -> usize {
        let mut names: Vec<&str> = self.ports.iter().map(|p| p.widget.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names.len()
    }

    /// The cycle as a single line: `a -> b -> a`.
    ///
    /// Collapsed to widget granularity. The port walk visits each widget
    /// twice -- once entering, once leaving -- so rendering it verbatim
    /// reads as a stutter (`right -> right -> left -> left`) that says
    /// nothing the labels do not. Which ports are involved is in the
    /// labelled spans, where there is room for them.
    pub fn walk(&self) -> String {
        let mut steps: Vec<String> = Vec::new();
        for port in &self.ports {
            if steps.last().map(String::as_str) != Some(port.widget.as_str()) {
                steps.push(port.widget.clone());
            }
        }
        steps.join(" -> ")
    }
}

impl Display for CombinationalCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.widget_count();
        write!(
            f,
            "Combinational cycle through {n} widget{}: {}",
            if n == 1 { "" } else { "s" },
            self.walk()
        )
    }
}

impl Diagnostic for CombinationalCycle {
    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.src)
    }
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        Some(Box::new(
            "Every widget on this path passes its input to its output combinationally, so the \
             signal has no register to wait at and the loop has no start.  Break it by \
             registering one of the hops -- a `dff::DFF` on any edge is enough -- or by \
             restructuring so the widgets are not mutually dependent within a cycle.",
        ))
    }
    fn labels<'a>(&'a self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + 'a>> {
        Some(Box::new(self.elements.iter().map(|(text, span)| {
            miette::LabeledSpan::new_primary_with_span(text.clone(), *span)
        })))
    }
}
