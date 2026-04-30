# RHIF Operational Semantics

> Normative reference for what RHIF executes. The implementation lives in `crates/rhdl-core/src/rhif/vm.rs`. This document is the prose statement of the small-step semantics; per-opcode detail is under [`opcodes/`](./opcodes/).

## Values

A **value** `v` is a [`TypedBits`](../../crates/rhdl-core/src/types/typed_bits.rs) — a `Kind` plus a vector of `BitX`:

```
BitX ::= Zero | One | X
```

`X` is the "don't care" or "uninitialized" bit. Every kind has a fully-`X` value (`TypedBits::dont_care_from_kind`). Values are immutable; opcodes read values and produce values. There is no in-place mutation.

A `TypedBits` value's `Kind` matches the `Kind` recorded in the producing slot's type environment (see [`type-system.md`](./type-system.md)).

## States

A **state** `σ` is a partial function from slots to values:

```
σ : Slot ↦ Value
```

The state's domain is the union of:

- **Initial register bindings** — the registers in `Object::arguments`, bound by the caller before the body runs.
- **Bindings created during execution** — the `lhs` of each opcode, bound when that opcode runs.

Literal slots are not in σ; they are read directly from the symbol table (`Object::symtab.lit_vec()`) when referenced.

The VM operates over a state implementation `VMState` that wraps a `reg_stack: &mut [Option<TypedBits>]` (one entry per register slot) and an immutable `literals` table.

## Reading and writing

The two primitive state operations:

```
read(σ, s) =
  if s = Literal(l) then literals[l]
  else if σ(Register(r)) = Some(v) then v
  else ICE: UninitializedRegister(r)

write(σ, s, v) =
  if s = Literal(l) then ICE: CannotWriteToRHIFLiteral(l)
  else σ[Register(r) ↦ v]
```

Reading an unbound register slot is an internal compiler error — well-formed RHIF (post-front-end and post-pass) never does this. Writing a literal slot is similarly an ICE.

## The execution judgement

The single-step judgement `σ ⊢ op ⇓ σ′` reads as: "executing `op` in state `σ` produces state `σ′`." For a sequence of opcodes:

```
                          σ₀ ⊢ op₀ ⇓ σ₁     σ₁ ⊢ op₁ ⇓ σ₂     …     σₙ₋₁ ⊢ opₙ₋₁ ⇓ σₙ
———————————————————————————————————————————————————————————————————————————————————————
σ₀ ⊢ [op₀; op₁; …; opₙ₋₁] ⇓ σₙ
```

