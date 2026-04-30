# RHIF Syntax

> Normative reference for what an RHIF program is, syntactically. This document echoes `crates/rhdl-core/src/rhif/spec.rs` in a stable form independent of the Rust source. Where they disagree, the Rust source is the syntactic ground truth, but this page is what compiler-level contributors should read first.

## Slots

A **slot** is an SSA-style name for a value. Concretely (from `spec.rs`):

```
Slot ::= Register(RegisterId)
       | Literal(LiteralId)
```

Two disjoint sub-namespaces:

- **Register slots** — written `r₀, r₁, …` — are the targets of opcodes. Each register is bound exactly once, by exactly one opcode's `lhs` field, except for the registers listed in [`Object::arguments`](#objects), which are bound by the caller of the kernel. After binding, a register's value is fixed.
- **Literal slots** — written `l₀, l₁, …` — are bound at compile time to constant `TypedBits` values stored in the symbol table. Literals are read-only: writing to a literal slot is an internal compiler error (ICE), not a runtime panic, because the front-end never emits such a write.

Slots are **typed**: every slot has a static [`Kind`](#kinds-types). The kind of a register slot is recorded in the symbol table (`Object::symtab`); the kind of a literal slot is the `Kind` of its `TypedBits` value.

The set of slots in scope is determined by the symbol table: any `Slot::Register(r)` referenced in an `OpCode` must be a key of `symtab.iter_reg()`; any `Slot::Literal(l)` must be a key of `symtab.iter_lit()`. References to undefined slots are an ICE.

## Opcodes

The [`OpCode` enum](../../crates/rhdl-core/src/rhif/spec.rs) has 19 variants, grouped here by purpose:

| Opcode | Purpose | LHS | Operands |
|---|---|---|---|
| `Noop` | placeholder for deleted ops | — | — |
| `Binary` | `lhs ← arg1 op arg2` | scalar / aggregate | two slots |
| `Unary` | `lhs ← op arg1` | scalar / aggregate | one slot |
| `Select` | `lhs ← if cond then a else b` | T | three slots |
| `Index` | `lhs ← arg[path]` | sub-value | one slot + path |
| `Assign` | `lhs ← rhs` | T | one slot |
| `Splice` | `lhs ← rhs[path := arg]` (functional update) | aggregate | two slots + path |
| `Repeat` | `lhs ← [v; n]` | array | one slot + length |
| `Struct` | `lhs ← S { fields, ..rest }` | struct | field slots + optional rest |
| `Tuple` | `lhs ← (v₀, …, vₙ)` | tuple | field slots |
| `Case` | `lhs ← match disc { table }` | T | discriminant + arms |
| `Exec` | `lhs ← f(args)` | f's return | callee id + args |
| `Array` | `lhs ← [v₀, …, vₙ]` | array | element slots |
| `Enum` | `lhs ← E::V { fields }` | enum | field slots + template |
| `AsBits` | `lhs ← arg as Bits<N>` | unsigned | one slot |
| `AsSigned` | `lhs ← arg as SignedBits<N>` | signed | one slot |
| `Resize` | `lhs ← arg.resize::<N>()` | bits / signed | one slot |
| `Retime` | `lhs ← signal::<C>(arg)` | `Signal<T, C>` | one slot |
| `Wrap` | `lhs ← Some/None/Ok/Err(arg)` | `Option<T>` / `Result<T, E>` | one slot + WrapOp |

Each opcode is documented in detail under [`opcodes/`](./opcodes/).

Every non-`Noop` opcode produces exactly one result, in the slot named by its `lhs` field. `Noop` produces no result. (See `OpCode::lhs()` in `spec.rs`.)

## Kinds (types)

A **kind** is a static type. The [`Kind` enum](../../crates/rhdl-core/src/types/kind.rs) is:

```
Kind ::= Bits(N)              -- unsigned bit-vector of width N
       | Signed(N)             -- two's-complement signed bit-vector of width N
       | Empty                 -- the unit type, ()
       | Tuple([Kind])         -- (T₀, …, Tₙ)
       | Array(Kind, N)        -- [T; N]
       | Struct(StructDef)     -- a named struct
       | Enum(EnumDef)         -- a named enum (with optional payloads)
       | Signal(Kind, Color)   -- T@C, a value tagged with a clock domain
       | Clock                 -- a clock signal carrier
       | Reset                 -- a reset signal carrier
```

Kind notation used in this spec:
- `b<N>` for `Bits(N)`, `s<N>` for `Signed(N)`.
- `()` for `Empty`.
- `(T₁, T₂)` for tuples; `(,)` for the empty tuple — equivalent to `()`.
- `[T; N]` for `Array(T, N)`.
- `{f₁: T₁, f₂: T₂}` for structs (eliding the name when unambiguous).
- `E` or `E[V₁(T) | V₂]` for enums.
- `T @ C` for `Signal(T, C)` — the at-sign mirrors the source-language convention.
- `clk` for `Clock`, `rst` for `Reset`.

Two kinds are **compatible** if they are equal under structural identity (e.g., two `Bits(8)` values are compatible, but `Bits(8)` and `Bits(7)` are not). The type rules (in `type-system.md`) use kind compatibility as the basis for well-typedness.

### The `Signal<T, C>` discipline

`Signal(T, C)` tags a value with a static clock-domain marker `C` (one of `Red, Orange, Yellow, Green, Blue, Indigo, Violet`). Most arithmetic and logical opcodes propagate the same `C` from inputs to output; mixing two distinct colours is rejected at type-check time. The `Retime` opcode is the only opcode that legitimately changes a slot's colour. Domain mixing is the type system's contribution to clock-domain safety; see `reset-clock.md`.

### `Clock` and `Reset`

`Clock` and `Reset` are nominal one-bit kinds used inside the `ClockReset` aggregate that the framework synthesises around a synchronous kernel. They are not produced by any opcode in the kernel body — they enter the kernel via the `cr` argument of a `Synchronous` kernel and leave via observation in `cr.reset.any()` (which lowers to `Index` + `Unary(Any)`). See `reset-clock.md`.

## Paths

A **path** describes a route into a `Digital` aggregate. Paths are used by `Index` and `Splice` to pick out / replace a sub-value. The [`PathElement`](../../crates/rhdl-core/src/types/path.rs) variants:

```
PathElement ::= Index(usize)              -- p[k], constant array index
              | TupleIndex(usize)         -- p.k, tuple field index
              | Field(name)               -- p.name, struct field
              | EnumDiscriminant          -- p#, the enum tag
              | EnumPayload(name)         -- p#V, the payload of variant V
              | EnumPayloadByValue(i64)   -- p#k, the payload of the variant with discriminant k
              | DynamicIndex(Slot)        -- p[s], runtime array index, with s a register or literal
              | SignalValue               -- p@, the inner value of a Signal<T, C>
```

A path is a sequence of `PathElement`s; it walks from the outer aggregate inward. The empty path is the identity. Paths are statically resolvable to a well-defined sub-kind, given the aggregate's kind.

A `DynamicIndex(s)` element is dynamic: its actual index is `read(s).as_i64()` at runtime, and the surrounding aggregate must be an `Array(_, N)` where `0 ≤ index < N`. Out-of-range indexing is implementation-defined (in the simulator, it is currently a Rust panic; in synthesised hardware, it is a multiplexer with undefined behaviour for the out-of-range case).

The path-typing judgement `(T, p) ⇒ T'` — "walking path `p` through kind `T` yields kind `T'`" — is described in `type-system.md` §Paths.

## Objects

An **`Object`** is the unit of compilation: one kernel function. The [`Object` struct](../../crates/rhdl-core/src/rhif/object.rs):

```
Object {
  symbols:     SymbolMap,                       -- source-position metadata
  symtab:      SymbolTable<TypedBits, Kind, …>, -- literal values + register kinds
  return_slot: Slot,                            -- the slot holding the kernel's return value
  externals:   BTreeMap<FuncId, Box<Object>>,   -- the callee table for `Exec` opcodes
  ops:         Vec<LocatedOpCode>,              -- the body, in execution order
  arguments:   Vec<RegisterId>,                 -- the kernel's input slots, in declaration order
  name:        String,
  fn_id:       FunctionId,
  flags:       Vec<KernelFlags>,
}
```

The body `ops` is a flat list of opcodes; there are no basic blocks because there is no control flow inside the IR — every opcode executes exactly once, in order, when the kernel is called.

Each opcode is wrapped in a [`LocatedOpCode`](../../crates/rhdl-core/src/rhif/object.rs) carrying a `SourceLocation` for diagnostics. The `Noop` opcode is a placeholder that allows passes to delete an op without re-numbering subsequent slots.

The well-formedness conditions on a complete `Object` — single-assignment property, symbol-table completeness, type-correctness, callee-table consistency — are listed in [`invariants/object.md`](./invariants/object.md).

## Auxiliary structures

A handful of helper structs are referenced by opcodes:

- **`FieldValue { member, value }`** — used by `Struct` and `Enum` to associate a member name (or tuple index) with the slot that supplies its value.
- **`Member`** — `Named(String)` for `S { foo: … }` or `Unnamed(u32)` for tuple structs / tuple variants like `(T₀, T₁)`.
- **`CaseArgument`** — discriminator pattern for a `Case` arm. Either `Slot(s)` (match if discriminant equals the value of `s`) or `Wild` (match anything; equivalent to a Rust `_` pattern).
- **`AluBinary`** — the binary-op selector. 16 variants: `Add, Sub, Mul, BitXor, BitAnd, BitOr, Shl, Shr, Eq, Lt, Le, Ne, Ge, Gt, XAdd, XSub, XMul`.
- **`AluUnary`** — the unary-op selector. 13 variants: `Neg, Not, All, Any, Xor, Signed, Unsigned, Val, XExt(usize), XShl(usize), XShr(usize), XNeg, XSgn`.
- **`WrapOp`** — `Ok | Err | Some | None`, identifying which `Option` / `Result` constructor a `Wrap` produces.
- **`FuncId`** — opaque index into `Object::externals`, identifying a callee for an `Exec` op.

## What an RHIF program is, formally

A well-formed RHIF program is a tuple `(Object, [arguments])` where the elements of `arguments` are runtime values whose kinds match `Object::arguments`. Executing the program runs the body `Object::ops` in order, then returns `read(Object::return_slot)`. The full operational semantics is in [`semantics.md`](./semantics.md).
