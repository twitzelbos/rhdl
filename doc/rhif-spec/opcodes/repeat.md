# `Repeat(lhs, value, len)`

Construct an array by repeating a value `n` times: `[v; n]`.

## Syntax

```
Repeat { lhs: Slot, value: Slot, len: u64 }
```

`len` is a static `u64`, not a slot. Length must be known at compile time.

## Type rule

```
Γ ⊢ value : T   Γ ⊢ lhs : Array(T, n)
————————————————————————————————————————
Γ ⊢ Repeat(lhs, value, n) : ok
```

## Dynamic semantics

```
read(σ, value) = v
v.repeat(n) = v_arr     -- n copies of v concatenated, kind = Array(kind(v), n)
————————————————————————————————————————————————————————————————————————————
σ ⊢ Repeat(lhs, value, n) ⇓ σ[lhs ↦ v_arr]
```

`v.repeat(n)` is `crates/rhdl-core/src/types/typed_bits.rs::repeat`.

## Pre-conditions

- `n > 0` (an `Array(T, 0)` is ill-formed; `n = 0` is rejected by upstream layers).

## Post-conditions

- `lhs` is bound to an array of `n` copies of `read(σ, value)`.

## Examples

```
// [bits::<8>(0); 64] in source
Repeat(buf, zero_byte, 64)   // buf : [Bits(8); 64]

// [false; 8]
Repeat(coils, false_lit, 8)  // coils : [Bits(1); 8]
```

## Lowering

`Repeat` lowers to RTL by emitting `n` copies of the bit-pattern in sequence; the resulting wire bundle is the array.

## Cross-references

- `Repeat` in `spec.rs`.
- `TypedBits::repeat` in `crates/rhdl-core/src/types/typed_bits.rs`.
- For arrays where the elements differ, see [`array.md`](./array.md).
