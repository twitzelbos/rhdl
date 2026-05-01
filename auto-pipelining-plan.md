# Auto-Pipelining for RHDL — Design Plan

A proposal for adding automatic pipelining to the RHDL compiler: take a `#[kernel]` function whose critical path exceeds a target clock period and insert pipeline registers automatically to meet timing while preserving functional equivalence.

This document is a design plan, not a feature specification. It surveys the prior art, situates the feature inside RHDL's existing IR architecture, proposes a phased implementation path, sketches the core algorithm, defines the user-visible API, and enumerates the open questions. References at the end use [n] in-text and resolve to the bibliography in §10.

---

## 1 — Motivation

Hardware designers spend a disproportionate share of their time on timing closure: rewriting working logic to break combinational paths that exceed a target frequency. The functional intent is clear; the *staging* is what's hard. Current practice in Verilog/SystemVerilog is to insert pipeline registers by hand, threading them through the design and adjusting downstream logic to compensate for added latency. This is error-prone, entangles the algorithm with timing concerns, makes the source unreadable, and is the reason most FPGA projects miss their initial timing target.

High-Level Synthesis (HLS) tools — Vitis HLS [12], Bambu [9], LegUp [3] — have shown that this work can be largely automated when the source is structured. Retiming algorithms — Leiserson and Saxe's foundational 1991 paper [5] and its descendants [4][7][8] — have shown that register *movement* across combinational logic is a polynomial-time problem with optimal solutions. Modern tools (Yosys [13], Vivado, Cadence Genus) all do retiming as a routine optimization. The CIRCT project [11] has a retime pass at MLIR level. The Spade HDL [10] takes a different route by giving users an explicit `pipeline` keyword.

RHDL is unusually well-positioned for this feature. Its compiler is a true multi-stage compiler [1], not a transpiler — RHIF (typed SSA) → RTL (untyped SSA) → NTL (netlist). It already has a longest-path timing estimator that maps NTL paths back to Rust source ([1, §2.4]). The kernel function is a pure `fn` over `Digital` types, which makes functional-equivalence testing trivial via the iterator-based simulator. And the type system already enforces clock-domain coloring through phantom-typed `Signal<T, Domain>`, so any pipelining transformation can preserve domain safety by construction.

The goal of this feature is to let an RHDL user write a kernel that expresses *what* the design computes, declare a target frequency, and have the compiler find a pipelined implementation that meets timing — with the original kernel function as the executable specification of correctness.

For the LLM-assisted workflow this is a force multiplier. An AI agent can propose a kernel that is functionally correct without worrying about timing; the compiler converts the candidate into a pipelined design that the agent then checks for timing-budget compliance. The agent's job moves from "thread registers through the algorithm" (hard for LLMs, requires global reasoning) to "specify the algorithm" (where LLMs are strong) plus "interpret the timing report" (mechanical).

---

## 2 — What "Auto-Pipelining" Means in RHDL

We distinguish three related operations that are sometimes conflated:

**Retiming.** Move existing registers across combinational logic to minimize the clock period without changing the register count. Polynomial-time in graph-edit complexity [5]. Functional behavior changes only in startup latency (initial state of relocated registers).

**Forward pipelining.** Add new registers to break long combinational paths. The total register count and cycle latency increase. Functional output is the original output delayed by the added latency. This is what users typically want when they say "auto-pipeline this for 250 MHz."

**Loop pipelining (II analysis).** Detect parallelism in a `for i in 0..N { ... }` loop body, fold the body into a pipeline with initiation interval *II*, accept new iterations every *II* cycles. Far more aggressive transformation; standard in HLS for software-style code. Out of scope for the first phases here.

This proposal targets **forward pipelining** as the primary feature, with retiming as a complementary optimization run after pipelining. Loop pipelining is deferred to Phase 3.

The transformation has three guarantees:

1. **Functional equivalence with latency offset.** For any input stream, output of the pipelined kernel at cycle `t + L` equals output of the original kernel at cycle `t`, where `L` is the inserted latency. Verified by direct simulation diff.
2. **Timing-budget compliance.** No combinational path between two consecutive registers (or between a register and a primary output) exceeds the user's target period.
3. **Domain safety preservation.** All inserted registers operate in the original kernel's clock domain. Cross-domain crossings are illegal as inputs to the auto-pipeliner; they remain illegal after pipelining.

---

## 3 — Where It Lives in the Compiler

