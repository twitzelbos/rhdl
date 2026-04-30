# `Struct(lhs, fields, rest, template)`

Construct a struct value. Combines a base value (the `template`, plus optionally a `rest` slot for the spread / functional-update style) with per-field overrides supplied by `fields`.

## Syntax

```
Struct {
    lhs:     Slot,
    fields:  Vec<FieldValue>,         // FieldValue { member: Member, value: Slot }
    rest:    Option<Slot>,            // optional ..rest source
    template: TypedBits,              // constant carrying the struct's Kind
}
```

## Type rule

```
template : Struct(S)
∀ field ∈ fields. Γ ⊢ field.value : kind_of(S, field.member)
rest = Some(r) ⇒ Γ ⊢ r : Struct(S)
Γ ⊢ lhs : Struct(S)
————————————————————————————————————————————————————————————————
Γ ⊢ Struct(lhs, fields, rest, template) : ok
```

The `template` is itself a `TypedBits` value of struct kind; it carries (a) the struct's `Kind`, and (b) the default values for fields that are neither in `fields` nor reachable via `rest`.

## Dynamic semantics

```
v_init = if rest = Some(r) then read(σ, r) else clone(template)
∀ field ∈ fields:
  v_field = read(σ, field.value)
  p = match field.member with
        Named(n)   → Field(n)
        Unnamed(k) → TupleIndex(k)
  v_init = v_init.splice(p, v_field)
————————————————————————————————————————————————————————————————
σ ⊢ Struct(lhs, fields, rest, template) ⇓ σ[lhs ↦ v_init]
```

The order of field splices does not matter for well-typed RHIF — each field corresponds to a distinct path. (The front-end emits each field exactly once.)

## Pre-conditions

- `template.kind()` is a `Struct(S)` kind.
- Every member referenced in `fields` is a member of `S` (named field or tuple-index).
- `rest`, if present, has kind `Struct(S)`.
- Every field's slot kind matches the declared kind of that field in `S`.

## Post-conditions

- `lhs` is bound to a struct of kind `Struct(S)` where every field's value comes from either:
  - the corresponding entry in `fields` (if present), else
  - the corresponding field of `rest` (if `rest = Some(r)`), else
  - the corresponding default in `template`.

## Examples

```
// Source: MyStruct { a: x, b: y }
Struct(lhs, [a → x, b → y], None, template = MyStruct{a: 0, b: 0})

// Source: MyStruct { a: x, ..base }
Struct(lhs, [a → x], Some(base), template = MyStruct{a: 0, b: 0})
```

The `template` for a no-`rest` construct provides the "implicit default" for fields not listed (which the front-end currently does not emit; all named fields of a struct are typically given explicitly). For tuple-structs with positional fields, `Member::Unnamed(k)` is used.

## Lowering

`Struct` lowers to a sequence of bit-level concatenations / splices in RTL, producing the bit-vector of the struct.

## Cross-references

- `Struct` and `FieldValue` in `spec.rs`.
- `TypedBits::splice` for the underlying field-set operation.
- See also [`enum.md`](./enum.md) for the analogous variant constructor.
