//! Per-widget combinational reachability.
//!
//! For every widget, this records which of its input fields can reach
//! which of its output fields *combinationally* — that is, along a path
//! containing no register. Four relations are kept, because a widget's
//! boundary has four interesting halves: its own `I`/`O` ports, and the
//! `D`/`Q` aggregates through which it talks to its children.
//!
//! | relation | meaning |
//! |---|---|
//! | `i_to_o` | input reaches output: a feedthrough |
//! | `i_to_d` | input reaches a child's input |
//! | `q_to_o` | a child's output reaches this widget's output |
//! | `q_to_d` | a child's output reaches another child's input |
//!
//! The last one is the interesting one. It is the only channel through
//! which two children of the same parent can form a combinational loop
//! between them, and it is invisible to any analysis that looks at one
//! widget at a time.
//!
//! # Why this is not the existing DRC
//!
//! [`crate::circuit::drc::no_combinatorial_paths`] answers "does this
//! whole design have a feedthrough" by flattening every widget into one
//! netlist and asking once. That is the right shape for its question and
//! the wrong shape for two others: it cannot say *which* input reaches
//! *which* output, and it cannot report a cycle in terms of the widgets
//! that form it — by the time the netlist is flat, the widget boundaries
//! that would make the diagnostic readable are gone.
//!
//! # Where the graph comes from
//!
//! Not from RHIF. `combinational-reachability-and-loop-detection.md` §4.2
//! specifies the intra-widget graph as a use-def walk over the RHIF
//! `Object`, but RHIF is not retained past stage 1 — [`Descriptor`] holds
//! an [`rtl::Object`]. Lowering that to a netlist with
//! [`build_ntl_from_rtl`] gives the same information in a more convenient
//! shape: the kernel's netlist has exactly the ports this analysis needs,
//! `[clock_reset, i, q]` in and `[o, d]` out, so the four relations fall
//! out of one reachability computation instead of needing the operand
//! senses of nineteen RHIF opcodes transcribed by hand.
//!
//! It is also bit-level rather than field-level, which is strictly more
//! precise than the design called for. The matrices are still *stored*
//! per field path, because that is what a diagnostic can name and what a
//! parent needs to compose; the extra precision is spent during the
//! computation and aggregated away at the end.

use crate::ast::SourcePool;
use miette::SourceSpan;
use std::collections::HashMap;

use crate::{
    Kind, RHDLError,
    common::symtab::RegisterId,
    compiler::optimize_ntl,
    ntl::{from_rtl::build_ntl_from_rtl, spec::WireKind, visit::visit_wires},
    rtl,
    types::path::{Path, bit_range, leaf_paths},
};

/// A dense rectangular bit matrix.
///
/// Rows are packed into `u64` words. Sized once and never resized, so a
/// row's word range is a fixed function of its index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BitMatrix {
    rows: usize,
    cols: usize,
    words_per_row: usize,
    words: Vec<u64>,
}

impl BitMatrix {
    /// An all-false matrix of the given shape.
    pub fn new(rows: usize, cols: usize) -> Self {
        let words_per_row = cols.div_ceil(64);
        Self {
            rows,
            cols,
            words_per_row,
            words: vec![0; rows * words_per_row],
        }
    }

    /// Number of rows.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Number of columns.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Is the bit at `(row, col)` set? Out-of-range reads are `false`
    /// rather than a panic, so that a caller holding an index from a
    /// differently-shaped matrix gets a conservative answer instead of a
    /// crash.
    pub fn get(&self, row: usize, col: usize) -> bool {
        if row >= self.rows || col >= self.cols {
            return false;
        }
        let w = row * self.words_per_row + col / 64;
        self.words[w] & (1u64 << (col % 64)) != 0
    }

    /// Set the bit at `(row, col)`. Out-of-range writes are ignored.
    pub fn set(&mut self, row: usize, col: usize) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        let w = row * self.words_per_row + col / 64;
        self.words[w] |= 1u64 << (col % 64);
    }

    /// Is any bit set anywhere?
    pub fn any(&self) -> bool {
        self.words.iter().any(|w| *w != 0)
    }

    /// Is any bit in `row` set?
    pub fn row_any(&self, row: usize) -> bool {
        if row >= self.rows {
            return false;
        }
        let base = row * self.words_per_row;
        self.words[base..base + self.words_per_row]
            .iter()
            .any(|w| *w != 0)
    }

    /// The column indices set in `row`.
    pub fn row_iter(&self, row: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.cols).filter(move |c| self.get(row, *c))
    }
}

