# Kernel Language Extensions — Design Spec

A specification for expanding the subset of Rust accepted inside `#[kernel]` functions. The current subset (documented in `CLAUDE.md` §4) is sufficient for hardware description but leaves significant Rust expressiveness unused. This document proposes specific extensions, organized by implementation cost, with per-feature lowering sketches and acceptance criteria.

This is the language-level companion to `auto-pipelining-plan.md` (compiler-pass-level work) and `widget-roadmap.md` (library-level work). All three are independently shippable parallel work streams.

---

## 1 — Goals and non-goals

**Goal.** Allow RHDL kernels to look more like ordinary Rust without expanding the set of synthesizable concepts. Every extension here must (a) preserve the property that "if it compiles, it's hardware" and (b) lower to the existing RHIF / RTL / NTL opcodes without introducing new fundamental hardware semantics. Extensions are sugar; the underlying model stays cycle-accurate, value-typed, and pure.

**Non-goal.** Closing the gap with full Rust. Heap allocation, dynamic dispatch, references with non-trivial lifetimes, recursive types, true IEEE 754 floats, and unsafe code are excluded by construction (see §6). The kernel will always be a strict subset.

**Why this matters.** A larger accepted subset means LLM-generated kernels need fewer stylistic translations to be valid. State machines written naturally in Rust — with `let-else`, `?`, range patterns, or-patterns — should compile as-is. Every desugaring the user has to do by hand is a place where an LLM (and a human) gets it wrong.

---

## 2 — Tier 1: Pure desugaring extensions

These are syntactic transformations performed in `rhdl-macro-core` or in the MIR-lowering pass. They reduce to constructs the IR already supports. No new RHIF opcodes; no new RTL/NTL passes.

### 2.1 `let-else`

Currently flagged as unsupported. Desugar:

```rust
let Some(x) = optional else { return default; };
// ↓
let x = match optional { Some(v) => v, None => return default };
```

**Lowering.** Pure AST transformation in the macro layer.
**Acceptance.** Existing kernel that uses `match Some/None`-then-bind works identically when rewritten with `let-else`. Test in `crates/rhdl/tests/binding.rs`.

### 2.2 ~~Or-patterns in match arms~~ — shipped 2026-04-29 (PR #3)

```rust
match state {
    State::A | State::B => handle_idle(),
    State::C => handle_busy(),
}
```

**Shipped:** `crates/rhdl-macro-core/src/kernel.rs::match_ex` flat-maps top-level or-patterns into one arm per alternative before the rest of the lowering pipeline runs.  Macro-layer transformation only; no IR change.  Emitted Verilog is byte-identical to the hand-written multi-arm form because RHIF `Case`'s `table: Vec<(CaseArgument, Slot)>` already permits multiple entries pointing at the same Slot.  Nested or-patterns inside tuple/struct/slice patterns (`(A | B, C)`) are caught by `pattern_has_nested_or` and rejected with a specific diagnostic that points at the manual distribution rewrite (`(A, C) | (B, C)`).  See `doc/book/src/kernels/match.md` for the user-facing documentation.

Original entry follows: currently each variant requires a separate arm. The desugaring expands to multiple arms with the same RHS, or — better — lowers to a discriminant-OR check in the existing `Case` opcode.

**Lowering.** RHIF `Case` already supports multiple discriminant values per branch (see `rhdl-core/src/rhif/spec.rs`). The macro layer needs to recognize the syntax and emit the corresponding `Case` arm with a multi-discriminant key. If the IR doesn't yet accept multi-discriminant keys, that's a small, well-isolated extension.
**Acceptance.** A state-machine kernel that previously required N redundant arms collapses to one arm with N alternatives, with byte-identical emitted Verilog.

### 2.3 Range patterns

```rust
match opcode {
    0x00..=0x1F => decode_arithmetic(opcode),
    0x80..=0xFF => decode_extended(opcode),
    _ => decode_other(opcode),
}
```

**Lowering.** Range pattern `lo..=hi` desugars to `x >= lo && x <= hi`. Composes with `Case` arms via guard expressions, or lowers to a chained `Select`.
**Acceptance.** An instruction-decoder kernel using ranges produces fewer NTL nodes than the equivalent multi-arm match, demonstrating that the lowering shares the comparator.

### 2.4 Match guards

```rust
match (state, count) {
    (State::Run, n) if n < N => keep_running(),
    (State::Run, _)           => stop(),
    _                         => idle(),
}
```

**Lowering.** Append the guard expression as an additional boolean to the arm's predicate. Lowers to `arm_match && guard ? rhs : next_arm`. Already representable as `Select` chains.
**Acceptance.** A guard that references a captured pattern binding compiles and produces correct simulation output. Reset-handling guards are particularly important and should be tested.

### 2.5 `@` bindings in patterns

```rust
match value {
    n @ 0..=15 => use_small(n),
    n          => use_large(n),
}
```

**Lowering.** `@` binding is a let-binding fused with a pattern. Pure macro-level transformation.
**Acceptance.** A kernel that uses `@` binding produces output identical to an equivalent kernel that does the let-binding manually.

### 2.6 Array destructuring patterns

```rust
let [a, b, c] = arr;
```

For fixed-size arrays. Slice rest patterns `[head, tail @ ..]` over fixed-size arrays are also tractable since `tail` has statically-known length.

**Lowering.** Array destructuring is a sequence of indexed loads. Already representable.
**Acceptance.** Equivalent to the corresponding `let a = arr[0]; let b = arr[1]; ...` form in emitted Verilog.

### 2.7 `?` operator on `Option` and `Result`

