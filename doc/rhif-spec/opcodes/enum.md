# `Enum(lhs, fields, template)`

Construct an enum value of a specific variant, with the variant's payload populated from `fields`.

## Syntax

```
Enum {
    lhs:      Slot,
    fields:   Vec<FieldValue>,    -- payload fields for this variant
    template: TypedBits,          -- carries the enum's Kind AND the discriminant
                                  -- for the variant being constructed
}
```

## Type rule

```
template : Enum(E)
discriminant_value(template) = k    -- k identifies the variant
∀ field ∈ fields. Γ ⊢ field.value : kind_of_payload(E, k, field.member)
Γ ⊢ lhs : Enum(E)
———————————————————————————————————————————————————————————————————————
Γ ⊢ Enum(lhs, fields, template) : ok
```

The variant being constructed is determined by `template.discriminant()` — the discriminant bits are pre-set in the template constant. `fields` populates the payload of that variant.

## Dynamic semantics

```
v_init = clone(template)
disc = v_init.discriminant().as_i64()
∀ field ∈ fields:
  v_field = read(σ, field.value)
  base_path = EnumPayloadByValue(disc)
  member_path = match field.member with
                  Named(n)   → Field(n)
                  Unnamed(k) → TupleIndex(k)
  v_init = v_init.splice(base_path :: member_path, v_field)
————————————————————————————————————————————————————————————————————
σ ⊢ Enum(lhs, fields, template) ⇓ σ[lhs ↦ v_init]
```

The discriminant is left as the template provides it; payload fields are spliced into the appropriate variant slot.

## Pre-conditions

- `template.kind()` is `Enum(E)`.
- The variant identified by `template`'s discriminant exists in `E`.
- Every member referenced in `fields` is a valid member of that variant's payload.

## Post-conditions

- `lhs` is bound to an enum value of `Enum(E)` whose discriminant matches `template`'s and whose payload fields are populated from `fields`.

## Examples

For `enum MyEnum { A, B(u8), C { x: u16 } }` (with discriminants `0`, `1`, `2`):

```
// MyEnum::A — no payload
Enum(lhs, [], template = MyEnum_A_template)

// MyEnum::B(x) — single unnamed payload field
Enum(lhs, [(Unnamed(0), x_slot)], template = MyEnum_B_template)

// MyEnum::C { x }
Enum(lhs, [(Named("x"), x_slot)], template = MyEnum_C_template)
```

## Notes

- Unlike Rust source, the discriminant in `template` is *not* derived from a variant name — it is precomputed by the proc-macro front-end. RHIF treats variants strictly by discriminant value.
- Payload fields not listed in `fields` retain whatever the `template` carries — typically `dont_care` of the field's kind.
- An enum value's bit layout is `[discriminant_bits | shared_payload_bits]`, where the payload region is sized to fit the *largest* variant. Variants with smaller payloads have unused upper bits per the discriminant alignment policy in `crates/rhdl-core/src/types/kind.rs::DiscriminantAlignment`.

## Lowering

`Enum` lowers to bit-level splices into a fixed-size value: the discriminant bits become a constant, and each payload field is spliced into the appropriate position determined by the variant's payload layout.

## Cross-references

- `Enum`, `FieldValue`, `Member` in `spec.rs`.
- `Kind::Enum` and the `Enum` / `Variant` / `DiscriminantLayout` types in `crates/rhdl-core/src/types/kind.rs`.
- See also [`struct.md`](./struct.md) for the analogous struct constructor.
