# FSM Architecture for RHDL — Design Plan

A proposal for first-class finite-state-machine support in RHDL: an ergonomic syntax for declaring FSMs, static analyses that catch a meaningful class of bugs at compile time, auto-generated state-diagram visualizations in rustdoc, and a formal-verification path that can prove temporal properties about the machine.

This is the fourth compiler-and-language design plan, alongside `auto-pipelining-plan.md`, `kernel-language-extensions.md`, and `vendor-primitive-architecture.md`. Like those, it is independently shippable in phases; some phases compose, but none blocks the others.

The plan rests on a simple observation: a large fraction of every RHDL widget is an FSM in disguise (the FIFO write-logic, every protocol PHY, every arbiter, every register-mapped peripheral), and the language already has *most* of the right primitives — ADTs with discriminants, exhaustive `match`, kernels as pure functions of `(state, input)`. What's missing is (a) the surface syntax to make the pattern read like an FSM rather than a generic Rust function, (b) the static analyses that exploit the structure, and (c) the formal-verification surface that the kernel-as-pure-fn invariant uniquely makes tractable.

---

## 1 — Motivation

Hardware design has been writing FSMs since 1956 (Mealy) and 1956 (Moore — same year, separately). Every modern HDL has some recognition of the FSM pattern:

- **SystemVerilog** has `unique`/`priority` case keywords plus SVA temporal assertions.
- **Bluespec** schedules guarded atomic actions; the scheduler proves correctness.
- **SpinalHDL** ships a `StateMachine` library with declarative syntax and a graph-export tool.
- **Spade** has a `state` keyword for staged state machines.
- **Chisel** uses match-based FSMs but offloads verification to ChiselTest / SymbiYosys.
- **nMigen / Amaranth** has `m.FSM()` blocks plus `Past`, `Stable`, `Cover` for SVA-style assertions and built-in SymbiYosys integration.

RHDL's current FSM idiom is the bare match-on-state pattern (see `crates/rhdl-fpga/src/fifo/write_logic.rs` for a production example). It is correct, exhaustive, and works — but it is also verbose, scatters the per-state outputs across separate match arms, and provides no leverage for the analyses other HDLs offer for free.

The cost of *not* providing FSM-specific tooling compounds. Widget #18 (UART), #19/20 (SPI), #21 (I²C), #25 (CAN), #28 (LIN), #31 (SENT), #37 (MIDI), #46 (battery management) — every one is an FSM. An ergonomic boost there is a force multiplier across half the roadmap. A static analysis catching unreachable states or deadlocked transitions catches bugs before any cycle is simulated. A formal-verification flow that proves "the FIFO write logic never overflows" is the kind of guarantee that moves RHDL into territory only Bluespec and academic Coq-verified HDLs reach today.

The kernel-as-pure-fn invariant — the same invariant that makes auto-pipelining sound — is exactly what formal verification of FSMs requires. Bounded model checking, symbolic execution, and SVA-style temporal proofs all require a referentially transparent state-transition function. RHDL has that by construction. Whatever tool chain we build, we will not be fighting against the language; we will be exploiting structure that's already there.

---

## 2 — Where FSMs live in RHDL today

The canonical pattern, drawn from the existing code:

```rust
#[derive(PartialEq, Digital, Copy, Clone, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Running { counter: b8 },
    Done,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
pub struct MyMachine {
    state: DFF<State>,
}

#[kernel]
pub fn my_machine(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let next = match q.state {
        State::Idle => if i.start {
            State::Running { counter: bits(0) }
        } else {
            State::Idle
        },
        State::Running { counter } => if counter == bits(255) {
            State::Done
        } else {
            State::Running { counter: counter + 1 }
        },
        State::Done => State::Idle,
    };
    let mut o = Out::dont_care();
    o.busy = matches!(q.state, State::Running { .. });
    o.done = matches!(q.state, State::Done);
    if cr.reset.any() {
        return (Out::default(), D { state: State::Idle });
    }
    (o, D { state: next })
}
```

Three things make this nice already: enum-with-payload state types are first-class, exhaustive `match` is enforced by `rustc`, and the kernel is a pure function of `(state, input)`. Three things make it less nice: the next-state logic and the per-state outputs live in separate places (a maintenance hazard), guards aren't supported yet (tracked in `kernel-language-extensions.md` §2.4), and the framework doesn't *know* this is an FSM — it's just a kernel that happens to wrap a `DFF<State>`.

---

## 3 — Design space — five layers

The work splits into five layers, each with different cost and different payoff. They compose: layer N is more useful when layers 1..N-1 are present, but layer N can ship without later layers.

| Layer | What it does | Cost | Payoff |
|---|---|---|---|
| 1 | Ergonomic `#[derive(Fsm)]` macro | low (~2 weeks) | high — better readability, less boilerplate |
| 2 | Static reachability + dead-state analysis | medium (~4 weeks) | high — catches bugs at compile time |
| 3 | Auto-generated state diagrams in rustdoc | low (~2 weeks) | medium-high — visualization, LLM-friendly |
| 4 | Invariant assertions + SymbiYosys integration | medium-high (~6 weeks) | very high — formal proofs of safety properties |
| 5 | Built-in bounded model checker | high (~6 months) | very high — self-contained verification |

The recommended phasing is 1, 2, 3, 4, 5 in order, with 1+2+3 shipping as a single coherent "FSM ergonomics + static analysis" track and 4 shipping as a separate "formal verification" track. Layer 5 is a research-grade follow-on.

---

## 4 — Layer 1: the `#[derive(Fsm)]` macro

### 4.1 Syntax

The principle: stay in idiomatic Rust syntax; do not invent new keywords; do not require rust-analyzer to understand a DSL. Hardware authors are already learning RHDL's kernel subset; learning a separate DSL on top would be a net-negative ergonomic. Prior art that gets this right: Rust's own `serde::{Serialize, Deserialize}` derives.

```rust
#[derive(Fsm, PartialEq, Digital, Copy, Clone, Debug, Default)]
#[fsm(initial = "Idle")]
pub enum State {
    #[default]
    Idle,
    Running { counter: b8 },
    Done,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state")]
pub struct MyMachine {
    state: DFF<State>,
}

#[kernel]
#[fsm_kernel(state_var = "q.state")]
pub fn my_machine(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    /* same body as before — just plain RHDL */
}
```

Three light-weight attributes carry all the metadata:

- `#[derive(Fsm)]` on the state enum marks the type as an FSM state, registering it for analysis. The macro emits a small impl of an `FsmState` trait that records the variant names, default variant, and discriminant layout. Cost: ~50 LOC of macro output per FSM.
- `#[fsm(state_field = "...")]` on the widget struct names the field that holds the state DFF. This is the only field-level annotation needed; everything else is inferred.
- `#[fsm_kernel(state_var = "...")]` on the kernel function names the local expression the kernel matches against. The default — `q.<state_field>` — covers the canonical case and the attribute is usually omittable.

The kernel body itself is **unchanged**. No new syntax inside the kernel. The macro layer just records where the FSM lives; the analysis layer reads the kernel's existing match-on-state structure.

This conservative choice is deliberate. It means:

