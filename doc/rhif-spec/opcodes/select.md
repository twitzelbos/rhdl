# `Select(lhs, cond, true_value, false_value)`

3-input multiplexer. The classical hardware `if/else`-as-mux. Both arms are evaluated (their slots are read); the condition picks which value lands in `lhs`.

## Syntax

```
Select { lhs: Slot, cond: Slot, true_value: Slot, false_value: Slot }
```

## Type rule

```
Γ ⊢ cond : Bits(1) (or Signal(Bits(1), C))
Γ ⊢ true_value : T   Γ ⊢ false_value : T   Γ ⊢ lhs : T
————————————————————————————————————————————————————————
Γ ⊢ Select(lhs, cond, true_value, false_value) : ok
```

`true_value` and `false_value` must have identical kinds; `lhs` matches them. `cond` must be a 1-bit `Bits` (optionally wrapped in `Signal<_, C>`).

`Signal` propagation: when `true_value` and `false_value` are `Signal<T, C>`, the result is `Signal<T, C>`. Mixing colours is a type error.

## Dynamic semantics

```
read(σ, cond) = v_c   read(σ, true_value) = v_t   read(σ, false_value) = v_f

  v_c.bits[0] = One   ⇒ v = v_t
  v_c.bits[0] = Zero  ⇒ v = v_f
  v_c.bits[0] = X     ⇒ v = dont_care(kind(v_t))
————————————————————————————————————————————————————————
σ ⊢ Select(lhs, cond, true_value, false_value) ⇓ σ[lhs ↦ v]
```

The condition is examined bit-by-bit; only bit 0 determines the choice. The `X` rule is conservative: an undefined condition produces a fully-`X` result, never an arbitrary one of the two values. This matches iverilog's 4-state behaviour for `cond ? a : b` with `cond` being `X`.

## Pre-conditions

- `kind(true_value) ≡ kind(false_value)`.
- `kind(cond)` is `Bits(1)` (possibly under a `Signal` wrapper).

## Post-conditions

- `lhs` is bound to one of the two inputs (or fully-`X` of the input kind, when `cond` is `X`).

## Notes

- Both arms always "execute" in the kernel sense — both `true_value` and `false_value` are read. This is the canonical reason that source-level guards in a `#[kernel]` body can't use partial-evaluation: every arm of an `if`/`else` evaluates regardless of the condition. The user must clamp inputs that would otherwise trip checks (e.g., shift bounds) at the operand level, not just at the result level. (See `barrel_shifter` widget for the canonical example.)
- The mux is two-input; multi-way selection lowers to a chain of `Select`s or a `Case` opcode.

## Cross-references

- `Select` in `spec.rs`.
- VM dispatch in `vm.rs::execute_block`.
- For multi-way selection, see [`case.md`](./case.md).