Execution is **strict**, **sequential**, and **deterministic**. Each `op_i` is fully evaluated before `op_{i+1}` begins; the final state σₙ is the result. Determinism: given the same initial state σ₀, the final σₙ is uniquely determined. (`X` propagation is deterministic — see [§X-propagation](#x-propagation-don't-care-arithmetic).)

## Per-opcode semantics

These are the small-step rules. Per-opcode detail is in [`opcodes/`](./opcodes/).

### Noop

```
σ ⊢ Noop ⇓ σ
```

Identity on state.

### Binary

```
read(σ, a₁) = v₁   read(σ, a₂) = v₂   binary(op, v₁, v₂) = v
———————————————————————————————————————————————————————————————
σ ⊢ Binary(op, lhs, a₁, a₂) ⇓ σ[lhs ↦ v]
```

Where `binary` is defined in [`runtime_ops.rs::binary`](../../crates/rhdl-core/src/rhif/runtime_ops.rs):

| `op`           | Semantics |
|----------------|-----------|
| `Add`, `Sub`   | Two's-complement, wraps modulo 2^N. |
| `Mul`          | Same-width same-signedness multiply, wraps. |
| `BitXor`, `BitAnd`, `BitOr` | Bitwise, width-preserving. |
| `Shl`, `Shr`   | Width-preserving shift. `Shr` of a `Signed` value is arithmetic (sign-extended); `Shr` of a `Bits` is logical. Shift amount is converted to `usize`. |
| `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge` | Compare; result is `Bits(1)`. Ordering on `Signed` is two's-complement (sign-aware); on `Bits` is unsigned. |
| `XAdd`, `XSub` | Widen both inputs to `max(N₁, N₂) + 1` bits, perform exact arithmetic, return that wider type. Never wraps. |
| `XMul`         | Widen to `N₁ + N₂` bits, perform exact multiply, return that wider type. Never wraps. |

Mixing `Bits` and `Signed` operands is rejected at runtime by the typed-bits machinery (`DynamicTypeError::BinaryOperationRequiresCompatibleType`); well-typed RHIF never reaches that error.

`Signal<T, C>` operands are unwrapped, the inner operation runs, and the result is re-wrapped as `Signal<T', C>` (where `T'` is the result kind of the inner operation). The wrapper does not appear in the runtime arithmetic.

### Unary

```
read(σ, a) = v   unary(op, v) = v'
————————————————————————————————————
σ ⊢ Unary(op, lhs, a) ⇓ σ[lhs ↦ v']
```

Where `unary` is defined in [`runtime_ops.rs::unary`](../../crates/rhdl-core/src/rhif/runtime_ops.rs):

| `op`         | Semantics |
|--------------|-----------|
| `Not`        | Bitwise complement (preserves kind). |
| `Neg`        | Two's-complement negation; only valid on `Signed(N)`. |
| `All`        | `1` iff every bit is `1`. Result `Bits(1)`. |
| `Any`        | `1` iff any bit is `1`. Result `Bits(1)`. |
| `Xor`        | Reduction-XOR over all bits. Result `Bits(1)`. |
| `Signed`     | Reinterpret a `Bits(N)` as `Signed(N)`. No bit change. |
| `Unsigned`   | Reinterpret a `Signed(N)` as `Bits(N)`. No bit change. |
| `Val`        | Strip a `Signal(T, C)` wrapper, yielding `T`. |
| `XExt(d)`    | Zero/sign-extend by `d` bits (zero-ext on `Bits`, sign-ext on `Signed`). |
| `XShl(d)`    | Multiply by `2^d` and widen so the result is exact (no overflow). |
| `XShr(d)`    | Divide by `2^d` (arithmetic on `Signed`, logical on `Bits`); result narrowed by `d` bits. |
| `XNeg`       | Sign-extend by 1 bit then negate. Always returns `Signed(N+1)`. |
| `XSgn`       | Zero-extend by 1 bit then reinterpret as signed. Always returns `Signed(N+1)`. |

### Select

```
read(σ, cond) = v_c   read(σ, true_value) = v_t   read(σ, false_value) = v_f

  v_c.bits[0] = One   ⇒ v = v_t
  v_c.bits[0] = Zero  ⇒ v = v_f
  v_c.bits[0] = X     ⇒ v = dont_care(kind(v_t))
————————————————————————————————————————————————————————
σ ⊢ Select(lhs, cond, true_value, false_value) ⇓ σ[lhs ↦ v]
```

3-input mux. Note the `X`-on-cond rule: an undefined condition does *not* implicitly select one branch; it produces a fully-`X` result of the appropriate kind.

### Index

```
read(σ, arg) = v   resolve(σ, p) = p_static   v.path(p_static) = v'
———————————————————————————————————————————————————————————————————
σ ⊢ Index(lhs, arg, p) ⇓ σ[lhs ↦ v']
```

Where `resolve(σ, p)` walks `p` and replaces each `DynamicIndex(s)` element with `Index(read(σ, s).as_i64() as usize)`. The resulting `p_static` is a path with no dynamic indices.

The path-walk `v.path(p_static)` is implemented by [`TypedBits::path`](../../crates/rhdl-core/src/types/typed_bits.rs); it produces the sub-value at the given route. Out-of-range static indices are an ICE (well-typed RHIF excludes them); out-of-range dynamic indices are converted to a Rust panic in the simulator and to "implementation-defined behaviour" in synthesised hardware.

### Assign

```
read(σ, rhs) = v
————————————————————————————————————
σ ⊢ Assign(lhs, rhs) ⇓ σ[lhs ↦ v]
```

A copy. Often a no-op after constant propagation.

### Splice

```
read(σ, orig) = v_o   resolve(σ, p) = p_static   read(σ, subst) = v_s
v_o.splice(p_static, v_s) = v'
————————————————————————————————————————————————————————————————————————
σ ⊢ Splice(lhs, orig, p, subst) ⇓ σ[lhs ↦ v']
```

Functional update: produce a fresh aggregate equal to `v_o` everywhere except at the path `p`, where the sub-value is `v_s`.

### Repeat

```
read(σ, value) = v   repeat(v, n) = v'
————————————————————————————————————————
σ ⊢ Repeat(lhs, value, n) ⇓ σ[lhs ↦ v']
```

Where `repeat(v, n)` returns an array of `n` copies of `v`.

### Struct

```
v_init = if rest = Some(r) then read(σ, r) else clone(template)
∀ field ∈ fields:
  v_field = read(σ, field.value)
  p_field = match field.member with Named(n) → Field(n) | Unnamed(k) → TupleIndex(k)
  v_init = v_init.splice(p_field, v_field)
————————————————————————————————————————————————————————————————————————————————
σ ⊢ Struct(lhs, fields, rest, template) ⇓ σ[lhs ↦ v_init]
```

The `template` is a constant `TypedBits` carrying the struct's kind (and default field values for the no-`rest` case). `rest` provides a base struct into which the listed fields are spliced; if `rest = None`, the splices land on the `template`'s clone.

### Tuple

```
∀ i. read(σ, fᵢ) = vᵢ
v = tuple([v₀, v₁, …, vₙ])
———————————————————————————————————————
σ ⊢ Tuple(lhs, [f₀, …, fₙ]) ⇓ σ[lhs ↦ v]
```

Where `tuple` (in `runtime_ops.rs`) concatenates the bit-representations and produces a `Tuple` kind.

### Case

```
read(σ, disc) = v_d
matched_arm = first (arg, slot) in table such that
  (arg = Wild) or (read(σ, arg) = v_d)
v = if matched_arm = Some((_, s)) then read(σ, s)
    else dont_care(kind(lhs))
———————————————————————————————————————————————————————
σ ⊢ Case(lhs, disc, table) ⇓ σ[lhs ↦ v]
```

The `Case` opcode is a ROM lookup: arms are scanned top-to-bottom; the first match wins. A `Wild` arm always matches. If no arm matches, the result is `dont_care` of the `lhs`'s kind. (Well-typed RHIF generated by the front-end always has a `Wild` arm at the end, so the no-match case is unreachable in practice — but the spec handles it for robustness.)

### Exec

```
∀ i. read(σ, args[i]) = vᵢ
result = execute(externals[id], [v₀, …, vₙ])
———————————————————————————————————————————————————————
σ ⊢ Exec(lhs, id, args) ⇓ σ[lhs ↦ result]
```

The callee is an `Object` indexed by `id` in `externals`. The `execute` function (`vm.rs::execute`) recursively runs the callee in a fresh state with `arguments` bound to `[v₀, …, vₙ]`, then returns `read(σ_callee, callee.return_slot)`.

Execution is strict and produces a single result; there is no exception path, no early return, no generator-style yielding.

### Array

```
∀ i. read(σ, eᵢ) = vᵢ
v = array([v₀, …, vₙ])
————————————————————————————————————————
σ ⊢ Array(lhs, [e₀, …, eₙ]) ⇓ σ[lhs ↦ v]
```

Array construction; concatenates bit-representations and produces an `Array(T, n+1)` kind.

### Enum

```
v_init = clone(template)
discriminant = template.discriminant().as_i64()
∀ field ∈ fields:
  v_field = read(σ, field.value)
  p_field = EnumPayloadByValue(discriminant) ::
              match field.member with Named(n) → Field(n) | Unnamed(k) → TupleIndex(k)
  v_init = v_init.splice(p_field, v_field)
————————————————————————————————————————————————————————————————————————————————————
σ ⊢ Enum(lhs, fields, template) ⇓ σ[lhs ↦ v_init]
```

The `template` is a constant `TypedBits` whose discriminant identifies the variant being constructed; payload fields are spliced into the right slot.

### AsBits / AsSigned / Resize

```
read(σ, arg) = v   v.unsigned_cast(n) = v'
————————————————————————————————————————————
σ ⊢ AsBits(lhs, arg, Some(n)) ⇓ σ[lhs ↦ v']

read(σ, arg) = v   v.signed_cast(n) = v'
————————————————————————————————————————————
σ ⊢ AsSigned(lhs, arg, Some(n)) ⇓ σ[lhs ↦ v']

read(σ, arg) = v   v.resize(n) = v'
————————————————————————————————————————————
σ ⊢ Resize(lhs, arg, Some(n)) ⇓ σ[lhs ↦ v']
```

- `unsigned_cast(n)`: zero-extend or truncate to `Bits(n)`.
- `signed_cast(n)`: sign-extend or truncate to `Signed(n)`.
- `resize(n)`: width-change preserving signedness — equivalent to `unsigned_cast` on `Bits`, `signed_cast` on `Signed`.

`len = None` is an ICE at the VM boundary.

### Retime

```
read(σ, arg) = v
v' = TypedBits { bits: v.bits, kind: Signal(v.kind, color) }   if color = Some(C)
v' = v                                                            if color = None
————————————————————————————————————————————————————————————————————————————————————
σ ⊢ Retime(lhs, arg, color) ⇓ σ[lhs ↦ v']
```

`Retime` rewraps a value with a `Signal` kind layer; the bits are unchanged. This is the only opcode that legitimately changes a value's `Color`. (See [`reset-clock.md`](./reset-clock.md).)

### Wrap

```
read(σ, arg) = v
v' = match op with
  Some → v.wrap_some(kind)
  None → v.wrap_none(kind)
  Ok   → v.wrap_ok(kind)
  Err  → v.wrap_err(kind)
————————————————————————————————————————————————————————————————————
σ ⊢ Wrap(op, lhs, arg, Some(kind)) ⇓ σ[lhs ↦ v']
```

Constructs an `Option<T>` or `Result<T, E>` value. `kind = None` is an ICE at the VM boundary.

## X-propagation (don't-care arithmetic)

`BitX::X` is propagated through opcodes per the rules of [`TypedBits`](../../crates/rhdl-core/src/types/typed_bits.rs):

- **Bitwise ops** (`BitAnd`, `BitOr`, `BitXor`, `Not`): `X & 0 = 0`, `X & 1 = X`, `X | 0 = X`, `X | 1 = 1`, `X ^ _ = X`, `!X = X`. (Standard 4-state logic.)
- **Arithmetic ops** (`Add`, `Sub`, `Mul`, `Neg`, etc.): if any operand bit is `X`, the entire result is fully-`X` of the result kind. (Conservative; mirrors the iverilog 4-state model.)
- **Comparison** (`Eq`, `Lt`, …): `X` in either operand makes the boolean result `X`.
- **Reduction** (`All`, `Any`, `Xor`): defined when all operand bits are non-`X`; otherwise `X`.
- **`Select` on `X`-cond**: produces a fully-`X` result (per the Select rule above).
- **Path walk**: walking through a path yields `X` for any sub-value reached through an `X` discriminant.

## Top-level execution

```
execute(obj, [v₀, …, vₙ]):
  precondition: |args| = |obj.arguments|
                ∀ i. kind(vᵢ) = obj.symtab[obj.arguments[i]]
  σ₀ = empty register state
  ∀ i. σ₀[Register(obj.arguments[i])] = vᵢ
  σ_final = run obj.ops to completion in σ₀
  return read(σ_final, obj.return_slot)
```

Argument-count and argument-kind mismatches are ICEs (rejected before opcodes run).

## Determinism and reproducibility

Given a well-typed `Object` and a vector of well-typed argument values, `execute` produces a unique result. There is no randomness, no clock dependence (clocks live above this layer), no I/O, and no allocation that affects the result.

The implementation may differ in the order it runs ops within a basic block — but the IR has no basic blocks; ops execute in their listed order. Passes that re-order ops must preserve the [single-assignment + def-before-use](./invariants/object.md) invariants, which together ensure that any total order of ops respecting the data-dependency graph produces the same final state.

## Termination

Every kernel execution terminates. There are no recursive calls (the front-end forbids them), no `while`/`loop`, and no opcode that loops internally — even `Repeat`, `Array`, `Tuple`, `Struct`, `Enum`, and `Case` walk fixed-size sequences. The number of opcode evaluations equals `|Object::ops|`, plus the (transitively-bounded) cost of `Exec` calls. RHIF programs are total functions on well-typed inputs.

## Relationship to `vm.rs`

The implementation in [`vm.rs`](../../crates/rhdl-core/src/rhif/vm.rs) is the canonical executable form of these rules. Where this spec and `vm.rs` disagree, the spec is normative — see CLAUDE.md §11.1 and `rhif-formalization-plan.md` §14. Reconcile by either updating the VM (if the spec captures the intended behaviour) or updating the spec (if the VM captures it and the spec was wrong).
