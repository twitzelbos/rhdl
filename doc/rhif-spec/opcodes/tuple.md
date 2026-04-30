# `Tuple(lhs, fields)`

Construct a tuple `(v₀, v₁, …, vₙ)`.

## Syntax

```
Tuple { lhs: Slot, fields: Vec<Slot> }
```

## Type rule

```
∀ i. Γ ⊢ fᵢ : Tᵢ
Γ ⊢ lhs : Tuple([T₀, T₁, …, Tₙ])
———————————————————————————————————————
Γ ⊢ Tuple(lhs, [f₀, …, fₙ]) : ok
```

## Dynamic semantics

```
∀ i. read(σ, fᵢ) = vᵢ
v = tuple([v₀, …, vₙ])    -- bits concatenated in order, kind = Tuple([T₀, …, Tₙ])
———————————————————————————————————————————————————————————————————————————————
σ ⊢ Tuple(lhs, [f₀, …, fₙ]) ⇓ σ[lhs ↦ v]
```

`tuple` is the helper in `runtime_ops.rs`.

## Pre-conditions

- `fields` may be empty (producing the unit `()`); `Tuple([])` is well-typed and produces a zero-width value.

## Post-conditions

- `lhs` is bound to a tuple value of kind `Tuple([T₀, …, Tₙ])` whose components are the read values, in order.

## Examples

```
// (a, b)
Tuple(lhs, [a, b])    // lhs : (T_a, T_b)

// ()
Tuple(lhs, [])        // lhs : ()
```

## Lowering

`Tuple` lowers to bit-concatenation — the result is the bit-representation of `f₀` followed by `f₁`, etc. No logic, just wires.

## Cross-references

- `Tuple` in `spec.rs`.
- `runtime_ops::tuple` for the implementation.
- See also [`array.md`](./array.md) for homogeneous-element aggregates.