/// Combinational reachability for a single widget.
///
/// See the module documentation for what the four relations mean and why
/// there are four of them.
// `Eq` is absent because `Path` is only `PartialEq`; the cache key in
// §4.4 hashes the matrix, which `Hash` alone supports.
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub struct ReachabilityMatrix {
    /// Leaf field paths of the widget's `I` type, in bit order.
    pub inputs: Vec<Path>,
    /// Leaf field paths of the widget's `O` type, in bit order.
    pub outputs: Vec<Path>,
    /// Leaf field paths of the widget's `D` type — the children's inputs.
    pub d_paths: Vec<Path>,
    /// Leaf field paths of the widget's `Q` type — the children's outputs.
    pub q_paths: Vec<Path>,
    /// `inputs` × `outputs`.
    pub i_to_o: BitMatrix,
    /// `inputs` × `d_paths`.
    pub i_to_d: BitMatrix,
    /// `q_paths` × `outputs`.
    pub q_to_o: BitMatrix,
    /// `q_paths` × `d_paths`.
    pub q_to_d: BitMatrix,
}

/// Leaf field paths of `kind`, excluding the zero-width ones.
///
/// [`leaf_paths`] treats `Kind::Empty` as a leaf, so a widget with no
/// children -- whose `D` and `Q` are `()` -- would otherwise report one
/// `d_path` and one `q_path` that address no bits. That is not merely
/// untidy: it makes "does this widget have children" unanswerable from
/// the matrix, and any assertion phrased that way silently vacuous.
fn sized_leaf_paths(kind: Kind) -> Vec<Path> {
    leaf_paths(&kind, Path::default())
        .into_iter()
        .filter(|p| {
            bit_range(kind, p)
                .map(|(range, _)| !range.is_empty())
                .unwrap_or(false)
        })
        .collect()
}

impl ReachabilityMatrix {
    /// A matrix with the right shape and nothing reachable.
    ///
    /// This is the honest answer for a widget that registers everything
    /// it touches, and the shape is still populated so that a parent can
    /// index it without special-casing.
    pub fn none(input_kind: Kind, output_kind: Kind, d_kind: Kind, q_kind: Kind) -> Self {
        let inputs = sized_leaf_paths(input_kind);
        let outputs = sized_leaf_paths(output_kind);
        let d_paths = sized_leaf_paths(d_kind);
        let q_paths = sized_leaf_paths(q_kind);
        let i_to_o = BitMatrix::new(inputs.len(), outputs.len());
        let i_to_d = BitMatrix::new(inputs.len(), d_paths.len());
        let q_to_o = BitMatrix::new(q_paths.len(), outputs.len());
        let q_to_d = BitMatrix::new(q_paths.len(), d_paths.len());
        Self {
            inputs,
            outputs,
            d_paths,
            q_paths,
            i_to_o,
            i_to_d,
            q_to_o,
            q_to_d,
        }
    }

    /// Does any input reach any output?
    ///
    /// The question [`crate::circuit::drc::no_combinatorial_paths`] asks,
    /// answered from the matrix.
    pub fn has_feedthrough(&self) -> bool {
        self.i_to_o.any()
    }
}

