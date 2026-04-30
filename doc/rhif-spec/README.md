# RHIF Specification

This directory is the prose specification for RHDL's Intermediate Form (RHIF). Per `rhif-formalization-plan.md` Phase 1, this is the **Level-1 normative reference** for what RHIF is and what it does.

> **Status: Phase 1, Level 1 (prose). Normative.** Where this spec and the implementation in `crates/rhdl-core/src/rhif/` disagree, this spec defines what the implementation *should* do. The implementation is buggy until they are reconciled.

## Reading order

For a compiler-level contributor:

1. [`overview.md`](./overview.md) — what RHIF is, its place in the pipeline, and how to read this spec.
2. [`syntax.md`](./syntax.md) — slots, kinds, opcodes, paths, the `Object` structure.
3. [`type-system.md`](./type-system.md) — the typing judgements per opcode.
4. [`semantics.md`](./semantics.md) — the operational semantics per opcode.
5. The relevant page under [`opcodes/`](./opcodes/).
6. [`invariants/object.md`](./invariants/object.md) — global well-formedness conditions.
7. [`invariants/passes.md`](./invariants/passes.md) — what each pass requires and guarantees.
8. [`invariants/lowering.md`](./invariants/lowering.md) — what RHIF → RTL → NTL → Verilog preserves.
9. [`reset-clock.md`](./reset-clock.md) — clock and reset modelling at the kernel boundary.

For a widget-level contributor: you do not need this spec. The widget contract is `Synchronous` / `Circuit` plus the kernel-language subset documented in `doc/book/`. RHIF is below that layer.

For an LLM agent picking up a compiler-level task: read [`overview.md`](./overview.md), then the opcode page(s) for the opcode(s) you're touching, then the relevant invariants page. Cite the specific spec section in your PR's Justification (per CLAUDE.md §11.1).

## Per-opcode pages

All 19 opcodes have dedicated pages under [`opcodes/`](./opcodes/):

- [`noop.md`](./opcodes/noop.md) — `Noop`
- [`binary.md`](./opcodes/binary.md) — `Binary` (16 ALU operations)
- [`unary.md`](./opcodes/unary.md) — `Unary` (13 ALU operations)
- [`select.md`](./opcodes/select.md) — `Select` (2:1 mux)
- [`index.md`](./opcodes/index.md) — `Index` (path-walk read)
- [`assign.md`](./opcodes/assign.md) — `Assign` (copy)
- [`splice.md`](./opcodes/splice.md) — `Splice` (functional update)
- [`repeat.md`](./opcodes/repeat.md) — `Repeat` (homogeneous array)
- [`struct.md`](./opcodes/struct.md) — `Struct` (struct constructor)
- [`tuple.md`](./opcodes/tuple.md) — `Tuple` (tuple constructor)
- [`case.md`](./opcodes/case.md) — `Case` (multi-way selection)
- [`exec.md`](./opcodes/exec.md) — `Exec` (kernel call)
- [`array.md`](./opcodes/array.md) — `Array` (heterogeneous-source array)
- [`enum.md`](./opcodes/enum.md) — `Enum` (variant constructor)
- [`as_bits.md`](./opcodes/as_bits.md) — `AsBits` (cast to `Bits<N>`)
- [`as_signed.md`](./opcodes/as_signed.md) — `AsSigned` (cast to `SignedBits<N>`)
- [`resize.md`](./opcodes/resize.md) — `Resize` (width change, preserve signedness)
- [`retime.md`](./opcodes/retime.md) — `Retime` (`Color` re-tagging)
- [`wrap.md`](./opcodes/wrap.md) — `Wrap` (`Option`/`Result` constructor)

## Relationship to source code

- `crates/rhdl-core/src/rhif/spec.rs` — the syntactic ground truth (the `OpCode` enum, operand types).
- `crates/rhdl-core/src/rhif/vm.rs` — the executable semantics.
- `crates/rhdl-core/src/rhif/object.rs` — the `Object` structure.
- `crates/rhdl-core/src/rhif/runtime_ops.rs` — the per-`AluBinary` / `AluUnary` runtime helpers.
- `crates/rhdl-core/src/types/typed_bits.rs` — the runtime value type.
- `crates/rhdl-core/src/types/kind.rs` — the `Kind` enum.
- `crates/rhdl-core/src/types/path.rs` — the `Path` and `PathElement` types.
- `crates/rhdl-core/src/compiler/rhif_passes/` — the pass implementations.

## Drift policy

Per CLAUDE.md §11.1 and `rhif-formalization-plan.md` §11:

- Every PR that modifies `crates/rhdl-core/src/rhif/spec.rs` must update the corresponding spec page(s) in this directory.
- The Phase 2 property-based test suite (when it ships) cross-checks the spec against the VM.
- Until Phase 2 ships, drift is enforced by review.

If you find a discrepancy between the spec and the implementation, file it as a defect; the default assumption is that the spec is normative and the implementation is buggy. Deviations from this default require a design rationale captured in the PR.

## Phase 2 onwards

This is Phase 1 (prose). Phase 2 (property-based VM testing) builds on this specification — every property in the test suite cites the spec section it's checking. Phases 3+ (PLT Redex, Coq, verified extraction) are research targets sketched in `rhif-formalization-plan.md`; they are not committed engineering work.