```rust
fn parse(buf: PacketBuf) -> Result<Header, ParseError> {
    let len = read_len(buf)?;
    let kind = read_kind(buf)?;
    Ok(Header { len, kind })
}
```

`Option`/`Result` already lower to RHIF's `Wrap` opcode. The `?` operator is sugar for `match x { Ok(v) => v, Err(e) => return Err(e.into()) }`.

**Lowering.** Macro-level desugaring. The `From`/`Into` conversion in the `Err` branch needs care — for the kernel subset, restrict `?` to cases where the error types match exactly (no `From::from` call), at least in Phase 1.
**Acceptance.** A kernel using `?` on `Option` produces identical simulation output to an equivalent `match`-and-`return` form. Test the kernel runs correctly when the early-return path is taken.

### 2.8 `for x in array` (value iteration)

```rust
for x in arr {
    sum = sum + x;
}
```

Currently the supported form is `for i in 0..N { ... arr[i] ... }`. Value iteration is a macro-level desugaring.

**Lowering.** Pure AST rewrite to `for i in 0..N { let x = arr[i]; ... }`.
**Acceptance.** Equivalent to indexed iteration in emitted Verilog.

### 2.9 Compile-time `assert!` / `static_assert`

```rust
const fn check_power_of_two(n: usize) -> bool { n != 0 && n & (n - 1) == 0 }

#[kernel]
fn widget<const N: usize>(...)
where rhdl::bits::W<N>: BitWidth,
{
    static_assert!(check_power_of_two(N), "N must be a power of two");
    ...
}
```

**Lowering.** Pure compile-time check; no hardware impact. Implementation reuses the existing `miette` diagnostic infrastructure for clear error messages.
**Acceptance.** A failed `static_assert` produces a compiler error with span pointing at the assertion. Successful asserts produce zero NTL nodes.

---

## 3 — Tier 2: Expressive additions (new IR or new methods)

These need real compiler work but are well-bounded.

### 3.1 More built-in methods on `Bits<N>` and `SignedBits<N>`

| Method | Maps to |
|---|---|
| `count_ones()` | popcount tree (logarithmic depth) |
| `count_zeros()` | popcount of `!self` |
| `leading_zeros()` | priority encoder of `self` reversed |
| `trailing_zeros()` | priority encoder of `self` |
| `reverse_bits()` | static wire permutation (zero gates) |
| `swap_bytes()` | static wire permutation (zero gates) |
| `rotate_left(n)`, `rotate_right(n)` | barrel shifter for runtime `n`; static wire permutation for const `n` |

Each is a self-contained lowering rule in `rhdl-core` plus a method on `rhdl-bits`. Several are dependencies of widgets in `widget-roadmap.md` (popcount for ECC, leading-zero count for floating-point normalization, reverse-bits for serial protocols and CRC reflect). Adding them centrally is strictly better than each widget rolling its own.

**Lowering.** New unary RHIF opcodes (`Popcount`, `Clz`, `Ctz`) lowering to NTL `Vector` reductions or new specialized NTL ops. `reverse_bits` and `swap_bytes` lower to NTL `Concat` with reordered wires (zero hardware cost — pure renaming).
**Acceptance.** Each method has Tier-1 unit tests in `rhdl-bits` (pure Rust correctness) and Tier-3 HDL snapshot tests in `rhdl-fpga` (verify the lowered Verilog).

### 3.2 Saturating arithmetic methods

```rust
let y = x.saturating_add(b8::MAX);
```

`Bits<N>::saturating_add()`, `saturating_sub()`, `saturating_mul()`. Currently the kernel uses 2's-complement wrapping. Saturation is essential for DSP — clipping samples instead of wrapping. Each lowers to a comparator + mux around the existing add/sub.

**Lowering.** Method on `Bits<N>` returning `Bits<N>`, recognized by the compiler and lowered to: compute the wide-result, check overflow, mux to MAX or MIN on overflow. No new IR opcode needed — composes existing `Binary`, `Select`, and `Cast`.
**Acceptance.** Test all four corner cases: positive overflow, negative overflow (signed), zero crossings.

### 3.3 Custom traits beyond `Digital` (compile-time monomorphized)

Today kernels can be generic over `T: Digital`. Letting users define marker traits — `trait Comparable: Digital` with associated functions — would let widgets be written generically over richer type families.

```rust
trait Comparable: Digital {
    fn compare(a: Self, b: Self) -> Ordering;
}

#[kernel]
fn min<T: Comparable>(a: T, b: T) -> T {
    match T::compare(a, b) { Ordering::Less => a, _ => b }
}
```

**Lowering.** Stays compile-time / monomorphized; never produces a `dyn Trait`. The `#[kernel]` macro performs type-directed dispatch at the same point Rust does.
**Acceptance.** A round-robin arbiter generic over a user's own `RequestPriority` trait compiles and simulates. No `dyn` object code is emitted.

### 3.4 Const arithmetic in generic positions

```rust
pub struct FIFO<const CAP: usize>
where rhdl::bits::W<{ CAP.next_power_of_two().ilog2() }>: BitWidth,
{
    ...
}
```

Stable Rust's `generic_const_exprs` is still nightly. RHDL's macro layer can pre-evaluate const expressions at the macro level and inject the resulting concrete constants into the type position. Very high payoff for any size-parameterized widget.

**Lowering.** `rhdl-macro-core` evaluates the const expression at macro time using a small const evaluator (or a stable subset of `evalexpr`, which is already a dependency). Emits the concrete constant in place. No effect on `rustc`'s view of generics.
**Acceptance.** A widget parameterized by capacity rather than address-bit-width compiles and produces the expected internal RAM size.

### 3.5 Closure desugaring to anonymous kernels

