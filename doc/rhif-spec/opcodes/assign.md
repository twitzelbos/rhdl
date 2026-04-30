# `Assign(lhs, rhs)`

Copy a value from one slot to another. Pure data movement; no transformation.

## Syntax

```
Assign { lhs: Slot, rhs: Slot }
```

## Type rule

```
Γ ⊢ rhs : T   Γ ⊢ lhs : T
————————————————————————————
Γ ⊢ Assign(lhs, rhs) : ok
```

`lhs` and `rhs` must have identical kinds; there is no implicit cast.

## Dynamic semantics

```
read(σ, rhs) = v
———————————————————————————————————
σ ⊢ Assign(lhs, rhs) ⇓ σ[lhs ↦ v]
```

## Pre-conditions

- `kind(lhs) ≡ kind(rhs)`.

## Post-conditions

- `lhs` is bound to the value of `rhs` at the time the opcode runs.

## When this op appears

- After a pass introduces a temporary that aliases another slot.
- As part of phi-merging-style transformations.
- When the front-end emits an explicit copy (rare; usually the IR-builder folds these).

## Optimisation note

Most `Assign`s are removed by the constant-propagation / copy-propagation passes before lowering. Surviving `Assign`s lower to a degenerate RTL copy (one wire to another) that the synthesis backend optimises out.

## Cross-references

- `Assign` in `spec.rs`.
- VM dispatch in `vm.rs::execute_block`.
- The constant-propagation pass in `crates/rhdl-core/src/compiler/rhif_passes/`.
