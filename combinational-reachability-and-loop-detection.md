# Combinational Reachability and Loop Detection — Design Plan

> **Status: design plan, not committed engineering work.** This document specifies a per-widget combinational input-output reachability analysis and a composition-level combinational-loop detection algorithm that runs *before* RHIF lowers to RTL and NTL. It subsumes the current `circuit::drc::no_combinatorial_paths` check, complements (rather than replaces) the existing NTL-level Kahn's-algorithm loop detector, and produces materially better diagnostics by reporting cycles at the widget-composition level rather than at flattened-opcode level.

---

## 1 — Motivation

RHDL today detects combinational loops correctly but late. Loop detection runs as the `ReorderInstructions` pass in NTL (`crates/rhdl-core/src/compiler/ntl_passes/reorder_instructions.rs`), which means a circuit with a structural cycle is fully lowered through RHIF → RTL → NTL before the diagnostic fires. The diagnostic itself, while span-precise, identifies the cycle in terms of *flattened netlist opcodes* — which is the right level for an algorithmic check (Kahn's algorithm finds the SCC) but the wrong level for a user trying to fix the bug.

Three observations create pressure to do better:

**The information needed is already present at RHIF.** Inside a kernel body, RHDL is purely combinational by construction (Rust enforces SSA-acyclic dataflow). Loops can only form across widget boundaries, where one widget's output combinationally feeds another widget's input. At the widget-composition level, every wire crossing a widget boundary already has a known status — registered (via a child widget's internal DFF) or live (combinational). This information is computable from RHIF directly without lowering.

**Compile-performance-plan.md wants to skip wasted work.** A circuit that's going to fail at NTL-level loop detection currently pays the full RHIF→RTL→NTL lowering cost on the way to that failure. Composition-level detection cuts this off at the earliest point where the cycle is structurally visible.

**Diagnostics quality is part of the AI-assist wedge.** Per `chisel-strategy.md` §6 ("Win on diagnostic quality") and CLAUDE.md §0 generally, RHDL's diagnostic surface is one of the differentiators against Chisel and BSV. A "combinational cycle through `widget_a.out` → `widget_b.in` → `widget_b.out` → `widget_c.in` → `widget_c.out` → `widget_a.in`" diagnostic is the diagnostic users (and LLMs) actually need to fix the bug. The current NTL diagnostic — pointing at opcodes after they've been flattened, renamed, and possibly re-ordered — is harder to read.

The fourth observation is forward-looking: the per-widget reachability matrix this analysis produces is exactly the metadata `package-manager-architecture.md` §9 anticipates needing for cross-crate clock-domain consistency claims. Computing it once for both purposes is strictly better than computing it twice.

---

## 2 — Goals and non-goals

### Goals

- **Per-widget combinational input-output reachability matrix** computed once per widget and exposed as part of the widget's `Descriptor`. Encodes which `I` inputs combinationally reach which `O` outputs, which `I` inputs combinationally reach which `D` outputs (sub-widget inputs), and which `Q` inputs (sub-widget outputs) combinationally reach which `O` outputs.
- **Composition-level cycle detection** that runs at widget elaboration time and reports cycles in widget-port terms.
- **Span-precise diagnostics** that name the specific user-visible widget instances and ports involved in a cycle, not flattened opcodes.
- **Subsume `no_combinatorial_paths`** — the existing DRC becomes a thin wrapper around querying the matrix.
- **Preserve the NTL-level loop detector as a backstop.** The composition-level analysis should catch every loop the NTL analysis would, but the NTL pass remains in place as a safety net (and as the canonical detection point for any loop that arose from a compiler bug rather than from user code).
- **Per-widget metadata exposed at the package boundary.** The reachability matrix is part of the widget's API contract — published with the widget in `package-manager-architecture.md` Tier-1 metadata.

### Non-goals (v1)

- **Replacing the NTL-level detection.** The NTL pass stays as a safety net. v1 is *additive* — earlier, better diagnostics; same correctness floor.
- **Timing analysis.** The matrix says *whether* there's a combinational path, not how *long* it is. Static timing estimation is a separate concern (touched by `auto-pipelining-plan.md`).
- **Cross-clock-domain analysis.** Phantom-typed clock domains are already enforced by RHDL's type system at RHIF level. The combinational-reachability analysis assumes domains are correct (which they are, by type).
- **Detecting *behaviorally* benign cycles** (cycles whose feedback resolves to a fixed point, e.g. `if x { x } else { 0 }`). These are structural cycles and are reported as such; downstream synthesis tools handle them inconsistently and RHDL takes the strict view.
- **Replacing `circuit::drc::no_combinatorial_paths` for users who explicitly want the feedthrough check.** The DRC stays available as a public API; its implementation just becomes a query against the matrix instead of an independent NTL graph traversal.

---

## 3 — Where this sits

This plan touches `rhdl-core` and the `Descriptor` machinery; it does not affect the macro layer, the type system, or the user-facing widget API.

Cross-references:

- **`architecture.md`** — the `Descriptor` is the canonical widget-metadata carrier. The new reachability matrix is added as a field on `Descriptor`. No architectural change; an additive extension consistent with §3 of architecture.md.
- **`compile-performance-plan.md`** — Phase 1 ("skip work in release") gains a new entry point: skip RHIF→RTL→NTL lowering when composition-level cycle detection has already failed.
- **`package-manager-architecture.md`** §9 — cross-crate clock-domain consistency uses the same matrix metadata, so this work should land before Package Manager Phase 2 (registry overlay), or the registry will need a separate matrix-derivation pass that duplicates the work.
- **`auto-pipelining-plan.md`** — the matrix gives auto-pipelining a precise per-widget feedthrough graph to work against. Auto-pipelining can run *after* this analysis without re-deriving the data.
- **`fsm-architecture.md`** — FSM widgets often have I→O combinational paths (the next-state and output logic both read inputs combinationally). The matrix correctly classifies these; no FSM-specific handling needed.
- **`stream-bus-architecture.md`** — `RCStream` widgets use the matrix to declare per-port combinational characteristics. Carloni relay stations break combinational paths; the matrix encodes this without special-casing.
- **`circuit::drc::no_combinatorial_paths`** — refactored to query the matrix. Public API unchanged; implementation simplified.
- **NTL `ReorderInstructions` pass** — kept in place as the safety-net backstop. Its diagnostic remains; it just rarely fires in practice once composition-level detection is wired in.

---

## 4 — The per-widget combinational reachability matrix

### 4.1 — The data structure

Each widget's `Descriptor` gains a `combinational_reachability` field of type `ReachabilityMatrix`:

```rust
/// Combinational reachability for a single widget.
///
/// For each (input slot, output slot) pair, encodes whether there is a
/// combinational path from input to output through the widget's interior.
/// Encodes paths through sub-widgets recursively via their own matrices.
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub struct ReachabilityMatrix {
    /// I-port input field paths.  Each entry is a `Path` into the widget's `I` type.
    pub inputs: Vec<Path>,

    /// O-port output field paths.  Each entry is a `Path` into the widget's `O` type.
    pub outputs: Vec<Path>,

    /// `i_to_o[i_idx][o_idx] == true` iff input field `inputs[i_idx]` combinationally
    /// reaches output field `outputs[o_idx]`.
    pub i_to_o: BitMatrix,

    /// `i_to_d[i_idx][d_idx] == true` iff input field `inputs[i_idx]` combinationally
    /// reaches sub-widget input field `d_paths[d_idx]`.  Used during composition.
    pub i_to_d: BitMatrix,

    /// `q_to_o[q_idx][o_idx] == true` iff sub-widget output field `q_paths[q_idx]`
    /// combinationally reaches output field `outputs[o_idx]`.  Used during composition.
    pub q_to_o: BitMatrix,

    /// `q_to_d[q_idx][d_idx] == true` iff sub-widget output field `q_paths[q_idx]`
    /// combinationally reaches sub-widget input field `d_paths[d_idx]`.
    /// Used during composition; this is the channel through which child-to-child
    /// combinational paths form in the parent's kernel.
    pub q_to_d: BitMatrix,

    /// `d_paths` and `q_paths` for the sub-widgets, in flat field-path form.
    /// Indexed in parallel with the sub-widget instance order in `Descriptor`.
    pub d_paths: Vec<Path>,
    pub q_paths: Vec<Path>,
}
```

`BitMatrix` is a packed bit array; for a widget with N inputs and M outputs, the four matrices total approximately `(NM + ND + QM + QD) / 8` bytes. For typical widgets this is under 64 bytes; for the largest widgets (a 64-port crossbar arbiter) it might reach a few KB.

The four matrices together capture **every** kind of combinational path that matters for cycle detection at composition time:

- `i_to_o` — feedthrough (input combinationally reaches output). This is what `no_combinatorial_paths` checks.
- `i_to_d` — input combinationally reaches sub-widget input. Required for cycle detection across hierarchy.
- `q_to_o` — sub-widget output combinationally reaches widget output. Required for cycle detection across hierarchy.
- `q_to_d` — sub-widget output combinationally reaches sub-widget input. Critical for child-to-child cycles within the same parent.

### 4.2 — Recursive computation algorithm

The matrix is computed bottom-up over the widget hierarchy:

```
fn compute_matrix(widget: &Descriptor) -> ReachabilityMatrix {
    // 1. Recursively compute matrices for every sub-widget instance.
    let sub_matrices: Vec<ReachabilityMatrix> = widget.sub_widgets()
        .map(|sub| sub.descriptor.combinational_reachability.clone())
        .collect();

    // 2. Build the kernel's intra-kernel data-flow graph.
    //    Nodes: every leaf field-path of I, Q (per sub-widget), kernel temporaries, O, D (per sub-widget).
    //    Edges: from each opcode's read operands to its write operands.
    let mut graph = build_intra_kernel_graph(&widget.kernel_object);

    // 3. Augment the graph with sub-widget edges from each sub-widget's matrix.
    //    For each sub-widget index s and each (i, o) in sub_matrices[s].i_to_o:
    //      add edge from D-field d.subN.i to Q-field q.subN.o
    for (s, sub_matrix) in sub_matrices.iter().enumerate() {
        for i_idx in 0..sub_matrix.inputs.len() {
            for o_idx in 0..sub_matrix.outputs.len() {
                if sub_matrix.i_to_o[i_idx][o_idx] {
                    let d_node = graph.node_for_d(s, &sub_matrix.inputs[i_idx]);
                    let q_node = graph.node_for_q(s, &sub_matrix.outputs[o_idx]);
                    graph.add_edge(d_node, q_node);
                }
            }
        }
    }

    // 4. For every (input, output) pair, compute reachability via the augmented graph.
    let mut matrix = ReachabilityMatrix::default();
    for i_path in widget.input_paths() {
        for o_path in widget.output_paths() {
            matrix.i_to_o[i_path][o_path] =
                graph.has_path(graph.node_for_i(i_path), graph.node_for_o(o_path));
        }
    }
    // Similarly for i_to_d, q_to_o, q_to_d.

    matrix
}
```

The intra-kernel graph (step 2) is built from the RHIF `Object`. Each opcode contributes edges from its read-operands to its write-operands. The opcode-to-edge mapping is small:

- `Binary { lhs, arg1, arg2 }` → edges `arg1 → lhs` and `arg2 → lhs`.
- `Select { lhs, cond, true_value, false_value }` → edges `cond → lhs`, `true_value → lhs`, `false_value → lhs`.
- `Index { lhs, arg, path: _ }` → edge `arg → lhs`.
- `Splice { lhs, orig, path: _, subst }` → edges `orig → lhs` and `subst → lhs`.
- ... (one rule per opcode, exhaustive over the 19 opcodes in `rhif::spec::OpCode`).

The graph is fundamentally just a use-def graph of the RHIF object. The single non-trivial piece is the augmentation in step 3, which threads sub-widget reachability into the parent's analysis.

### 4.3 — Leaf-widget specialization

For widgets with no sub-widgets (where `Q = ()` and `D = ()`), the analysis simplifies dramatically. The `q_to_o`, `q_to_d`, and `i_to_d` matrices are all empty. Only `i_to_o` is non-trivial, and it's computed by a direct DFS over the kernel's RHIF graph from each input field to each output field.

This is the same shape as the existing `no_combinatorial_paths` check, just phrased per-widget rather than for the whole circuit.

### 4.4 — Caching

The matrix depends only on the widget's RHIF `Object` and on the matrices of its sub-widgets. It's deterministic (no compile-time randomness, no thread-local state, no environmental dependencies) so caching is straightforward:

- Cache key: hash of `(kernel.hash_value(), [sub_matrix.hash() for sub in sub_widgets])`.
- Cache value: `ReachabilityMatrix`.
- Cache lifetime: per-compilation (for now); per-build with cargo's incremental cache (Phase 4 of compile-performance-plan).

The cache hits whenever a widget is instantiated multiple times in a design (e.g., a parameterized FIFO instantiated at three different sizes).

---

## 5 — Composition-level cycle detection

### 5.1 — The algorithm

Once every widget has its `ReachabilityMatrix`, cycle detection at any composition level is a simple graph-cycle check on the *augmented intra-kernel graph* used in §4.2 step 3.

A combinational cycle exists if and only if the augmented graph contains a directed cycle — which, for a leaf widget, is impossible (kernel SSA is acyclic by construction), and for a composite widget can only arise from the sub-widget edges added in step 3.

The check is:

```rust
fn check_combinational_cycles(widget: &Descriptor) -> Result<(), CycleError> {
    let graph = build_augmented_graph(widget);
    if let Some(cycle) = graph.find_cycle() {
        return Err(CycleError::from_cycle(widget, cycle));
    }
    // Recurse into sub-widgets
    for sub in widget.sub_widgets() {
        check_combinational_cycles(&sub.descriptor)?;
    }
    Ok(())
}
```

`find_cycle()` is a standard DFS-based cycle detector (the petgraph crate, already a dependency, exposes `is_cyclic_directed` and `tarjan_scc` — Tarjan's algorithm for strongly-connected components is preferred because it identifies the specific cycle members rather than just yes/no).

The recursion is well-defined and terminates: each call descends one level in the widget hierarchy.

### 5.2 — Where it runs in the compiler pipeline

The check runs after RHIF type-checking and after every widget's `Descriptor` has been finalized, but *before* any RHIF→RTL lowering happens. The natural integration point is `Synchronous::descriptor()` and `Circuit::descriptor()` — both currently produce a `Descriptor`; both are extended to compute and embed the `ReachabilityMatrix` and to run the cycle check.

If the check fails, `descriptor()` returns the cycle error. This means the existing `logic_loop.rs` test (which calls `uut.descriptor("uut".into())` and expects an error) continues to work — but the error type changes from `NetLoopError` (NTL-level) to `CombinationalCycle` (composition-level) with much better diagnostics.

The NTL-level `ReorderInstructions` pass remains in place; it just very rarely fires for user code now, since composition-level detection caught the cycle earlier.

### 5.3 — Performance

For a widget hierarchy of N total instances and average matrix size M_avg (rows × columns), the analysis is:

- Bottom-up matrix computation: O(N · M_avg) graph operations, plus O(K) opcode-to-graph-edge translation per kernel of K opcodes.
- Cycle detection: O(N · (V + E)) for V graph nodes and E edges per composition level.

For a typical ~50-widget design (a small SoC) with average widget size of 200 RHIF opcodes, this is on the order of a few milliseconds — much faster than the lowering work that's currently being wasted on circuits that fail. Net compile-time win, even before factoring in the early-exit benefit.

---

## 6 — Diagnostics

### 6.1 — The cycle error format

```rust
#[derive(Debug, Error)]
pub struct CombinationalCycle {
    src: SourcePool,
    /// The widget instances and ports forming the cycle, in cycle order.
    pub edges: Vec<CycleEdge>,
}

#[derive(Debug, Clone)]
pub struct CycleEdge {
    /// User-visible widget instance name (e.g., "core.fifo.write_logic").
    pub from_widget: String,
    /// Output port path (e.g., "full" or "data.bytes[2]").
    pub from_port: Path,
    /// Source span of the kernel statement that wired this edge.
    pub from_span: SourceSpan,
    pub to_widget: String,
    pub to_port: Path,
    pub to_span: SourceSpan,
}
```

The miette diagnostic is rendered with one labeled span per cycle edge, plus a summary in the error message:

```
error: combinational cycle through 3 widgets
  ┌─ src/example.rs:42:5
  │
42 │     d.b.input = q.a.output;
   │     ^^^^^^^^^^^^^^^^^^^^^^^ a.output → b.input
43 │     d.c.input = q.b.output;
   │     ^^^^^^^^^^^^^^^^^^^^^^^ b.output → c.input
44 │     d.a.input = q.c.output;
   │     ^^^^^^^^^^^^^^^^^^^^^^^ c.output → a.input (closes cycle)
   │
   = help: this combinational cycle has no register breaking the loop.  Insert a flip flop
           on one of the edges, or restructure the data flow to avoid the cycle.
```

The cycle path is reported in the order traced; the closing edge is marked. Each edge points at the kernel statement that wired it, not at flattened opcodes.

### 6.2 — Diagnostic for I→O feedthroughs

The existing `no_combinatorial_paths` DRC produces a similar diagnostic but for paths from primary inputs to primary outputs. The new infrastructure produces this from the same matrix:

```
warning (when no_combinatorial_paths is enabled): combinational path from input to output
  ┌─ src/example.rs:30:5
  │
30 │     o.data = q.subA.passthrough;
   │     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ input.req combinationally reaches output.data
   │
   = help: a combinational input-to-output path may cause timing closure issues.
           Consider adding a registration stage or relaxing the no-feedthrough constraint.
```

Whether this fires as a warning, an error, or not at all is configurable per the DRC's existing API. The matrix lookup is one bit in `i_to_o`.

---

## 7 — Relationship to existing checks

| Check | Lives at | Triggers | Status under this plan |
|---|---|---|---|
| `circuit::drc::no_combinatorial_paths` | NTL graph build | Opt-in user call | Refactored to query the matrix. Public API unchanged. |
| NTL `ReorderInstructions` Kahn's algorithm | NTL pass pipeline | Every compile that produces NTL | Stays in place as backstop; rarely fires in practice. |
| New `CombinationalCycle` check | Widget descriptor finalization | Every `descriptor()` call | New; this plan. |

The three checks operate at different IR levels and on different graph structures; they complement rather than replace each other. The composition-level check has the best diagnostics; the NTL-level check has the strongest correctness guarantee (it sees the fully flattened netlist); the DRC has the most user-controllable scope.

---

## 8 — Phasing

### Phase 1 — Per-widget reachability matrix — **SHIPPED**

Compute the `ReachabilityMatrix` for every widget at descriptor finalization. Expose it on `Descriptor`. No behavior change for users yet; this is data-gathering work.

**As shipped, with three deviations from §4 above:**

- **The graph is built from RTL, not RHIF.** §4.2 specifies a use-def walk over the RHIF `Object`, but RHIF is not retained past stage 1 — `Descriptor::kernel` is an `rtl::Object`. Lowering it with `build_ntl_from_rtl` yields a netlist whose ports are exactly `[clock_reset, i, q]` in and `[o, d]` out for a synchronous widget (`[i, q]` for an asynchronous one, whose clocks travel inside `I`), so all four relations fall out of a single reachability computation. This also removes the need to transcribe the operand senses of nineteen RHIF opcodes by hand, which §4.2 lists as the bulk of the work.
- **The analysis is bit-level; only the storage is field-level.** §9 lists bit-level as a v2 stretch. It turned out to be the *easier* option rather than the harder one, because the netlist is already bit-level and `leaf_paths` + `bit_range` already exist to aggregate. The matrices are still stored per field path, which is what a diagnostic can name.
- **No cache, and the measurement is why.** §4.4 specifies one, and Phase 1 lists a hit-rate measurement as a deliverable. Measured overhead on the full workspace suite is 0.6% (299.8s against a 297.9s baseline for `rhdl-fpga`'s 1377 tests), so a cache would be optimising something that is not costing anything. Worth revisiting if Phase 3's cycle detection is more expensive, or when a design appears whose widget count makes it matter — but building it now would be speculative machinery with a hit-rate metric attached to justify itself.

The first version *did* cost 31% (390s against the same baseline) because it kept a `HashSet<usize>` per netlist register. Packed bitsets over dense register indices removed it. Recorded because the naive shape of this analysis is genuinely slow, and anyone extending it in Phase 3 will be tempted by the same convenient data structure.

Deliverables:
- `ReachabilityMatrix` struct in `rhdl-core/src/circuit/reachability.rs`.
- Computation algorithm in the same module (recursive, bottom-up).
- Tests against a corpus of existing widgets, with committed expected matrices.
- Integration into `Synchronous::descriptor()` and `Circuit::descriptor()`.
- Cache hit-rate measurement on the existing widget corpus (sanity check that caching is working).

Acceptance: every widget in `crates/rhdl-fpga/src/` has a computed matrix that round-trips through the existing test suite without behavior change.

### Phase 2 — Subsume `no_combinatorial_paths` — **SHIPPED**

Refactor `circuit::drc::no_combinatorial_paths` to query the matrix instead of doing its own NTL graph traversal. Public API unchanged; behavior unchanged; implementation simplified.

**As shipped, with two departures from the sketch above:**

- **The netlist walk is retained, for spans only.** The matrix records which *fields* are connected, not which opcodes connected them, and spans live on opcodes — so the matrix cannot reproduce the diagnostic, and the diagnostic has a committed expectation file. The verdict now comes from `i_to_o`; the walk runs only when the verdict is "there is a path", to say where. The clean case — overwhelmingly the common one, since dozens of widget tests assert it as a property — no longer builds a graph over the flattened netlist at all.
- **The matrix must be computed on *optimised* NTL.** This was not in the plan and is a correctness requirement rather than a tuning choice. The raw `build_ntl_from_rtl` lowering keeps every dataflow dependence the kernel's source has, including the vacuous ones: a Carloni relay assigns `stop_out = true` in *both* arms of `if i.stop_in`, so the raw netlist has `stop_in` selecting between two constants. The existing DRC never saw that, because `ntl::builder::Builder::build` optimises. Analysed raw, the matrix over-approximated — and over-approximation is not harmlessly conservative here, because Phase 3 turns these relations into loop *errors*, so a path with no hardware behind it would reject a valid design. On `SyncFIFO<b8, 4>` optimising removed 6 of 11 `i_to_o` entries and 4 of 12 `i_to_d` entries. Cost: 1.6% on the workspace suite, against 0.6% for the raw version.

Phase 1 also left five descriptor builders with a defaulted matrix — `function`, `array`, `chain`, `adapter`, `phantom`. An empty matrix reads as "no feedthrough", so once the DRC trusts it those become *false negatives*: the check passes silently on a widget that has a path. Four needed wiring before Phase 2 was sound (`phantom` is genuinely empty — all four of its kinds are `Kind::Empty`), and `array`, `chain` and `adapter` needed composition logic of their own because they have no kernel and empty `D`/`Q`: an array is the block diagonal of its element, a chain is a boolean matrix product, an adapter is a passthrough.

Deliverables:
- New implementation that queries `descriptor.combinational_reachability.i_to_o`.
- Existing `faulty_reducer` test continues to pass (committed expectation file unchanged).
- Performance benchmark showing the new implementation is at least as fast as the old.

Acceptance: zero behavior change observable from outside the function.

### Phase 3 — Composition-level cycle detection and diagnostic — **SHIPPED**

Add the new `CombinationalCycle` error and run the cycle check during descriptor finalization. Update `logic_loop.rs` test and others to expect the new diagnostic format.

Deliverables:
- `CombinationalCycle` error type in `rhdl-core/src/circuit/error.rs`.
- Cycle detection algorithm in `rhdl-core/src/circuit/reachability.rs`.
- Wired into `descriptor()` for both `Synchronous` and `Circuit`.
- Updated `logic_loop.rs` expectation file showing the new (better) diagnostic.
- New tests for cycles spanning 3, 4, and 5 widgets to validate diagnostic clarity.
- Documentation update in CLAUDE.md §0 and `architecture.md` §3.

**As shipped, with one correction to §5.1 above and two gaps left open:**

- **§5.1 says the check is "a simple graph-cycle check on the augmented intra-kernel graph", and that wording is load-bearing.** The first implementation here read the cycle out of `q_to_d` instead — asking which child outputs reach which child inputs and looking for a ring. It reported nonsense (`left -> left -> left` on the two-widget case). The reason is worth writing down: **the matrix is a transitive closure computed by a fixpoint, and a fixpoint over a cyclic graph saturates.** On exactly the designs where a cycle exists, `q_to_d` goes dense and every pair of ports looks connected, so the matrix is structurally incapable of locating the cycle that makes it dense. The check must run on the edge graph, before the fixpoint. That is also cheaper: a cyclic design skips the fixpoint entirely, and its matrix would have been meaningless.
- **The netlist-level backstop has lost its only test.** `crates/rhdl/tests/logic_loop.rs` was the sole thing exercising `ReorderInstructions`, and the composition-level check now reports first, so that route is gone. §7 above says the NTL pass "stays in place as backstop"; it does, but nothing tests it. A direct pass-level test needs a cyclic `ntl::Object`, and the only public constructor — `Builder::build` — runs the whole optimiser, so a hand-built netlist is transformed before the pass sees it. Open.
- **`reorder_instructions.rs` panics rather than erroring on one malformed input.** Found while attempting that test: `write_regs_to_op[&failed]` is an unguarded map index, so a needed register with no writer panics. Two lines below, the analogous case returns an ICE properly. Hard to reach through the normal pipeline, but it is a compiler panic. Open.

Acceptance: the existing `logic_loop.rs` test fails with the new diagnostic, the new diagnostic is materially clearer than the old NTL-level one, and the NTL-level `ReorderInstructions` continues to fire as a safety net for any cycle that escapes the new check (which should be zero for well-formed user code).

---

## 9 — Risks and open questions

**Matrix maintenance across IR passes.** The matrix depends on the widget's RHIF `Object`. Any pass that mutates the kernel must invalidate or recompute the matrix. Mitigation: the matrix is computed at descriptor finalization (post all RHIF passes); if a future pass changes the RHIF after finalization, that pass must explicitly recompute. Pattern matches the existing symbol-table-completeness invariant.

**Per-widget computational cost on very large widgets.** A widget with N inputs and M outputs has a matrix of size O(NM). For a 64-input arbiter or a 256-bit-wide DSP block, this can be a few KB of metadata per widget. Cache hits keep total memory bounded across instances, but the *worst-case* widget is still large. Mitigation: encode the matrix as a sparse bit-set when density is low, packed when dense. Most widgets are sparse.

**Recursive analysis interacts with widget instantiation depth.** A deeply hierarchical design (10+ levels deep) requires 10+ recursive calls. Stack-safe iteration is preferred; the implementation should use an explicit stack rather than direct recursion. Standard pattern; well-understood.

**False positives via over-conservative analysis.** The matrix tracks reachability at the field-path level, not at the bit level. A widget that reads bit 0 of an input and writes bit 7 of an output has a marked combinational path between them even if the bits are independent. This is correct (any cycle through this widget is a real cycle in the netlist), but it can mark a *fewer-than-expected* number of cycle as resolvable by the user via "but bit 0 doesn't actually feed bit 7." Mitigation: bit-level analysis is a v2 feature; v1 is field-path-level. The current NTL check is bit-level so the safety net catches the difference if it matters.

**Cross-clock-domain cycles.** RHDL's type system enforces that combinational mixing across clock domains is a type error — `Signal<T, Red>` and `Signal<T, Blue>` cannot be combined without an explicit `Retime`. This means cross-domain cycles cannot exist in well-typed code. The matrix doesn't need to special-case domains; the type system has already done that work.

**Widgets with parametric inputs whose count varies with const generics.** A widget like `pub struct Crossbar<const N: usize>` has N input ports, where N is a const generic. The matrix depends on N. Mitigation: matrices are computed per *monomorphization*, not per generic widget definition. Same shape as how the rest of the compiler handles generics.

**`no_combinatorial_paths` users who depend on the current diagnostic format.** The DRC currently produces a `CombinatorialPath` diagnostic; under Phase 2, it produces a query-derived but semantically equivalent diagnostic. No user code should depend on the exact text format, but if any does, the change is observable. Mitigation: keep the diagnostic text identical or near-identical in Phase 2; reserve format changes for an explicit user opt-in.

**The matrix as published API metadata.** Per `package-manager-architecture.md` §9, the matrix becomes part of the widget's API contract. This means semver implications: changing the matrix is at minimum a MINOR bump; in some cases (introducing a new I→O feedthrough that downstream consumers had assumed wasn't present) it's a MAJOR bump. Mitigation: document the matrix's semver implications in `package-manager-architecture.md` §4 as part of Phase 2 of the package manager work; integrate the matrix into `cargo rhdl semver-check`.

**Determinism.** The matrix must be deterministic for reproducibility (per `package-manager-architecture.md` §5). The graph algorithms must use deterministic iteration order (`BTreeMap`, sorted vectors, no `HashMap` iteration). Same discipline as the rest of the compiler post-determinism cleanup.

---

## 10 — Validation

How we know the analysis is correct:

**Test 1 — Existing widget corpus produces matrices that round-trip.** Every widget in `crates/rhdl-fpga/src/` is compiled with the new analysis enabled. Every existing test continues to pass. The matrix for each widget is committed as a snapshot file under `crates/rhdl-fpga/snapshots/reachability/`. Any change to a widget's interface or kernel that should change its matrix manifests as a snapshot diff that must be reviewed.

**Test 2 — Cycle detection catches what NTL detection catches.** Every test in the existing test suite that expects an NTL `NetLoopError` is augmented to also expect (and accept either) the composition-level `CombinationalCycle` error. The cycle should be caught at the new layer; the NTL safety net catches anything that escapes (which should be zero in well-formed test cases).

**Test 3 — The new diagnostic is materially better.** A panel of new tests (cycles spanning 3, 4, 5, and 7 widgets at varying hierarchy depths) is added with `expect_test` snapshots of the rendered diagnostic. Manual review of these snapshots is part of the Phase 3 acceptance criterion. The bar: a developer reading the diagnostic should be able to identify the cycle members and the user-visible kernel statements that wired them, without consulting any other tool.

**Test 4 — Performance.** A benchmark is added that compiles a corpus of 30 representative widgets with and without the new analysis and asserts that compile time has not regressed. Expected outcome: compile time *improves* for circuits that fail the cycle check (by skipping subsequent lowering); compile time stays approximately constant for circuits that pass.

**Test 5 — The matrix is invariant to widget instantiation count.** Two tests: one widget instantiated once, the same widget instantiated 100 times (e.g., a parameterized array of FIFOs). The matrix per instance is identical; the cycle-check time grows linearly. Validates that caching is working.

---

## 11 — Crate organization

Implementation lives in `rhdl-core`:

```
crates/rhdl-core/src/circuit/
  reachability.rs           NEW — matrix data structure and algorithms
  drc.rs                    UPDATED — no_combinatorial_paths refactored to query the matrix
  error.rs                  UPDATED — CombinationalCycle error added
  scoped_name.rs            UNCHANGED
  ...
```

No new crates. No changes to public re-exports for v1 — `ReachabilityMatrix` and `CombinationalCycle` are pub-via-`rhdl-core` but not surfaced on the `rhdl` meta-crate's prelude in v1. They become part of the package-manager-published metadata (per `package-manager-architecture.md` §9) once that work lands.

---

## 12 — References

- Existing implementation: `crates/rhdl-core/src/compiler/ntl_passes/reorder_instructions.rs` (NTL-level Kahn's algorithm).
- Existing implementation: `crates/rhdl-core/src/circuit/drc.rs` (`no_combinatorial_paths` DRC).
- `architecture.md` §3 — Inside `rhdl-core` (where the new module sits in the IR-stage taxonomy).
- `compile-performance-plan.md` — early-exit-on-failure pattern.
- `package-manager-architecture.md` §9 — cross-crate clock-domain consistency, the downstream consumer of the matrix.
- `auto-pipelining-plan.md` — the matrix is also useful for pipeline-cut-point selection.
- `fsm-architecture.md` §6 — FSM widgets with combinational next-state logic; the matrix correctly classifies them.
- petgraph crate — used for the graph data structure (already a `rhdl-core` dependency).
- Tarjan, Robert. "Depth-first search and linear graph algorithms." SIAM J. Comput. 1, no. 2 (1972): 146–160. (For SCC-based cycle isolation.)
- Kahn, A. B. "Topological sorting of large networks." Communications of the ACM 5, no. 11 (1962): 558–562. (For the existing NTL-level algorithm.)

---

## 13 — Decisions captured

These are normative as part of accepting this document. Revisiting them requires sign-off per CLAUDE.md §0.

1. **Combinational reachability is computed per widget at descriptor finalization, exposed as part of `Descriptor`, and recomputed only when the widget's RHIF or sub-widget matrices change.** This places the analysis at the right IR layer and avoids redundant traversal during downstream passes.

2. **Composition-level cycle detection runs at `descriptor()` finalization for every `Synchronous` and `Circuit` widget.** It is mandatory, not opt-in. Failure produces a span-precise `CombinationalCycle` diagnostic.

3. **The NTL-level `ReorderInstructions` pass remains as a safety-net backstop.** It is not removed in v1. v2 may reconsider once Phase 3 has soaked in production for at least a quarter and the new analysis has demonstrated parity with the NTL check on a diverse corpus.

4. **`circuit::drc::no_combinatorial_paths` keeps its public API; its implementation becomes a matrix query.** No user-observable change in v1.

5. **The matrix is field-path-level, not bit-level.** Bit-level analysis is a v2 stretch; v1 is conservative (correct but may flag cycles that bit-level analysis would resolve as benign).

6. **The matrix is part of the published API contract via `package-manager-architecture.md` §9.** Changes to a widget's matrix carry semver implications documented in the package manager plan.

7. **Performance regression in v1 is not acceptable.** The composition-level analysis runs in addition to existing passes in Phase 1. Phase 3 (when the analysis fully replaces the eager-NTL-lowering path for failing circuits) must produce a net compile-time improvement; if it doesn't, the integration is rolled back pending a performance fix.

8. **The cross-crate metadata format is part of `package-manager-architecture.md`, not this plan.** This document defines the in-process matrix data structure; `package-manager-architecture.md` defines the on-the-wire serialization.