```rust
let mapper = |x: b8| -> b8 { x.wrapping_add(1) };
let mapped = stream.map(mapper);
```

Today you pass a kernel by name as `type Kernel = ...`. Closure-like syntax could be desugared at macro time into a synthetic `#[kernel] fn` and the surrounding type plumbing. Pure sugar — no new expressive power — but it makes `Map<T, S>::try_new::<...>()` ergonomics much friendlier.

**Lowering.** Macro-level rewrite. Restrictions: closure must be non-capturing (no environment), have explicit input and output types. Captured environment would require a state struct, which takes us out of the "pure desugaring" budget into Tier-3 territory.
**Acceptance.** A `stream::map` that previously required a named `#[kernel]` function compiles with an inline closure and produces identical simulation output.

### 3.6 Fixed-point Q-format type

```rust
let a: Q<4, 12> = ...;  // Q4.12 — 4 integer bits, 12 fractional bits
let b: Q<4, 12> = ...;
let c = a + b;          // Q5.12 (one extra integer bit for carry)
let d = a * b;          // Q8.24 (full-precision)
let e: Q<4, 12> = d.truncate();
```

Largely a library addition over `SignedBits<I + F>`, with a smarter `Mul` lowering that produces an extra-width intermediate and a documented `.truncate()` / `.saturate()` step. Mostly exists already in user space; would benefit from being canonicalized.

**Lowering.** The arithmetic operations on `Q<I, F>` lower to existing `SignedBits` arithmetic with a width-tracking type wrapper. The IR sees only the underlying bit operations.
**Acceptance.** A FIR filter widget written using `Q<I, F>` produces bit-identical output to the equivalent hand-written `SignedBits` form, with strictly cleaner source code.

---

## 4 — Tier 3: Research-grade

These are not quick wins but are worth scoping.

### 4.1 Minifloat / bfloat16 with hardware semantics

Not full IEEE 754. A fixed-precision variant with explicit hardware semantics: subnormal handling defined to flush-to-zero, NaN propagation defined as a single sentinel, round-to-nearest-even mandatory. Useful for ML inference accelerators on FPGAs. Reference: XLS (Google) has a minifloat library worth studying [3].

### 4.2 Slice patterns over fixed-size arrays beyond destructuring

```rust
match buf {
    [hdr_kind, len_lo, len_hi, payload @ ..] => parse(hdr_kind, len_lo, len_hi, payload),
    _ => malformed(),
}
```

Today the workaround is explicit indexing. The challenge is that `payload @ ..` needs a statically-known length, which is true for fixed-size arrays but the macro layer must compute it. Useful for stream-processing kernels.

### 4.3 Bounded `while-let`

```rust
let mut buf = packet.payload();
while let Some(byte) = buf.next() {
    process(byte);
}
```

A `while let` pattern with a compile-time-known iteration cap (the array length, in this case) would generalize the current `for i in 0..N` constraint. Useful for protocol parsers that want early termination. Adds compiler complexity around loop-bound inference and `break` semantics.

### 4.4 Try-into-error conversions

The Phase-1 `?` extension restricts to identical-error-type cases. A future extension would allow `From`/`Into` conversion in the `?`-error-path, matching standard Rust idioms. This requires letting trait-method calls participate in kernel lowering, which intersects with §3.3.

### 4.5 Iterator combinators on fixed-size arrays

`array.iter().fold(...)`, `array.iter().sum()`, `array.iter().enumerate()`, `array.iter().zip(other)`. Each desugars to a const-bounded `for` loop, but the rules for what counts as a synthesizable iterator chain need careful definition. Could be a compelling demo of "RHDL kernels read like ordinary Rust."

---

## 5 — Phasing

### Phase 1 — Pattern desugarings (one PR, ~2 weeks)

Bundle: `let-else`, or-patterns, range patterns, match guards, `@` bindings, array destructuring, `for x in array`, compile-time `assert`. All are macro-layer transformations; one mid-sized PR. Unblocks readable state-machine kernels for UART/SPI/I2C.

### Phase 2 — `Bits<N>` method library (one PR, ~2 weeks)

Bundle the bit-manipulation methods: `count_ones`, `count_zeros`, `leading_zeros`, `trailing_zeros`, `reverse_bits`, `swap_bytes`, `rotate_left`, `rotate_right`. Plus the saturating-arithmetic methods. Each is independently testable. Direct dependency of widgets in `widget-roadmap.md`.

### Phase 3 — `?` on Option / Result (one PR, ~1 week)

Restricted to identical-error-type cases. Builds on the existing `Wrap` opcode in RHIF.

### Phase 4 — Custom traits and const generic arithmetic (one PR each, ~3 weeks total)

Larger scope; touches the macro layer's type-directed dispatch and adds a const evaluator. High value for widget genericity.

### Phase 5 — Closure desugaring (~1 week)

Strictly non-capturing closures only. Ergonomic win for `stream::map`-style APIs.

### Phase 6+ — Tier 3 research items

Fixed-point type as a library before being a compiler feature. Minifloat after `Bits<N>` math is mature. Slice patterns and `while-let` after `for x in array` is in practice.

---

## 5.5 — Type-system library prerequisites and the kernel-domain gap

