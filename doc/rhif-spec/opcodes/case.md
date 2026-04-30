# `Case(lhs, discriminant, table)`

Multi-way selection on a discriminant. The classical "ROM lookup" or "case statement" of the IR. Used to lower `match` expressions on enums and other small-domain values.

## Syntax

```
Case {
    lhs:           Slot,
    discriminant:  Slot,
    table:         Vec<(CaseArgument, Slot)>,
}

CaseArgument ::= Slot(Slot)   -- match if disc == read(slot)
              | Wild         -- always match
```

## Type rule

```
Γ ⊢ disc : T_disc
∀ (arg, val) ∈ table.
  arg = Wild ∨ Γ ⊢ arg : T_disc
∀ (_, val) ∈ table. Γ ⊢ val : T
Γ ⊢ lhs : T
————————————————————————————————————————————
Γ ⊢ Case(lhs, disc, table) : ok
```

All arms must have the same result kind `T` (the kind of `lhs`). All non-`Wild` discriminator slots must match the kind of `discriminant`.

## Dynamic semantics

```
read(σ, disc) = v_d
matched = first (arg, val) in table such that:
            arg = Wild   OR  read(σ, arg) = v_d
v = if matched = Some((_, val)) then read(σ, val)
    else dont_care(kind(lhs))
————————————————————————————————————————————————————————
σ ⊢ Case(lhs, disc, table) ⇓ σ[lhs ↦ v]
```

Arms are scanned top-to-bottom; the **first** matching arm wins. A `Wild` arm matches any discriminant value, including `X`. (If you place `Wild` first, every later arm is unreachable.)

If no arm matches — including the `X`-discriminant case where no `Wild` is present — the result is a fully-`X` value of `lhs`'s kind.

## Pre-conditions

- All non-`Wild` discriminator slots have the same kind as the `discriminant`.
- All arm-result slots have the same kind, which is `lhs`'s kind.

## Post-conditions

- `lhs` is bound to either the value of the first matching arm or `dont_care(kind(lhs))` if none match.

## Notes

- The front-end normally emits a `Wild` arm at the end of `table` to make the no-match case unreachable. Direct hand-built Case opcodes can omit it; the `dont_care` rule handles that case.
- The VM reads the discriminator slots of preceding arms (top-to-bottom) until the first match; once an arm matches, only that arm's *value* slot is read, not subsequent arms' values. This is asymmetric with `Select`, where both the true and false arms' values are read on every call. The asymmetry follows from the implementation in `vm.rs`. Each arm's discriminator and value slot must be bound by the time `Case` runs; well-typed RHIF + def-before-use guarantees this.
- The order of discriminator-slot reads is well-defined: top-to-bottom until the first match. The implementation in `vm.rs` actually evaluates the discriminator for each arm via `find` over the table, which terminates at the first match.

## Examples

```
// match q.state {
//     SlaveState::Idle => v0,
//     SlaveState::Receiving => v1,
//     _ => v_default,
// }
Case(lhs, q.state, [
    (Slot(idle_lit),       v0),
    (Slot(receiving_lit),  v1),
    (Wild,                 v_default),
])
```

## Lowering

`Case` lowers to a multiplexer tree in RTL. The discriminator slots become equality-comparisons; the result is a chain of priority-encoded selects.

## Cross-references

- `Case` and `CaseArgument` in `spec.rs`.
- `vm.rs::execute_block` for the matching algorithm.
- See also [`select.md`](./select.md) for the 2-way variant.
