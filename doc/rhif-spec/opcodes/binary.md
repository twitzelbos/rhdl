# `Binary(op, lhs, arg1, arg2)`

Binary ALU operation. Sixteen `AluBinary` variants cover arithmetic, bitwise, comparison, and widening forms.

## Syntax

```
Binary { op: AluBinary, lhs: Slot, arg1: Slot, arg2: Slot }
```

## `AluBinary` variants

| Variant | Symbol | Result kind | Notes |
|---|---|---|---|
| `Add` | `a + b` | same as operands | wraps mod 2^N |
| `Sub` | `a - b` | same as operands | wraps mod 2^N |
| `Mul` | `a * b` | same as operands | wraps; same-signedness required |
| `BitXor` | `a ^ b` | same as operands | bitwise |
| `BitAnd` | `a & b` | same as operands | bitwise |
| `BitOr`  | `a \| b` | same as operands | bitwise |
| `Shl` | `a << b` | same as `a` | logical shift |
| `Shr` | `a >> b` | same as `a` | arithmetic on `Signed`, logical on `Bits` |
| `Eq` | `a == b` | `Bits(1)` | |
| `Ne` | `a != b` | `Bits(1)` | |
| `Lt` | `a < b` | `Bits(1)` | sign-aware |
| `Le` | `a <= b` | `Bits(1)` | sign-aware |
| `Gt` | `a > b` | `Bits(1)` | sign-aware |
| `Ge` | `a >= b` | `Bits(1)` | sign-aware |
| `XAdd` | `a +x b` | width `max(N₁,N₂)+1` | exact, no wrap |
| `XSub` | `a -x b` | width `max(N₁,N₂)+1` | exact, no wrap |
| `XMul` | `a *x b` | width `N₁+N₂` | exact, no wrap |

## Type rule

For arithmetic, bitwise, and shift ops (`op` not a comparison and not in `{XAdd, XSub, XMul}`):

```
Γ ⊢ a₁ : T   Γ ⊢ a₂ : T   Γ ⊢ lhs : T
where T ∈ {Bits(N), Signed(N), Signal(Bits(N), C), Signal(Signed(N), C)}
————————————————————————————————————————————————————————————————————————
Γ ⊢ Binary(op, lhs, a₁, a₂) : ok
```

For shifts, the shift-amount kind may differ from `T` (any `Bits(M)` or `Signed(M)`); the result kind matches `a₁`.

For comparisons:

```
Γ ⊢ a₁ : T   Γ ⊢ a₂ : T   Γ ⊢ lhs : Bits(1)   (or Signal(Bits(1), C) when T = Signal(_, C))
————————————————————————————————————————————————————————————————————————————————————————————
Γ ⊢ Binary(op, lhs, a₁, a₂) : ok
```

For `XAdd` / `XSub`:

```
Γ ⊢ a₁ : T₁   Γ ⊢ a₂ : T₂   T₁, T₂ same signedness
Γ ⊢ lhs : (Bits(max(N₁, N₂)+1) if unsigned else Signed(max(N₁, N₂)+1))
————————————————————————————————————————————————————————————————————————
Γ ⊢ Binary(XAdd | XSub, lhs, a₁, a₂) : ok
```

For `XMul`:

```
Γ ⊢ a₁ : T₁   Γ ⊢ a₂ : T₂   T₁, T₂ same signedness
Γ ⊢ lhs : (Bits(N₁+N₂) if unsigned else Signed(N₁+N₂))
————————————————————————————————————————————————————————————————————————
Γ ⊢ Binary(XMul, lhs, a₁, a₂) : ok
```

In every variant, mixing `Bits` and `Signed` is rejected (statically — and dynamically by `runtime_ops` if it slips through).

## Dynamic semantics

```
read(σ, a₁) = v₁   read(σ, a₂) = v₂   binary(op, v₁, v₂) = v
————————————————————————————————————————————————————————————————
σ ⊢ Binary(op, lhs, a₁, a₂) ⇓ σ[lhs ↦ v]
```

- **`Add`, `Sub`, `Mul`** wrap modulo 2^N (two's-complement on `Signed`). Defined for both kinds.
- **`Shl`** shifts left, filling LSBs with `0`. The shift amount is read as `usize`; shifts ≥ N are implementation-defined (in the VM, the high bits drop off normally; in synthesis, may saturate).
- **`Shr`** shifts right. On `Bits`, fills MSBs with `0` (logical); on `Signed`, fills MSBs with the sign bit (arithmetic).
- **Comparisons** produce `Bits(1)` with `0` for false, `1` for true. `Lt`, `Le`, `Gt`, `Ge` interpret operands per their signedness.
- **`XAdd`, `XSub`, `XMul`** widen operands to the result kind, perform exact arithmetic via `BigInt` (see `runtime_ops::xadd` / `xsub` / `xmul`), and never wrap.

`X`-propagation: any `X` bit in either operand makes the entire result fully-`X` for arithmetic / comparison opcodes; bitwise opcodes propagate per-bit per the standard 4-state rules.

## Pre-conditions

- Operand kinds match the type rule.
- For `Mul`, `XMul`: operand kinds have the same signedness.
- For shifts: the shift amount is non-negative when interpreted as integer; otherwise behaviour is implementation-defined.

## Post-conditions

- `lhs` is bound to the result of the operation, with the kind given by the type rule.

## Lowering

Each `Binary` opcode lowers to one or more RTL `Binary` operations of the same flavour, plus extension ops if needed (e.g. `XAdd` lowers to a wider RTL adder; `Mul` lowers to a multiplier primitive). See [`invariants/lowering.md`](../invariants/lowering.md) and `crates/rhdl-core/src/rtl/spec.rs`.

## Cross-references

- `Binary` and `AluBinary` in `spec.rs`.
- `runtime_ops::binary` for the per-variant implementation.
- `mul`, `xadd`, `xsub`, `xmul` private fns in `runtime_ops.rs` for the widening / signed-aware paths.
