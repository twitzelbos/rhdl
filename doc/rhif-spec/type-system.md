# RHIF Type System

> Normative reference for when an RHIF opcode is well-typed. The judgements here are stated in inference-rule notation augmented with prose, and detail per opcode is in the corresponding [`opcodes/`](./opcodes/) page.

## Type environments

A **type environment** Γ is a partial map from slots to kinds:

```
Γ : Slot ↦ Kind
```

For an `Object`, Γ is the union of:

- `{ r ↦ Object::symtab[r] | r ∈ Object::symtab.iter_reg() }`, and
- `{ l ↦ Object::symtab[l].kind() | l ∈ Object::symtab.iter_lit() }`.

In other words: every register slot gets the kind recorded in the symbol table, and every literal slot gets the kind of its `TypedBits` value.

The type-lookup judgement is written `Γ ⊢ s : T` — "in environment Γ, slot `s` has kind `T`."

## Kind compatibility

Two kinds `T₁` and `T₂` are **compatible** (written `T₁ ≡ T₂`) if they are identical under structural identity. Two `Bits(N)` values are compatible iff their widths are equal. Two `Signal(T, C)` values are compatible iff their inner kinds and colours are equal. There is no implicit coercion: `Bits(7)` and `Bits(8)` are incompatible; `Bits(8)` and `Signed(8)` are incompatible; `Bits(8)` and `Signal(Bits(8), Red)` are incompatible.

The exceptions to "no coercion" are the explicit cast opcodes (`AsBits`, `AsSigned`, `Resize`, `Retime`); these are the only opcodes that change a slot's kind in a non-structural way.

## Well-typedness of an opcode

The judgement `Γ ⊢ op : ok` says that opcode `op` is well-typed under environment Γ. The judgement is structural: each opcode has its own typing rule, given below.

A well-typed opcode satisfies the contract that *its operands have the expected kinds, and its `lhs` slot has the kind that the opcode produces.* The kind of `lhs` is determined by the surrounding `Object`'s symbol table; the typing rule says that the produced kind matches what is recorded.

## Per-opcode typing rules

The full per-opcode rule, with explanations and examples, is in [`opcodes/`](./opcodes/). The compact rules below define the type system at a glance.

### Noop

```
————————————
Γ ⊢ Noop : ok
```

Always well-typed; produces nothing.

### Binary

For `op ∈ {Add, Sub, BitXor, BitAnd, BitOr, Shl, Shr, Mul}` and operand kind `T` ∈ {`b<N>`, `s<N>`, `T@C`}:

```
Γ ⊢ a₁ : T   Γ ⊢ a₂ : T_rhs   Γ ⊢ lhs : T
where T_rhs = T (for arithmetic)
   or T_rhs is any bit/signed kind (for shift)
————————————————————————————————————————————
Γ ⊢ Binary(op, lhs, a₁, a₂) : ok
```

For `op ∈ {Eq, Lt, Le, Ne, Ge, Gt}`:

```
Γ ⊢ a₁ : T   Γ ⊢ a₂ : T   Γ ⊢ lhs : Bits(1)
————————————————————————————————————————————
Γ ⊢ Binary(op, lhs, a₁, a₂) : ok
```

For widening ops `XAdd` / `XSub`:

```
Γ ⊢ a₁ : T₁   Γ ⊢ a₂ : T₂   Γ ⊢ lhs : T'
where T' = b<max(N₁, N₂) + 1> if T₁, T₂ are unsigned
        or s<max(N₁, N₂) + 1> if T₁, T₂ are signed
————————————————————————————————————————————————————
Γ ⊢ Binary(XAdd | XSub, lhs, a₁, a₂) : ok
```

For widening multiply `XMul`:

```
Γ ⊢ a₁ : T₁   Γ ⊢ a₂ : T₂   Γ ⊢ lhs : T'
where T' = b<N₁ + N₂> if both unsigned
        or s<N₁ + N₂> if both signed
————————————————————————————————————————————————
Γ ⊢ Binary(XMul, lhs, a₁, a₂) : ok
```

