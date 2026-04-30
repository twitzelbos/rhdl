# Object Well-Formedness

> Normative reference for what makes an `Object` (an RHIF program) well-formed. These are the global invariants that hold *between* opcodes; per-opcode well-formedness is in [`type-system.md`](../type-system.md). A pass that produces an Object that does not satisfy these invariants is buggy.

An `Object` is **well-formed** if every property below holds.

## Single-assignment

Every register slot is bound at most once.

Formally: for every register slot `r` referenced by `Object::ops`, there is **at most one** opcode in `Object::ops` whose `lhs` is `Slot::Register(r)`. Equivalently, `count({ op | op ∈ Object::ops, op.lhs() = Some(Slot::Register(r)) }) ≤ 1`.

Exceptions: argument slots (those in `Object::arguments`) are bound by the caller, not by an opcode. They are referenced as operand slots (read) but never as `lhs`.

Why: SSA simplifies dataflow analysis. Many passes assume single-assignment when computing dependencies.

## Definition before use

Every read of a register slot precedes the opcode that defines it (in execution order).

Formally: for every `Object::ops[i]` that reads a register slot `r` (i.e., `r` is one of `op.arg1`, `op.arg2`, `op.cond`, …, or appears in `op.path` as a `DynamicIndex(_)`), if there exists a defining opcode `Object::ops[j]` with `op_j.lhs() = Some(Slot::Register(r))`, then `j < i`.

If `r` is a register that is not defined anywhere in `Object::ops`, it must be an argument slot (`r ∈ Object::arguments`).

Why: Reading an unbound register is an ICE in the VM. Well-formed RHIF cannot trip that ICE; it is enforced statically.

## Symbol-table completeness

Every slot referenced in `Object::ops` (or in `Object::return_slot`, or in `Object::arguments`) is registered in `Object::symtab`.

Formally:
- Every `Slot::Register(r)` referenced anywhere is a key of `Object::symtab.iter_reg()`.
- Every `Slot::Literal(l)` referenced anywhere is a key of `Object::symtab.iter_lit()`.

Why: Reading from / writing to an unregistered slot is an ICE. Symbol-table completeness is what gives every slot a kind.

## Type-correctness

Every opcode in `Object::ops` is well-typed under the type environment derived from `Object::symtab`. See [`type-system.md`](../type-system.md) for the per-opcode rules.

Additionally:
- `Object::return_slot` has a recorded kind.
- For each `r ∈ Object::arguments`, `Object::symtab[r]` is the kind that callers will supply.

Why: Type-correct RHIF lowers cleanly to RTL; type-incorrect RHIF produces undefined Verilog. The type system also catches clock-domain mixing (see [`reset-clock.md`](../reset-clock.md)).

## Externals consistency

For every `Exec(_, id, args)` opcode in `Object::ops`:
- `id ∈ Object::externals.keys()`.
- The callee `func = Object::externals[id]` is itself well-formed (recursively).
- `args.len() = func.arguments.len()`.
- For each `i`, `kind(args[i]) ≡ kind(func.arguments[i])`.

The graph of "calls" induced by the externals map is **acyclic**: no kernel calls itself transitively. (The proc-macro front-end forbids recursion in source; this invariant is the IR-level statement of that prohibition.)

Why: Recursion would make termination undecidable; an acyclic call graph guarantees termination.

## Argument and return-slot kinds

- `Object::arguments` is a list of register slots (not literals); each has a kind in the symbol table.
- `Object::return_slot` may be either a register or a literal slot; it has a kind in the symbol table.

The kernel's signature is `(Object::arguments) ↦ Object::return_slot`. This is the contract that callers (`Exec`) and surrounding `Synchronous`/`Circuit` machinery rely on.

## No nested `Signal`

If `T = Signal(T', C)`, then `T'` is not itself `Signal(_, _)`.

This invariant is enforced by the `Retime` typing rule and the front-end's kind construction. It guarantees that `T@C` always names a single clock-domain wrapper around a "real" kind.

Why: Nested `Signal`s would mean a value has two clock domains simultaneously, which has no hardware meaning.

## Path well-formedness

Every `Path` referenced in an `Index` or `Splice` opcode walks to a well-defined sub-kind of the aggregate's kind. Specifically, for `Index(_, arg, p)` with `kind(arg) = T`:

- The path-typing judgement `(T, p) ⇒ T'` succeeds.
- Every `DynamicIndex(s)` element of `p` has `s` registered in the symbol table with a kind that is `Bits(_)` or `Signed(_)` (i.e., integer-convertible).

The same applies to `Splice`.

Why: A path that doesn't walk cleanly is meaningless; the VM's `path` and `splice` operations would either ICE or produce undefined behaviour.

## Literal slots are read-only

No opcode in `Object::ops` has `lhs = Slot::Literal(_)`. Literals are bound at compile time (via `Object::symtab.iter_lit()`) and never overwritten.

Why: Writing to a literal is an ICE in the VM. Well-formed RHIF cannot do this.

## `Noop` neutrality

A `Noop` opcode is well-formed in any context. It produces no `lhs`, has no preconditions, and does not affect state. Passes may freely insert or delete `Noop`s without breaking other invariants.

Why: `Noop` is the deletion placeholder; passes need to be able to delete an opcode in O(1) without re-shrinking the Vec or re-numbering subsequent opcodes.

## How invariants are checked

There is no single "is this Object well-formed" function in `rhdl-core`. The invariants above are spread across:

- The proc-macro front-end (which constructs the initial Object in well-formed shape).
- Each pass (which preserves well-formedness by construction or by post-condition).
- The VM (which catches at runtime any well-formedness violation that slipped through earlier checks: undefined registers, write-to-literal, missing length, etc., all surfaced as `ICE` errors).
- Type-checking (which catches type-correctness violations).

Phase 2 of the spec plan (per `rhif-formalization-plan.md` §5) introduces property-based tests that *programmatically* check each invariant against random and corpus-derived Objects.

## What a pass must preserve

A pass's contract — applied to every pass — is:

- **Input** is well-formed.
- **Output** is well-formed.

A pass that violates this contract is buggy. The minimum bar is "all of the invariants above hold after the pass runs."

Some passes make stronger guarantees (e.g., the constant-propagation pass guarantees that, after it runs, every reachable register has its value determined at compile time iff its inputs do). Those stronger guarantees are documented per pass in [`passes.md`](./passes.md).

## Cross-references

- `Object` struct in `crates/rhdl-core/src/rhif/object.rs`.
- `vm.rs::execute` for the runtime checks that catch leftover violations.
- The pass list in `crates/rhdl-core/src/compiler/rhif_passes/`.