> **Why this section exists.** The earlier sections (§§2–4) extend the *syntax* of the kernel — what Rust spellings the proc-macro accepts. This section addresses a parallel concern: the *type-system library surface* in `crates/rhdl-core/src/types/`. Two of the items below are concrete, ready-to-implement library helpers that close real footguns surfaced by the Phase-2 RHIF property test work (see `rhif-formalization-plan.md`). The third is a research-grade language extension that the first two would set up, but that has independent value as a planning anchor.
>
> All three were surfaced as concrete consequences of building the Phase-2 random-program generators (PRs #15, #16, #17). They are bundled here because they form a coherent arc — from "make the type system easier to construct correctly" to "let the type system express the safe-input domain so the kernel doesn't need runtime checks for it."

### 5.5.1 — Public `Kind::option_of(T)` and `Kind::result_of(T, E)` helpers

**What.** Two new public methods on `Kind` (in `crates/rhdl-core/src/types/kind.rs`):

```rust
impl Kind {
    /// Construct the `Option<T>` enum kind in the canonical shape
    /// expected by `TypedBits::wrap_some` / `wrap_none` and the
    /// `Kind::is_option` predicate.
    pub fn option_of(payload: Kind) -> Kind { ... }

    /// Construct the `Result<T, E>` enum kind in the canonical
    /// shape expected by `TypedBits::wrap_ok` / `wrap_err`.
    pub fn result_of(ok: Kind, err: Kind) -> Kind { ... }
}
```

**Why we'd want this.**

The shape of `Option<T>` and `Result<T, E>` as `Kind::Enum` values is non-trivial: 2 variants in a specific order with specific names (`"None"` then `"Some"`; `"Ok"` then `"Err"`); a 1-bit MSB-aligned unsigned discriminant; the payload variant carrying a single-element tuple of the inner type; and a name string in the format `"Option::<{T:?}>"`.

This shape is currently established in *two separate places* — the `Digital` derive in `rhdl-macro-core` produces it, and `Kind::is_option` validates it. There is no public constructor. Anyone constructing `Option<T>` outside the proc-macro path (synthetic random-program generators, hand-built test fixtures, future external IR consumers) must re-derive the shape from `is_option`'s validation predicate, with the risk of getting it subtly wrong.

The Phase-2 random-program generator hit this exactly: `build_option_kind` lives privately in `crates/rhdl-core/src/rhif/property_tests.rs`, re-deriving the same shape the proc-macro already knows. That's a duplicated source of truth.

**Why it matters that this is harder than it looks: the canonical-form trap.**

`Kind::Enum` uses `internment::Intern<Enum>` for structural identity. Two `Kind::Enum` values are `==` only if their interned pointers match. If a user constructs `Option<Bits<8>>` via the proc-macro derive *and* via a hand-rolled helper, even a one-character difference (a stray space, a different name format, a different field ordering) produces two non-equal kinds that `is_option` accepts but `==` distinguishes.

In practice this would silently break any pass that does `kind == <Option<Bits<8>> as Digital>::static_kind()`. The pass would see "two Option<Bits<8>> kinds, neither equal to the other" and miss one of them. No diagnostic; just incorrect behaviour.

**Downstream benefits.**

1. **One canonical source of truth** for the shape of `Option<T>` and `Result<T, E>`. The proc-macro derive can call this helper instead of inlining the construction, eliminating the duplication.
2. **Test fixtures get safer.** Anyone writing a test that constructs an `Option<Bits<N>>` value (for VM inputs, for kind assertions, for RHIF construction) uses the canonical helper rather than reverse-engineering the shape.
3. **Random-program generators stop forking the type system.** The Phase-2 `build_option_kind` private helper goes away; the public helper takes its place.
4. **External IR consumers get a stable API.** Anyone (a future Coq mechanization, an IDE plugin, a debugger UI) that needs to construct `Option<T>` works against a stable public API rather than reading proc-macro internals.
5. **Sets up §5.5.3.** The refinement-types extension (below) needs canonical kind constructors for the bounded-integer shapes (`BoundedBits<N, MAX>`). The same problem appears at larger scale; solving it for `Option`/`Result` first establishes the pattern.

**Implementation sketch.**

`option_of` follows the form already documented in `Kind::is_option`:

```rust
pub fn option_of(payload: Kind) -> Kind {
    Kind::make_enum(
        &format!("Option::<{payload:?}>"),
        vec![
            Kind::make_variant("None", Kind::Empty, 0),
            Kind::make_variant("Some", Kind::make_tuple(vec![payload].into()), 1),
        ],
        Kind::make_discriminant_layout(
            1,
            DiscriminantAlignment::Msb,
            DiscriminantType::Unsigned,
        ),
    )
}
```

The complementary `is_option` predicate becomes the spec — any change to `option_of`'s output must keep `is_option` accepting it. Same for `result_of` and `is_result` (already present).

**Test discipline.**

The headline correctness test is the *equivalence-with-derive* assertion:

```rust
#[test]
fn option_of_matches_derive() {
    assert_eq!(
        Kind::option_of(<bool as Digital>::static_kind()),
        <Option<bool> as Digital>::static_kind(),
    );
    assert_eq!(
        Kind::option_of(<Bits<8> as Digital>::static_kind()),
        <Option<Bits<8>> as Digital>::static_kind(),
    );
    // Plus a struct-payload case, a tuple-payload case, and a
    // nested-Option case (which is structurally fine — RHDL's
    // `no nested Signal` rule applies to Signal, not to Option).
}
```

If the helper produces the wrong shape, this test fails immediately and the helper is retracted.

**Cost.** ~50 LOC of public API + ~50 LOC of equivalence tests. Single small PR, no compiler-pass changes, no IR changes.

**Position in the plan.** Ship before §5.5.2 (it's a dependency) and well before §5.5.3 (which presupposes it). Compatible with shipping in the same PR as §5.5.2.

---

### 5.5.2 — Safe `TypedBits::enum_variant(kind, variant_name, payload)` constructor

**What.** A new public method on `TypedBits` (in `crates/rhdl-core/src/types/typed_bits.rs`):

```rust
impl TypedBits {
    /// Construct a `TypedBits` value of `kind` (which must be a
    /// `Kind::Enum`) representing the named variant, with the
    /// supplied payload bits inserted at the correct position
    /// per the kind's discriminant layout.
    pub fn enum_variant(
        kind: Kind,
        variant_name: &str,
        payload: TypedBits,
    ) -> Result<TypedBits>;
}
```

**Why we'd want this.**

Constructing a `TypedBits` of enum kind manually requires knowing the *bit layout*: where the discriminant lives (per `Kind::pad`), how variant payloads are placed (MSB-aligned vs LSB-aligned), how the discriminant is encoded (unsigned vs signed; how many bits). This layout is documented in `Kind::pad`'s comments but is implicit in the derive-generated bit emission.

Concrete example from the Phase-2 enum random-program generator:

```rust
// Build template: discriminant = 1 (variant B), payload = zero.
// MSB-aligned 1-bit discriminant → discriminant lives at the
// last (highest) bit position.
let mut bits = vec![BitX::Zero; enum_kind.bits()];
*bits.last_mut().unwrap() = BitX::One;
let template = TypedBits::new(bits, enum_kind);
```

This works, but the author must know that "the discriminant goes in the last bit" — a specific consequence of `MSB`-alignment that is not visible from the variant API. Anyone unfamiliar with `Kind::pad`'s rules would write `bits[0] = One` (LSB-style) and produce a value that has discriminant 0 instead of 1. The VM would then dispatch on the wrong variant; the resulting bug would surface as "case dispatch picks the wrong arm" with no obvious cause.

**Why this is harder than it looks.**

The bit layout is a function of *both* the variant's payload size and the enum's full discriminant layout. For variant `B(payload)` of an enum where the largest variant has a 64-bit payload but `B` has only an 8-bit payload, the bits emitted for `B`'s payload must be *padded* to fit the largest payload region — and the padding goes in a specific place (depends on `MSB`/`LSB` alignment). The proc-macro derive handles this through `Kind::pad`; manual constructors silently produce wrong bit patterns that the VM will accept but interpret incorrectly.

This is the same canonical-form trap as §5.5.1 but at the value level rather than the kind level. A `TypedBits` whose bits don't match the kind's expected layout is "well-formed" by the type system (the bit count matches `kind.bits()`) but semantically wrong (the VM reads the discriminant from the wrong position).

**Downstream benefits.**

1. **Tests stop encoding implicit layout knowledge.** Hand-built enum test fixtures use `TypedBits::enum_variant(kind, "B", payload)` instead of `bits.last_mut() = One`. The intent is explicit; the layout is an implementation detail of `enum_variant`.
2. **Random-program generators get safer.** Phase-2's `generate_enum_program` constructs the template by direct bit manipulation; with `enum_variant`, that becomes a single function call.
3. **The `Wrap` opcode constructors compose with this.** `wrap_some(kind, payload)` is conceptually `enum_variant(kind, "Some", Tuple([payload]))` plus a discriminant check. Either `wrap_some` becomes a thin wrapper over `enum_variant`, or both share a common bit-layout helper.
4. **Diagnostic surface improves.** A wrong variant name returns `Err` (not a panic, not a silent miscompare); a payload-kind mismatch is caught at construction time rather than at the next VM read.
5. **Sets up §5.5.3.** Refinement / bounded-integer types need a parallel "construct a `Bits<N>` value with a runtime bound predicate" helper. The same canonical-construction pattern.

**Implementation sketch.**

`TypedBits::enum_variant(kind, name, payload)`:
1. Verify `kind` is `Kind::Enum`; otherwise return `Err(DynamicTypeError::CannotConstructEnumVariantOfNonEnumKind)`.
2. Look up the variant by name; if not found, `Err(DynamicTypeError::UnknownEnumVariant)`.
3. Verify `payload.kind == variant.kind`; if mismatched, `Err(DynamicTypeError::EnumVariantPayloadKindMismatch)`.
4. Pad the payload bits to the largest variant's size per the discriminant layout (use the existing `Kind::pad` infrastructure).
5. Concatenate the discriminant bits and the padded payload bits per the alignment rule.
6. Return `TypedBits { bits, kind }`.

The implementation reuses `Kind::pad`, `Kind::discriminant_layout`, and the existing per-variant kind lookup; it doesn't introduce new layout logic.

**Test discipline.**

The headline correctness test is *bit-equivalence with the proc-macro-derived `Digital::bin()`*:

```rust
#[test]
fn enum_variant_matches_digital_bin() {
    let derived: TypedBits = MyEnum::B(42u8).typed_bits();
    let helper = TypedBits::enum_variant(
        <MyEnum as Digital>::static_kind(),
        "B",
        42u8.typed_bits(),
    ).unwrap();
    assert_eq!(derived.bits(), helper.bits());
    assert_eq!(derived.kind(), helper.kind());
}
```

If the helper emits a different bit pattern than `Digital::bin()` for the same logical value, this test fails. Cover at least: `Option`-shaped enum, a `Result`-shaped enum, an enum with mixed-size variant payloads (so the padding logic is exercised), an enum with `LSB`-aligned discriminant, and an enum with multi-bit discriminant.

**Cost.** ~80 LOC of implementation + ~100 LOC of tests covering the layout corner cases. Single small PR, bundle with §5.5.1.

**Position in the plan.** Ship as a single PR with §5.5.1. The two together are the "type-system library improvements" PR.

---

### 5.5.3 — Refinement / bounded-integer types — closing the kernel-domain gap

> **Status: research target.** A genuine language extension; estimated 2–6 months for a competent compiler engineer. Sketched here so future work has a starting point.

**What.** Extend the kernel type system with refinement-typed integer kinds:

```rust
// Sugar at the kernel layer; lowers to Bits<N> + a static
// well-formedness check at construction time.
type SmallIdx = BoundedBits<8, 64>;        // 8-bit value, MUST be < 64
type RegId    = BoundedBits<5, 32>;        // 5-bit value, MUST be < 32
```

The type system would then enforce, at *type-check time*, that any value of kind `BoundedBits<N, MAX>` satisfies `value < MAX`. Constructing such a value requires either a literal that statically satisfies the bound, or an explicit `try_into` from a wider unconstrained `Bits<N>` (returning `Option<BoundedBits<N, MAX>>`).

**Why we'd want this.**

The kernel-language type system today says "any `Bits<8>` value is a `Bits<8>`," but a kernel that uses one as an array index into a 64-element buffer rejects 192 of the 256 values at *runtime* (via `RHDLDynamicTypeError::ArrayIndexOutOfBounds`). The type system permits values the kernel rejects.

This is the *kernel-domain gap*: the static type predicate is broader than the safe runtime domain. The runtime check (in the simulator and the synthesised hardware) is the safety net.

This shows up everywhere:

- **Array indexing.** `q.req_buf[idx]` where `q.req_buf: [Bits<8>; 64]` and `idx: Bits<8>` — 75% of `Bits<8>` values trip the bound.
- **Shift amounts.** `arg << amount` where `arg: Bits<32>` and `amount: Bits<8>` — values ≥ 32 trip `ShiftAmountMustBeLessThan`.
- **Discriminant values.** A `match` on a `Bits<3>` discriminant where only 5 of the 8 values are valid variant tags — the remaining 3 silently produce `dont_care` results.
- **FSM state transitions.** A `Bits<5>` representing a 21-state FSM — 11 of the 32 representable values are unreachable; reaching one would be a kernel bug, but the type system can't say so.

The Phase-2 property test work surfaced this empirically: random-bit fuzzing of widget inputs hits an out-of-domain ICE rate of ~25–100% depending on the widget. The Phase-2 `StructuredFirstCycle` workaround sidesteps the gap for one specific case (random `q` on protocol PHYs), but it's a workaround, not a solution.

**Why it matters for hardware specifically.**

Hardware doesn't have exceptions or panics. An out-of-range dynamic index in synthesised RTL is *implementation-defined*: depending on the synthesis tool, it might wrap, saturate, produce X, or emit a multiplexer with undefined behaviour for the out-of-range case. The simulator's panic is a development-time signal; the synthesised hardware just produces wrong data.

If the type system tracked the bound, the compiler would either:
- (a) Reject the kernel at compile time when an unbounded value is used as a bounded operand without an explicit `try_into`.
- (b) Lower the bound to a hardware check that explicitly handles the out-of-range case (e.g., generates a multiplexer that returns a known default for out-of-range indices, plus an error flag).

(a) is the type-system view; (b) is the codegen view. The right answer probably combines both: by default reject at compile time, but offer an explicit `BoundedBits::clamping_index` (or similar) for users who want the codegen to handle it.

**Why this is hard.**

Refinement types are a known-difficult research area (LiquidHaskell, RefinementML, F*, Z3-backed solvers). For RHDL specifically, the constraints are simpler than the general case:
- Bounds are integer constants, not arbitrary predicates.
- All values are finite-bit-width integers.
- No recursion, no higher-order functions, no closures with captures.

This narrows the problem from "general refinement types" to "value-range tracking on integer kinds with const-bound upper limits." That's tractable — it's essentially what type-state programming does for state machines, but on integer values.

The implementation likely involves:
1. A new kind `Kind::BoundedBits(N, MAX)` (and `BoundedSigned(N, MIN, MAX)`) added to `crates/rhdl-core/src/types/kind.rs`.
2. Type-rule extensions in the proc-macro (and in `crates/rhdl-core/src/types/`) so the inferred type of `idx_lit` (a literal) is `BoundedBits<W, MAX>` where `MAX` is the literal's value+1.
3. Subtyping: `BoundedBits<N, MAX1>` is a subtype of `BoundedBits<N, MAX2>` when `MAX1 ≤ MAX2`. Subtyping of `BoundedBits<N, MAX>` to `Bits<N>` (the unbounded supertype) is fine; the reverse needs an explicit `try_into`.
4. Dynamic index annotations: `arr[idx]` requires `idx: BoundedBits<_, N>` where `N = arr.len()`. Otherwise the kernel doesn't compile.
5. Runtime support: a `BoundedBits::try_new(value: Bits<N>) -> Option<BoundedBits<N, MAX>>` constructor that performs the bound check at runtime — the only place runtime checks remain.

**Downstream benefits.**

1. **The type system catches what the simulator currently catches with a panic.** "This `Bits<8>` cannot be used as an index into `[T; 64]`" becomes a compile-time error with a `miette` diagnostic, not a silent ICE at runtime.
2. **Synthesis becomes safer.** The compiler knows the bound, so it can either reject (the safe default) or generate explicit hardware (the opt-in path). Either way the silent "implementation-defined" case in synthesised RTL goes away.
3. **Property tests can fully randomise.** Random `BoundedBits<N, MAX>` values are by-construction in-domain, so the Phase-2 property suite no longer needs the `StructuredFirstCycle` workaround for protocol PHYs. Lowering correctness can run on fully-random `q` for any widget.
4. **State-machine encoding improves.** A `BoundedBits<5, 21>` FSM state is a self-documenting type — the bound says "this is a 21-state machine," and the type system enforces the unreachable-state constraint.
5. **Documentation becomes mechanical.** The bound is the documentation. `arr[idx: BoundedBits<8, 64>]` documents itself; today's `arr[idx: Bits<8>]` requires a comment explaining the implicit invariant.
6. **LLM-generated kernels get safer.** An LLM is likely to use `Bits<8>` for an FSM state in a 5-state machine, then forget that values 5–255 are unreachable. With `BoundedBits<3, 5>`, the LLM gets a compile error if it tries; with `Bits<8>`, the bug ships.

**What §§5.5.1 and 5.5.2 contribute.**

The refinement extension multiplies the surface of the type-system construction API: every `BoundedBits<N, MAX>` is a new `Kind` that needs a constructor, every `BoundedBits<N, MAX>` value needs a layout-correct `TypedBits`. The same canonical-form-trap that motivates §5.5.1 and §5.5.2 reappears at larger scale.

If §§5.5.1 and 5.5.2 ship first, the refinement extension can plug into the existing canonical construction pattern instead of forking it. If they don't ship, the refinement extension would either inline the kind construction (introducing N more places where the type-system shape can drift from the proc-macro derive) or block on §§5.5.1–5.5.2 anyway. Cleaner to ship the dependencies first.

**What §5.5.3 does not solve.**

- **Dynamic predicates.** `BoundedBits<N, MAX>` is a constant bound at type level; it cannot express "this value is the count of 1-bits in `arr`," which would require Z3-style solving. Out of scope.
- **Cross-kernel proofs.** A kernel that takes a `BoundedBits<8, 64>` from a caller relies on the caller having proved the bound. The type system tracks this through normal subtyping, but inter-kernel proofs across multiple compilation units may need additional machinery. Likely fine in practice given RHDL's strict no-recursion model.
- **Saturating arithmetic.** `a + b` where `a, b: BoundedBits<8, 64>` overflows the bound when the sum is ≥ 64. The type system needs an arithmetic rule that produces `BoundedBits<8, 127>` (sum bound), or an explicit `saturating_add` method that re-clamps. This is a design choice.

**Implementation sketch (high-level phases).**

1. **Phase A — Kind extension** (~3 weeks). Add `Kind::BoundedBits(N, MAX)` and `Kind::BoundedSigned(N, MIN, MAX)` to `types/kind.rs`. Extend `Kind::is_*`, `Kind::pad`, and `Kind::bits()` to handle them. Update `TypedBits` construction. No proc-macro changes yet.
2. **Phase B — Subtyping** (~3 weeks). Type-rule extensions in `rhdl-core::types::infer` so a `BoundedBits<N, MAX1>` flows to a `BoundedBits<N, MAX2>` context iff `MAX1 ≤ MAX2`, and to a `Bits<N>` context unconditionally.
3. **Phase C — Literal inference** (~2 weeks). Extend the proc-macro so an integer literal is inferred at the tightest possible bound; e.g., `4` in a context expecting `BoundedBits<8, _>` is inferred as `BoundedBits<8, 5>`.
4. **Phase D — Index typing** (~3 weeks). Make `arr[idx]` require `idx: BoundedBits<_, N>`. Provide a clear `miette` diagnostic when not satisfied.
5. **Phase E — Try-into API** (~1 week). Add `BoundedBits::try_new` and `Bits::clamp_to_bound` (or similar). Ergonomic surface for the explicit-conversion case.
6. **Phase F — Widget migration** (~2 weeks). Convert the protocol-PHY widgets (Modbus, CAN, etc.) to use `BoundedBits` for their internal indices and verify the property suite passes without the `StructuredFirstCycle` workaround.

Total: ~3–4 months of focused work. The estimate assumes a single experienced compiler engineer; with LLM assistance and good tooling, possibly faster.

**Test discipline.**

- Every type rule in `rhif-spec/type-system.md` gets a corresponding `BoundedBits` extension entry.
- Every widget that uses dynamic indexing gets a `BoundedBits` migration test that verifies the kernel still compiles and produces bit-identical Verilog.
- The Phase-2 property suite's `StructuredFirstCycle` workaround can be removed for any widget whose indices are `BoundedBits`-typed; that's the validation that the gap closed.

**Position in the plan.**

After §§5.5.1 and 5.5.2 (small dependencies). After Phase 4 of the existing kernel-language phasing (§5) — that work establishes the const-generic-arithmetic infrastructure that bounded-integer types depend on. Probably Phase 7 or 8 in the overall ordering.

---

## 6 — What stays forbidden

These are not items deferred to a later phase; they are intentionally outside the language for hardware-modelling reasons. Document them clearly so users (and LLMs) don't waste time trying.

**References (`&`, `&mut`).** The kernel's value-only model is what makes pipelining and retiming sound; introducing aliasing would entangle the IR with lifetime analysis and break the "kernel is a pure function" invariant. Auto-pipelining (per `auto-pipelining-plan.md`) depends on this.

> **Considered and rejected: accepting `&T` / `&[T; N]` in helper-kernel parameters as a macro-layer desugaring.** The argument was: hardware has no references, so `&T` and `T` are operationally identical at the IR; the macro could strip the `&` before lowering and let users / LLMs write idiomatic Rust (`fn helper(buf: &[b8; 16])`). Rejected because the cost/benefit is negative:
>
> 1. **Zero new expressiveness.** All `Digital` types are `Copy`, all are bit-encoded and small (the largest realistic `Digital` struct is around 100 bytes). There is no copy-avoidance argument inside a kernel; references and values lower to identical IR.
> 2. **The benefit is purely cosmetic** — supporting a stylistic preference imported from heap-allocating Rust where references matter for ownership. In the kernel subset, ownership is not a thing.
> 3. **The cost is a sprawl of new diagnostics** for every adjacent thing that *can't* be allowed: `&mut T` (no, breaks purity), `&[T]` slices (no, runtime length), `&T` in `let` bindings (no, alias tracking), `&T` in return types (no, lifetime), `&T` on top-level kernels (no, framework contract), `&&T` / `&Box<T>` / `&Vec<T>` (rejected by other rules but with confusing messages once `&Bits<8>` works two lines up). Each is a hand-crafted `miette` diagnostic to write, test, and keep consistent.
> 4. **Two ways to write the same thing** — review burden, style debates, no semantic difference. Strictly negative for code review and for LLM-assisted refactor (which this project values heavily).
>
> **Better fix.** Improve the *existing* rejection diagnostic so the user (or LLM) gets it right the first time. A clear span on the `&` plus the message "RHDL kernels pass `Digital` values by value — all `Digital` types are `Copy`. Replace `&T` with `T`" teaches the underlying rule (pass-by-value is the model) instead of papering over it. One-line span fix in the existing rejection path. See `widget-roadmap.md` task for the diagnostic improvement.

**Heap allocation, `Vec`, `Box`, `String`.** No hardware story; would require dynamic resource allocation that doesn't exist on FPGAs. Fixed-size arrays cover the legitimate use cases.

**`dyn Trait` / trait objects.** Dynamic dispatch needs runtime vtables. Static dispatch via monomorphization (which the Tier-2 custom traits would use) is fine.

**Recursive types.** `enum Tree { Leaf, Node(Box<Tree>) }` cannot have a finite hardware representation. Should produce a clear compiler error pointing at the recursion site.

**`unsafe`.** No legitimate use case in the kernel subset. Hardware-level type punning (`mem::transmute`) is unnecessary because the type system already exposes bit-level views via `Digital::bin()`.

**Floating-point IEEE 754.** The full IEEE spec — denormals, NaN propagation rules, rounding modes — requires hundreds of gates per op and dedicated FPU-like hardware. Worth doing eventually as a black-boxed library widget rather than as a kernel primitive. Tier-3 minifloat (§4.1) is the realistic compromise.

**Lambdas / closures with captures.** Captured environments would require closure structs. Non-capturing closures (Tier-2 §3.5) are pure sugar; capturing closures break the value-only model.

**Runtime-bounded iteration (`while`, `loop`).** Hardware needs known upper bounds on cycle counts to fit in finite gates. Const-bounded `for` and the proposed bounded `while-let` (§4.3) are the supported forms.

---

## 7 — Acceptance criteria for the spec as a whole

Each individual extension shipping satisfies CLAUDE.md's contract — code, tests at every applicable tier, documentation, validation artifacts, and a CHANGELOG entry — for both the language feature itself and at least one widget that uses it.

A Tier-1 / Tier-2 extension is "done" when:

1. The kernel-language change compiles and all existing kernel tests still pass.
2. New tests in `crates/rhdl/tests/` cover the new syntax — typically a positive test (well-formed kernel) and a negative test (kernel mis-using the feature, expecting a `miette` diagnostic).
3. At least one existing widget is rewritten to use the new feature, with the resulting Verilog identical to the pre-extension version (sanity check that no semantics changed).
4. The book gets a new `kernels/<feature>.md` chapter referenced from `doc/book/src/SUMMARY.md`.
5. `CLAUDE.md` §4 is updated to reflect the new allowed/forbidden state.

---

## 8 — References

[1] Basu, Samit. "RHDL: Rust as a Hardware Description Language." LATTE '25, March 2025. (`doc/latte25/latte.tex`.)

[2] *The Rust Reference* — pattern matching, range patterns, or-patterns, slice patterns. https://doc.rust-lang.org/reference/patterns.html

[3] XLS Project (Google). "Accelerated HW Synthesis." https://google.github.io/xls/ . The `xls/dslx/stdlib/float.x` minifloat library is a relevant reference for hardware-friendly floating-point semantics.

[4] Rust RFC 3137 — `let-else` statements. https://rust-lang.github.io/rfcs/3137-let-else.html

[5] Rust RFC 1492 — `..=` range patterns. https://rust-lang.github.io/rfcs/1492-dotdoteq-range-patterns.html

[6] Rust Tracking Issue 95228 — `generic_const_exprs`. https://github.com/rust-lang/rust/issues/76560 . The unstable feature whose stable equivalent we approximate with macro-time const evaluation.

[7] Skarman, F., Gustafsson, O. "Spade: An Expression-Based HDL With Pipelines." OSDA 2023. — Spade also takes a "Rust-like syntax, hardware-restricted subset" approach; a useful comparison for design choices around what to admit and what to forbid.

[8] Bluespec System Verilog. Arvind, R.S. Nikhil. "Bluespec System Verilog: Efficient, Correct RTL from High-Level Specifications." MEMOCODE 2004. — The atomic-action / scheduler approach to hardware DSL design, contrasted with RHDL's value-functional approach.

---

## 9 — Open questions

- **Granularity of CHANGELOG entries.** Is a per-extension CHANGELOG entry enough, or does each acceptance-test widget rewrite warrant its own?
- **Stability story.** Should new kernel-language features be opt-in via a `#[rhdl(features(...))]` attribute, with stabilization criteria, or accepted as base language?
- **Diagnostic quality.** A poorly-implemented `?` operator that produces "expected `Result`, found `_`" diagnostics is worse than not having `?` at all. Per-extension testing must include negative-path diagnostic snapshots via `expect_test`.
- **LLM eval harness.** Does the kernel-language extension move the needle on the LLM-assisted evaluation harness proposed in `rhdl-deep-dive.md` §3? Run a corpus pre- and post-extension to measure.