- Existing widgets can opt into FSM tooling by adding three attribute lines and zero code changes.
- LLM-generated kernels work as-is — the macro is a *declaration* that says "this widget is an FSM," not a transformation.
- The `Synchronous` trait, the `D`/`Q` derive machinery, the existing reset-handling idiom, and every other widget convention compose with FSM tooling without modification.
- If a future agent wants to add a more declarative DSL on top (à la SpinalHDL `StateMachine`), it can lower to this same attribute-annotated form — the underlying contract stays.

### 4.2 What `#[derive(Fsm)]` actually emits

The derive expands to an implementation of:

```rust
pub trait FsmState: Digital {
    const VARIANTS: &'static [FsmVariantDescriptor];
    const INITIAL: usize;  // index into VARIANTS
    fn variant_name(&self) -> &'static str;
    fn variant_index(&self) -> usize;
}

pub struct FsmVariantDescriptor {
    pub name: &'static str,
    pub discriminant: TypedBits,
    pub field_kinds: &'static [Kind],  // for variants with payloads
}
```

This metadata is what the static analyses (Layer 2) and the diagram generator (Layer 3) consume. It costs ~100 LOC per FSM at compile time and produces zero runtime cost — the trait is purely a static reflection surface.

### 4.3 Optional: `#[fsm_state(...)]` per-variant decoration

For richer diagram output, individual variants can carry display hints:

```rust
#[derive(Fsm, ...)]
pub enum State {
    #[fsm_state(label = "idle, waiting for start")]
    #[default]
    Idle,
    #[fsm_state(label = "running, counter = {counter}")]
    Running { counter: b8 },
    #[fsm_state(label = "complete", terminal)]
    Done,
}
```

These are pure display metadata, consumed only by the diagram generator. The `terminal` flag is a hint to the static analysis that a state with no outgoing transitions is *intentional* rather than a deadlock bug.

### 4.4 Acceptance criteria for Layer 1

Per `CLAUDE.md` §11.1:

1. `#[derive(Fsm)]` compiles and produces a valid `FsmState` trait impl for any `Digital`-derived enum.
2. The metadata is correct — variant names, discriminants, payload kinds match the source enum.
3. Three existing FSM-shaped widgets (`fifo::write_logic`, `core::round_robin_arbiter`, `core::crc`) are rewritten to opt into the FSM derives, with byte-identical emitted Verilog (the macro is purely additive metadata).
4. New chapter `doc/book/src/fsm/derive.md` documenting the syntax with one worked example.
5. Tests in `crates/rhdl-macro-core/src/expect/` snapshotting the `#[derive(Fsm)]` expansion.

---

## 5 — Layer 2: static reachability and dead-state analysis

### 5.1 What the extractor produces (formal definition)

Given an RHDL kernel `K` with declared FSM state field `<state_field>` of enum type `E`, the FSM transition graph `G(K) ⊆ Variants(E) × Variants(E)` is the relation

```text
(s, t) ∈ G(K)  ⟺  ∃ input I such that
                      evaluating K under (q.<state_field> = s, other inputs = I, cr.reset = false)
                      produces  d.<state_field> = t
```

This definition is about the kernel's I/O behaviour (a pure function of `(cr, i, q) → (o, d)` per the kernel-as-pure-fn invariant in `architecture.md` §1), not about its syntactic structure.  Whether the kernel uses `match q.<state>`, multiple matches, nested `if`s, a `dont_care()` + field-set construction, or any other RHDL-legal shape, the transition graph is determined by the *function* the kernel computes — not the AST that defines it.

Three load-bearing properties:

1. **It is about the kernel's I/O, not its syntax.**  Production widgets have between 1 and 5 `match q.<state>` expressions per kernel; only one (or sometimes zero) is the FSM-transition function, with the others doing output computation, phase classification, or similar non-transition logic.  The extractor identifies the relevant data flow by walking back from `d.<state_field>`, not by guessing at syntactic position.
2. **It is about reachability in one cycle, not over time.**  `(s, t)` is in the graph iff the kernel can move from `s` to `t` in a single evaluation.  Multi-cycle reachability is derived by composition — that lives in the leaf analyses (unreachable states, deadlock candidates), not in the extractor.
3. **It excludes the reset path.**  The `cr.reset = false` constraint reflects the convention that reset is treated as out-of-band rather than as a per-state edge.  The canonical RHDL reset pattern `if cr.reset.any() { d.<state_field> = INIT; ... }` lowers to a `Select` whose condition reads `cr.reset`; the extractor recognises this shape and skips the reset-override branch.

The leaf analyses (unreachable states, dead transitions, deadlock candidates, non-deterministic transitions, self-loop saturation) consume the extracted graph; their definitions are unchanged from earlier revisions of this section.

### 5.2 Implementation

The extractor lives in `crates/rhdl-core/src/fsm/extraction.rs` and implements the algorithm in §5.3 below.  It is a pure function from `(ops, return_slot, descriptor, literal_lookup)` to `ExtractionResult { transitions, unanalyzable }` — no IR mutation, no `Pass` registry entry.  The leaf analyses (`analyze_fsm_structure`) consume its output and emit miette diagnostics.

The pass is *advisory only* — it does not transform the IR.  Diagnostics surface as warnings unless the user opts into errors via `#[fsm(strict)]` on the widget struct.

### 5.3 Algorithm (principled, §5.1-derived)

```
input:  kernel K (RHIF), FSM descriptor (state_field, variants, initial)
output: TransitionGraph G, list of Unanalyzable diagnostics

1. Locate the state slot.
   Let R be K's return value (a Tuple slot).  Let D be the slot for
   the d-component of R.  The state slot is the slot S such that
   walking D backward through Splices and Structs, S is the most
   recent value spliced into path [<state_field>] of D.  In SSA this
   is unambiguous.

   If D is not a Splice/Struct chain (e.g., it is a function argument
   or comes from an opcode the locator doesn't recognise), surface a
   single kernel-level Unanalyzable diagnostic and stop.

2. For each source variant s in the descriptor:

   2a. Compute the set T(s) of values d.<state_field> can take under
       constraint q.<state_field> = s.  Backward data-flow walk from
       S with constraint propagation:

       - Literal of state type → singleton set {variant index}
       - OpCode::Enum producing a discriminant value → singleton set
         (or per-arm Unanalyzable if discriminant matches no variant)
       - OpCode::Index reading q.<state_field> under constraint
         q.<state_field>=s → {s}  (a self-loop produced by the
         canonical kernel-top default `d.<state_field> = q.<state_field>`)
       - OpCode::Assign → recurse on rhs
       - OpCode::Splice with path = [<state_field>] → recurse on subst
         (an explicit override)
       - OpCode::Splice with non-state path → recurse on orig (state held)
       - OpCode::Struct with explicit <state_field> → recurse on its value
       - OpCode::Struct without explicit <state_field> → empty
         (field comes from template, typically dont_care)
       - OpCode::Select with reset condition → walk false-branch only
         (skip reset-override; see §5.1 property 3)
       - OpCode::Select with non-reset condition → union of both branches;
         empty branch contributes a self-loop on s
       - OpCode::Case with discriminant reading q.<state_field> → only
         the arm whose CaseArgument matches s's discriminant (or the
         Wild arm) contributes; other arms are constraint-eliminated.
         This distinguishes the FSM-transition Case from
         output-computation Cases on q.<state>.
       - OpCode::Case with non-state discriminant → union of all arms;
         empty arm contributes a self-loop on s
       - Any other opcode: empty (no info from this slot)

   2b. For each t in T(s), add (s, t) to G.

   2c. If T(s) is empty (only happens when the chain doesn't mention
       q.<state_field> AND no explicit override along any path), add
       (s, s) — pure implicit self-loop from the kernel-top default.

3. Return G and the list of Unanalyzable diagnostics.
```

