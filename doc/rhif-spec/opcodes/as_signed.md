# `AsSigned(lhs, arg, len)`

Cast a value to `SignedBits<N>`. Equivalent to `arg as SignedBits<N>` in source.

## Syntax

```
AsSigned(Cast { lhs: Slot, arg: Slot, len: Option<usize> })
```

`len = None` is permitted in early IR; the VM rejects `None` as an ICE.

## Type rule

```
Γ ⊢ arg : T_arg   T_arg ∈ {Bits(M), Signed(M)}
len = Some(n)
Γ ⊢ lhs : Signed(n)
————————————————————————————————————————————————
Γ ⊢ AsSigned(lhs, arg, len) : ok
```

The argument's signedness does not constrain the result — `AsSigned` always produces signed. Width may differ.

## Dynamic semantics

```
read(σ, arg) = v   v.signed_cast(n) = v'
————————————————————————————————————————————
σ ⊢ AsSigned(lhs, arg, Some(n)) ⇓ σ[lhs ↦ v']
```

`signed_cast(n)`:
- If `v` is `Bits(M)` and `M = n`: reinterpret as `Signed(n)` — bits unchanged, kind flipped.
- If `v` is `Bits(M)` and `M < n`: zero-extend to `Bits(n)`, then reinterpret. (Note: source-level `as SignedBits<N>` from a wider unsigned would zero-extend; from a narrower unsigned, the high bit is `0`.)
- If `v` is `Bits(M)` and `M > n`: truncate to `Bits(n)`, then reinterpret. The MSB of the truncated value becomes the sign bit.
- If `v` is `Signed(M)` and `M = n`: identity.
- If `v` is `Signed(M)` and `M < n`: sign-extend.
- If `v` is `Signed(M)` and `M > n`: truncate (overflow becomes implementation-defined per two's-complement wrap).

## Pre-conditions

- `arg`'s kind is `Bits(_)` or `Signed(_)`.
- `len = Some(n)` at execution time.

## Post-conditions

- `lhs` is bound to a `Signed(n)` value.

## Examples

```
// (b: bits::<8>) as signed::<8>
AsSigned(lhs, b, Some(8))     // lhs : Signed(8); same bits, signed kind

// Sign-extend a Signed(4) to Signed(8)
AsSigned(lhs, small_signed, Some(8))
```

## Cross-references

- `AsSigned` in `spec.rs`.
- `TypedBits::signed_cast` for the implementation.
- See also [`as_bits.md`](./as_bits.md) (the unsigned counterpart) and [`resize.md`](./resize.md) (signedness-preserving width change).
