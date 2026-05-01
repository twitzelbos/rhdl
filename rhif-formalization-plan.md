# RHIF Formalization Plan

A plan for formalizing the semantics of RHDL's intermediate form (RHIF) at five increasing levels of rigor — from "the spec is the implementation" (where we are today) up through "Coq-mechanized soundness with extracted verified passes" (where CompCert is). The plan is staged so that the cheap, high-leverage levels ship near term and the academically-ambitious levels are sketched as research targets without committing to them.

This is the seventh compiler-and-language design plan, alongside `auto-pipelining-plan.md`, `kernel-language-extensions.md`, `vendor-primitive-architecture.md`, `fsm-architecture.md`, `stream-bus-architecture.md`, and `rule-architecture.md`. Unlike those, this plan is *foundational* — it doesn't add new compiler features; it specifies the contract that all the other plans operate against.

The motivating observation: every one of the six design plans above adds opcodes, lowering rules, or analyses to the RHIF / RTL / NTL stack. Each pass author currently has to read existing pass code to discover what semantics RHIF actually has — there is no contract. That's tractable for the original author (Samit) and tractable-with-effort for a careful human contributor. It is *not* tractable for an LLM agent picking up a compiler-level task: the agent has nowhere to look for the ground truth except the implementation, which is also the thing they're modifying. A formal RHIF specification — at any level above zero — eliminates that asymmetry.

---

## 1 — Motivation

### 1.1 The empirical observation

Compiler-level work in RHDL today follows a pattern: the contributor reads the existing pass implementations, infers the semantic invariants the IR maintains, and writes a new pass that *appears* to preserve them. The correctness of the result depends on the contributor having read enough existing passes to internalize the implicit contract. There is no document anywhere that says, in normative form: "RHIF programs satisfy property X; every pass must preserve property X; here is the precise definition of X."

For widget authors this is not a problem — widgets sit far above RHIF and don't need to know how the IR is shaped. For *compiler-level* contributors, it is increasingly a problem as the IR grows. Every design plan in the family above adds at least one opcode or lowering rule:

- `kernel-language-extensions.md` — new lowering rules for or-patterns, range patterns, guards, etc.
- `auto-pipelining-plan.md` — new register-insertion pass; relies on a notion of "semantics-preserving transformation" that is currently informal.
- `fsm-architecture.md` — new RHIF-walking analysis for state-transition graph extraction.
- `vendor-primitive-architecture.md` — new `PrimitiveRequest` NTL opcode; semantics?
- `stream-bus-architecture.md` — new lowering pattern for `Option<Item<T, F>>` that the auto-pipeliner needs to recognize.
- `rule-architecture.md` — new macro-layer scheduler synthesis that must produce well-typed RHIF.

Each plan implicitly assumes that "RHIF is well-defined enough that I can extend it." That assumption is correct for now, but it grows shakier with each addition. The spec-as-implementation will, eventually, harbor an inconsistency that produces silently wrong hardware.

### 1.2 What Claude Code (and other agents) actually need

When an LLM agent picks up a task like "add support for `?` operator on Option," it needs to answer questions like:

- What is the dynamic semantics of the `Wrap` opcode?
- What is the type-checking rule for `Wrap`?
- What is the relationship between `Wrap` and the existing `Case` opcode?
- What pass invariants must I preserve when introducing `Wrap` into a kernel?
- How does `Wrap` lower to RTL? What invariants must the RTL form satisfy?
- If I write a new pass that consumes `Wrap`, what well-formedness conditions can I assume?

Today the agent answers all of these by reading `rhdl-core::rhif::spec.rs`, `rhdl-core::compiler::rhif_passes::*`, `rhdl-core::compiler::lower_rhif_to_rtl.rs`, and the relevant downstream RTL/NTL code. That's roughly 5,000 lines of code to internalize for one targeted change. The agent gets it right *most* of the time, but the failure modes are exactly the ones we cannot afford in a compiler — silent semantic drift across passes.

A normative RHIF specification — even at Level 1 (prose) — collapses this 5,000-line read to a one-document read. The agent's prompt becomes "implement `?` per `rhif-spec.md` §<opcode>" instead of "implement `?` after inferring the contract from N existing passes." This is the most concrete, immediate payoff of formalization.

### 1.3 The longer arc

Beyond LLM-friendliness, formalization serves three industrial purposes:

- **Pass authoring with proven invariants.** Each pass declares the invariants it preserves; the spec defines what invariants exist; reviewers verify both. Compiler bugs become declared-property violations, not "well, this widget's snapshot changed and we don't know why."
- **Cross-pass reasoning.** If pass A claims to preserve invariant X and pass B claims to require invariant X, composition is sound by construction. Without a spec, this composition is empirical.
- **Verification readiness.** If RHDL ever wants to make claims like "this kernel is provably correct against this property," the foundation is a formal IR. CompCert proved that a formal-IR + verified-pass approach is feasible for production compilers; it took 11 person-years for C. RHDL's IR is much simpler than C; the comparable effort is months, not years.

---

## 2 — What "formal RHIF" means (and doesn't mean)

A spectrum, not a binary.

| Level | Description | Cost | Payoff |
|---|---|---|---|
| 0 | Implementation is the spec (today) | $0 | none beyond what we have |
| 1 | Prose specification document — every opcode's syntax, type rule, dynamic semantics, and pass invariants written in normative English | ~3 weeks | LLM-friendly, contributor onboarding, contract for pass authors |
| 2 | Property-based VM testing — random RHIF programs verified to satisfy declared properties (well-typedness, progress, semantic equivalence across passes) | ~4 weeks | empirical confidence that the spec matches the implementation |
| 3 | Executable operational semantics in PLT Redex / K Framework — a separate, runnable definition of RHIF semantics that can be tested against the implementation | ~2 months | rigorous semantic ground truth; tractable for non-experts |
| 4 | Coq mechanization — RHIF syntax, type system, and operational semantics encoded as Coq inductive types; soundness theorems (type preservation, progress) proved | ~6 months | provable correctness theorems available |
| 5 | Verified extraction — Coq-verified compiler passes extracted to executable Rust/OCaml; full CompCert-style guarantee that the deployed compiler matches the formal spec | ~2 years | "this compiler is provably correct" |

Levels 0–2 are engineering work. Level 3 is engineering with a strong specification skill. Levels 4–5 are research projects with full-time-faculty-and-PhD-student scale. We commit to Levels 1 and 2; we sketch the path through Levels 3–5 without committing.

The key word is *normative*: a formal RHIF spec says what RHIF *must do*, not what it *currently does*. A pass that violates the spec is buggy; an existing implementation that disagrees with the spec is wrong (and the spec is right, modulo design errors). This distinction is what turns Level 1 from "developer documentation" into "contract."

---

## 3 — Current state

Today, the de-facto specification of RHIF lives in three places:

- **`rhdl-core/src/rhif/spec.rs`** — the syntactic enumeration of opcodes (`OpCode`), the operand types (`Slot`, `CaseArgument`), the auxiliary structures (`Binary`, `Unary`, `Select`, `Index`, `Assign`, `Splice`, `Repeat`, `Struct`, `Tuple`, `Case`, `Exec`, `Array`, `Enum`, `AsBits`, `AsSigned`, `Resize`, `Retime`, `Wrap`). This file is the closest thing to a syntactic spec we have; it's well-organized but does not document semantics.
- **`rhdl-core/src/rhif/vm.rs`** — the executable semantics. A function that takes an RHIF `Object` and inputs, and produces outputs. This is the operational ground truth; if the spec disagrees, the VM is what runs.
- **The pass implementations** in `rhdl-core/src/compiler/rhif_passes/*.rs` — each pass implicitly assumes some invariants and produces other invariants. The contract between passes is unwritten.

What's *not* written down anywhere:

- The type rule for each opcode (when is `Binary { lhs, op, arg1, arg2 }` well-typed?)
- The well-formedness conditions for an `Object` (every Slot has a unique definition; the symbol table is complete; etc.)
- The invariants each pass preserves (e.g., "after `RemoveExtraRegistersPass`, every register is read at least once")
- The invariants each pass requires (e.g., "`ConstantPropagation` requires that the symbol table be complete")
- The relationship between RHIF semantics and RTL semantics (what does it mean for the lowering to be correct?)
- The relationship between RHIF semantics and the iterator-based simulator (does the simulator faithfully reflect RHIF execution?)

This last gap is particularly important. The iterator-based simulator (`rhdl-core/src/sim/`) is what users primarily test against; the iverilog round-trip is what they trust as ground truth. The relationship between the simulator's semantics, RHIF's VM semantics, and the eventual Verilog's semantics is not spelled out. Most of the time these agree; when they don't, debugging is hard.

---

