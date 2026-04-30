# Pass Invariants

> Normative reference for what each RHIF-level pass requires of its input and what it guarantees about its output. Passes live in `crates/rhdl-core/src/compiler/rhif_passes/`. Every pass's per-section entry below is normative for that pass; if the pass implementation drifts, this document defines the intended behaviour.

## The universal contract

Every pass implements `trait Pass` with method `fn run(Object) -> Result<Object>`. The universal contract:

- **Input:** a well-formed `Object` per [`invariants/object.md`](./object.md).
- **Output:** a well-formed `Object` per [`invariants/object.md`](./object.md).
- **Semantic equivalence:** for every input `[v₀, …, vₙ]` valid against the input's argument kinds, `execute(input_obj, [v₀, …, vₙ])` and `execute(output_obj, [v₀, …, vₙ])` produce the same result.

Passes may strengthen any of these guarantees. Below, each pass is listed with:

- **Purpose** — what the pass does, in one line.
- **Requires** — pre-conditions on input beyond well-formedness.
- **Preserves** — invariants the pass maintains through its rewrite.
- **Establishes** — invariants the pass adds to its output.

The pass list mirrors the files in `crates/rhdl-core/src/compiler/rhif_passes/`.

---

## `check_clock_domain`

**Purpose.** Static check that no opcode mixes operands of different `Color`s without an intervening `Retime`.

**Requires.** Type-correct input.

**Preserves.** Everything (this is a check pass; it does not mutate the IR).

**Establishes.** For every opcode in `Object::ops`, all operands either share a `Color` or do not have one; the only opcode whose result `Color` differs from its operand `Color` is `Retime`.

If a violation is found, the pass returns an `RHDLError` with a `miette` diagnostic pointing at the offending op. No partial output is produced.

---

## `check_for_rolled_types`

**Purpose.** Reject types that appear "rolled" — typically a non-monomorphised generic that survived to RHIF. Rolled types are a sign the front-end didn't fully expand; downstream passes assume monomorphised kinds.

**Requires.** Well-formed input.

**Preserves.** Everything.

**Establishes.** No rolled-type kinds appear in `Object::symtab`.

---

## `check_rhif_flow`

**Purpose.** Static check on the dataflow graph: every read precedes its write, every register is defined before use, no SSA violations.

**Requires.** Well-formed input.

**Preserves.** Everything.

**Establishes.** The "definition before use" and "single-assignment" invariants of [`invariants/object.md`](./object.md) are satisfied.

This pass is the canonical late check that downstream passes do not produce IR violating the SSA discipline.

---

## `check_rhif_type`

**Purpose.** Verify per-opcode type correctness against [`type-system.md`](../type-system.md).

**Requires.** Well-formed input (in particular, symbol-table completeness).

**Preserves.** Everything.

**Establishes.** Every opcode in the output is well-typed under the type environment derived from `Object::symtab`.

---

## `constant_propagation`

**Purpose.** Replace operands that are known constants (literal slots, or registers whose defining opcode produces a known value) with literal slots. Where a register's value can be computed at compile time, pre-compute it.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness; type-correctness.

**Establishes.** Every register slot whose value is uniquely determined by the kernel's argument-independent inputs has been replaced with a literal slot whose value matches that determined value. (Other registers are unchanged.)

---

## `dead_code_elimination`

**Purpose.** Replace any opcode whose `lhs` is unused (never read by another op or by `Object::return_slot`) with a `Noop`.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** For every non-`Noop` opcode in the output, its `lhs` is read at least once by another opcode or by `Object::return_slot`.

---

## `lower_dynamic_indices_with_constant_arguments`

**Purpose.** Replace `DynamicIndex(s)` path elements where `s` is known to be a constant with `Index(k)` (where `k = read(s).as_i64() as usize`).

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** No `DynamicIndex(s)` element appears in any path where `s` is provably constant.

This pass is the bridge between source-level dynamic indexing and the RTL-level static-mux selection: where the dynamic index is provably constant, the lowering is wire routing rather than a multiplexer, saving area.

---

## `lower_inferred_casts`

**Purpose.** Resolve the `len: Option<usize>` field of `AsBits` / `AsSigned` / `Resize` opcodes to `Some(_)`.

**Requires.** Well-formed input. Type-inference must have run beforehand to determine the result kinds.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** Every `Cast`-style opcode in the output has `len = Some(_)`. Reaching the VM with `len = None` is an ICE; this pass eliminates that ICE class.

---

## `lower_inferred_retimes`

**Purpose.** Resolve the `color: Option<Color>` field of `Retime` opcodes to `Some(_)`.

**Requires.** Well-formed input. Type-inference must have determined the surrounding `Signal<_, _>` kinds.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** Every `Retime` opcode in the output has `color = Some(_)`.

---

## `partial_initialization_check`