The two structural innovations relative to the heuristic that shipped in PR #2 / PR #6:

1. **Start from `d.<state_field>` (the kernel's output), not from the first `match` (a syntactic guess).**  This is what makes the algorithm robust to multi-match kernels — every production protocol-PHY widget has 2–5 `match q.<state>` expressions, only one of which is the next-state function.
2. **Propagate the constraint `q.<state_field> = s` through Cases and Selects.**  This is what lets the algorithm distinguish the FSM-transition Case from a `match q.<state>` used for output computation: under the constraint, the output-computation Case's result reduces to a single value (which isn't a state-typed value anyway, so it's irrelevant), while the FSM-transition Case's result reduces to the s-th arm's result (which IS a state value).

### 5.4 Acceptance criteria for Layer 2

1. **Corpus equivalence (the gold standard).**  For every widget in the FSM corpus, the extractor produces output without `Unanalyzable` diagnostics, and the derived graph is pinned by an `expect_test` snapshot that the reviewer verifies against the kernel.  Corpus snapshots ship with the widget reorganization PR (the production corpus widgets do not yet exist on main); on main, the synthetic adversarial integration tests in `crates/rhdl-fpga/src/doc.rs` cover the same kernel-language idioms.
2. **No silent miscompiles.**  Where the extractor cannot derive a sound answer (kernel return shape unrecognised, malformed enum discriminant), it surfaces a precise `Unanalyzable` diagnostic naming the offending construct.  Silently producing a wrong graph is a contract violation.
3. **Required kernel patterns.**  The extractor MUST handle: multiple `match q.<state>` expressions per kernel; conditional code paths nested arbitrarily deep around the transition logic; let-binding form; side-effect form; the canonical kernel-top default + selective override pattern; arms with payload-bound variants; or-pattern arms; `Wild` arms; the canonical `if cr.reset.any() { d.<state_field> = INIT }` reset block.
4. **Soundness.**  Every transition the kernel can actually produce (excluding reset, per §5.1 property 3) must be in the graph (zero false negatives).
5. **Documented over-approximation budget.**  The extractor MAY conservatively over-approximate by including edges the kernel could in principle reach but won't under reasonable inputs due to *cross-DFF invariants* the extractor can't see (e.g., `can_master`'s outer `if q.state == CanState::Idle && i.start { d.field = Sof }` makes every `CanField` state appear to have an edge back to `Sof`, even though by construction `q.state == Idle` only co-occurs with `q.field == Sof`).  Such over-approximation is sound; it produces extra edges in the rendered diagram but never missed transitions.
6. **Diagnostics actionable for LLM-driven workflows.**  Every `Unanalyzable` diagnostic carries (a) the source variant name (or `<kernel>` for kernel-level diagnostics), (b) the unrecognised opcode or pattern, (c) a hint pointing the user at the supported alternatives.
7. **No widget snapshot regressions.**  `cargo test --all` on the workspace produces zero HDL snapshot diffs.  The extractor is purely advisory and additive; no IR opcode changes, no codegen changes.

### 5.4.1 Implicit self-loops: opt-in via `#[fsm(allow_implicit)]`

The canonical RHDL kernel pattern (kernel-top default `d.<state_field> = q.<state_field>` + selective override) makes a state's "no explicit transition" path produce an implicit self-loop that's structurally indistinguishable from "no transitions at all."  An earlier revision of this section flagged this as a NECESSARY follow-up because the Layer 2 `DeadlockCandidate` diagnostic couldn't fire for any state — every variant got at least an implicit self-loop, masking the deadlock.

**Resolved by structural opt-in.**  The `FsmWidgetTag` carries an `allow_implicit: bool` flag, parsed from `#[fsm(allow_implicit)]` on the widget struct.  The extractor honours it:

- **`allow_implicit = false` (default).**  The extractor only emits transitions for *explicit* writes to `d.<state_field>` inside an arm.  Arms that fall through to the kernel-top default — including `Index`-reads of `q.<state_field>` produced by that default — contribute nothing to the graph.  A state whose only would-be outgoing edge was an implicit self-loop appears with zero outgoing edges; the analysis layer fires `DeadlockCandidate` for it.
- **`allow_implicit = true`.**  The canonical "default + selective override" pattern is recognised; the extractor adds `(s, s)` for every arm that holds the state in place via the kernel-top default.  This is what the corpus widgets need; they opt in explicitly.

This pushes the choice to the widget author.  Authors who use the canonical pattern declare it explicitly; the attribute documents intent.  Authors who don't get strict deadlock checking by default — forgotten transitions are caught loudly.

**Composition with `strict`.**  A widget with `#[fsm(strict, allow_implicit)]` still gets the strict-mode error escalation, just on a graph that includes the implicit holds.  Without `allow_implicit`, deadlock-on-no-explicit-transitions becomes a hard error at synthesis time.

**Migration impact.**  Every existing FSM-tagged widget had to add the attribute (or accept that arms relying on the implicit hold would now appear as zero-outgoing-edge states and fire `DeadlockCandidate`).  On main this was just the synthetic adversarial widgets in `doc.rs`; on the refactor branch this is all 27 corpus widgets — each one's `#[fsm(...)]` line gets the additional flag.

**Test coverage.**  `fsm::extraction::tests` covers both modes:

- `principled_implicit_hold_masks_deadlock_state` (legacy name) — pins the `allow_implicit = true` behaviour: implicit self-loops emitted, deadlock-y state appears with self-loop edge, analysis layer sees no deadlock.
- `strict_mode_kernel_top_default_alone_yields_no_transitions` — pins the new default: with `allow_implicit = false`, kernel-top default alone produces zero transitions for any variant.
- `strict_mode_guarded_transition_emits_only_explicit_edge` — pins the can_master `Id`-arm shape under strict mode: only the explicit `Idle → Running` edge is emitted; the else-branch's implicit hold doesn't appear.
- `strict_mode_explicit_self_loop_via_literal_is_preserved` — documents the design's trade-off: writing `d.state = State::A` literally inside arm `A` IS a real explicit self-loop and stays in the graph.

### 5.4.2 Soundness rigor (status: items 1 + 2 shipped; item 3 deferred)

The current extractor's soundness against the principled definition (§5.1) rests on three load-bearing claims that started as *structurally plausible* but not *provably sound*.  This PR resolves the first two:

1. **`Select` constraint propagation for `q.<state_field> == X` comparisons (✅ shipped this PR).**  When a `Select`'s condition is a `Binary(Eq)` whose operands trace to `q.<state_field>` and a state-typed literal, the walker statically resolves the condition under the source-variant constraint and walks only the matching branch.  Implemented in `resolve_state_eq_condition`; pinned by `principled_select_constraint_propagation_on_state_eq`, `principled_select_constraint_propagation_handles_swapped_operands`, and `principled_select_constraint_propagation_falls_back_on_opaque_cond` in `fsm::extraction::tests`.  An FSM with `if q.<state_field> == StateX { ... }` inside transition logic now produces the tight constraint-propagated graph instead of the union over-approximation.
2. **Property-based testing against the RHDL simulator (✅ shipped this PR).**  Two property-based tests in `rhdl_fpga::doc::tests` enumerate every `(source variant, input)` combination for representative adversarial widgets, call the kernel function directly via `DigitalFn3::func()`, observe `d.<state_field>` after the call, and assert that every simulator-observed transition is in the extractor's output (soundness — no false negatives against the executable semantics).  This converts the algorithm's correctness from "structurally plausible against the documented kernel patterns" to "empirically validated against RHDL's simulator on synthetic widgets exercising the algorithm's main features."  Tests: `adv_sideeffect_conditional::property_simulator_observed_is_subset_of_extractor_output` (canonical 3-state cycle with implicit holds + guarded transitions) and `adv_can_master_guarded_else_writes_other_field::property_simulator_observed_is_subset_of_extractor_output` (the can_master shape with else-branch writing a different field).
3. **Constraint propagation through `Case` is intuitively right but not formally proven** (still deferred).  The algorithm asserts: "if a `Case`'s discriminant traces back to `Index(q, [<state_field>])`, only the arm whose `CaseArgument` matches the source variant's discriminant contributes to the result."  This presupposes the RHIF lowering preserves the dispatch semantics through the discriminant-extraction chain.  No formal proof; the property-based tests above empirically validate this on the test widgets, but a future RHIF pass that reuses or aliases the discriminant slot in unexpected ways could silently break it.

The remaining acceptance gap is **structural**: the algorithm is validated against the executable semantics for the canonical kernel patterns and against the RHIF data-flow shape it expects, but soundness for *arbitrary* kernels (and stability against future RHIF lowering changes) requires the formal-semantics work below.

#### Reset detection is a structural pattern match (status: shipped, structural by construction)

The algorithm recognises `Select(Unary(OrReduce, Index(_, [.reset])), ...)` as the canonical reset block.  A kernel that writes reset detection in a non-canonical form (e.g., `let r = cr.reset.any(); if r { ... }` with intermediate let-bindings, or a different boolean-reduction op, or `cr.reset.0` instead of `.any()`) would either be missed (producing extra edges) or false-positive (skipping a non-reset condition that happens to read `.reset`).  The corpus uses one pattern; the extractor handles that pattern; widgets that drift from it silently drift from the spec.  Mitigation: a future Layer 2 advisory diagnostic could flag non-canonical reset shapes (filed as a non-NECESSARY follow-up since CLAUDE.md §3 documents the canonical pattern as required).

#### Research-grade follow-up (deferred, not committed)

**Formal RHIF semantics + a proof of the algorithm's soundness.**  RHDL doesn't have a formal RHIF semantics yet.  Without it, every static analysis on RHIF is "structurally plausible against the patterns we tested" rather than "proven sound for all kernels."  This is the rigorous endpoint — Coq/Lean formalisation of the RHIF small-step semantics, then a proof that the extractor's output relates correctly to every kernel's behaviour.  6+ months of research-grade work; flagged as the asymptote, not committed for this follow-up cycle.

After items 1 + 2 shipped, treat the extractor's output as "validated against the corpus, the documented kernel patterns, and the executable semantics for representative widgets; sound by construction within that envelope; sound-for-arbitrary-kernels remains plausible but only proven when item 3 lands."

### 5.5 Prior implementations and their limitations

Three iterations preceded the current principled algorithm:

- **PR #2 (`feat/fsm-architecture`)** shipped the v1 extractor recognising only the *let-binding form* (`let next = match q.state { ... }; d.state = next;`) by finding the first `Case` opcode and reading its arms.  Excluded ~95% of real RHDL widgets, which use the side-effect form.
- **PR #6 (`feat/fsm-extractor-side-effects`)** extended the heuristic with a side-effect-form walker (Splice path-targeting `[<state_field>]` etc.).  Validated only against synthetic widgets in `doc.rs`; the per-PR §5.5 status note explicitly acknowledged that "the real 27-widget corpus will be the bigger validation."  First live test against `core::can_master` then produced spurious `Unanalyzable` on 4 of 13 arms (guarded transitions with implicit-hold else-branches).
- **PR #7 (`feat/fsm-extractor-implicit-self-loops`)** added implicit-self-loop semantics to fix the `can_master` `Unanalyzable` regression, but the heuristic was still "find the first Case opcode" — which on real `can_master` selects the `raw_bit` output-computation match instead of the FSM-transition match, producing 13 wrong transitions out of 20.

The principled algorithm replaces all of this with the data-flow walk in §5.3, validated against the full 27-widget corpus per §5.4 #1.  The `Unanalyzable` surface is now reserved exclusively for kernel-level shape problems and per-arm malformed-IR cases, pinned by negative tests in `rhdl_core::fsm::extraction::tests`.

---

## 6 — Layer 3: auto-generated state diagrams

### 6.1 What it produces

For every widget with `#[derive(Fsm)]` in scope, the rustdoc gets an auto-generated state diagram. The same metadata that drives Layer 2's analysis drives the diagram: nodes are variants, edges are transitions, labels are derived from `#[fsm_state(label = "...")]` attributes.

The renderer emits two formats:

- **Inline SVG** for rustdoc, via the existing `badascii_doc` machinery or a small new svg emitter that takes the transition graph and lays it out using a tractable algorithm (Sugiyama for hierarchical graphs; force-directed for cyclic ones). The SVG is embedded directly in the doc, so it survives offline rustdoc usage without external Graphviz dependency.
- **Graphviz `dot`** for the user, via `cargo run --example fsm_dump --package rhdl-fpga`. Useful for piping into `dot`, `xdot`, or external graph-analysis tools.

### 6.2 Implementation

A new helper crate `rhdl-fsm-diagram` (or a module inside `rhdl-fpga::doc`) that takes a `FsmState`-implementing type plus the transition graph from Layer 2 and produces SVG. The graph is computed at compile time by the Layer-2 pass and stashed in the widget's `Descriptor`; the rustdoc tooling reads it from there.

For the diagram itself, layout is the only meaningful complexity. Two passes:

1. Compute strongly-connected components of the transition graph. Render SCCs as boxes; non-SCC components as standard nodes.
2. Lay out using a layered algorithm (Sugiyama) for the DAG of SCCs and a circular layout inside each SCC.

This avoids the dependency on Graphviz at build time while producing diagrams that look approximately like Graphviz's `dot` output.

### 6.3 LLM-friendliness

A specifically interesting property: the generated diagram is a *structural* representation of the FSM that an LLM agent can read alongside the source. When an agent modifies the kernel (adds a transition, splits a state), the regenerated diagram shows the structural diff. This is a faster feedback loop than reading match arms — pattern matching against a graph is something LLMs do well.

The diagrams should be embedded in the rustdoc as both SVG and as a structured representation (JSON or a Rust constant) so that an LLM-driven workflow can consume the structured form when generating or modifying widgets.

### 6.4 Acceptance criteria for Layer 3

1. Every widget with `#[derive(Fsm)]` in scope produces a state diagram in its rustdoc, with no extra annotations needed by the widget author.
2. The diagram is up-to-date with the source by virtue of being derived from it — no `cargo run --example` step required for rustdoc.
3. Three existing FSM widgets have their rustdoc inspected and the diagrams reviewed for accuracy.
4. The structured (JSON) representation is also available for LLM-tool consumption.

### 6.5 Status (2026-04-29)

**Phase 3 shipped end-to-end across two PRs:**

**Phase 3b — kernel→diagram connector** (`feat/fsm-auto-transitions`, PR #4):
- `rhdl_core::fsm::extract_widget_transitions::<W>()` — compiles `W::Kernel` through Stage 1, runs `extract_canonical_transitions` against the resulting RHIF, returns the `ExtractionResult { transitions, unanalyzable }`.
- `rhdl_core::fsm::extract_widget_transitions_strict::<W>()` — same but errors out on any `Unanalyzable` diagnostic and returns the sorted transitions directly.
- `rhdl_fpga::doc::write_fsm_diagram::<W>(filename)` — calls the strict extractor, builds the diagram, renders to SVG, writes the markdown file.  No author-curated `FSM_TRANSITIONS` const required.
- `rhdl_fpga::doc::assert_fsm_transitions_match::<W>(manual)` — drift-check for any widget vintages still carrying author-curated lists.

**Phase 3c — rustdoc auto-injection** (`feat/fsm-rustdoc-autoinject`):
- `#[fsm_doc]` attribute macro (`rhdl_macro_core::fsm_doc`) — when placed on an FSM-tagged widget struct, emits `#[doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/doc/<WidgetName>_fsm.md"))]` automatically.  No per-widget `#![doc = include_str!(...)]` boilerplate.
- `#[fsm_doc(file = "...")]` form for the rare case where the conventional filename doesn't match the struct name.
- `rhdl_fpga::doc::assert_fsm_diagram_up_to_date::<W>(filename)` — drift-check helper for the on-disk SVG file (catches "kernel changed but author forgot to re-run the example").
- End-to-end demonstration: `rhdl_fpga::doc::demo::AutoDocMachine` — `cargo doc` produces a struct page with the SVG embedded, no author-written include line.

**Both Phase 3 acceptance criteria from §6.4 are now met:**

- ✅ #1 ("no extra annotations needed by the widget author") — `#[derive(Fsm)] + #[derive(FsmWidget)] + #[fsm(...)] + #[fsm_doc]` cover everything.  The `#[fsm_doc]` line replaces the old 3-line `#![doc = include_str!(...)]` block.
- ✅ #2 ("diagram up-to-date by virtue of being derived from source") — the markdown file's *content* is auto-derived from the kernel; `assert_fsm_diagram_up_to_date` catches drift in CI.

**Phase 3d — `cargo test` is the refresh trigger:**
- `rhdl_fpga::doc::refresh_and_check_fsm_diagram::<W>(filename)` — combined helper that *rewrites* the on-disk file from the current kernel and verifies the result.  Designed to be called from a `#[test]`.
- The author workflow becomes a single command: edit kernel → `cargo test` → diagram is fresh and the next `cargo doc` build picks it up via the `#[fsm_doc]`-emitted include.  No more "remember to run `cargo run --example <name>` after every kernel change".
- The strict no-refresh `assert_fsm_diagram_up_to_date` remains available as a CI canary that catches *renderer-level* regressions (e.g., a change to the SVG layout algorithm).

A *true* `build.rs`-driven auto-emit (Phase 3e if it ever becomes worth doing) would remove even the `cargo test` invocation, but Rust's build model makes this awkward — `build.rs` runs before the lib compiles and can't reach into the widget kernels without a circular dependency.  The realistic implementation paths (recursive cargo invocation, libloading-based reflection, a `cargo rhdl` subcommand) all carry significant complexity.  The `cargo test`-driven approach above already collapses the dev cycle to a single command using only Rust's standard tooling, which is the honest limit of what's worth building here.

---

## 7 — Layer 4: invariant assertions and SymbiYosys integration

### 7.1 Syntax

Properties live as attributes on the kernel function:

```rust
#[kernel]
#[fsm_invariant(after_reset = "state != State::Error")]
#[fsm_invariant("(state == State::Running) implies output.busy")]
#[fsm_liveness(eventually = "state == State::Done", from = "State::Initialized", within = 10000)]
#[fsm_cover(reachable = "state == State::Running { counter: bits(100) }")]
pub fn my_machine(cr: ClockReset, i: In, q: Q) -> (Out, D) { /* ... */ }
```

Four kinds of properties:

- `fsm_invariant`: a Boolean expression that must hold every cycle. Lowers to `assert property (@(posedge clock) (cond));` in SVA.
- `fsm_liveness`: a property that must eventually hold, optionally bounded. Lowers to `assert property (@(posedge clock) eventually cond);` (unbounded) or `assert property (@(posedge clock) ##[1:N] cond);` (bounded).
- `fsm_cover`: a coverage point — does the design ever reach this state? Lowers to `cover property (@(posedge clock) cond);`. Useful as both verification and dead-code detection.
- `fsm_assume`: an assumption about the environment that the proof can rely on. Lowers to `assume property (@(posedge clock) cond);`.

The expression language is a strict subset of the kernel-accepted expression language: equality, comparison, Boolean ops, field access, `matches!`, no calls (to keep symbolic execution tractable in Layer 5). The lowering parses the expression at compile time and emits the corresponding SVA expression directly.

### 7.2 SymbiYosys flow

The user runs:

```sh
cargo rhdl prove --package rhdl-fpga --widget MyMachine --engine smtbmc:z3 --depth 64
```

(`cargo rhdl` is a new cargo subcommand we'd add in `rhdl-toolchains`; alternative is a standalone `rhdl-prove` binary.)

This:

1. Compiles the widget to Verilog with the SVA properties embedded.
2. Generates a `.sby` config file declaring the SVA properties as `assert` mode (for invariants and liveness) or `cover` mode (for coverage points).
3. Invokes `sby` (SymbiYosys) on the generated config.
4. Parses the result; on counterexample, presents the witness as a structured trace (cycle-by-cycle inputs and resulting state evolution), with line numbers pointing into the kernel source.

The user-visible workflow is: write the property, run one command, get either "PROVED" or a concrete failing trace. Same UX as `cargo test` but for formal proofs.

### 7.3 Why SymbiYosys?

It's the canonical open-source formal-verification frontend for Verilog/SystemVerilog. Driven by Yosys; supports multiple proof engines (smtbmc, abc-pdr, z3, boolector, yices, cvc4); handles BMC, k-induction, and unbounded proofs; and is what nMigen/Amaranth, Spade, and SymbiFlow all use. Free, mature, and well-documented. Replicating it in pure RHDL would be Layer 5; integrating with it is Layer 4.

### 7.4 Acceptance criteria for Layer 4

1. Three widgets get a property suite that proves a meaningful invariant. Recommended initial corpus: `fifo::write_logic` (proves: write_address never wraps past read_address in the absence of an overflow), `core::crc` (proves: result matches reference for known test vectors), `core::round_robin_arbiter` (proves: liveness — every requesting client eventually grants).
2. The `cargo rhdl prove` subcommand is documented in the book.
3. CI runs `sby` on the verified widgets if `iverilog` is available; otherwise skips with a documented reason.
4. The Verilog-emission path supports SVA properties without breaking the existing `iverilog` round-trip — assertions are guarded behind a compilation flag so iverilog (which doesn't natively support all SVA) still simulates cleanly.

### 7.5 Artifact organization and CI integration

The SVA-bearing Verilog, SymbiYosys configs, proof-status records, and counterexample traces are first-class committed artifacts — not transient build outputs. They get the same review, snapshotting, and regression-detection treatment as the existing VCD digests and HDL emission snapshots.

#### Two emission modes

Verilog emission grows two distinct flavors:

- **`descriptor.hdl_for_synth(&target)`** — plain Verilog, no SVA. Goes to the synthesis flow (Vivado, nextpnr, Yosys synthesis). This is what the existing `descriptor.hdl()` becomes by default.
- **`descriptor.hdl_for_prove()`** — Verilog with embedded SVA. Goes to SymbiYosys. Never goes to synthesis.

Vendor synthesizers have inconsistent SVA support; iverilog has very limited support. Mixing the two corrupts both flows. Two modes keep responsibility clean: synth tools see only synthesizable Verilog; sby sees Verilog + SVA together. The kernel source is the same; only the emission stage differs.

#### Per-widget `prove/` directory

Mirroring the existing `vcd/` convention, every FSM widget with formal-verification properties gets a sibling `prove/` directory:

```
crates/rhdl-fpga/
├── src/<cat>/<widget>.rs              # source (with #[fsm_invariant] etc.)
├── examples/<widget>.rs               # runnable example
├── doc/<widget>.md                    # waveform markdown
├── vcd/<widget>/<widget>.vcd          # committed reference VCD (digest-checked)
└── prove/<widget>/
    ├── <widget>.sv                    # SVA-bearing Verilog (golden, expect_test snapshot)
    ├── <widget>.sby                   # SymbiYosys config
    ├── proved.toml                    # property-by-property proof status (golden)
    └── ce/                            # counterexamples committed only when caught
        └── <bug-name>/
            ├── trace.vcd              # the failing waveform
            └── repro.sby              # the exact config that found it
```

This places proof artifacts next to widgets, just like waveforms. A reviewer reading the widget can see immediately what's been proved without running sby.

#### `<widget>.sv` as an `expect_test` snapshot

The SVA-bearing Verilog file gets the same treatment as the existing Tier-3 HDL snapshot tests: an `expect_test` golden that the compiler-pass tests assert against. Compiler changes that alter the emitted SVA require explicit re-blessing with `UPDATE_EXPECT=1`, exactly like the existing snapshot machinery.

This catches a specific bug: a compiler optimization that's semantics-preserving in the synthesizable Verilog but accidentally weakens an assertion. The SVA snapshot is what catches it.

#### `proved.toml` schema

The property-by-property proof status, committed and reviewed as a golden file:

```toml
[meta]
widget = "fifo::write_logic::FIFOWriteCore"
generated = "2026-04-30T10:23:00Z"
sby_version = "0.50"

[invariant.write_address_in_range]
status = "proved"
engine = "smtbmc:z3"
depth = 32
note = "induction succeeded; address never overflows past read_address absent overflow latch"

[invariant.full_implies_no_advance]
status = "proved_to_depth"
engine = "smtbmc:z3"
depth = 64
note = "BMC verified; full proof requires k-induction (deferred)"

[liveness.eventually_drains]
status = "proved"
engine = "k-induction"
bound = 1024
note = "with fairness assumption on next signal"

[cover.exercises_overflow]
status = "reached"
cycle = 156
note = "overflow latch tested in cycle 156"

[invariant.no_corruption_under_stall]
status = "disabled"
reason = "requires environment fairness model not yet implemented"
```

A regression — a property going from `proved` to `proved_to_depth` or to `unknown` — surfaces as a diff in the golden file caught by the same review process that catches Verilog snapshot diffs.

The status enum has seven values:

- **`proved`** — sby completed and proved the property unconditionally.
- **`proved_to_depth`** — BMC up to depth N found no counterexample but didn't complete an inductive proof. Real bugs at depth > N would be missed.
- **`reached`** — for `fsm_cover` properties, the cycle number at which the property was covered during proof exploration.
- **`unknown`** — sby exhausted resources without conclusion. A real warning sign.
- **`failed`** — counterexample found. The trace lives in `ce/<bug-name>/`. Build fails until the property is fixed or explicitly disabled.
- **`disabled`** — explicitly skipped. Must include a `reason` field documenting why.
- **`new`** — property added but not yet run; CI will run it on the next pass and update the status.

The `note` field carries human-readable context that survives across re-runs.

#### Source-mapping in emitted SVA

Every SVA property in the emitted `<widget>.sv` carries a comment pointing back to the RHDL source:

```systemverilog
// RHDL: crates/rhdl-fpga/src/fifo/write_logic.rs:42 (fsm_invariant)
//   "after_reset = state != State::Error"
assert property (@(posedge clock) (state != 2'd3));
```

When sby's counterexample report references line N of the SystemVerilog, the user can grep for it in the source comments and find the original RHDL attribute. Without this, debugging is brutal.

#### Counterexamples as regression artifacts

When sby finds a counterexample, two things happen:

1. The build fails with a `miette`-decorated error: which property, which RHDL source line, the cycle of the violation, the input sequence that caused it.
2. The counterexample trace is automatically saved to `prove/<widget>/ce/<auto-name>/`. The user reviews it, gives it a meaningful name (e.g., `ce/full_after_reset_race/`), and commits it.

Even after the bug is fixed, the counterexample stays committed. It becomes a permanent regression test — every future build of the widget proves that this specific failure mode is no longer reachable. This is the most valuable artifact in the whole flow because it's the bug-shaped answer to "what should we never let happen again."

#### Tiered CI strategy

Sby is heavyweight (z3/boolector dependencies, multi-second proofs even for small widgets, multi-minute proofs for non-trivial ones). The right CI integration is tiered:

- **Per-PR**: run a fast subset on widgets touched by the PR. BMC at depth 32, single engine (smtbmc:z3 by default). Targets ~10 seconds per widget. Skipped cleanly if `sby` isn't installed (so contributors without the tool aren't blocked).
- **Nightly**: run the full proof matrix on all widgets. K-induction where applicable, deeper BMC bounds, multiple engines for cross-checking. Targets ~10 minutes total for a few hundred widgets.
- **Pre-release**: same as nightly plus exhaustive bounds (BMC depth 1024+, multiple solvers in parallel, fairness-assumption variants).

The CI configuration lives in `crates/Justfile`, with the standard `just prove`, `just prove-fast`, `just prove-full` invocations.

#### `cargo rhdl prove` UX

Mirrors `cargo test`:

```sh
cargo rhdl prove                              # all widgets, all properties
cargo rhdl prove --package rhdl-fpga          # one package
cargo rhdl prove --widget FIFOWriteCore       # one widget
cargo rhdl prove --property write_address_in_range  # one property
cargo rhdl prove --bless                      # update proved.toml goldens
cargo rhdl prove --engine smtbmc:boolector    # override engine
cargo rhdl prove --depth 256                  # override BMC depth
cargo rhdl prove --output trace.vcd           # save the witness/counterexample
cargo rhdl prove --sby-bin /path/to/sby       # custom sby installation
```

This integrates with the test infrastructure so an agent can drive proofs the same way it drives tests.

---

## 8 — Layer 5: built-in bounded model checker

The aspirational research-grade follow-on. RHDL has the kernel as a pure Rust function; symbolic execution of that function over `(state, input)` for K cycles, using a SAT/SMT solver, gives bounded model checking inside the language's own toolchain — no external Verilog flow required.

The implementation would integrate `z3` via its Rust bindings or `boolector` via FFI. Each kernel becomes a transition function `T: (State, Input) → State × Output`. BMC unrolls T for K steps and asserts that all configured invariants hold; the solver searches for a counterexample.

This is a 6+ month project on its own, but the payoff is significant: an LLM agent can propose a kernel and *prove* a property in seconds without invoking external tools. For the LLM-assisted-development thesis, this is the killer feature — agents become formally verifiable in a closed loop.

### 8.1 Why this is uniquely tractable in RHDL

- **Kernel-as-pure-fn.** No closures, no I/O, no heap. Each kernel call is a pure mathematical function. Symbolic execution is sound.
- **ADTs with finite discriminants.** State spaces are statically bounded.
- **The IR is already amenable.** RHIF is typed SSA — exactly the IR shape symbolic-execution engines want to consume.
- **Validation infrastructure already exists.** The iterator-based simulator, the existing `expect_test` machinery, and the `cargo test`-as-eval-harness all generalize to "verify that the BMC's counterexamples are real failures."

The principal risk is solver performance — 16-bit-wide arithmetic in a state struct can blow up the SMT formula. Mitigations: bitblasting helpers in the lowering, decomposition of multi-field states into independent sub-FSMs where possible, opt-in proof-engine selection (k-induction for invariants, BMC with depth bounds for liveness).

This layer is out of scope for an initial implementation. It belongs in the document as a destination, not as a near-term task.

---

## 9 — Validation requirements

Per `CLAUDE.md` §11.1, every layer is a compiler-level change with the full PR contract: one feature per PR, tests at every level, Justification section in the PR description, documentation in code + book + this design plan, CHANGELOG entry naming the guarantee preserved.

Specific test requirements per layer:

- **Layer 1.** Macro-expansion snapshot tests in `crates/rhdl-macro-core/src/expect/`. Three real-widget integration tests (FIFO write logic, round-robin arbiter, CRC) each rewriting an existing kernel with `#[derive(Fsm)]` and verifying the emitted Verilog is byte-identical.
- **Layer 2.** Pass-level unit tests with `expect_test` snapshots of the diagnostic output on hand-crafted FSMs (one with an unreachable state, one with a deadlock, one with a non-deterministic transition under guards). Negative test that an FSM with no issues produces no warnings.
- **Layer 3.** Snapshot test of the SVG output for a known FSM. Snapshot of the structured JSON representation. Manual review of three real widgets' diagrams.
- **Layer 4.** Each of the three corpus widgets gets a property suite proved end-to-end via `sby`. Negative test: a deliberately-broken FIFO with an off-by-one error in `write_logic` produces a counterexample trace pointing at the bug.
- **Layer 5.** Out of scope for this design plan; addressed in a future plan if and when the layer is undertaken.

---

## 10 — Risks and open questions

**State-update extraction is structural pattern matching.** Layer 2's transition-graph extraction relies on recognizing the canonical `match q.state { ... } -> next_state` shape. Kernels that compute next state through a non-canonical path (e.g., assign each field of `D.state` separately based on different conditions) will be invisible to the analysis or will produce conservative "unknown target" edges. We need to either restrict the FSM idiom to the canonical pattern, or invest in a more sophisticated analysis. Initial recommendation: restrict, document, and produce a clear diagnostic when the analysis can't determine the FSM structure.

**Output computation analysis.** Outputs (Mealy vs. Moore distinction) are derived from `(state, input)` in the same kernel. Layer 2's reachability analysis only covers state-to-state transitions; output verification belongs in Layer 4. There's a useful intermediate layer — "this output is a pure function of state alone (Moore) vs. depends on input (Mealy)" — that we could surface as a diagnostic but is deferred until Layer 4 brings in the expression-level analysis machinery.

**State explosion in BMC (Layer 5).** A state struct with two 16-bit fields has 4 billion states. Bitblasted SMT formulas grow accordingly. This is the standard model-checking risk. Spec-level mitigations: state abstraction (collapsing payload values to a small symbolic alphabet for proof purposes), decomposition into sub-FSMs, k-induction for invariants that don't need the full state space.

**Liveness properties without fairness assumptions.** Pure liveness ("eventually X") is undecidable without bounded depth or fairness assumptions about the environment. SymbiYosys handles this via bounded liveness or k-induction with explicit `fairness` properties on inputs. Layer 4 must require either a bound or a fairness assumption — open question whether to default to one or require explicit choice.

**Diagram layout quality.** Auto-layout of state graphs is hard. Sugiyama handles DAGs nicely; cyclic FSMs (most of them) need force-directed or other techniques and the result can be ugly. Initial implementation aims for "good enough"; if quality is bad, fall back to dumping `dot` and asking the user to render with Graphviz manually.

**Composition with kernel-language-extensions.** Match guards (kernel-language-extensions §2.4) interact with Layer 2's non-determinism analysis: once guards land, "potentially non-deterministic transition" diagnostics become more important. The two design plans should ship in coordination — Layer 2 should be designed assuming guards exist, even if guards land first.

**Composition with auto-pipelining.** Pipelined FSMs are an active research area in HLS. Auto-pipelining (per `auto-pipelining-plan.md`) will eventually want to retime FSMs, but the recurrence on the state-DFF feedback edge bounds how much retiming is possible. The FSM analysis in Layer 2 should produce the transition graph in a form auto-pipelining can consume to compute recurrence-bounded II for FSMs that include compute on state transitions (e.g., a pipeline-stage-fronted FSM controlling a multi-cycle multiply).

**Naming convention for FSM-aware widgets.** Should there be a separate widget category in `rhdl-fpga` for FSM-shaped widgets, or should they continue to live in `core`/`fifo`/`stream`/etc. as today? Recommendation: keep them where they are. The FSM derive is metadata; it's not a structural reorganization.

**LLM eval harness for FSMs.** A specific evaluation: take a corpus of N widget kernels with known FSM structure, ask an LLM agent to propose modifications (add a state, refactor transitions, add an output), and measure whether the modifications preserve the static-analysis diagnostics and the formal-verification proofs. This is the "FSM-aware LLM workflow" that the layered tooling enables. Out of scope for this design plan; worth flagging as a measurement target once Layer 4 ships.

---

## 11 — Phasing summary

| Phase | Deliverable | Status | Depends on |
|---|---|---|---|
| 1 | `#[derive(Fsm)]` macro + 3 widget rewrites | shipped (PR #2) | Nothing |
| 2 | Static reachability + dead-state pass (RHIF-level extractor + analyzer) | shipped (PR #2; side-effect-form support added in PR #6) | Phase 1 |
| 3a | Diagram renderer + JSON / SVG / dot emitters | shipped (PR #2) | Phase 1, 2 |
| 3b | Kernel→diagram connector: `extract_widget_transitions` + `write_fsm_diagram` (no manual `FSM_TRANSITIONS` const) | shipped (`feat/fsm-auto-transitions`, PR #4) | Phase 1, 2, 3a |
| 3c | `#[fsm_doc]` attribute macro auto-injects `#[doc = include_str!(...)]` into the widget's rustdoc — removes the per-widget boilerplate line | shipped (`feat/fsm-rustdoc-autoinject`, PR #5) | Phase 3b |
| 3d | `cargo test`-driven auto-refresh via `refresh_and_check_fsm_diagram` (`cargo run --example` step no longer needed) | shipped (`feat/fsm-rustdoc-autoinject`, PR #5) | Phase 3c |
| 4 | `#[fsm_properties(...)]` + SVA emission | shipped (PR #2) | Phase 1, 2 |
| 4b | `cargo rhdl prove` SymbiYosys driver + corpus proofs | not yet shipped | Phase 4 |
| 5 | Built-in bounded model checker | not yet shipped (research-grade) | Phase 4b |

Phases 1+2+3a ship together as PR #2 ("FSM ergonomics + analysis substrate"). Phase 3b ships as `feat/fsm-auto-transitions` (PR #4) and lets every widget drop its author-curated `FSM_TRANSITIONS` const. Phase 3c+3d ship as `feat/fsm-rustdoc-autoinject` (PR #5). PR #6 (`feat/fsm-extractor-side-effects`) extended the Phase-2 extractor to handle the side-effect `d.state = X` form (the canonical idiom used by ~95% of widgets). Phase 4 is its own track. Phase 5 is research-grade, not committed.

The first widget rewrites that opt into the FSM derives ship in `refactor/use-fsm-and-or-patterns` (post-merge of PR #2 and PR #3): `core::can_master` (CanField as the FSM enum, plus or-pattern collapse of three field-classification matches) and `core::one_wire_master` (OneWireState).  See the relevant CHANGELOG entries for the detailed before/after.

---

## 12 — References

[1] Mealy, G.H. *A Method for Synthesizing Sequential Circuits.* Bell System Technical Journal, 1955. — The foundational Mealy machine paper. Outputs depend on state and input.

[2] Moore, E.F. *Gedanken-Experiments on Sequential Machines.* Automata Studies, Princeton University Press, 1956. — The Moore-machine companion. Outputs depend on state alone.

[3] Cleaveland, R., and Hennessy, M. *Testing Equivalence as a Bisimulation Equivalence.* Formal Aspects of Computing, 1993. — Theoretical underpinning for FSM equivalence checking.

[4] IEEE 1800-2017. *SystemVerilog Language Reference Manual,* §16: SystemVerilog Assertions (SVA). — The canonical specification of `assert property`, `cover property`, `assume property`, `eventually`, etc.

[5] Wolf, C. *Yosys Open Synthesis Suite.* https://yosyshq.net/yosys/ — The Yosys ecosystem; SymbiYosys is the formal-verification frontend.

[6] Wolf, C., et al. *SymbiYosys.* https://symbiyosys.readthedocs.io/ — The formal-verification driver Layer 4 integrates with. Supports smtbmc, abc-pdr, multiple SMT solvers.

[7] Amaranth HDL Project. *Formal Verification.* https://amaranth-lang.org/docs/amaranth/latest/stdlib/formal.html — Python-language precedent for the same architectural pattern (declarative FSMs + SymbiYosys integration).

[8] SpinalHDL. *State Machine Library.* https://spinalhdl.github.io/SpinalDoc-RTD/master/SpinalHDL/Libraries/fsm.html — Scala-language precedent. Worth studying for the syntactic design choices we're *not* adopting (declarative DSL keywords).

[9] Skarman, F., and Gustafsson, O. *Spade: An Expression-Based HDL With Pipelines.* OSDA 2023. — Spade's `state` keyword is a closer cousin to RHDL's design philosophy than SpinalHDL's DSL approach.

[10] Bluespec System Verilog. Arvind, R.S. Nikhil. *Bluespec System Verilog: Efficient, Correct RTL from High-Level Specifications.* MEMOCODE 2004. — The atomic-action / scheduler approach as an alternative to explicit FSMs.

[11] de Moura, L., and Bjørner, N. *Z3: An Efficient SMT Solver.* TACAS 2008. — The SMT solver Layer 5 would integrate with.

[12] Niemetz, A., and Preiner, M. *Boolector: A Bit-Vector Decision Procedure.* SAT 2017. — Alternative SMT solver for bit-vector-heavy hardware.

[13] Cimatti, A., Clarke, E., Giunchiglia, F., and Roveri, M. *NUSMV: A New Symbolic Model Verifier.* CAV 1999. — Foundational symbolic model checker; useful reference for the BMC algorithm in Layer 5.

[14] Sugiyama, K., Tagawa, S., and Toda, M. *Methods for Visual Understanding of Hierarchical System Structures.* IEEE Transactions on Systems, Man, and Cybernetics, 1981. — The layered graph drawing algorithm Layer 3 would use for state diagrams.

[15] Basu, Samit. *RHDL: Rust as a Hardware Description Language.* LATTE '25, March 2025. — In-tree at `doc/latte25/latte.tex`. The kernel-as-pure-fn invariant that this design plan exploits is established here.

---

## 13 — Decisions captured

For the record (also reflected in `architecture.md` and `CLAUDE.md` once shipped):

- **The FSM surface is metadata, not new syntax.** `#[derive(Fsm)]` plus two attribute hints, not a DSL with new keywords. Keeps rust-analyzer working and LLM-friendly.
- **The state enum is the source of truth.** All metadata flows from the `Digital`-derived state enum; the widget struct and kernel just declare where it lives.
- **Reset handling stays where it is.** The existing "reset comes last" idiom in the kernel is unchanged. The FSM macro doesn't reorganize reset logic.
- **Static analysis is advisory by default.** Diagnostics are warnings unless the user opts into errors. Layer 2 must not break the existing widget corpus.
- **Formal verification ships via SymbiYosys (Layer 4) before any in-house BMC (Layer 5).** Reuse the open-source formal-verification ecosystem first; only build our own when we have empirical data on what's missing.
- **The SVA expression sublanguage is a strict subset of the kernel-accepted expression language.** No calls, no payloads-of-payloads, no recursion. Keeps Layer 5's symbolic execution tractable when it lands.
- **Diagrams are auto-generated from metadata.** The state-diagram artifact is a *consequence* of the FSM derive, not a separately-maintained file. Diagrams stay in sync with code by construction.
