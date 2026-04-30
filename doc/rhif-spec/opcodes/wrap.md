# `Wrap(op, lhs, arg, kind)`

Construct an `Option<T>` or `Result<T, E>` value. The kernel-level lowering of `Some(x)`, `None`, `Ok(x)`, `Err(x)`.

## Syntax

```
Wrap { op: WrapOp, lhs: Slot, arg: Slot, kind: Option<Kind> }

WrapOp ::= Some | None | Ok | Err
```

`kind = None` is permitted in early IR; the front-end may emit `None` until inference resolves it. The VM rejects `None` as an ICE (per `vm.rs::OpCode::Wrap` — `WrapMissingKind`).

## Type rule

For `op = Some` (or `op = Ok`), with `kind = Some(Option<T>)` (or `Result<T, E>`):

```
Γ ⊢ arg : T   Γ ⊢ lhs : Option<T>     -- (or Result<T, E>)
————————————————————————————————————————————————————————
Γ ⊢ Wrap(Some, lhs, arg, Some(Option<T>)) : ok
```

For `op = None`:

```
Γ ⊢ arg : ()   Γ ⊢ lhs : Option<T>
————————————————————————————————————
Γ ⊢ Wrap(None, lhs, arg, Some(Option<T>)) : ok
```

The `None` constructor still requires an `arg` slot for IR-uniformity; it is `()` (unit) and ignored at runtime.

For `op = Err`:

```
Γ ⊢ arg : E   Γ ⊢ lhs : Result<T, E>
———————————————————————————————————————
Γ ⊢ Wrap(Err, lhs, arg, Some(Result<T, E>)) : ok
```

## Dynamic semantics

```
read(σ, arg) = v
v' = match op with
       Some → v.wrap_some(kind)
       None → v.wrap_none(kind)
       Ok   → v.wrap_ok(kind)
       Err  → v.wrap_err(kind)
————————————————————————————————————————————
σ ⊢ Wrap(op, lhs, arg, Some(kind)) ⇓ σ[lhs ↦ v']
```

The `wrap_*` helpers in `crates/rhdl-core/src/types/typed_bits.rs` produce a `TypedBits` of the requested `Option` / `Result` kind, with the discriminant set to identify the variant and the payload set from `arg` (where applicable).

## Pre-conditions

- `kind = Some(_)` at execution time.
- `arg`'s kind matches the variant being constructed:
  - `Some`: `kind(arg) = T` where `lhs : Option<T>`.
  - `None`: `kind(arg) = ()`.
  - `Ok`: `kind(arg) = T` where `lhs : Result<T, E>`.
  - `Err`: `kind(arg) = E` where `lhs : Result<T, E>`.

## Post-conditions

- `lhs` is bound to an `Option<T>` or `Result<T, E>` value with the selected variant's discriminant and (for `Some`/`Ok`/`Err`) the payload populated from `arg`.

## Why this is its own opcode (and not a special case of `Enum`)

`Option` and `Result` are technically enums and could in principle be constructed with the `Enum` opcode. They are split out for two reasons:

- **Front-end ergonomics.** The proc-macro lowers `Some(x)` in source to `Wrap(Some, …)` directly, without having to resolve which struct kind `Option<T>` corresponds to. The wrap kind is filled in by inference at the type-check phase.
- **Future special-casing.** `Option` / `Result` are common enough that downstream passes can recognise them by opcode and apply specialised optimisations (e.g., the `?` operator desugar). Keeping them syntactically distinct from generic enums keeps that pattern-match cheap.

## Examples

```
// Some(x)
Wrap(Some, lhs, x, Some(Option<T>))

// None
Wrap(None, lhs, unit_lit, Some(Option<T>))

// Ok(x)
Wrap(Ok, lhs, x, Some(Result<T, E>))

// Err(e)
Wrap(Err, lhs, e, Some(Result<T, E>))
```

## Cross-references

- `Wrap` and `WrapOp` in `spec.rs`.
- `TypedBits::wrap_ok`, `wrap_err`, `wrap_some`, `wrap_none` in `crates/rhdl-core/src/types/typed_bits.rs`.
- See also [`enum.md`](./enum.md) for the generic enum constructor.
