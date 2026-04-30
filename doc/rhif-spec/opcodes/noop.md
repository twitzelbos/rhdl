# `Noop`

A do-nothing instruction. Used by passes as a placeholder when an opcode is logically deleted but the pass does not want to (or cannot) re-shrink the body vector.

## Syntax

```
Noop
```

No `lhs`, no operands.

## Type rule

```
————————————
Γ ⊢ Noop : ok
```

Always well-typed.

## Dynamic semantics

```
σ ⊢ Noop ⇓ σ
```

Identity on state.

## Pre-conditions

None.

## Post-conditions

State is unchanged. Specifically, no slot is bound or modified.

## When this op appears

- After dead-code elimination, when a pass deletes an opcode but leaves a hole.
- As a result of `RemoveExtraRegistersPass` or similar simplifications.
- Hand-written tests of the IR plumbing.

## Lowering

`Noop` is dropped during RHIF → RTL lowering; it produces no RTL. (See [`invariants/lowering.md`](../invariants/lowering.md).)

## Cross-references

- `OpCode::Noop` in `crates/rhdl-core/src/rhif/spec.rs`.
- VM dispatch in `crates/rhdl-core/src/rhif/vm.rs::execute_block`.
