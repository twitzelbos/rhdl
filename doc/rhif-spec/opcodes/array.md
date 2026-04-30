# `Array(lhs, elements)`

Construct an array from an explicit element list `[v₀, v₁, …, vₙ]`.

## Syntax

```
Array { lhs: Slot, elements: Vec<Slot> }
```

## Type rule

```
∀ i. Γ ⊢ eᵢ : T
Γ ⊢ lhs : Array(T, |elements|)
————————————————————————————————————————
Γ ⊢ Array(lhs, [e₀, …, eₙ]) : ok
```

All elements must have the same kind `T`. The result kind is `Array(T, n+1)` for `n+1` elements.

## Dynamic semantics

```
∀ i. read(σ, eᵢ) = vᵢ
v = array([v₀, …, vₙ])    -- bits concatenated, kind = Array(kind(v₀), n+1)
————————————————————————————————————————————————————————————————————————
σ ⊢ Array(lhs, [e₀, …, eₙ]) ⇓ σ[lhs ↦ v]
```

The `array` helper in `runtime_ops.rs` concatenates the per-element bit-vectors and derives the array kind from the first element's kind.

## Pre-conditions

- `elements` is non-empty (an empty `Array(_, 0)` is not produced by the front-end and would have ambiguous kind).
- All elements have the same kind.

## Post-conditions

- `lhs` is bound to an array of `|elements|` items, in order.

## Examples

```
// Source: [a, b, c, d]
Array(lhs, [a, b, c, d])    // lhs : [T; 4]
```

## Comparison with `Repeat`

| | `Array` | `Repeat` |
|---|---|---|
| Element source | One slot per element | One slot, replicated |
| Element values | May differ | All identical |
| Length | From `elements.len()` | From `len: u64` |

For arrays where most elements come from a uniform source (e.g., per-register-file initial values that all start at 0), `Repeat` is more concise. For arrays whose elements come from different parts of the IR, `Array` is required.

## Lowering

`Array` lowers to bit-concatenation in RTL — the result is the bit-representation of `e₀` followed by `e₁`, etc.

## Cross-references

- `Array` in `spec.rs`.
- `runtime_ops::array` for the implementation.
- See also [`repeat.md`](./repeat.md) for the homogeneous variant and [`tuple.md`](./tuple.md) for heterogeneous-kind aggregates.