**Purpose.** Reject programs where a register is read only after a partial-aggregate write (e.g., a struct field is set, but the struct is read before the rest is set). RHDL's `dont_care()` constructor requires all fields read downstream to be assigned; partial reads are the canonical "silently produces undefined hardware" footgun.

**Requires.** Well-formed input.

**Preserves.** Everything.

**Establishes.** Every aggregate read at any point reaches a fully-initialised value at that point.

---

## `pre_cast_literals` / `precast_integer_literals_in_binops`

**Purpose.** Ensure that integer literals in binary-op contexts have the correct kind ahead of arithmetic. Source-language integer literals can be polymorphic until they hit a concrete operand's kind; these passes pin them.

**Requires.** Well-formed input. Type inference has determined operand kinds.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** For every `Binary(op, _, a₁, a₂)` opcode, `kind(a₁) = kind(a₂)` (modulo signedness for shift / widening ops).

---

## `precompute_discriminants`

**Purpose.** For `Enum` opcodes whose `template`'s discriminant can be statically chosen, set the template's discriminant bits at compile time so the VM doesn't have to extract them at runtime.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** Every `Enum` opcode's `template.discriminant()` is a fully-defined value.

---

## `propagate_literals`

**Purpose.** When a register's defining opcode is an `Assign` from a literal, replace all reads of that register with reads of the literal directly.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** No `Assign(reg, literal)` opcode survives where `reg` is read elsewhere; the reads are short-circuited to the literal.

---

## `remove_empty_cases`

**Purpose.** Replace `Case` opcodes with no arms (or with only `Wild` arms producing `dont_care`) by `Assign(lhs, dont_care_literal)` or by `Noop` if `lhs` is dead.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** No `Case` opcode in the output is degenerate.

---

## `remove_extra_registers`

**Purpose.** Eliminate registers that are aliases of other registers (introduced by intermediate `Assign`s).

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** No register's only writer is an `Assign` from another register that itself has only one read.

---

## `remove_unneeded_muxes`

**Purpose.** Replace `Select(lhs, cond, x, x)` (both arms identical) with `Assign(lhs, x)`. Replace `Select(lhs, true_lit, x, _)` with `Assign(lhs, x)`. Etc.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** No `Select` opcode in the output is reducible to a copy.

---

## `remove_unused_literals`

**Purpose.** Drop literals from `Object::symtab` that are not referenced by any opcode.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** Every literal slot in `Object::symtab` is referenced at least once.

---

## `remove_unused_registers`

**Purpose.** Drop registers from `Object::symtab` that are not referenced by any opcode.

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** Every register slot in `Object::symtab` is either the `lhs` of some opcode or is in `Object::arguments`.

---

## `remove_useless_casts`

**Purpose.** Drop cast opcodes that are no-ops (e.g., `Resize` to the same width and kind as the input).

**Requires.** Well-formed input.

**Preserves.** Semantic equivalence; well-formedness.

**Establishes.** No cast opcode in the output is a no-op.

---

## `symbol_table_is_complete`

**Purpose.** Verify that every slot referenced by any opcode is in `Object::symtab`.

**Requires.** Well-formed input.

**Preserves.** Everything.

**Establishes.** The "symbol-table completeness" invariant of [`invariants/object.md`](./object.md) is satisfied.

This pass is a canonical guard that downstream passes do not introduce slot references without registering them.

---

## Pass ordering

The pass driver in `crates/rhdl-core/src/compiler/` runs passes in a fixed order. The ordering reflects the dependencies among `Establishes`/`Requires` clauses above. As an example:

- `check_for_rolled_types` runs early (other passes assume monomorphised kinds).
- `lower_inferred_casts` runs before `constant_propagation` (constant-prop needs to evaluate casts; uninferred ones are unevaluable).
- `dead_code_elimination` and `remove_unused_*` typically run late (after constant propagation has surfaced all trivially-removable code).

The exact order is in the driver. When adding a new pass, document its position in the ordering and the reason — typically, "after pass X because it requires X's `Establishes`."

## Adding a new pass

Per CLAUDE.md §11.1, every compiler-level PR that adds a pass must:

1. Add the pass implementation under `crates/rhdl-core/src/compiler/rhif_passes/`.
2. Add unit tests with `expect_test` snapshots of input/output IR.
3. Add an entry in this document with `Purpose`, `Requires`, `Preserves`, `Establishes`.
4. Justify the pass's position in the pipeline ordering in the PR description.
5. Run the full `cargo test --all` suite without `UPDATE_EXPECT=1` to confirm no widget-snapshot regressions; if any change, audit each diff in the PR description.

## Cross-references

- The pass driver in `crates/rhdl-core/src/compiler/` invokes these passes in order.
- The `Pass` trait in `crates/rhdl-core/src/compiler/mir/pass.rs` (or similar) — the contract every pass implements.
- For widget-level tests that catch pass regressions, see the Tier-3 HDL snapshot tests across `crates/rhdl-fpga`.
