# `Retime(lhs, arg, color)`

Tag (or re-tag) a value with a clock-domain `Color`. The kernel-level analogue of `signal::<Color>(v)`.

## Syntax

```
Retime { lhs: Slot, arg: Slot, color: Option<Color> }
```

`color = None` is permitted in early IR; passes resolve it to `Some(_)` before lowering. Reaching the lowering phase with `None` is an ICE.

## Type rule

For `color = Some(C)`:

```
Γ ⊢ arg : T   T is not Signal(_, _)    -- no nested signals
Γ ⊢ lhs : Signal(T, C)
————————————————————————————————————————————————
Γ ⊢ Retime(lhs, arg, Some(C)) : ok
```

`Retime` does not strip a `Signal`; it adds one. To strip, use `Index(_, [SignalValue])` or `Unary(Val, _)`.

## Dynamic semantics

```
read(σ, arg) = v
v' = TypedBits {
    bits: v.bits,
    kind: Signal(v.kind, color),
}
————————————————————————————————————
σ ⊢ Retime(lhs, arg, Some(color)) ⇓ σ[lhs ↦ v']
```

Bits are unchanged; only the kind acquires (or replaces) a `Signal` wrapper.

For `color = None`, the runtime falls through and `lhs` gets `arg`'s value unchanged. This branch should not appear at runtime in well-typed RHIF.

## Pre-conditions

- `arg`'s kind is not itself `Signal(_, _)` — i.e., RHIF does not allow `Signal<Signal<T, C₁>, C₂>`.
- `color = Some(_)` by the time the VM runs the op.

## Post-conditions

- `lhs` is bound to a value of kind `Signal(kind(arg), color)`.

## Why this exists

Clock-domain mixing is a *type system* check. Two slots with kinds `Signal<T, Red>` and `Signal<T, Blue>` cannot directly be combined by an arithmetic opcode — the `Color`s differ. `Retime` is the only opcode that legitimately changes the colour, and it represents either:

- An explicit synchroniser (the source-language `Sync1Bit`-style constructs lower to `Retime` after their internal flip-flop chain).
- A boundary crossing where the developer has documented a synchronisation strategy.

The compiler does not check that a `Retime` corresponds to a real synchroniser — that responsibility is the developer's. The compiler enforces only that *colour mixing happens via* `Retime`, not by accident.

## Cross-references

- `Retime` in `spec.rs`.
- `Color` in `crates/rhdl-core/src/types/domain.rs`.
- For the surrounding clock / reset model, see [`reset-clock.md`](../reset-clock.md).
