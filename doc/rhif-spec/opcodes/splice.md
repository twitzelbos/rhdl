# `Splice(lhs, orig, path, subst)`

Functional update: produce a fresh aggregate equal to `orig` everywhere except at `path`, where the sub-value is replaced by `subst`. The dual of `Index`.

## Syntax

```
Splice { lhs: Slot, orig: Slot, path: Path, subst: Slot }
```

The field is named `orig` in the spec but the source-language convention is "the original value." Older code may also call it `rhs`. The semantics are unchanged.

## Type rule

```
Γ ⊢ orig : T   (T, p) ⇒ T'   Γ ⊢ subst : T'   Γ ⊢ lhs : T
∀ DynamicIndex(s) ∈ p. Γ ⊢ s : Bits(M) ∨ Signed(M)
————————————————————————————————————————————————————————————
Γ ⊢ Splice(lhs, orig, p, subst) : ok
```

The path walk yields the kind that `subst` must match; the result kind matches `orig`.

## Dynamic semantics

```
read(σ, orig) = v_o
∀ DynamicIndex(s) ∈ p. read(σ, s) = vᵢ; vᵢ.as_i64() = nᵢ
p_static = p with each DynamicIndex(s) replaced by Index(nᵢ)
read(σ, subst) = v_s
v_o.splice(p_static, v_s) = v'
———————————————————————————————————————————————————————————————
σ ⊢ Splice(lhs, orig, p, path, subst) ⇓ σ[lhs ↦ v']
```

`v_o.splice(p_static, v_s)` is implemented in `crates/rhdl-core/src/types/typed_bits.rs::splice`. It is purely functional — `v_o` is unchanged; the result is a fresh `TypedBits`.

## Pre-conditions

- The path walks to a kind compatible with `subst`.
- For dynamic indices, the resolved index is in range (`< n` for an array of length `n`); out-of-range behaviour is implementation-defined.

## Post-conditions

- `lhs` is bound to a fresh aggregate of the same kind as `orig`, with the sub-value at `p` replaced.

## Examples

```
// Set a struct field
Splice(lhs, s, [Field("data")], v)
   // lhs : MyStruct equal to s but with .data = v

// Set an array element (constant index)
Splice(lhs, s, [Index(3)], v)
   // lhs : [T; N] equal to s but with [3] = v

// Set an array element (dynamic index)
Splice(lhs, s, [DynamicIndex(idx)], v)

// Set a tuple field
Splice(lhs, s, [TupleIndex(0)], v)

// Set the payload of an enum variant
Splice(lhs, s, [EnumPayload("Variant"), Field("field")], v)
```

## Idiom: register-update on writes

The canonical write to a register file is:

```rust
d.regs[i.write_addr] = i.write_data;   // source-language
```

Lowers to:

```
Splice(d.regs, q.regs, [DynamicIndex(i.write_addr)], i.write_data)
```

(Plus surrounding logic to gate the splice on `write_enable`.) This is why `register_file.rs` works without per-element flip-flop selection.

## Lowering

`Splice` lowers to RTL `Splice` (the static-path case) or to a multiplexer + write enable (the dynamic-index case). The bit-level result of `Splice(orig, p, subst)` is `orig` with the bit-range corresponding to `p` replaced by the bits of `subst`.

## Cross-references

- `Splice` in `spec.rs`.
- `TypedBits::splice` in `crates/rhdl-core/src/types/typed_bits.rs`.
- For the dual (read instead of write), see [`index.md`](./index.md).