/// Spans for a cycle's hops, looked up only when there is a cycle.
///
/// Rebuilds and re-optimises the kernel's netlist, which is wasteful and
/// entirely fine: this runs once, on the way to returning an error, and
/// keeping it off the common path means the analysis itself carries no
/// span bookkeeping.
///
/// The span for a hop into a child is the location of the opcode that
/// writes that child's input register — which is the kernel statement
/// that wired the hop, the span §6.1 of the design plan asks for.
pub(crate) fn cycle_spans(
    kernel: &rtl::Object,
    output_kind: Kind,
    d_kind: Kind,
    d_paths: &[Path],
    cycle: &[CyclePort],
) -> (Option<SourcePool>, Vec<(Option<String>, SourceSpan)>) {
    let Ok(ntl) = optimize_ntl(build_ntl_from_rtl(kernel)) else {
        return (None, Vec::new());
    };
    let d_bits: Vec<Option<RegisterId<WireKind>>> = ntl
        .outputs
        .iter()
        .skip(output_kind.bits())
        .map(|w| w.reg())
        .collect();
    let span_for = |path: &Path| -> Option<SourceSpan> {
        let (range, _) = bit_range(d_kind, path).ok()?;
        let reg = d_bits.get(range.start).copied().flatten()?;
        let ndx = ntl.ops.iter().position(|lop| {
            let mut writes_it = false;
            visit_wires(&lop.op, |sense, wire| {
                if sense.is_write() && wire.reg() == Some(reg) {
                    writes_it = true;
                }
            });
            writes_it
        })?;
        Some(SourceSpan::from(ntl.code.span(ntl.ops[ndx].loc?)))
    };

    let mut elements = Vec::new();
    let last = cycle.len().saturating_sub(1);
    for (n, port) in cycle.iter().enumerate() {
        // The walk is closed, so entry 0 is the same port as the last
        // one. Labelling both would print the closing hop twice.
        if port.side != PortSide::Input || n == 0 {
            continue;
        }
        // Which port fed this one -- the previous step in the walk.
        let Some(prev) = n.checked_sub(1).and_then(|i| cycle.get(i)) else {
            continue;
        };
        let label = if n == last {
            format!("{prev} -> {port} (closes the cycle)")
        } else {
            format!("{prev} -> {port}")
        };
        let full = Path::default().field(&port.widget).join(&port.port);
        if let Some(idx) = d_paths.iter().position(|p| *p == full)
            && let Some(span) = span_for(&d_paths[idx])
        {
            elements.push((Some(label), span));
        }
    }
    (Some(ntl.code.source()), elements)
}

/// Which side of a child instance a cycle passes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortSide {
    /// The signal entering the child.
    Input,
    /// The signal leaving it.
    Output,
}

/// One port on one child instance, as a step in a cycle.
#[derive(Clone, Debug, PartialEq)]
pub struct CyclePort {
    /// The child's field name in the parent — the name a user wrote.
    pub widget: String,
    /// The port's path inside that child.
    pub port: Path,
    /// Whether the signal is arriving or leaving.
    pub side: PortSide,
}

