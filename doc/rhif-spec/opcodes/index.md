# `Index(lhs, arg, path)`

Pick out a sub-value from an aggregate. Reads `arg` and walks `path` through it; the resulting sub-value is bound to `lhs`. The hardware analogue is wire-routing — `Index` does not synthesise to logic, only to wires (with the exception of `DynamicIndex`, which becomes a multiplexer).

## Syntax

```
Index { lhs: Slot, arg: Slot, path: Path }
```

## Type rule

```
Γ ⊢ arg : T   (T, p) ⇒ T'   Γ ⊢ lhs : T'
∀ DynamicIndex(s) ∈ p. Γ ⊢ s : Bits(M) ∨ Signed(M)
———————————————————————————————————————————————————
Γ ⊢ Index(lhs, arg, p) : ok
```

Where `(T, p) ⇒ T'` is the path-walking judgement defined in [`type-system.md`](../type-system.md#path-typing).

The path-walking rules in summary:

| `PathElement` | Walks | Required `T`-shape |
|---|---|---|
| `Index(k)` | array element `k` | `Array(_, n)`, `k < n` |
| `TupleIndex(k)` | tuple field `k` | `Tuple(_)`, `k ≤ n` |
| `Field(name)` | struct field | `Struct(_)` with that field |
| `EnumDiscriminant` | the discriminant bits | `Enum(_)` |
| `EnumPayload(name)` | the variant's payload | `Enum(_)` with that variant |
| `EnumPayloadByValue(k)` | payload of variant w/ disc `k` | `Enum(_)` |
| `DynamicIndex(s)` | element `read(s)` of an array | `Array(_, n)`, `s : integer-like` |
| `SignalValue` | inner value | `Signal(_, _)` |

## Dynamic semantics

```
read(σ, arg) = v
∀ DynamicIndex(s) ∈ p. read(σ, s) = vᵢ; vᵢ.as_i64() = nᵢ
p_static = p with each DynamicIndex(s) replaced by Index(nᵢ)
v.path(p_static) = v'
————————————————————————————————————————————————————————————
σ ⊢ Index(lhs, arg, p) ⇓ σ[lhs ↦ v']
```

The path is resolved cycle-by-cycle: dynamic-index slots are read first to produce an integer, the static-form path is constructed, then the typed-bits path-walk yields the sub-value.

`v.path(p_static)` is implemented in `crates/rhdl-core/src/types/typed_bits.rs::path`. It is purely functional; it never mutates `v`.

## Pre-conditions

- The path is well-typed against the aggregate kind.
- For `DynamicIndex(s)`, `read(σ, s)` evaluates to a non-negative integer that, when converted to `usize`, is `< n` (the array length). Out-of-range dynamic indices are implementation-defined: the simulator currently panics; synthesised hardware produces an undefined value.

## Post-conditions

- `lhs` is bound to the sub-value produced by the path walk.

## Examples

```
// Static field access
Index(lhs, s, [Field("data")])      // s : MyStruct, lhs : kind_of(MyStruct, "data")

// Tuple index
Index(lhs, s, [TupleIndex(0)])      // s : (T1, T2), lhs : T1

// Array index, constant
Index(lhs, s, [Index(3)])           // s : [T; N], lhs : T (requires 3 < N)

// Array index, dynamic
Index(lhs, s, [DynamicIndex(idx)])  // s : [T; N], idx : Bits(M); lhs : T

// Enum discriminant
Index(lhs, s, [EnumDiscriminant])   // s : MyEnum, lhs : Bits(d) where d = disc width

// Compound path: pull `field` out of element 5
Index(lhs, s, [Index(5), Field("field")])

// Strip a Signal layer
Index(lhs, s, [SignalValue])        // s : Signal(T, C), lhs : T
```

## Lowering

`Index` lowers to RTL wire-routing for the static-path case (no logic, just wire selection). For `DynamicIndex`, it lowers to an N-input multiplexer keyed on the dynamic slot. See [`invariants/lowering.md`](../invariants/lowering.md).

## Cross-references

- `Index` in `spec.rs`.
- `Path` and `PathElement` in `crates/rhdl-core/src/types/path.rs`.
- `TypedBits::path` in `crates/rhdl-core/src/types/typed_bits.rs`.
- For functional-update (the dual operation), see [`splice.md`](./splice.md).