RHDL has three IRs [1, §3]. Auto-pipelining could in principle live at any of them; in practice each location has different trade-offs.

**RHIF level.** Typed SSA over named Rust types. Pipelining here would mean inserting `Retime` ops between RHIF instructions. Pro: keeps source-code locality, error messages stay readable. Con: too coarse — many RHIF ops lower into multiple RTL/NTL nodes with non-trivial delay, so timing estimation at this level is approximate at best. Reasonable for *user-directed* pipeline boundaries, not for auto-insertion.

**RTL level.** Untyped SSA, bit-level. Better delay estimation than RHIF. Still slightly coarse — a Stage-3 pass on `LowerCase` or `LowerSelects` may decompose a single RTL node into many gates.

**NTL level.** Netlist with `Wire` granularity. Each node is a primitive gate-equivalent operation [1, §3]. Delay estimation here is the most accurate, and the existing longest-path estimator already operates here. **This is the right place** for the auto-pipeliner.

The pass ordering is: Stage 3 NTL optimizations run to a fixed point, then the auto-pipeliner runs on the optimized NTL, then a final cleanup pass (constant propagation, register-elimination) runs on the pipelined NTL, then Verilog emission.

The pipelining pass produces a transformed NTL `Object` with additional `Wire`s acting as pipeline-stage registers. Existing infrastructure (`SingleRegisterWrite`, `CheckForUndriven`, the symbol-table invariant pass) is reused unchanged.

### 3.1 — Dependency on the combinational reachability matrix

The auto-pipeliner consumes the per-widget combinational reachability matrix specified in `combinational-reachability-and-loop-detection.md`. Specifically:

