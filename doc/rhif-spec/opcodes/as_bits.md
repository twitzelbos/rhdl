# `AsBits(lhs, arg, len)`

Cast a value to `Bits<N>`. Equivalent to `arg as Bits<N>` in source.

## Syntax

```
AsBits(Cast { lhs: Slot, arg: Slot, len: Option<usize> })
```

`len = None` is permitted in early IR (the front-end may emit `None` until inference resolves it); the VM rejects `None` as an ICE.

## Type rule

```
Γ ⊢ arg : T_arg   T_arg ∈ {Bits(M), Signed(M)}
len = Some(n)
Γ ⊢ lhs : Bits(n)
————————————————————————————————————————————————
Γ ⊢ AsBits(lhs, arg, len) : ok
```

The argument's signedness does not constrain the result — `AsBits` always produces unsigned. Width may differ from the source.

## Dynamic semantics

```
read(σ, arg) = v   v.unsigned_cast(n) = v'
————————————————————————————————————————————
σ ⊢ AsBits(lhs, arg, Some(n)) ⇓ σ[lhs ↦ v']
```

`unsigned_cast(n)`:
- If `v.len() < n`: zero-extend.
- If `v.len() > n`: truncate to the low `n` bits.
- If `v.len() = n`: reinterpret bits as unsigned (drops a `Signed` wrapper).

## Pre-conditions

- `arg`'s kind is `Bits(_)` or `Signed(_)`.
- `len = Some(n)` at execution time. (`None` is an ICE per `vm.rs::OpCode::AsBits` — `BitCastMissingRequiredLength`.)

## Post-conditions

- `lhs` is bound to a `Bits(n)` value.

## Examples

```
// (signed_x as bits::<8>)
AsBits(lhs, signed_x, Some(8))    // lhs : Bits(8)

// Truncate from b16 to b8
AsBits(lhs, big, Some(8))         // lhs : Bits(8)

// Widen from b4 to b8 (zero-extend)
AsBits(lhs, small, Some(8))       // lhs : Bits(8)
```

## Cross-references

- `AsBits` in `spec.rs` (it shares the `Cast` struct with `AsSigned` and `Resize`).
- `TypedBits::unsigned_cast` for the implementation.
- See also [`as_signed.md`](./as_signed.md) and [`resize.md`](./resize.md).
