# `Resize(lhs, arg, len)`

Width-change a value while preserving its signedness. Equivalent to `arg.resize::<N>()` in source.

## Syntax

```
Resize(Cast { lhs: Slot, arg: Slot, len: Option<usize> })
```

`len = None` is permitted in early IR; the VM rejects `None` as an ICE.

## Type rule

```
Γ ⊢ arg : T_arg   T_arg ∈ {Bits(M), Signed(M)}
len = Some(n)
Γ ⊢ lhs : T_result
where T_result = Bits(n) if T_arg = Bits(_)
              or Signed(n) if T_arg = Signed(_)
————————————————————————————————————————————————————
Γ ⊢ Resize(lhs, arg, len) : ok
```

The result keeps the same signedness as the input. Resizing a `Bits` produces `Bits`; resizing a `Signed` produces `Signed`.

## Dynamic semantics

```
read(σ, arg) = v   v.resize(n) = v'
————————————————————————————————————————————
σ ⊢ Resize(lhs, arg, Some(n)) ⇓ σ[lhs ↦ v']
```

`v.resize(n)`:
- For `Bits(M)`: equivalent to `unsigned_cast(n)` — zero-extend or truncate.
- For `Signed(M)`: equivalent to `signed_cast(n)` — sign-extend or truncate.

## Pre-conditions

- `arg`'s kind is `Bits(_)` or `Signed(_)`.
- `len = Some(n)` at execution time.

## Post-conditions

- `lhs` is bound to a value of kind `Bits(n)` or `Signed(n)`, matching the signedness of `arg`.

## Examples

```
// b16 → b8 (truncate)
Resize(lhs, big, Some(8))           // lhs : Bits(8)

// b8 → b16 (zero-extend)
Resize(lhs, small, Some(16))        // lhs : Bits(16)

// s8 → s16 (sign-extend)
Resize(lhs, small_s, Some(16))      // lhs : Signed(16)
```

## When to use `Resize` vs `AsBits` / `AsSigned`

- **`Resize`**: width change, keep signedness. The "ergonomic" cast in source.
- **`AsBits`**: width change, force unsigned.
- **`AsSigned`**: width change, force signed.

`Resize` is the most common cast in widget code; `AsBits` and `AsSigned` are needed only when changing signedness.

## Cross-references

- `Resize` in `spec.rs`.
- `TypedBits::resize` for the implementation.
- See also [`as_bits.md`](./as_bits.md), [`as_signed.md`](./as_signed.md).
