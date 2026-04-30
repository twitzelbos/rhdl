# `Unary(op, lhs, arg1)`

Unary ALU operation. Thirteen `AluUnary` variants cover bit-twiddling, sign manipulation, and width-changing operations.

## Syntax

```
Unary { op: AluUnary, lhs: Slot, arg1: Slot }
```

## `AluUnary` variants

| Variant | Symbol | Result kind | Notes |
|---|---|---|---|
| `Not`        | `!a` | same as `a` | bitwise complement |
| `Neg`        | `-a` | same as `a` | two's-complement; only on `Signed(N)` |
| `All`        | `a.all()` | `Bits(1)` | reduction-AND |
| `Any`        | `a.any()` | `Bits(1)` | reduction-OR |
| `Xor`        | `a.xor()` | `Bits(1)` | reduction-XOR |
| `Signed`     | `a.as_signed()` | `s<N>` if `a : b<N>` | reinterpret |
| `Unsigned`   | `a.as_unsigned()` | `b<N>` if `a : s<N>` | reinterpret |
| `Val`        | `a.val()` | strip a `Signal` layer | only on `Signal(T, _)` |
| `XExt(d)`    | `a.xext::<d>()` | `b<N+d>` or `s<N+d>` | zero/sign-extend |
| `XShl(d)`    | `a << d` (widening) | `b<N+d>` or `s<N+d>` | exact left shift |
| `XShr(d)`    | `a >> d` (narrowing) | `b<max(N-d, 0)>` or signed | exact right shift |
| `XNeg`       | `a.xneg()` | `s<N+1>` | extend by 1, then negate, always signed |
| `XSgn`       | `a.xsgn()` | `s<N+1>` | extend by 1, reinterpret as signed |

## Type rule

```
Γ ⊢ a : T   Γ ⊢ lhs : T'
————————————————————————————
Γ ⊢ Unary(op, lhs, a) : ok
```

Where `T'` depends on `op` per the table above. Mismatched recorded `lhs` kinds are a typing error.

`Signal<T, C>` operands: most ops lift through `Signal` — the inner kind is operated on, the outer `Signal(_, C)` is preserved on the result. The exception is `Val`, which strips `Signal`.

## Dynamic semantics

```
read(σ, a) = v   unary(op, v) = v'
————————————————————————————————————
σ ⊢ Unary(op, lhs, a) ⇓ σ[lhs ↦ v']
```

The implementation lives in `runtime_ops::unary`. Highlights:

- **`Not`** flips every bit (`X` → `X`, `0` ↔ `1`).
- **`Neg`** computes two's-complement `-v`. The most-negative `Signed` value negated wraps to itself.
- **`All`** returns `1` iff every bit of `v` is `1`; `0` if any bit is `0`; `X` if any bit is `X` and no bit is `0`.
- **`Any`** dual of `All`: `1` if any bit is `1`; `0` if all bits are `0`; `X` if any bit is `X` and no bit is `1`.
- **`Xor`** XOR-reduces all bits. `X` propagates.
- **`Signed`/`Unsigned`** are re-interpretations: bits unchanged, kind flipped.
- **`Val`** unwraps `Signal(T, C)` to `T`. Bits unchanged.
- **`XExt(d)`** zero-extends a `Bits(N)` to `Bits(N+d)`, or sign-extends a `Signed(N)` to `Signed(N+d)`.
- **`XShl(d)`** left-shifts `v` by `d`, widening to `N+d` bits so the shift is exact (no overflow).
- **`XShr(d)`** right-shifts `v` by `d`, narrowing to `N-d` bits (saturated at 0). Arithmetic on `Signed`, logical on `Bits`.
- **`XNeg`** = `XExt(1)` then `as_signed()` (if needed) then `Neg`. Always returns `Signed(N+1)`.
- **`XSgn`** = `XExt(1)` then `as_signed()`. Always returns `Signed(N+1)`.

## Pre-conditions

- `Neg` requires `arg1` to be `Signed(N)`. Negation of `Bits(N)` is rejected.
- `Signed` requires `arg1` to be `Bits(N)`; `Unsigned` requires `Signed(N)`.
- `Val` requires `arg1` to be `Signal(T, C)` for some `T, C`.
- `XShr(d)` with `d > N` returns `Bits(0)` / `Signed(0)`, which is the empty bit-vector.

## Post-conditions

- `lhs` is bound to the unary result, with the kind given by the type rule.

## Cross-references

- `Unary` and `AluUnary` in `spec.rs`.
- `runtime_ops::unary` for per-variant implementation.
- `TypedBits::xext`, `xshl`, `xshr`, `val`, `as_signed`, `as_unsigned` etc. in `crates/rhdl-core/src/types/typed_bits.rs`.