impl std::fmt::Display for CyclePort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Path`'s `Debug` renders as `.field[0]`, which is what the
        // existing loop diagnostic uses, so labels read the same way.
        let port = format!("{:?}", self.port);
        // `Path`'s Display leads with a `.` for a field, which reads
        // oddly after the widget name only when the path is empty.
        if port.is_empty() {
            write!(f, "{}", self.widget)
        } else {
            write!(f, "{}{}", self.widget, port)
        }
    }
}

/// Find a cycle in an edge list, returned in traversal order.
///
/// # Why this cannot use the matrix
///
/// The obvious implementation asks `q_to_d` which child outputs reach
/// which child inputs and looks for a ring. That does not work, and the
/// way it fails is instructive: the matrix is a *transitive closure*
/// computed by a fixpoint, and a fixpoint run over a graph with a cycle
/// saturates — every node on the cycle ends up holding every source that
/// can reach any of them. So on exactly the designs where a cycle exists,
/// `q_to_d` goes dense and every pair of ports looks connected. Asking it
/// which ports form the cycle yields a ring of length two through
/// whichever port was visited first.
///
/// So the check runs on the edge graph, before the fixpoint, which is
/// also what the design plan specifies — "a graph-cycle check on the
/// augmented intra-kernel graph". Running it first is a bonus: a design
/// with a loop skips the fixpoint entirely, and its matrix would have
/// been meaningless anyway.
fn find_cycle_in(edges: &[(usize, Vec<usize>)], node_count: usize) -> Option<Vec<usize>> {
    // `edges` is (written, [read]) — the dataflow direction is read to
    // written, so that is the direction traversed.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for (w, reads) in edges {
        for r in reads {
            if *r < node_count && *w < node_count {
                adj[*r].push(*w);
            }
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unseen,
        OnPath,
        Done,
    }
    let mut mark = vec![Mark::Unseen; node_count];
    // Iterative rather than recursive: a wide kernel has thousands of
    // netlist registers and this would otherwise be a stack overflow
    // rather than a diagnostic.
    for start in 0..node_count {
        if mark[start] != Mark::Unseen {
            continue;
        }
        let mut path: Vec<usize> = vec![start];
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        mark[start] = Mark::OnPath;
        while let Some((node, edge)) = stack.pop() {
            if edge < adj[node].len() {
                stack.push((node, edge + 1));
                let next = adj[node][edge];
                match mark[next] {
                    Mark::OnPath => {
                        let at = path.iter().position(|n| *n == next)?;
                        let mut cycle = path[at..].to_vec();
                        cycle.push(next);
                        return Some(cycle);
                    }
                    Mark::Unseen => {
                        mark[next] = Mark::OnPath;
                        path.push(next);
                        stack.push((next, 0));
                    }
                    Mark::Done => {}
                }
            } else {
                mark[node] = Mark::Done;
                path.pop();
            }
        }
    }
    None
}

/// Reachability for `N` copies of one widget, wired element-wise.
///
/// `[T; N]` has `I = [T::I; N]` and `O = [T::O; N]`, and element `k`
/// sees only `i[k]` and drives only `o[k]`. So the matrix is block
/// diagonal: whatever the element passes through, it passes through in
/// its own lane, and no lane reaches another.
///
/// The array has no kernel and empty `D`/`Q` -- it is pure wiring, and
/// the elements are addressed positionally rather than by field name --
/// so the generic path cannot derive this and it is spelled out here.
pub(crate) fn array_of(
    element: &ReachabilityMatrix,
    n: usize,
    input_kind: Kind,
    output_kind: Kind,
) -> Result<ReachabilityMatrix, RHDLError> {
    let mut out = ReachabilityMatrix::none(input_kind, output_kind, Kind::Empty, Kind::Empty);
    let index_of = |paths: &[Path], p: &Path| paths.iter().position(|q| q == p);
    for k in 0..n {
        let lane = Path::default().index(k);
        for (ei, e_in) in element.inputs.iter().enumerate() {
            let Some(row) = index_of(&out.inputs, &lane.clone().join(e_in)) else {
                continue;
            };
            for (eo, e_out) in element.outputs.iter().enumerate() {
                if !element.i_to_o.get(ei, eo) {
                    continue;
                }
                if let Some(col) = index_of(&out.outputs, &lane.clone().join(e_out)) {
                    out.i_to_o.set(row, col);
                }
            }
        }
    }
    Ok(out)
}

/// Reachability for two widgets in series.
///
/// `Chain<A, B>` feeds `A`'s output straight into `B`'s input, so a path
/// from the chain's input to its output has to cross both: a boolean
/// matrix product. A register anywhere in either half breaks it, which is
/// why chaining two registered widgets is not a feedthrough even though
/// each half is wired to the next.
///
/// `A::O` and `B::I` are the same type, so their leaf paths are the same
/// list and the product needs no bit-level translation.
pub(crate) fn series(
    a: &ReachabilityMatrix,
    b: &ReachabilityMatrix,
    input_kind: Kind,
    output_kind: Kind,
) -> Result<ReachabilityMatrix, RHDLError> {
    let mut out = ReachabilityMatrix::none(input_kind, output_kind, Kind::Empty, Kind::Empty);
    for ai in 0..a.i_to_o.rows() {
        for mid in a.i_to_o.row_iter(ai) {
            // `mid` indexes A's outputs, which are B's inputs. The two
            // lists are the same because the types are.
            let Some(b_row) = b.inputs.iter().position(|p| Some(p) == a.outputs.get(mid)) else {
                continue;
            };
            for bo in b.i_to_o.row_iter(b_row) {
                out.i_to_o.set(ai, bo);
            }
        }
    }
    Ok(out)
}

/// Reachability for a widget that only re-types its interface.
///
/// An `Adapter` moves a synchronous circuit into an asynchronous
/// context: the bits are the same bits, so whatever the inner widget
/// passes through, the wrapper passes through.
pub(crate) fn passthrough(
    inner: &ReachabilityMatrix,
    input_kind: Kind,
    output_kind: Kind,
    d_kind: Kind,
    q_kind: Kind,
) -> Result<ReachabilityMatrix, RHDLError> {
    let mut out = ReachabilityMatrix::none(input_kind, output_kind, d_kind, q_kind);
    // Indices are positional rather than by path, because the wrapper's
    // paths carry a `Signal` step the inner's do not. The leaf order is
    // the bit order in both, so position is the correspondence.
    for r in 0..inner.i_to_o.rows().min(out.i_to_o.rows()) {
        for c in inner.i_to_o.row_iter(r) {
            out.i_to_o.set(r, c);
        }
    }
    Ok(out)
}

/// What the analysis found.
///
/// A cycle and a matrix are mutually exclusive rather than merely
/// unlikely together: the matrix is a transitive closure, and a closure
/// over a cyclic graph saturates into uselessness. So a design with a
/// combinational loop has no meaningful matrix, and reporting the loop is
/// the only thing left to do with it.
// The two variants differ a lot in size, and boxing the big one would
// add an allocation per widget to save nothing: an `Outcome` is returned
// once per descriptor and consumed immediately by `into_matrix`, never
// stored or collected.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Outcome {
    /// No cycle; here is the reachability.
    Acyclic(ReachabilityMatrix),
    /// A combinational cycle, in traversal order, closing on itself.
    Cycle(Vec<CyclePort>),
}

/// Turn an [`Outcome`] into a matrix, or into the error a cycle deserves.
///
/// One place, so that every descriptor builder reports a cycle the same
/// way and none of them can quietly treat one as "no reachability".
pub(crate) fn into_matrix(
    outcome: Outcome,
    kernel: Option<&rtl::Object>,
    output_kind: Kind,
    d_kind: Kind,
    d_paths: &[Path],
) -> Result<ReachabilityMatrix, RHDLError> {
    match outcome {
        Outcome::Acyclic(matrix) => Ok(matrix),
        Outcome::Cycle(ports) => {
            let (src, elements) = match kernel {
                Some(k) => cycle_spans(k, output_kind, d_kind, d_paths, &ports),
                None => (None, Vec::new()),
            };
            Err(RHDLError::CombinationalCycle(Box::new(
                crate::circuit::error::CombinationalCycle {
                    src: src.unwrap_or_default(),
                    ports,
                    elements,
                },
            )))
        }
    }
}

/// A child's contribution to its parent's analysis.
pub struct ChildReach<'a> {
    /// The field name the child occupies in the parent's `D` and `Q`.
    pub field: String,
    /// The child's `I` kind, needed to turn its input paths into bit
    /// ranges. The matrix stores paths, not the kinds they index into.
    pub input_kind: Kind,
    /// The child's `O` kind, for the same reason.
    pub output_kind: Kind,
    /// The child's own matrix.
    pub matrix: &'a ReachabilityMatrix,
}

/// Compute the matrix for a synchronous widget.
///
/// `kernel` is `None` for a widget that is a black box to the compiler —
/// `DFF`, the RAMs, the CDC primitives. Those get [`ReachabilityMatrix::none`]:
/// every one of them registers what it carries, so nothing feeds through.
///
/// That is the same assumption the existing netlist graph makes, where
/// [`crate::ntl::graph::make_net_graph`] skips every `BlackBox` op when
/// adding edges. It is worth writing down that the assumption is about
/// the black boxes that exist rather than about black boxes in general:
/// `reset::negation` is combinational (`assign o = ~i`), and only escapes
/// being a counterexample because it carries a reset, and reset is
/// excluded from this analysis anyway. A vendor primitive that carried
/// data combinationally would need its feedthrough declared rather than
/// assumed — see `vendor-primitive-architecture.md`, which plans exactly
/// that.
pub(crate) fn compute_synchronous(
    kernel: Option<&rtl::Object>,
    input_kind: Kind,
    output_kind: Kind,
    d_kind: Kind,
    q_kind: Kind,
    children: &[ChildReach<'_>],
) -> Result<Outcome, RHDLError> {
    // A synchronous kernel is `fn(clock_reset, i, q)`, so `i` is the
    // second port.
    compute(kernel, 1, input_kind, output_kind, d_kind, q_kind, children)
}

/// Compute the matrix for an asynchronous widget.
///
/// Same analysis; the only difference is where `i` sits in the kernel's
/// port list. An asynchronous kernel is `fn(i, q)` -- its clocks and
/// resets travel inside `I` as `Signal<ClockReset, _>` fields rather than
/// as a separate port -- so `i` is the first port, not the second.
pub(crate) fn compute_asynchronous(
    kernel: Option<&rtl::Object>,
    input_kind: Kind,
    output_kind: Kind,
    d_kind: Kind,
    q_kind: Kind,
    children: &[ChildReach<'_>],
) -> Result<Outcome, RHDLError> {
    compute(kernel, 0, input_kind, output_kind, d_kind, q_kind, children)
}

/// The analysis proper. `i_port` is the index of `i` in the kernel's
/// netlist input list.
fn compute(
    kernel: Option<&rtl::Object>,
    i_port: usize,
    input_kind: Kind,
    output_kind: Kind,
    d_kind: Kind,
    q_kind: Kind,
    children: &[ChildReach<'_>],
) -> Result<Outcome, RHDLError> {
    let mut out = ReachabilityMatrix::none(input_kind, output_kind, d_kind, q_kind);
    let Some(kernel) = kernel else {
        return Ok(Outcome::Acyclic(out));
    };
    // Optimised, not raw. This matters for correctness, not speed: the
    // raw lowering keeps every dataflow dependence the kernel's source
    // has, including the semantically vacuous ones. A Carloni relay
    // assigns `stop_out = true` in both arms of `if i.stop_in`, so the
    // raw netlist has `stop_in` selecting between two constants -- a
    // dataflow dependence with no hardware behind it. `optimize_ntl`
    // collapses it, and the existing DRC has always seen the collapsed
    // form because `Builder::build` optimises.
    //
    // Analysing the raw form makes the matrix an over-approximation, and
    // an over-approximation is not harmlessly conservative here: Phase 3
    // turns these relations into loop *errors*, so a path that does not
    // exist in the hardware would reject a valid design.
    let ntl = optimize_ntl(build_ntl_from_rtl(kernel))?;

    // The kernel's netlist ports are `[clock_reset, i, q]` in for a
    // synchronous widget and `[i, q]` for an asynchronous one, with
    // `[o, d]` out either way, concatenated bitwise. Clock and reset are
    // skipped where they are a separate port:
    // every widget's reset reaches every output by construction, so
    // including it would make the matrix uniformly true and useless.
    let i_bits: Vec<RegisterId<WireKind>> = ntl.inputs.get(i_port).cloned().unwrap_or_default();
    let q_bits: Vec<RegisterId<WireKind>> = ntl.inputs.get(i_port + 1).cloned().unwrap_or_default();
    let o_width = output_kind.bits();
    let o_bits: Vec<Option<RegisterId<WireKind>>> =
        ntl.outputs.iter().take(o_width).map(|w| w.reg()).collect();
    let d_bits: Vec<Option<RegisterId<WireKind>>> =
        ntl.outputs.iter().skip(o_width).map(|w| w.reg()).collect();

    // Boundary bits get a source index: the `i` bits first, then `q`.
    let n_i = i_bits.len();
    let n_src = n_i + q_bits.len();
    let words = n_src.div_ceil(64);

    // Registers get dense indices so the source sets can live in one
    // flat `Vec<u64>` rather than a map of hash sets. That matters more
    // than it looks: the first version kept a `HashSet<usize>` per
    // register and more than doubled the workspace's test time, because
    // a large widget has thousands of registers and every fixpoint round
    // walked every set element by hand. Packed words turn each union
    // into a handful of ORs.
    let mut reg_idx: HashMap<RegisterId<WireKind>, usize> = HashMap::new();
    // Takes the map as an argument rather than capturing it, so the
    // borrow checker allows interning while other parts of the map are
    // being read.
    fn intern(r: RegisterId<WireKind>, map: &mut HashMap<RegisterId<WireKind>, usize>) -> usize {
        let next = map.len();
        *map.entry(r).or_insert(next)
    }

    // Edges, as (written index, read indices). Built once so the fixpoint
    // can sweep them repeatedly without re-walking opcodes.
    let mut edges: Vec<(usize, Vec<usize>)> = Vec::new();
    for lop in &ntl.ops {
        // A kernel netlist contains no black boxes -- those appear only
        // when a widget's netlist absorbs its children -- so there is
        // nothing to skip here, and skipping would be wrong if there
        // were.
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        visit_wires(&lop.op, |sense, wire| {
            if let Some(r) = wire.reg() {
                if sense.is_read() {
                    reads.push(r);
                } else {
                    writes.push(r);
                }
            }
        });
        let reads: Vec<usize> = reads.into_iter().map(|r| intern(r, &mut reg_idx)).collect();
        for w in writes {
            let w = intern(w, &mut reg_idx);
            edges.push((w, reads.clone()));
        }
    }

    // A child contributes an edge from each of its input bits to each of
    // its output bits that it says are connected. In the parent those are
    // `d` bits and `q` bits respectively, so the edge closes a path that
    // leaves the kernel, crosses the child, and comes back.
    for child in children {
        let field = Path::default().field(&child.field);
        let (d_range, _) = bit_range(d_kind, &field)?;
        let (q_range, _) = bit_range(q_kind, &field)?;
        for (ci, c_in) in child.matrix.inputs.iter().enumerate() {
            let (c_in_range, _) = bit_range(child.input_kind, c_in)?;
            for (co, c_out) in child.matrix.outputs.iter().enumerate() {
                if !child.matrix.i_to_o.get(ci, co) {
                    continue;
                }
                let (c_out_range, _) = bit_range(child.output_kind, c_out)?;
                for d_off in c_in_range.clone() {
                    for q_off in c_out_range.clone() {
                        let d_idx = d_range.start + d_off;
                        let q_idx = q_range.start + q_off;
                        if let (Some(Some(d_reg)), Some(q_reg)) =
                            (d_bits.get(d_idx), q_bits.get(q_idx))
                        {
                            let w = intern(*q_reg, &mut reg_idx);
                            let r = intern(*d_reg, &mut reg_idx);
                            edges.push((w, vec![r]));
                        }
                    }
                }
            }
        }
    }

    // Before the fixpoint, because the fixpoint saturates over a cycle
    // and would destroy the very structure being looked for. See
    // `find_cycle_in`.
    //
    // Any cycle found here necessarily crosses at least one child, which
    // is what makes the reported walk nameable: `optimize_ntl` above runs
    // `ReorderInstructions`, so a kernel whose own dataflow were cyclic
    // would already have failed. Every cycle reaching this point must
    // therefore use one of the child edges added just now, and so has at
    // least one `d` and one `q` boundary register on it.
    if let Some(cycle) = find_cycle_in(&edges, reg_idx.len()) {
        // Name the hops. Only the boundary registers -- a child's input
        // or output bit -- have names a user would recognise; interior
        // kernel registers are dropped, which is what turns a walk over
        // netlist registers into a walk over widget ports.
        let port_of = |interned: usize| -> Option<CyclePort> {
            let named = |bits: &[Option<RegisterId<WireKind>>],
                         paths: &[Path],
                         kind: Kind,
                         side: PortSide|
             -> Option<CyclePort> {
                let off = bits
                    .iter()
                    .position(|b| b.and_then(|r| reg_idx.get(&r).copied()) == Some(interned))?;
                let path = paths
                    .iter()
                    .find(|p| bit_range(kind, p).is_ok_and(|(r, _)| r.contains(&off)))?;
                let (widget, port) = children.iter().find_map(|c| {
                    let prefix = Path::default().field(&c.field);
                    Some((c.field.clone(), path.strip_prefix(&prefix).ok()?))
                })?;
                Some(CyclePort { widget, port, side })
            };
            named(&d_bits, &out.d_paths, d_kind, PortSide::Input).or_else(|| {
                let q_as_opt: Vec<Option<RegisterId<WireKind>>> =
                    q_bits.iter().map(|r| Some(*r)).collect();
                named(&q_as_opt, &out.q_paths, q_kind, PortSide::Output)
            })
        };
        let ports: Vec<CyclePort> = cycle.iter().filter_map(|n| port_of(*n)).collect();
        return Ok(Outcome::Cycle(ports));
    }

    // Seed: each boundary bit is its own source. Interned after the edges
    // so that a boundary bit the kernel never reads still gets an index.
    let mut seeds: Vec<(usize, usize)> = Vec::new();
    for (n, r) in i_bits.iter().chain(q_bits.iter()).enumerate() {
        seeds.push((intern(*r, &mut reg_idx), n));
    }
    let n_regs = reg_idx.len();
    let mut sets = vec![0u64; n_regs * words];
    for (reg, src) in seeds {
        sets[reg * words + src / 64] |= 1u64 << (src % 64);
    }

    // Forward fixpoint. Unordered ops and cycles are both fine: a cycle
    // just means the sets stop growing a round later than they otherwise
    // would.
    let mut scratch = vec![0u64; words];
    loop {
        let mut changed = false;
        for (w, reads) in &edges {
            scratch.iter_mut().for_each(|x| *x = 0);
            for r in reads {
                let base = r * words;
                for k in 0..words {
                    scratch[k] |= sets[base + k];
                }
            }
            let base = w * words;
            for k in 0..words {
                let before = sets[base + k];
                let after = before | scratch[k];
                if after != before {
                    sets[base + k] = after;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let source_bits = |reg: Option<&Option<RegisterId<WireKind>>>| -> Vec<usize> {
        let Some(Some(r)) = reg else {
            return Vec::new();
        };
        let Some(idx) = reg_idx.get(r) else {
            return Vec::new();
        };
        let base = idx * words;
        (0..n_src)
            .filter(|b| sets[base + b / 64] & (1u64 << (b % 64)) != 0)
            .collect()
    };

    // Read the four relations off the source sets, aggregating bit
    // indices into the field paths a caller can name.
    let record = |dst_paths: &[Path],
                  dst_kind: Kind,
                  dst_bits: &[Option<RegisterId<WireKind>>],
                  into_i: &mut BitMatrix,
                  into_q: &mut BitMatrix|
     -> Result<(), RHDLError> {
        for (dst_idx, dst_path) in dst_paths.iter().enumerate() {
            let (range, _) = bit_range(dst_kind, dst_path)?;
            for off in range {
                for src in source_bits(dst_bits.get(off)) {
                    if src < n_i {
                        into_i.set(src, dst_idx);
                    } else {
                        into_q.set(src - n_i, dst_idx);
                    }
                }
            }
        }
        Ok(())
    };
    // `i_*` rows are indexed by input bit, `q_*` rows by q bit; both are
    // aggregated to field paths afterwards.
    let mut i_to_o_bits = BitMatrix::new(n_i, out.outputs.len());
    let mut q_to_o_bits = BitMatrix::new(q_bits.len(), out.outputs.len());
    record(
        &out.outputs.clone(),
        output_kind,
        &o_bits,
        &mut i_to_o_bits,
        &mut q_to_o_bits,
    )?;
    let mut i_to_d_bits = BitMatrix::new(n_i, out.d_paths.len());
    let mut q_to_d_bits = BitMatrix::new(q_bits.len(), out.d_paths.len());
    record(
        &out.d_paths.clone(),
        d_kind,
        &d_bits,
        &mut i_to_d_bits,
        &mut q_to_d_bits,
    )?;

    // Collapse the bit-indexed rows onto field paths.
    out.i_to_o = fold_rows(&i_to_o_bits, &out.inputs, input_kind)?;
    out.i_to_d = fold_rows(&i_to_d_bits, &out.inputs, input_kind)?;
    out.q_to_o = fold_rows(&q_to_o_bits, &out.q_paths, q_kind)?;
    out.q_to_d = fold_rows(&q_to_d_bits, &out.q_paths, q_kind)?;
    Ok(Outcome::Acyclic(out))
}

/// Turn a bit-indexed row space into a field-path-indexed one.
///
/// A field reaches a destination if any of its bits does.
fn fold_rows(src: &BitMatrix, paths: &[Path], kind: Kind) -> Result<BitMatrix, RHDLError> {
    let mut out = BitMatrix::new(paths.len(), src.cols());
    for (idx, path) in paths.iter().enumerate() {
        let (range, _) = bit_range(kind, path)?;
        for bit in range {
            for col in src.row_iter(bit) {
                out.set(idx, col);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bit_matrix_stores_and_reads_back() {
        let mut m = BitMatrix::new(3, 100);
        assert!(!m.any());
        m.set(0, 0);
        m.set(1, 63);
        m.set(1, 64);
        m.set(2, 99);
        assert!(m.any());
        assert!(m.get(0, 0));
        assert!(m.get(1, 63));
        assert!(m.get(1, 64), "the word boundary is not a special case");
        assert!(m.get(2, 99));
        assert!(!m.get(0, 1));
        assert!(m.row_any(1));
        assert!(!m.row_any(0) || m.get(0, 0));
        assert_eq!(m.row_iter(1).collect::<Vec<_>>(), vec![63, 64]);
    }

    /// Out-of-range access answers conservatively rather than panicking.
    ///
    /// A parent indexing a child's matrix with a stale index should get
    /// "not reachable" and a wrong answer it can debug, not a crash in
    /// the middle of descriptor construction.
    #[test]
    fn out_of_range_access_is_false_not_a_panic() {
        let mut m = BitMatrix::new(2, 2);
        m.set(5, 5);
        assert!(!m.get(5, 5));
        assert!(!m.get(0, 9));
        assert!(!m.row_any(7));
        assert!(!m.any(), "an ignored write must not land somewhere else");
    }

    #[test]
    fn an_empty_matrix_is_well_formed() {
        let m = BitMatrix::new(0, 0);
        assert!(!m.any());
        assert!(!m.get(0, 0));
        assert_eq!(m.rows(), 0);
        assert_eq!(m.cols(), 0);
    }
}