- **`i_to_o`** identifies feedthrough paths through a widget; these are candidate cuts for inserting pipeline registers at the widget boundary.
- **`i_to_d`** and **`q_to_o`** identify combinational paths that cross sub-widget boundaries; these are candidate cuts for inter-widget retiming.
- **`q_to_d`** identifies combinational paths between sibling sub-widgets; these are precisely the edges where Carloni latency-insensitive relay stations (per `stream-bus-architecture.md` Phase 2's `RCStreamRelay`) can be inserted without breaking same-cycle protocol assumptions. The Carloni LID theorem [14] guarantees correctness of relay-station insertion *only* on LID-compliant edges; the matrix is the input that identifies which edges qualify.

**Sequencing decision:** the reachability work in `combinational-reachability-and-loop-detection.md` Phases 1-3 must land before any auto-pipelining phase begins. Otherwise the auto-pipeliner has to re-derive matrix-equivalent information, the two analyses drift apart, or there is a costly refactor when the matrix lands. This decision is recorded normatively here (as the first auto-pipelining sequencing constraint) and in CLAUDE.md §1's strategic-design-documents subsection.

**Auto-pipelining is a matrix-mutating pass.** When the auto-pipeliner inserts a register on an edge, that edge's `i_to_o` (or analogous) entry transitions from `true` to `false` — the path is no longer combinational. The post-pipelining `Descriptor` must carry a recomputed matrix. This is the pattern documented in `combinational-reachability-and-loop-detection.md` §9 ("Matrix maintenance across IR passes") and applies here directly. The recomputation is incremental — only widgets that gained inserted registers need their matrices updated.

**Matrix correctness is a precondition for soundness.** The auto-pipeliner's cut selection presumes the matrix correctly identifies every combinational path. If the matrix is unsound (a true combinational path is marked as registered), the auto-pipeliner could insert a cut that violates a same-cycle assumption. This is why the matrix work has its own rigorous validation contract per `combinational-reachability-and-loop-detection.md` §10 — soundness flows downstream into auto-pipelining.

---

## 4 — Prior Art

A focused survey of the algorithms and tools the auto-pipeliner inherits from.

### 4.1 Retiming (graph-theoretic foundations)

Leiserson and Saxe's 1991 paper [5] established retiming as a polynomial-time graph problem. The synchronous circuit is modeled as a weighted directed graph G = (V, E, d, w) where vertices are combinational gates with delay `d(v)`, edges represent connections, and `w(e)` is the register count along edge `e`. The clock period is the maximum delay between consecutive registers. The retiming problem is to find a vertex labeling `r: V → Z` such that the new register count `w'(u, v) = w(u, v) + r(v) - r(u)` is non-negative on every edge and the new clock period is minimized.

The FEAS algorithm runs in O(|V|·|E|) per period candidate; binary searching the candidate periods gives O(|V|·|E|·log|V|·log·D_max) overall, where D_max is the maximum total delay [5]. Modern variants (Pan and Liu 1996 [7], Cong and Wu 1998 [4], Singh and Brown 2002 [8]) refine this for FPGA mapping and incremental synthesis.

The crucial limitation: retiming alone cannot improve a clock period below the maximum delay of any single combinational *cycle* (a feedback loop). For purely feed-forward designs (most pipelined data-paths), there is no cycle and retiming combined with new-register insertion can reach arbitrary speeds in principle.

### 4.2 Forward pipelining

Adding new registers (as opposed to moving existing ones) is mathematically a min-cost cut problem on the timing DAG. For a target period `T`, every path of total delay > T must be cut by at least one register. Since cuts can share, the minimum register count is the minimum number of "anti-chains" needed to cover all over-budget paths.

This is solvable by a linear-programming relaxation in many practical cases, or by a Bellman-Ford-style longest-path/feasibility algorithm derived from Leiserson-Saxe's FEAS, modified to allow synthesis of new registers [4]. The output is a placement of registers on edges of G such that arrival times never exceed T between any pair of consecutive register boundaries.

For RHDL, the relevant nuance is that the existing longest-path estimator [1, §2.4] already computes arrival times. Extending it to suggest cut sets for a target T is a natural progression of the same data structure.

### 4.3 HLS pipelining

Vitis HLS [12], Bambu [9], LegUp [3], and academic systems like Calyx [6] are far more aggressive than retiming. They take software-like source (typically C/C++ or DSL), build a control-data flow graph, schedule operations across pipeline stages, allocate functional units, and generate the resulting state machine and data path. Loop pipelining with II analysis [3] is the standard differentiator.

Calyx [6] is particularly relevant as a design influence — it is an MLIR-style intermediate language with explicit timing, designed at Cornell (the same group as the LATTE workshop where RHDL was presented). Calyx separates control from datapath and exposes scheduling to the user, complementing rather than replacing language-level expression.

For RHDL, full HLS is out of scope. The kernel function model intentionally does not provide arbitrary control flow; it provides combinational logic plus structural register declarations. Auto-pipelining inherits scheduling discipline from the user's kernel structure, which is already cycle-by-cycle.

### 4.4 Existing HDLs with pipelining as a first-class feature

**Spade** [10] introduces a `pipeline` keyword that lets users mark pipeline stages explicitly. The compiler then handles register insertion. Spade is the closest spiritual predecessor for what we want to give RHDL users at the API level, though Spade's user-directed style is more like our Phase 2 (annotated stages) than the fully-automatic Phase 1.

**Bluespec System Verilog** [2] uses guarded atomic actions and a scheduler to infer transition timing automatically, but at the cost of a very different programming model from imperative HDLs. RHDL inherits little from Bluespec syntactically but the underlying philosophy — that the language describes *what* and the compiler picks *when* — is the same.

**Chisel/Diplomacy** uses an annotation system (`Pipeline()`) and works closely with FIRRTL transforms; the transform is essentially retiming.

**Yosys** [13] has an `--retime` pass implementing Leiserson-Saxe. It is the canonical open-source retiming reference.

**CIRCT** [11] has a `--retime` MLIR pass that is the modern reimplementation of Yosys-style retiming on the MLIR/CIRCT stack.

---

## 5 — Phased Implementation Plan

### Phase 1 — Combinational-kernel forward pipelining (target: 3–6 months)

Restrict to *pure* kernels: `#[kernel] fn f(input: I) -> O`, no `D`/`Q`, no internal state. The kernel is mathematically a function `I → O`, and the compiler is free to insert any latency it needs.

**Reachability-matrix consumption:** Phase 1 reads the per-widget `i_to_o` sub-matrix to identify candidate cut points. Pure-combinational kernels have empty `i_to_d`, `q_to_o`, and `q_to_d` matrices (no sub-widgets), so only `i_to_o` matters here. The cut algorithm augments the matrix with NTL-level delay annotations and runs min-cut against the target period.

Scope:
- New `#[kernel(pipeline(target_freq_mhz = N))]` (or `target_period_ns`, or `stages = K`) attribute parsed by `rhdl-macro-core/src/kernel.rs`.
- New NTL pass `auto_pipeline` that runs after Stage-3 optimization. Reads the timing model and the `i_to_o` matrix, identifies cuts, inserts pipeline-register `Wire`s, and recomputes the matrix on the transformed widget per `combinational-reachability-and-loop-detection.md` §9.
- New circuit-level wrapper (`PipelinedFunc<K, L>` or similar) that exposes the pipelined kernel as a `Synchronous` circuit with explicit latency `L`.
- Functional-equivalence testbench in `rhdl-fpga` that runs the original `K` and the pipelined wrapper side-by-side.

Deliverables:
- A pipelined ALU example demonstrating > 2× clock-period improvement over un-pipelined.
- The Tier-2 / Tier-3 / Tier-4 tests from `CLAUDE.md` extended to assert (a) functional equivalence at offset `L` and (b) post-pipelining estimated period ≤ target.
- A new chapter in `doc/book/src/` documenting the feature.

### Phase 2 — Stateful synchronous-kernel pipelining (target: +6–12 months)

Allow kernels with state: `(o, d) = kernel(cr, i, q)`. Now the kernel has feedback paths (the `D`/`Q` register loop), which introduces hazards.

**Reachability-matrix consumption:** Phase 2 reads all four sub-matrices. `i_to_o` and `q_to_o` identify the live output paths the pipeliner must preserve at the cycle boundary; `i_to_d` and `q_to_d` identify the feedback edges the hazard analysis classifies. The `q_to_d` matrix in particular surfaces the precise edges where Carloni `RCStreamRelay` insertion is sound — making the relay-station mechanism from `stream-bus-architecture.md` Phase 2 the canonical pipelining primitive at every LID-compliant boundary.

Sub-features:
- Hazard analysis on the NTL graph guided by the matrix: classify each feedback edge identified in `q_to_d` as RAW-hazardous (write-then-read with insufficient latency), WAW-hazardous, etc.
- Two strategies offered to the user:
  - **Bypass logic:** insert forwarding paths that resolve hazards without stalling. Lower latency, more area.
  - **Stalling handshake:** emit a `ready/valid` interface and stall the pipeline on hazard. More area-efficient for rare-hazard cases.
- For LID-compliant `RCStream` boundaries, prefer Carloni relay-station insertion (which is sound by the LID theorem) over hand-rolled bypass logic.
- Accumulator and counter-style kernels are the canonical hard case; the recurrence has zero slack.
- For recurrences that cannot be pipelined (true scalar feedback dependency), the compiler reports an error pointing at the offending edge in the RHIF source — using the matrix's source-span metadata to identify the kernel statement.

### Phase 3 — Loop pipelining and II analysis (target: +12 months)

**Reachability-matrix consumption:** Phase 3 builds directly on the composition-level cycle detector from `combinational-reachability-and-loop-detection.md` §5. Recurrence cycles in loop iterations are exactly the structure the cycle detector finds; the iteration-interval bound is determined by the longest cycle's delay. Phase 3 is therefore unblocked by the cycle-detection work landing — without it, Phase 3 has to invent its own cycle analysis.

- Detect `for i in 0..N { ... }` patterns where the loop body is independent across iterations (or has tractable cross-iteration dependency).
- Schedule iterations into a pipeline with II = 1 where possible.
- For loops with carried dependency, use the recurrence-bounded II as identified by the cycle detector.
- This brings RHDL into HLS-equivalent territory for a specific, well-defined subset of kernels.

---

## 6 — Algorithm Sketch (Phase 1)

Given an NTL `Object` representing a pure combinational kernel with delay model `d: Wire → R+` and a target period `T`, produce an NTL `Object'` such that all combinational paths between consecutive registers have delay ≤ T.

### Step 1 — Build the timing DAG

Construct a directed acyclic graph from NTL where vertices are operations producing `Wire`s and edges are dependencies. Annotate each vertex `v` with delay `d(v)` from the existing estimator.

The skeleton of this DAG is precisely the per-widget reachability matrix from `combinational-reachability-and-loop-detection.md`: every entry in `i_to_o` corresponds to a path that must be representable in the timing DAG. The auto-pipeliner reads the matrix to bound the search space (only paths the matrix marks combinational need delay analysis); the timing DAG augments the matrix with delay annotations on each vertex.

### Step 2 — Compute arrival times

Topological sort and forward-propagate the arrival time:

```
A(v) = max over u in predecessors(v) of (A(u) + d(v))
```

with `A(v) = d(v)` for primary inputs.

### Step 3 — Identify over-budget paths

Edges (u, v) where `A(v) > T` must be cut.

### Step 4 — Compute minimum cut

For each maximal path P with `A(end-of-P) > T`, place a register at the latest edge in P that resets the running arrival time to ≤ T. This greedy approach is provably within a constant factor of optimal for the path-cover formulation. For optimal, formulate as min-flow with capacity-1 edges and solve via standard LP.

For RHDL Phase 1 we use the greedy variant; the LP variant is a Phase-2 refinement.

### Step 5 — Insert registers and update the IR

For each chosen cut edge, allocate a new `Wire` representing the pipeline register, redirect the consumer's input to the register output, and add an `Assign` node to drive the register from the original producer. Track per-stage latency so the wrapper can declare total `L`.

### Step 6 — Verify

Re-run the arrival-time computation. All arrival times between consecutive registers must be ≤ T. If any path still exceeds T (which happens when individual NTL nodes have delay > T — a fundamentally infeasible target), emit a compiler error pointing at the offending NTL node and back to the originating Rust source via `rhdl-span`.

### Step 7 — Optional retime pass

Run a Leiserson-Saxe retiming pass on the now-pipelined graph to redistribute registers for further period improvement. This can be skipped in Phase 1 if implementation time is tight.

### Complexity

- Steps 1–3: O(|V| + |E|).
- Step 4 greedy: O(|E|).
- Step 4 LP-optimal: O(|V|³) worst case, polynomial in practice.
- Step 5: O(|cuts|).

For typical RHDL kernels (hundreds to thousands of NTL nodes), the whole pass runs in milliseconds.

---

## 7 — User-Visible API

Three proposed annotation forms, picking one or supporting all:

```rust
// Target a specific frequency. Compiler picks stage count.
#[kernel(pipeline(target_freq_mhz = 250))]
fn complex_alu(...) -> ... { ... }

// Target a specific stage count. Compiler picks register placement.
#[kernel(pipeline(stages = 4))]
fn complex_alu(...) -> ... { ... }

// Target a period with a stage cap. Compiler reports error if infeasible.
#[kernel(pipeline(target_period_ns = 4.0, max_stages = 8))]
fn complex_alu(...) -> ... { ... }
```

The compiler emits a synthesizable wrapper:

```rust
pub struct ComplexAluPipelined;

impl SynchronousIO for ComplexAluPipelined {
    type I = /* input type of complex_alu */;
    type O = /* output type of complex_alu */;
    type Kernel = /* compiler-generated pipelined kernel */;
}

// Generated constant expressing the latency:
impl ComplexAluPipelined {
    pub const LATENCY: usize = /* L */;
}
```

The user instantiates `ComplexAluPipelined::default()` in a parent circuit just like any other widget. The wrapper's input is registered (per Phase-1 contract) and output appears `LATENCY` cycles later.

For variable-rate or stallable usage, an opt-in attribute exposes a `ready/valid` handshake at the wrapper boundary:

```rust
#[kernel(pipeline(target_freq_mhz = 250, handshake = "ready_valid"))]
fn complex_alu(...) -> ... { ... }
```

This becomes important for Phase 2 (stateful kernels with hazards) and is forward-compatible.

---

## 8 — Validation

Per `CLAUDE.md`'s four-tier validation contract, every pipelined widget must pass:

**Tier 1 — Functional equivalence.** Direct kernel call: pipelined kernel's output at cycle `t + L` equals original kernel's output at cycle `t`. Property test over `n ≥ 1024` random inputs of each kind. Required for every pipelined kernel.

**Tier 2 — Iterator simulation.** Run a representative input stream through both the original and pipelined wrappers, diff outputs at the latency offset. Catches sequencing and reset-propagation bugs in the wrapper itself.

**Tier 3 — HDL emission snapshot.** `expect_test` pin of the emitted Verilog. Catches regressions in the auto-pipeliner pass.

**Tier 4 — `iverilog` round-trip.** Both `.rtl()` and `.ntl()` paths. Same as for any other widget.

**Tier 5 — Timing assertion.** A new test type for pipelined kernels: compute the post-pipelining longest-path estimate and assert it ≤ user-specified target. Without this assertion the auto-pipeliner is silently broken.

A *meta-test* lives in `rhdl-fpga`: a randomly-generated combinational kernel of varying complexity is pipelined at varying targets, and the (Tier 1, Tier 5) assertions are checked on each. This serves as the integration test for the auto-pipelining pass itself, separate from any individual widget.

For external timing calibration: optionally feed the emitted Verilog through Yosys and compare Yosys's reported critical path with our internal estimate. Persistent divergence indicates our delay model needs adjustment.

---

## 9 — Risks and Open Questions

**Delay-model fidelity.** The longest-path heuristic [1, §2.4] is not a substitute for vendor STA. Initial calibration against published cell delays and a test corpus on Lattice iCE40 / Xilinx 7-series would be required before claiming "this widget meets 250 MHz on hardware X."

**ADTs and discriminant-dependent paths.** A `match` on an enum produces multiple paths whose lengths depend on the variant. The pipeliner must compute the *worst-case* path across variants but should not register-insert on unused variant branches. The NTL `Case` op already encodes this; the algorithm extension is straightforward but needs care.

**Memory accesses.** RAM and BRAM reads have multi-cycle delay that is hardware-dependent. Treat BRAMs as black boxes with declared latency in the timing model. The existing `core::ram` widgets have this information in their descriptors; auto-pipelining must respect it.

**Multi-cycle multipliers.** DSP-block multipliers on real FPGAs are pipelined inside the silicon. We need a vendor-supplied delay/latency model. For Phase 1, treat as black boxes with conservative latency.

**Reset semantics of inserted registers.** Pipeline registers must reset to a known state (otherwise the first `L` cycles of output are X). The natural choice is to reset every pipeline-stage register to the all-zeros value in its bit width. But for user-defined `T: Digital` types, "zero" may not be a sensible default; we use `T::dont_care()` and document the post-reset latency requirement explicitly.

**Loops with carried recurrences.** Phase 3 (II-aware loop pipelining) hits a classical limit: the minimum II is bounded by the recurrence. For algorithms like running sum or feedback filters, this can preclude pipelining altogether. The compiler must detect this case and report it as an error rather than silently producing wrong-throughput hardware.

**Backpressure interaction.** Many RHDL widgets use ready/valid handshake. Inserting registers on the data path without inserting matching registers on the ready/valid path would introduce skid-buffer hazards. The Phase 2 design must address this; a useful starting point is the existing `stream::stream_buffer` widget which is exactly a properly-pipelined ready/valid stage.

**LLM-driven refactoring fallback.** When pure pipelining can't meet a target (e.g. a deep enum match), the timing-budget violation is best addressed by the LLM agent suggesting an algorithmic restructuring of the kernel — splitting a wide combinational block into a small state machine, for example. This is a separate workflow but worth flagging here as the natural escalation path.

**Sequencing risk: matrix work must land first.** Per §3.1, all three auto-pipelining phases consume the per-widget reachability matrix from `combinational-reachability-and-loop-detection.md`. If auto-pipelining starts before that work lands, the auto-pipeliner has to re-derive matrix-equivalent information ad-hoc within each phase — leading to either (a) two analyses that drift apart (correctness risk), or (b) a costly refactor when the matrix lands (engineering cost). Mitigation: make matrix-Phase-1 a hard prerequisite for auto-pipelining-Phase-1; matrix-Phase-3 (composition-level cycle detection) a hard prerequisite for auto-pipelining-Phase-3 (loop pipelining). Tracked in CLAUDE.md §1's strategic-design-documents subsection.

---

## 10 — References

[1] Basu, Samit. "RHDL: Rust as a Hardware Description Language." LATTE '25, Rotterdam, March 2025. (In-tree at `doc/latte25/latte.tex`.)

[2] Bluespec, Inc., and Arvind. *Bluespec System Verilog Reference Guide.* See also Nikhil, R.S., "Bluespec System Verilog: Efficient, Correct RTL from High-Level Specifications," MEMOCODE 2004.

[3] Canis, A., Choi, J., Aldham, M., Zhang, V., Kammoona, A., Anderson, J.H., Brown, S., Czajkowski, T. "LegUp: High-Level Synthesis for FPGA-Based Processor/Accelerator Systems." FPGA 2011.

[4] Cong, J., Wu, C. "Optimal FPGA Mapping and Retiming with Efficient Initial State Computation." DAC 1998. — Foundational FPGA-specific extension of retiming.

[5] Leiserson, C.E., Saxe, J.B. "Retiming Synchronous Circuitry." *Algorithmica* 6(1), 5–35, 1991. — The seminal retiming paper. The FEAS algorithm and its complexity bounds originate here.

[6] Nigam, R., Atapattu, S., Thomas, S., Li, Z., Bauer, T., Ye, Y., Koti, A., Sampson, A., Zhang, Z. "A Compiler Infrastructure for Accelerator Generators." ASPLOS 2021. — Calyx, the Cornell IL with explicit timing semantics.

[7] Pan, P., Liu, C.L. "Optimal Clock Period FPGA Technology Mapping for Sequential Circuits." DAC 1996. — Important precursor to integrated retiming-during-mapping.

[8] Singh, D.P., Brown, S.D. "Integrating Retiming into FPGA Synthesis." *IEEE Transactions on VLSI Systems*, 2002. — FPGA-aware retiming with state preservation.

[9] Pilato, C., Ferrandi, F. "Bambu: A Modular Framework for the High Level Synthesis of Memory-Intensive Applications." FPL 2013.

[10] Skarman, F., Gustafsson, O. "Spade: An Expression-Based HDL With Pipelines." Open Source Design Automation Conference (OSDA), 2023. — Cited in the LATTE '25 paper [1] as an HDL with explicit pipeline syntax.

[11] CIRCT Project. "Circuit IR Compilers and Tools." LLVM Foundation. https://circt.llvm.org/ . The `--retime` pass under the `arc`/`hw` dialects implements MLIR-style retiming.

[12] Xilinx, Inc. *Vitis HLS User Guide* (UG1399). The reference manual for the most widely deployed commercial HLS pipelining tool. Pragmas of interest: `#pragma HLS PIPELINE`, `#pragma HLS UNROLL`.

[13] Wolf, Clifford. *Yosys Open Synthesis Suite.* The `retime` pass at `passes/sat/retime.cc` implements Leiserson-Saxe retiming. https://yosyshq.net/yosys/

[14] Carloni, L.P., McMillan, K.L., Sangiovanni-Vincentelli, A.L. "Theory of Latency-Insensitive Design." *IEEE Transactions on Computer-Aided Design of Integrated Circuits and Systems*, 20(9), 2001. — The Carloni LID theorem proves that relay-station insertion on LID-compliant edges preserves correctness. The basis for `RCStreamRelay` in `stream-bus-architecture.md` Phase 2 and the soundness argument for inter-widget pipelining in this plan's §3.1.

[15] `combinational-reachability-and-loop-detection.md` — in-tree design plan. The per-widget reachability matrix this plan consumes; the composition-level cycle detector this plan's Phase 3 builds on.

---

## 11 — Next Concrete Steps

In order:

1. **Land the reachability-matrix work first** (per `combinational-reachability-and-loop-detection.md` Phase 1, task #74 in the queue). Without the per-widget matrix exposed on `Descriptor`, every step below has to re-derive the same data ad-hoc. This is the hard prerequisite called out in §3.1 and §9 ("Sequencing risk").
2. **Survey the existing timing estimator's interface** in `rhdl-core/src/ntl/` and `rhdl-core/src/compiler/stage3.rs`. Determine whether arrival-time computation is reusable or needs refactoring into a separate analysis module.
3. **Prototype the greedy cut algorithm** on a hand-built NTL graph for a simple combinational kernel (e.g. an 8-bit adder tree). Use the matrix's `i_to_o` to bound the search space.
4. **Add the `pipeline(...)` attribute parsing** to `rhdl-macro-core/src/kernel.rs` as a no-op pass-through first, to land the syntactic surface independently of the algorithm.
5. **Build the pipelined-wrapper code generation** alongside the existing kernel-to-circuit lowering. Verify functional equivalence on a non-pipelined test (latency = 0) before adding any registers.
6. **Wire in the auto-pipeliner pass** as a Stage-3 NTL pass that runs only when a kernel carries the `pipeline` attribute. Ensure the pass recomputes the widget's reachability matrix on its output (per §3.1's "matrix-mutating pass" decision).
7. **Land the meta-test** that randomly generates kernels and asserts (functional equivalence, timing budget) under random target frequencies.
7. **Calibrate the delay model** against Yosys-reported critical paths on a corpus of representative widgets.

Each step is independently shippable and independently testable per `CLAUDE.md`'s contract. Phase 1 is complete when (1)–(7) are in main and a non-trivial example demonstrates a measurable Fmax improvement.