For `Mul`: requires same-width same-signedness operands and produces same-width result (no widening — wraps on overflow per two's complement).

In all binary cases, mixing signed and unsigned is rejected: if `T₁.is_signed() ⊕ T₂.is_signed()`, the rule fails. (See `runtime_ops.rs::mul`, `xadd`, etc.)

`Signal<T, C>` operands: the rules above lift through `Signal` — if both operands are `T@C`, the result is `T'@C`. Mixing colours is rejected. The kernel does not strip the `Signal` wrapper; only `Index` with a `SignalValue` path does.

See [`opcodes/binary.md`](./opcodes/binary.md) for the full rule per `AluBinary` variant.

### Unary

```
Γ ⊢ a : T   Γ ⊢ lhs : T'
————————————————————————————
Γ ⊢ Unary(op, lhs, a) : ok
```

Where `T'` depends on `op`:

| op | Result kind T' |
|---|---|
| `Not` | `T` |
| `Neg` | `T` (T must be `s<N>`) |
| `All`, `Any`, `Xor` | `Bits(1)` |
| `Signed` | `s<N>` if `T = b<N>` |
| `Unsigned` | `b<N>` if `T = s<N>` |
| `Val` | the inner kind of `T` (strips a `Signal` layer) |
| `XExt(d)` | `b<N+d>` or `s<N+d>` depending on `T` |
| `XShl(d)` | `b<N+d>` or `s<N+d>` |
| `XShr(d)` | `b<max(N-d, 0)>` or `s<max(N-d, 0)>` |
| `XNeg` | `s<N+1>` (always signed; widens by 1) |
| `XSgn` | `s<N+1>` (always signed; widens by 1) |

See [`opcodes/unary.md`](./opcodes/unary.md).

### Select

```
Γ ⊢ cond : Bits(1)   Γ ⊢ a : T   Γ ⊢ b : T   Γ ⊢ lhs : T
———————————————————————————————————————————————————————————
Γ ⊢ Select(lhs, cond, a, b) : ok
```

A 3-input mux. `Signal<Bits(1), C>` is also accepted as `cond` if the result kind is `T@C` for the same `C`.

See [`opcodes/select.md`](./opcodes/select.md).

### Index

```
Γ ⊢ arg : T   (T, p) ⇒ T'   Γ ⊢ lhs : T'
———————————————————————————————————————————
Γ ⊢ Index(lhs, arg, p) : ok
```

Where `(T, p) ⇒ T'` is the path-walking judgement (below). All slots referenced by `DynamicIndex(s)` elements of `p` must have an integer-convertible kind (in practice, `Bits(N)` or `Signed(N)` of any width).

See [`opcodes/index.md`](./opcodes/index.md).

### Assign

```
Γ ⊢ rhs : T   Γ ⊢ lhs : T
————————————————————————————
Γ ⊢ Assign(lhs, rhs) : ok
```

A no-op-kind copy. Both sides must have identical kinds.

See [`opcodes/assign.md`](./opcodes/assign.md).

### Splice

```
Γ ⊢ orig : T   (T, p) ⇒ T'   Γ ⊢ subst : T'   Γ ⊢ lhs : T
———————————————————————————————————————————————————————————
Γ ⊢ Splice(lhs, orig, p, subst) : ok
```

Functional update at a path: the `lhs` is `orig` with the sub-value at `p` replaced by `subst`.

See [`opcodes/splice.md`](./opcodes/splice.md).

### Repeat

```
Γ ⊢ value : T   Γ ⊢ lhs : Array(T, n)
————————————————————————————————————————
Γ ⊢ Repeat(lhs, value, n) : ok
```

See [`opcodes/repeat.md`](./opcodes/repeat.md).

### Struct

```
Γ ⊢ template : Struct(S)
∀ field ∈ fields. Γ ⊢ field.value : kind_of(S, field.member)
Γ ⊢ rest : Struct(S) ∨ rest = None
Γ ⊢ lhs : Struct(S)
————————————————————————————————————————————————————————————
Γ ⊢ Struct(lhs, fields, rest, template) : ok
```

The `template` carries the struct kind; `rest` (if present) provides a base value into which the listed fields are spliced. Field-value kinds must match the struct's declaration.

See [`opcodes/struct.md`](./opcodes/struct.md).

### Tuple

```
Γ ⊢ f₀ : T₀   Γ ⊢ f₁ : T₁   …   Γ ⊢ fₙ : Tₙ
Γ ⊢ lhs : (T₀, T₁, …, Tₙ)
———————————————————————————————————————————————
Γ ⊢ Tuple(lhs, [f₀, …, fₙ]) : ok
```

See [`opcodes/tuple.md`](./opcodes/tuple.md).

### Case

```
Γ ⊢ disc : T_disc
∀ (arg, slot) ∈ table.
  (arg = Wild) ∨ (Γ ⊢ arg : T_disc)
∀ (_, slot) ∈ table. Γ ⊢ slot : T
Γ ⊢ lhs : T
————————————————————————————————————————————
Γ ⊢ Case(lhs, disc, table) : ok
```

All arms must produce values of the same kind `T`; all non-`Wild` arms must have discriminator slots of the same kind as the discriminant.

See [`opcodes/case.md`](./opcodes/case.md).

### Exec

```
externals[id] = func
∀ i. Γ ⊢ args[i] : Object::symtab[func.arguments[i]]
Γ ⊢ lhs : kind_of(func.return_slot)
————————————————————————————————————————————————————
Γ ⊢ Exec(lhs, id, args) : ok
```

Argument count and kinds must match the callee's `arguments`; the result kind matches the callee's `return_slot`.

See [`opcodes/exec.md`](./opcodes/exec.md).

### Array

```
Γ ⊢ e₀ : T   Γ ⊢ e₁ : T   …   Γ ⊢ eₙ : T
Γ ⊢ lhs : Array(T, n + 1)
————————————————————————————————————————————————
Γ ⊢ Array(lhs, [e₀, …, eₙ]) : ok
```

All elements must have the same kind.

See [`opcodes/array.md`](./opcodes/array.md).

### Enum

```
Γ ⊢ template : Enum(E)
∀ field ∈ fields. Γ ⊢ field.value : kind_of(E.variant_of(template), field.member)
Γ ⊢ lhs : Enum(E)
————————————————————————————————————————————————————————————————————————————
Γ ⊢ Enum(lhs, fields, template) : ok
```

The variant being constructed is determined by `template.discriminant()`; `fields` populates that variant's payload.

See [`opcodes/enum.md`](./opcodes/enum.md).

### AsBits / AsSigned / Resize

```
Γ ⊢ arg : T_arg   Γ ⊢ lhs : T_result
where T_arg ∈ {Bits(M), Signed(M)} and T_result is determined by `len`
————————————————————————————————————————————————————————————————————————
Γ ⊢ AsBits | AsSigned | Resize(lhs, arg, len) : ok
```

- `AsBits(_, _, n)` produces `Bits(n)`.
- `AsSigned(_, _, n)` produces `Signed(n)`.
- `Resize(_, _, n)` produces `Bits(n)` if `T_arg = Bits(_)`, `Signed(n)` if `T_arg = Signed(_)`.

`len` must be `Some(n)` at the time the opcode is type-checked. (Front-end may emit `None`; a pass resolves it to `Some(_)` before lowering. Reaching the VM with `None` is an ICE.)

See [`opcodes/as_bits.md`](./opcodes/as_bits.md), [`opcodes/as_signed.md`](./opcodes/as_signed.md), [`opcodes/resize.md`](./opcodes/resize.md).

### Retime

```
Γ ⊢ arg : T   Γ ⊢ lhs : Signal(T, C)
———————————————————————————————————————
Γ ⊢ Retime(lhs, arg, Some(C)) : ok
```

Where `T` is *not* itself a `Signal(_, _)` (no nested signals).

`color = None` is permitted in early IR but must be resolved before lowering.

See [`opcodes/retime.md`](./opcodes/retime.md).

### Wrap

For `op = Some` with `kind = Some(Option<T>)`:

```
Γ ⊢ arg : T   Γ ⊢ lhs : Option<T>
————————————————————————————————————
Γ ⊢ Wrap(Some, lhs, arg, Option<T>) : ok
```

For `op = None`:

```
Γ ⊢ arg : ()   Γ ⊢ lhs : Option<T>
————————————————————————————————————
Γ ⊢ Wrap(None, lhs, arg, Option<T>) : ok
```

For `op = Ok` with `kind = Some(Result<T, E>)`:

```
Γ ⊢ arg : T   Γ ⊢ lhs : Result<T, E>
————————————————————————————————————
Γ ⊢ Wrap(Ok, lhs, arg, Result<T, E>) : ok
```

For `op = Err`:

```
Γ ⊢ arg : E   Γ ⊢ lhs : Result<T, E>
————————————————————————————————————
Γ ⊢ Wrap(Err, lhs, arg, Result<T, E>) : ok
```

`kind` carries the desired `Option` / `Result` kind (the front-end emits this so the wrapper need not re-infer it).

See [`opcodes/wrap.md`](./opcodes/wrap.md).

## Path typing

The path-typing judgement `(T, p) ⇒ T'` walks a path through a kind:

```
———————————          (Index(k))     T = Array(T', n)   k < n
(T, ε) ⇒ T          ————————————————————————————————————————
                     (T, Index(k) :: rest) ⇒ walk(T', rest)

(TupleIndex(k))      T = Tuple([T₀, …, Tₙ])   k ≤ n
————————————————————————————————————————————————————
(T, TupleIndex(k) :: rest) ⇒ walk(Tₖ, rest)

(Field(f))           T = Struct{…, f: Tₖ, …}
————————————————————————————————————————————————————
(T, Field(f) :: rest) ⇒ walk(Tₖ, rest)

(EnumDiscriminant)   T = Enum(E)
———————————————————————————————————————————————————————————————————
(T, EnumDiscriminant :: rest) ⇒ walk(b<width(E.discriminant)>, rest)

(EnumPayload(V))     T = Enum(E)   V is a variant of E
———————————————————————————————————————————————————————
(T, EnumPayload(V) :: rest) ⇒ walk(payload_of(E, V), rest)

(EnumPayloadByValue(k))  T = Enum(E)   k is a discriminant of E
————————————————————————————————————————————————————————————————
(T, EnumPayloadByValue(k) :: rest) ⇒ walk(payload_of(E, k), rest)

(DynamicIndex(s))    T = Array(T', n)   Γ ⊢ s : Bits(M) ∨ Signed(M)
————————————————————————————————————————————————————————————————————
(T, DynamicIndex(s) :: rest) ⇒ walk(T', rest)

(SignalValue)        T = Signal(T', C)
————————————————————————————————————————
(T, SignalValue :: rest) ⇒ walk(T', rest)
```

Statically out-of-range indices (e.g., `Index(5)` into `Array(T, 3)`) are rejected at type-check time. Dynamic out-of-range indices are run-time / synthesis-time problems, not type errors.

## Well-typedness of an Object

An `Object` is **well-typed** if every opcode in `Object::ops` is well-typed under the type environment defined by its symbol table, *and* the cross-cutting global invariants hold:

- **Argument typing.** For each `r ∈ Object::arguments`, `r` has a recorded kind and that kind is the kind that callers will supply. The type rule for `Exec` enforces this on the call side.
- **Return typing.** `kind(Object::return_slot)` is the kind that callers will receive.
- **No untyped slots.** Every slot referenced by any opcode is in the symbol table.

The full set of structural invariants — single-assignment, definition-before-use, etc. — is in [`invariants/object.md`](./invariants/object.md). Well-typedness is one of those invariants.

## A note on `dont_care`

The runtime model includes a partial value `X` (`BitX::X`) representing "don't care." Every kind has a corresponding `dont_care` value (see `TypedBits::dont_care_from_kind`). The type system does not distinguish a `dont_care` value from a fully-defined value of the same kind — both have the same `Kind`, and the same opcodes apply. The semantic distinction shows up in [`semantics.md`](./semantics.md).