## 4 — Level 1: prose specification

The minimum-viable formalization. A single document — `rhif-spec.md` or a structured directory — that for each opcode normatively documents:

- **Syntax**: the operands and their types (already in `spec.rs`, but echoed here in a stable form independent of the Rust source).
- **Type rule**: when is the opcode well-typed? Expressed in inference-rule notation (Γ ⊢ ... : T) or in structured prose.
- **Dynamic semantics**: what does the opcode compute? Expressed as a small-step or denotational semantic rule, in prose.
- **Pre-conditions**: what must hold for the opcode to be well-formed in a context (e.g., "the LHS slot must be unbound before this op")?
- **Post-conditions**: what does the opcode guarantee (e.g., "after this op, the LHS slot is bound to a value of type T")?

Plus, document-level sections covering:

- **Object well-formedness**: the global invariants of an RHIF `Object` (single-assignment property, complete symbol table, every used slot has a definition).
- **Pass invariants**: what every pass must preserve (well-formedness, type-correctness, semantic equivalence on all reachable inputs). What individual passes additionally preserve or require.
- **Lowering relations**: the relationship between RHIF and RTL (every RHIF execution corresponds to an RTL execution that produces the same observable values).
- **Reset and clock semantics**: how `cr.reset.any()` and clock edges are modeled in the IR.

### 4.1 Format

The spec is a *companion document* to `spec.rs`, not a replacement. Both will exist; both will be normative for their concern (syntax in `spec.rs`, semantics in `rhif-spec.md`). Where they disagree, the Rust code wins for the specific test it captures, but the spec wins as the contract — the implementation is buggy if it can't be reconciled with the spec.

A possible structure:

```
doc/rhif-spec/
├── overview.md              # what RHIF is, design philosophy, naming
├── syntax.md                 # the OpCode enum, operand types, well-formedness
├── type-system.md            # type rules per opcode, with inference rules
├── semantics.md              # operational semantics per opcode
├── invariants/
│   ├── object.md             # global Object well-formedness invariants
│   ├── passes.md             # per-pass invariants (preserved + required)
│   └── lowering.md           # RHIF ↔ RTL ↔ NTL semantic correspondence
├── opcodes/
│   ├── binary.md
│   ├── unary.md
│   ├── select.md
│   ├── ...                   # one file per opcode, each with syntax/types/semantics
│   └── wrap.md
└── reset-clock.md            # reset and clock semantics, ClockReset modeling
```

Each opcode page is the most-frequently-read artifact. It should be ~30 lines: a syntax block, a type rule, a semantic rule, pre/post-conditions, and a couple of sentences of intuition.

### 4.2 Effort

~3 weeks of focused work. The structure is mechanical; the per-opcode content can be drafted from `spec.rs` plus a careful read of `vm.rs`. Most of the time goes to the cross-cutting concerns (well-formedness, pass invariants, lowering relations) where the existing implementation is sparsely documented.

### 4.3 Why this alone justifies the work

A Level-1 prose spec is what an LLM agent (and a careful human contributor) reads when picking up a compiler-level task. Even without proof, the existence of a single normative document collapses the onboarding effort from "read all of `rhdl-core` and infer the contract" to "read `rhif-spec.md` and the relevant pass." That is a 100x reduction in cognitive load. For LLMs specifically, it's the difference between "agent can credibly do this" and "agent gets it almost right but introduces subtle invariant violations."

---

## 5 — Level 2: property-based VM testing

After prose comes empirical verification. The RHIF VM (`rhif/vm.rs`) executes RHIF programs. We extend it with a property-based test suite that verifies the spec.

### 5.1 Properties to test

