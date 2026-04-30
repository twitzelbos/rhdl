# RHIF Specification — Overview

> **Status: Phase 1, Level 1 (prose).** Per `rhif-formalization-plan.md`, this directory is the prose ground truth for RHIF semantics. Where this spec and the implementation disagree, the spec defines what the implementation *should* do — the implementation is buggy until they are reconciled. The Rust file `crates/rhdl-core/src/rhif/spec.rs` is the syntactic ground truth; this directory is the semantic ground truth. Both are normative for their concern.

## What RHIF is

RHIF — *RHDL Intermediate Form* — is the first of RHDL's three intermediate representations. It is the **typed, three-address, single-assignment IR** that the proc-macro front-end lowers `#[kernel]` Rust into, and that the compiler optimises and lowers to RTL.

Every instance of RHIF in the compiler is an [`Object`](./syntax.md#objects), which represents a single kernel — a pure function from `Digital` inputs to `Digital` outputs. There are no global variables, no shared state, no I/O, no allocation. The IR is deliberately small: 19 opcodes (one of which is `Noop`).

## Place in the RHDL pipeline

```
Rust source (#[kernel])
    │
    │  proc-macro: parse → AST → MIR → RHIF
    ▼
RHIF  (this spec)
    │
    │  rhdl-core::compiler::rhif_passes::*
    │  (analysis + optimisation; in-place RHIF rewrites)
    ▼
RHIF  (still well-typed; same observable semantics)
    │
    │  rhdl-core::compiler::lower_rhif_to_rtl
    ▼
RTL                            ← untyped SSA, see crates/rhdl-core/src/rtl/spec.rs
    │
    ▼
NTL                            ← netlist, see crates/rhdl-core/src/ntl/spec.rs
    │
    ▼
Verilog (printed via rhdl-vlog AST)
```

This spec covers RHIF only. The RTL and NTL forms are described in their respective `spec.rs` files; their semantic correspondence with RHIF is documented in `invariants/lowering.md`.

## Design principles

1. **Typed.** Every slot has a static [`Kind`](./syntax.md#kinds-types). There is no `Any`, no untyped wire, no implicit coercion between widths or signedness.
2. **Three-address.** Every opcode produces at most one result slot (`lhs`) and reads from a fixed set of operand slots. There are no nested expressions in the IR; complex expressions in the source are flattened into temporaries.
3. **Single-assignment (SSA-on-registers).** Each non-literal slot is the `lhs` of at most one opcode. Reading a slot before its defining opcode has executed is undefined behaviour. (See `invariants/object.md` for the full well-formedness contract.)
4. **Pure.** A kernel is a pure function: same inputs in, same outputs out, no side effects, no I/O, no time. Sequential semantics — registers, clocks, resets — live one level up, in `Synchronous` and `Circuit`.
5. **Small.** 19 opcodes. The complexity that source-language users see (control flow, pattern matching, struct construction with field updates, etc.) is desugared by the front-end into combinations of these 19.
6. **Verilog-faithful.** RHIF is designed to lower cleanly to a netlist. Every opcode has a known, finite resource cost. `Mul` is a multiplier, `Binary(Add)` is an adder, `Select` is a mux, `Index` is wire-routing. Nothing here is a "high-level helper" that hides hardware.

## What is *not* in RHIF

- **No control flow.** No `if`/`else` opcode (use `Select`); no `while`/`for` (kernels statically unroll their loops in the front-end); no jumps, no labels, no exceptions.
- **No allocation.** No `Vec`, no heap. `Array`, `Tuple`, `Struct`, `Enum` are stack-shaped aggregates with fixed sizes.
- **No references.** Every operand and result is by value. There is no `&T`.
- **No closures, no function values.** Calls happen via `Exec(FuncId, ...)`; the function table is fixed at compile time.
- **No mutation through paths.** `Splice` is an expression that produces a new aggregate; it does not mutate in place.
- **No clocks or registers.** Clocks, resets, flip-flops, and the post-reset value contract live in the `Synchronous` / `Circuit` layer, not in the kernel function.

## Reading this spec

If you are extending the compiler — adding an opcode, writing a new pass, modifying a lowering — read in this order:

1. [`syntax.md`](./syntax.md) — what an RHIF program is, syntactically. The opcode list, the slot model, the kind system, the structure of `Object`.
2. [`type-system.md`](./type-system.md) — the well-typedness judgement Γ ⊢ op : ok.
3. [`semantics.md`](./semantics.md) — the operational semantics judgement σ ⊢ op ⇓ σ′.
4. The relevant page under [`opcodes/`](./opcodes/) for each opcode you're touching.
5. [`invariants/object.md`](./invariants/object.md) — global well-formedness invariants on a whole `Object`.
6. [`invariants/passes.md`](./invariants/passes.md) — the contract every pass must preserve, and the pre/post-conditions of individual passes.
7. [`invariants/lowering.md`](./invariants/lowering.md) — what it means for RHIF → RTL → NTL → Verilog to be semantics-preserving.
8. [`reset-clock.md`](./reset-clock.md) — how clocks, resets, and the surrounding `Synchronous`/`Circuit` machinery interact with RHIF semantics.

If you are widget-level, you do not need to read this spec at all. The widget contract is `Synchronous` / `Circuit` plus the kernel-language subset documented in `doc/book/`. RHIF is below that layer.

## Naming conventions used in the spec

- **Slots** are written `s`, `s₁`, `s₂`, `lhs`, `arg`. Where ambiguity matters, register slots are written `r` and literal slots `l`.
- **Kinds** are written `T`, `T₁`, `T₂`. Specific kinds are written in their compact forms: `b8` for `Bits<8>`, `s8` for `SignedBits<8>`, `T@C` for `Signal<T, C>`, `(T₁, T₂)` for tuples, `[T; N]` for arrays.
- **Type environments** are written Γ. The judgement Γ ⊢ s : T means "in environment Γ, slot s has kind T."
- **States** are written σ. A state is a partial map from slots to values. The judgement σ ⊢ op ⇓ σ′ means "executing op in state σ produces state σ′."
- **Values** are written `v`, `v₁`, `v₂`. Values are runtime `TypedBits`.
- **Paths** are written `p`. A path is a sequence of `PathElement`s — see [`syntax.md`](./syntax.md#paths).

## Versioning and drift

Per CLAUDE.md §11.1 and `rhif-formalization-plan.md` §11, every PR that modifies `rhdl-core/src/rhif/spec.rs` must update the corresponding spec page (or pages) in this directory. The Phase 2 property-based test suite (when it ships) catches drift between the spec and the VM. Until then, spec drift is enforced by review.

If you find a discrepancy between the spec and the implementation, file it as a defect against either the spec or the implementation — whichever is wrong. The default assumption is that the spec is normative and the implementation is buggy; deviations require a design rationale.