- **Well-typedness preservation.** Every pass takes a well-typed `Object` and produces a well-typed `Object`. Random small RHIF programs are generated, checked for well-typedness, run through each pass, and checked again.
- **Semantic preservation across passes.** Every pass is observation-equivalent: for any input to the kernel the pass operates on, the pre-pass and post-pass kernel produce the same outputs (modulo the pass's stated transformations).
- **Lowering correctness.** RHIF → RTL lowering preserves observable behavior: for any kernel and input, the RHIF VM result equals the RTL VM result.
- **Lowering bisimulation.** A finer property: every step of the RHIF execution corresponds to a sequence of RTL steps that produce the same intermediate values (up to lowering's introduction of intermediate registers).
- **Symbol-table completeness.** After every pass, every used Slot has a definition; no Slot is referenced before being defined.
- **Single-assignment.** After every pass (which is in SSA form), every Slot has exactly one assignment.

Each property becomes a `proptest`-style test in `rhdl-core/tests/spec/`. The generators produce random RHIF programs of a specified shape; the runners exercise each pass against the property.

### 5.2 Coverage strategy

Two complementary corpora:

- **Synthetic random programs.** Generated by the proptest harness. Cover wide patterns; small per-program; many programs.
- **Real-widget corpus.** Every widget in `rhdl-fpga::*` produces an RHIF Object as part of its build. We shadow the existing widget tests with property-based "did the spec hold?" assertions on each widget's compiled RHIF.

The latter catches regressions in real widgets the moment they happen. The former catches edge cases the user-facing tests don't cover.

### 5.3 Effort

~4 weeks, including building the random-program generators (which is the hardest part — generating type-correct RHIF programs requires shrinking and type-aware fuzzing).

---

## 6 — Level 3: executable operational semantics

The next-level rigor: a *separate*, runnable definition of RHIF semantics, written in PLT Redex (a Racket-based semantics-engineering tool) or K Framework (a meta-language for semantics). The separate definition serves as a second oracle alongside the Rust VM; if they disagree, one of them is wrong.

### 6.1 What this looks like

A PLT Redex model would encode:

- The RHIF syntax as a Redex grammar.
- The type system as Redex judgments.
- The operational semantics as Redex reduction rules.

The Redex model is roughly 500–1500 lines of Racket; it is checkable by Redex's `redex-check`, runnable on small RHIF programs, and amenable to formal-style proofs.

### 6.2 What it gets us

- **Independence from the Rust implementation.** Bugs in the Rust VM that match bugs in the Rust pass code don't show up via the Rust-only Level-2 testing; they can show up against the Redex semantics if the bug isn't replicated there.
- **A semantic ground truth that reads like math.** Researchers, paper-readers, and formal-verification practitioners can engage with the Redex model directly without needing to read Rust.
- **A foundation for Level 4.** A Coq mechanization is much easier when there's already a specified small-step semantics; PLT Redex is the bridge.

### 6.3 Effort

~2 months. Significant skill required — Redex is not widely known. Could be done by an interested contributor or a faculty/PhD collaborator.

This is where the work transitions from "engineering investment" to "research collaboration." We don't commit to Level 3 in this plan; we sketch what it would look like so that anyone who wants to do it has a starting point.

---

## 7 — Level 4: Coq mechanization

The aspirational level. Encode RHIF in Coq (or Lean 4, or Isabelle/HOL), prove soundness theorems, eventually prove pass correctness.

### 7.1 What this looks like

In Coq:

```coq
Inductive opcode : Type :=
  | Binary : alu_binary -> slot -> slot -> slot -> opcode
  | Unary  : alu_unary  -> slot -> slot -> opcode
  | Select : slot -> slot -> slot -> slot -> opcode
  | Case   : slot -> slot -> list (case_arg * slot) -> opcode
  | (* ... *)
.

Inductive well_typed_op : type_env -> opcode -> Prop :=
  | wt_binary : forall Γ op lhs a1 a2 t,
      Γ ⊢ a1 : t -> Γ ⊢ a2 : t ->
      well_typed_op Γ (Binary op lhs a1 a2)
  | (* ... *)
.

Inductive step : state -> state -> Prop := (* ... *).

Theorem type_preservation : forall s s',
  well_typed_state s -> step s s' -> well_typed_state s'.
Proof. (* ... *) Qed.

Theorem progress : forall s,
  well_typed_state s ->
  (terminal s) \/ (exists s', step s s').
Proof. (* ... *) Qed.
```

The mechanization gives us machine-checked theorems about RHIF. The standard pair (type preservation + progress) is the Wright-Felleisen recipe for type-soundness; it's a few months of Coq work for a moderately-experienced practitioner.

Beyond soundness, individual passes can be proved correct:

```coq
Theorem const_prop_preserves_semantics : forall obj,
  well_typed obj -> equiv_obj obj (constant_propagation obj).
```

This is the CompCert pattern. Each pass becomes a proven semantics-preserving transformation.

### 7.2 What it gets us

- Machine-checked theorems about RHIF that survive proof-checking forever.
- A precedent for verified hardware-compiler infrastructure (Kami, VeriCert, etc. have done this for related problems).
- Strong claims about correctness that could appear in academic publications, customer marketing materials, or regulatory submissions.

### 7.3 What it costs

The Wright-Felleisen soundness pair: 3–6 months of Coq work for someone with experience. CompCert-style verified compilation of all passes: 1–3 person-years (and that's after Level 3 is in place). This is research collaboration; it's not engineering investment.

### 7.4 What we recommend

Sketch the Coq encoding so that an interested researcher (PhD student, postdoc, faculty) can pick it up if they want. Do not commit to it as engineering work. If someone shows up wanting to do this, we provide the Level-1 spec, the Level-2 test corpus, and the Level-3 Redex semantics as a foundation, and they do the Coq work as their research.

---

## 8 — Level 5: verified extraction

The peak: extract Coq-verified passes back to Rust (or OCaml, the standard Coq extraction target), so the deployed compiler is provably correct against the formal spec. This is the CompCert pattern: the Coq-extracted OCaml compiler is what users actually run.

### 8.1 What this looks like

Coq's extraction mechanism produces OCaml from Coq programs. The extracted OCaml is then compiled by a regular OCaml compiler. For Rust extraction, projects like **MetaCoq** and **hax** are exploring the path.

For RHDL specifically: replace the hand-written pass implementations with Coq-extracted equivalents. The generated Rust is functionally identical to the Coq specification; any bug in the deployed compiler is by definition a bug in the Coq spec.

### 8.2 What it costs

CompCert is the closest reference: 11 person-years of work to ship a verified C compiler. RHDL's RHIF is much simpler than C — no aliasing, no pointers, no complex type system, fixed-size data — but the verified-compiler discipline is the same. A reasonable estimate is 1–3 person-years for a PhD-student-or-equivalent.

### 8.3 What we recommend

Do not commit. Sketch as the long-term destination. The work is not justified until and unless RHDL is being adopted in safety-critical settings (avionics, medical, automotive) where formal-correctness arguments have economic value.

---

## 9 — Phasing

We commit to Levels 1 and 2. We sketch Levels 3–5 as research targets.

| Phase | Deliverable | Effort | Dependencies | Status |
|---|---|---|---|---|
| 1 | Prose RHIF specification: `doc/rhif-spec/` directory + per-opcode pages + invariants + lowering relations | ~3 weeks | nothing | **Shipped 2026-04-30** ([`doc/rhif-spec/`](./doc/rhif-spec/)) |
| 2 | Property-based VM testing: random-program generators + per-property tests + widget-corpus shadowing | ~4 weeks | Phase 1 | **Shipped 2026-04-30** ([`crates/rhdl-core/src/rhif/well_formedness.rs`](./crates/rhdl-core/src/rhif/well_formedness.rs), [`crates/rhdl-core/src/rhif/property_tests.rs`](./crates/rhdl-core/src/rhif/property_tests.rs), [`crates/rhdl-fpga/src/widget_*.rs`](./crates/rhdl-fpga/src/)).  CI integration + extended random program coverage are Phase 2 follow-ups |
| 3 (research) | PLT Redex / K Framework operational semantics | ~2 months | Phase 1 | sketched |
| 4 (research) | Coq mechanization with soundness theorems | ~6 months | Phase 3 | sketched |
| 5 (research) | Verified extraction | ~2 years | Phase 4 | sketched |

Phases 1 and 2 are engineering tasks with definite deliverables. Phases 3–5 are research collaboration — they ship if a researcher picks them up. The plan documents what they'd look like so a researcher has somewhere to start.

---

## 10 — Validation

For Phase 1 (prose spec):

- Every opcode in `rhdl-core::rhif::spec.rs::OpCode` has a corresponding page in `doc/rhif-spec/opcodes/`.
- Every pass in `rhdl-core::compiler::rhif_passes::*` has its preserved-and-required invariants documented in `doc/rhif-spec/invariants/passes.md`.
- The spec is reviewed by Samit (or designated maintainer) for accuracy against the implementation.
- An LLM agent can be prompted with the spec + a clear task, and produce correct compiler-pass code in one shot more often than today's baseline.

For Phase 2 (property-based testing):

- Every claimed invariant in the prose spec has at least one corresponding property test.
- The widget corpus (every `rhdl-fpga::*` widget) is checked against the property suite.
- The CI runs the property suite on every PR; failure means a spec violation has been detected.
- A meta-test: deliberately introduce a known invariant violation in a copy of one pass and verify the property suite catches it.

For Phases 3–5: the standard academic-publication validation (peer review, mechanized proofs, conference papers).

---

## 11 — Risks and open questions

**Drift between spec and implementation.** Once a prose spec exists, it can drift. The risk is that the spec becomes outdated and stops reflecting reality. Mitigations: (a) the Phase 2 property tests check spec-vs-implementation; (b) a CI check that the spec's per-opcode pages match the `OpCode` enum's variants exactly; (c) every change to `spec.rs` requires a corresponding update to the spec doc, enforced by review.

**Spec ambiguity.** Prose specs can be ambiguous in ways that machine-checked specs cannot. A pass author can in principle satisfy "the letter" of the spec while violating its spirit. Mitigations: (a) Phase 2 tests check the actually-claimed properties; (b) the spec has worked examples for each opcode; (c) ambiguous cases are flagged in the spec as "implementation-defined" so future tightening is possible.

**Scope creep into the IR.** Once the spec exists, every IR change becomes "and update the spec." This is correct overhead but it is overhead. New opcodes go through more friction, which is a feature for stability and a bug for rapid prototyping.

**LLM-generated spec drift.** Tempting to use an LLM to keep the spec in sync with the implementation. This works for routine updates but fails for semantic changes — the LLM may "fix" the spec to match a buggy implementation rather than fix the implementation. Mitigation: every spec update requires human review.

**Coq mechanization without engineering follow-through.** A Phase 4 Coq mechanization that doesn't get integrated into Phase 5 verified extraction is academic vanity. It produces papers but doesn't change the deployed compiler. The plan should not start Phase 4 unless there's a credible path to Phase 5; alternatively, Phase 4 should be framed as "research artifact" rather than "engineering deliverable."

**Cost of spec maintenance vs. payoff.** If RHDL is a one-person project, the spec is overhead with no clear ROI. If RHDL has multiple contributors (or, increasingly, LLM agents acting as contributors), the spec amortizes quickly. The plan is contingent on the latter being true.

**The "Coq up" framing might be ambitious for the wrong reason.** Claude Code (or another agent) wanting Coq-mechanized semantics might be reaching for the wrong solution. The actual problem is "I don't know what RHIF semantics are." Level 1 solves that. Level 4 over-solves it. The plan must not let the agent's appetite for formality drive the project past where the engineering ROI runs out.

**Compositionality across the design plan family.** Each design plan adds at least one opcode or pass. The spec must be updated *as those plans land*, not retroactively. Otherwise the spec ages immediately. This means every compiler-level design plan should include "update the RHIF spec" as part of its acceptance criteria; CLAUDE.md §11.1 should be amended to enforce this once Phase 1 of the spec ships.

---

## 12 — Comparison with related work

**CompCert** (Leroy et al., 2006–present). The canonical verified-compiler project. C source → assembly, with proofs in Coq that the extraction is observation-equivalent. ~100,000 lines of Coq, 11 person-years. The blueprint for Level 5.

**Vellvm** (Zhao et al., 2012–present). Formal semantics of LLVM IR in Coq. Demonstrates that machine-IR formalization is tractable for production systems. Closer in spirit to what RHIF formalization would look like than CompCert is.

**CakeML** (Kumar et al., 2014–present). Verified ML compiler, end-to-end from source language through machine code. Uses HOL4 rather than Coq.

**Kami** (Choi et al., 2017). Coq-based hardware DSL with formal semantics. The closest precedent for verified hardware in a dependently-typed framework. Demonstrates that the path from "formal hardware-IR semantics" through "verified compilation" exists for HDLs.

**VeriCert** (Lopes & Daniel, 2021). Verified high-level synthesis from C to Verilog, using Coq. The most-recent and most-relevant project; shows that verified C-to-Verilog is achievable in a research-grade timeline.

**PLT Redex** (Felleisen, Findler, Flatt, 2009). The semantic-engineering tool for Level 3.

**K Framework** (Roşu, Şerbănuţă, 2010–present). An alternative semantic-engineering tool. Used for the formal semantics of C, Java, Solidity, and others.

**Bluespec** (Arvind, Nikhil, ~2000–present). The closest HDL precedent for compile-time-verified hardware. Bluespec's atomic-rule semantics are formally specified in the company's published papers, though the proprietary compiler is not Coq-verified. Validates the "rule semantics + compile-time scheduler" approach that `rule-architecture.md` adopts.

**ACL2 with hardware extensions** (Slobodova, Murphy). IBM's industrial-strength formal-verification system, applied to hardware proofs. Demonstrates that mechanization scales to commercial-grade designs.

The relevant precedent for RHDL is "small, typed, SSA IR + proven semantic preservation across a small number of passes." That's much closer to Vellvm or the verified passes of CompCert (the optimization phase, not the C frontend) than it is to verified C from scratch. The work is non-trivial but not unprecedented.

---

## 13 — References

[1] Leroy, X. *Formal verification of a realistic compiler.* Communications of the ACM, 52(7), 2009. — The CompCert project, the canonical verified-compiler reference.

[2] Zhao, J., Nagarakatte, S., Martin, M.M.K., Zdancewic, S. *Formalizing the LLVM Intermediate Representation for Verified Program Transformations.* POPL 2012. — Vellvm; the closest precedent for formalizing a real-world compiler IR.

[3] Felleisen, M., Findler, R.B., Flatt, M. *Semantics Engineering with PLT Redex.* MIT Press, 2009. — The PLT Redex tool and methodology for Level 3.

[4] Choi, J., Vijayaraghavan, M., Sherman, B., Chlipala, A., Arvind. *Kami: A Platform for High-Level Parametric Hardware Specification and its Modular Verification.* ICFP 2017. — The closest formal-hardware-IR precedent in Coq.

[5] Lopes, N. P., Daniel, O. *VeriCert: Verified High-Level Synthesis.* ASPLOS 2021. — Verified C-to-Verilog HLS in Coq. The most-relevant precedent for what an RHDL Level-5 would look like.

[6] Roşu, G., Şerbănuţă, T.F. *An Overview of the K Semantic Framework.* Journal of Logic and Algebraic Programming, 79(6), 2010. — The K Framework alternative to PLT Redex.

[7] Wright, A.K., Felleisen, M. *A Syntactic Approach to Type Soundness.* Information and Computation, 115(1), 1994. — The type-preservation + progress recipe for Level-4 soundness theorems.

[8] Pierce, B.C. *Types and Programming Languages.* MIT Press, 2002. — The textbook on type systems and operational semantics; the standard reference for the Level-3 / Level-4 work.

[9] Kumar, R., Myreen, M.O., Norrish, M., Owens, S. *CakeML: A Verified Implementation of ML.* POPL 2014. — Verified ML compiler using HOL4.

[10] Sozeau, M., Anand, A., Boulier, S., Cohen, C., Forster, Y., et al. *The MetaCoq Project.* Journal of Automated Reasoning, 64, 2020. — Coq-to-Rust extraction; relevant for Level 5.

[11] Basu, Samit. *RHDL: Rust as a Hardware Description Language.* LATTE '25, March 2025. — The RHDL paper. The compiler-and-IR architecture that this plan would formalize.

---

## 14 — Decisions captured

For the record (also reflected in `architecture.md` and `CLAUDE.md` once shipped):

- **Levels 1 and 2 are committed engineering work.** The plan ships a prose RHIF spec and a property-based VM test suite. These are engineering deliverables with definite scope and effort.
- **Levels 3, 4, and 5 are research targets, not committed work.** They are documented in this plan so that an interested researcher has a starting point. They ship if and when a contributor (graduate student, postdoc, faculty collaborator) picks them up; they do not block any other RHDL work.
- **The prose spec is normative.** Where the spec and the implementation disagree, the spec defines what the implementation should do. The implementation is buggy until they are reconciled.
- **The spec is a companion to `spec.rs`.** Both exist. The Rust file is the syntactic ground truth; the Markdown directory is the semantic ground truth.
- **Spec drift is enforced via CI.** Every PR that changes `spec.rs` must update the corresponding spec page; a CI check verifies the cross-reference. Property-based tests catch implementation drift from the spec.
- **The spec is required reading for compiler-level work.** Per CLAUDE.md §11.1, every compiler-level PR includes a "what spec property does this preserve" Justification entry. The spec is what that justification refers to.
- **The plan does not commit to Coq.** The plan documents Levels 4 and 5 because they are the academic destination; it does not promise to deliver them. Anyone investing engineering time on Level 4 / 5 should validate the ROI against the alternative use of that time.
- **The plan is foundational, not feature-additive.** It does not add a new compiler feature; it specifies the contract under which existing and future features operate. Every other design plan in the family becomes more rigorous once this one ships, because each plan can refer to the formal spec rather than re-deriving the contract.
