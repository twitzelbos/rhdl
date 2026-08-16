# From RV32I pain points to surgical compiler fixes

*Captured 2026-05-02; corrected after audit of the existing API surface. Source material: `crates/rhdl-rv32i/rv32i-analysis.md` and `crates/rhdl-rv32i/my_experience_05012026_by_claude.md`. Audience: an engineer with bandwidth between Alto wrap-up and RCStream Phase 1 launch.*

---

## Frame

The RV32I implementation surfaced seven distinct pain points. After re-auditing the current `rhdl-bits` and `rhdl-macro-core` API surface (see `notes/bits-and-bitarrays-syntax-guide.md`), three of those pain points are **partially already addressed in the existing API** under a different syntax than the rv32i decoder author used. Two are genuine gaps requiring new compiler features. One is documentation. One is a macro-layer modification.

This document triages each pain point honestly: capability that already exists, ergonomic wrappers around existing capability, genuinely new capability, modification, and documentation. None require new IR opcodes or new compiler passes.

The work is parallel-safe with respect to RCStream Phase 1, the early DSP widgets, and Alto wrap-up — these all live in `rhdl-macro-core` and `rhdl-bits`, while the active widget work lives in `rhdl-fpga` and `rhdl-rule`. No file overlap.

> **Status correction (2026-05-02):** Earlier versions of this document scoped Tasks #78 (bit slicing) and #81 (numeric match patterns on `Bits<N>`) as full new capabilities. They are not. Match on `Bits<N>` with constructor-form literal patterns (`Bits::<3>(0b001) => ...`) **already works** per `crates/rhdl/tests/literals.rs:30-36`; bit slicing **already works** as `(x >> LO).resize::<W>()`. The surgical fixes are ergonomic wrappers, not capability additions. The bits-and-bitarrays-syntax-guide.md is the authoritative reference for what's already shipped.

---

## Triage at a glance

| # | Pain point | Fix | Effort | Stable Rust? | Already partially works? | Touches |
|---|---|---|---|---|---|---|
| A | `bits::<N>(literal)` ceremony at every literal | `bits!()` proc-macro for inference | 1-2 days | ✅ yes | No — proc-macro doesn't exist | `rhdl-macro` |
| B | If-else-if cascades on `Bits<N>` for funct3-style dispatch | **DROPPED** per purity rule — use the already-shipped constructor form `Bits::<3>(0b001) => ...` instead | 0 (refactor only) | ✅ pure Rust, no compiler change | **Yes — constructor form `Bits::<3>(0b001) => ...` works today** | `crates/rhdl-rv32i/src/decoder.rs` (refactor) |
| C | `(x >> N) & mask` instead of `x[hi:lo]` | `slice!(x, HI, LO)` proc-macro | 2-4 days | ✅ yes | **Yes — `(x >> LO).resize::<W>()` already works** | `rhdl-macro` |
| D | 12-line B-type / J-type immediate construction | `concat!()` + `repeat!()` proc-macros | ~1 week | ✅ yes | No — proc-macros don't exist | `rhdl-macro` |
| E | 12-element `Q`/`D` tuple ceiling | Struct-shaped Q/D when count > 12 | 2-3 weeks | ✅ yes | No — derive emits tuples | `rhdl-macro-core` (`SynchronousDQ` derive) |
| F | `bits::<32>(!0x88u32 as u128)` compiled cleanly, crashed iverilog | Tighter literal validation | ~1 week | ✅ yes | Unknown — needs verification | `rhdl-macro-core`, `rhdl-vlog` |
| G | `d.X` vs `q.X` semantics confusion | Diagnostic + book-chapter fix | 2-3 days + docs | ✅ yes | No — diagnostic doesn't exist | `rhdl-macro-core` (lint only) |

Total scoped work across all six remaining items (B is now a refactor): **2-4 weeks of one engineer**. Items A, C, G are afternoon-to-week jobs that an engineer can knock off between deeper work; items D, F are 1-week single-PR shippable; item E is the only one needing a small architectural conversation before implementation. Item B (formerly a compiler change) is a few hours of decoder refactoring against the already-shipped constructor form.

**The corrected framing:** items B and C are not "add a missing capability" — they are "ship a more ergonomic spelling of an existing capability." Item B further collapses to "refactor the decoder" because the existing constructor form is already pure Rust and works; the proposed bare-integer form failed the rust-purity test. The rv32i decoder author either didn't know the working idioms (per `notes/bits-and-bitarrays-syntax-guide.md`) or chose the verbose form for stylistic consistency. Either way, the deliverable is the cleaner spelling, not the underlying mechanism.

**Stable Rust + purity constraint.** All proposed compiler changes target stable Rust *and* must pass the rust-purity test from a coder's perspective (per `notes/bits-and-bitarrays-syntax-guide.md` §9): the syntax the user types must be syntax rustc would accept anywhere, not just inside the magic context. Macros are fine; methods are fine; AST rewriting that makes rustc-illegal syntax legal is rejected. The first design pass for Task #78 was a method `slice<const HI, const LO>(self) -> Bits<{HI - LO + 1}>`; this requires `feature(generic_const_exprs)`, which is **not on a stabilization path** (the replacement, `min_generic_const_args`, is in prototype phase as of mid-2025 with no committed stabilization timeline as of May 2026, and may be blocked on the new trait solver). Task #78 has been re-scoped as a `slice!()` proc-macro that computes the result width at expansion time. Task #81 was originally proposed as a kernel-macro AST rewrite (bare-integer match arms); per the purity rule this is **dropped** in favour of teaching the already-shipped constructor form.

---

## A. `bits!()` macro — 1-2 days, pure addition

### The pain

Every literal in `decoder.rs`:

```rust
let opcode_bits: Bits<7> = (instr & bits::<32>(0x7F)).resize();
let f3_zero: Bits<3> = bits::<3>(0);
let f3_one:  Bits<3> = bits::<3>(1);
// ... seven more constants ...
let f7_alt:  Bits<7> = bits::<7>(0b0100000);
```

The `bits::<N>(literal)` ceremony adds visual noise to every line. Compounds badly when chained or used in match arms.

### The fix

Add a procedural macro `bits!()` that infers the bit width from the literal's value and the binding's declared type:

```rust
let opcode_bits: Bits<7> = (instr & bits!(0x7F)).resize();
let f3_zero: Bits<3> = bits!(0);
let f3_one:  Bits<3> = bits!(1);
let f7_alt:  Bits<7> = bits!(0b0100000);
```

The macro expands to `bits::<N>(literal)` where `N` is taken from the surrounding type context if available, or computed from the literal's bit-width if not. This is the same shape as `format!()` calling `Display::fmt` — it's syntactic sugar around an existing function.

### Why surgical

It's a single function-like proc-macro in `rhdl-macro` that emits an existing call. No IR involvement, no kernel-language change, no semantics change.

### Acceptance

- A literal `bits!(0x1F)` resolves to `Bits<5>` (or whatever the context demands).
- Used inside a `#[kernel]` body, the emitted Verilog is byte-identical to the equivalent `bits::<N>(0x1F)` form.
- Snapshot test on the rv32i decoder shows ~40 lines of literal ceremony compressed to ~40 lines of `bits!()` calls — same line count but vastly less visual noise. (The bigger gains are in conjunction with B and C below.)

---

## B. Match-on-`Bits<N>` — DROPPED per purity rule, refactor uses already-shipped form

### What already works today

**Match on `Bits<N>` with constructor-form literal patterns is shipped and tested.** From `crates/rhdl/tests/literals.rs:30-36`:

```rust
match x {
    Bits::<8>(0) => b3(0),
    Bits::<8>(1) => b3(1),
    Bits::<8>(2) => b3(1),
    Bits::<8>(3) => b3(2),
    _ => b3(4),
}
```

The kernel macro accepts `Bits::<N>(literal)` patterns; the VM and Verilog round-trip both work; or-patterns compose with this form (per task #30, shipped). So the rv32i decoder funct3 dispatch could already be:

```rust
match funct3 {
    Bits::<3>(0b000) => if funct7 == b7(0b0100000) { AluOp::Sub } else { AluOp::Add },
    Bits::<3>(0b001) => AluOp::Sll,
    Bits::<3>(0b010) => AluOp::Slt,
    Bits::<3>(0b011) => AluOp::Sltu,
    Bits::<3>(0b100) => AluOp::Xor,
    Bits::<3>(0b101) => if funct7 == b7(0b0100000) { AluOp::Sra } else { AluOp::Srl },
    Bits::<3>(0b110) => AluOp::Or,
    Bits::<3>(0b111) => AluOp::And,
}
```

That's already a major readability win over the if-else-if cascade. The rv32i decoder didn't use this form for either of two reasons: (a) the author didn't know it worked, or (b) preferred the explicit pre-declared constants for stylistic reasons. The bits-syntax-guide now documents the working idiom.

### Why the bare-integer form is dropped

The original task #81 proposal was: extend the kernel macro to accept `match funct3 { 0b001 => ... }` (bare integer literal patterns) on `Bits<N>` scrutinees by AST-rewriting them to `Bits::<3>(0b001) => ...` before rustc sees them.

That proposal **fails the rust-purity test** (per `notes/bits-and-bitarrays-syntax-guide.md` §9). The bare form `match my_bits { 0b001 => ... }` is rejected by plain rustc with "mismatched types: expected `Bits<3>`, found integer." Making it work requires the kernel macro to rewrite syntax that rustc would otherwise reject — the same AST-rewrite mechanism that makes `RuleCtx<W>` magical inside `rule_kernel!`. From a Rust coder's perspective this is surprise magic: copy the line outside the kernel context and rustc rejects it.

The constructor form `Bits::<3>(0b001) => ...` is shipped, pure Rust, and reads acceptably. Use it.

### What ships instead — refactor only

There is no compiler change for item B. The deliverable is a refactor of `crates/rhdl-rv32i/src/decoder.rs` to use the constructor form for all funct3/funct7 dispatch, replacing the eight pre-declared `f3_zero`/`f3_one`/etc. constants and the if-else-if cascades. Effort: a few hours. No new task; merge with the existing rv32i polish task.

### Acceptance for the refactor

- All five funct3 dispatches in `decoder.rs` (R-type, I-type, Load, Store, Branch, System) use match-with-constructor-patterns.
- The eight `f3_*` named constants are deleted.
- Tier-3 HDL snapshot expectations are byte-identical (the constructor form lowers to the same Verilog as the if-else-if cascade).
- Line count in `decoder.rs` drops by approximately 100 lines.

### Restrictions

- Patterns must fit in the scrutinee's bit width. `match (b3) { 0b1000 => ... }` is a compile error.
- Range patterns (`0b000..=0b011 => ...`) are out of scope for v1; they are a §2 follow-on.
- Or-patterns inside numeric literals (`0b001 | 0b010 => ...`) compose with the existing or-pattern support.
- Wildcard `_` works as it does today.

### Acceptance

- Matching on `Bits<3>`, `Bits<7>`, etc. with integer-literal arms compiles and produces byte-identical Verilog to the if-else-if cascade form.
- The existing `match` exhaustiveness check still fires; missing literal patterns require an `_` arm or a compile error.
- Negative test: a literal pattern wider than the scrutinee produces a clear miette diagnostic.
- Refactor RV32I `decoder.rs` to use the new form; line count reduces by ~120 lines (~33%) with no behavior change. Use `expect_test` snapshot diff as the audit.

### Where this fits in the plan

This is the highest-priority entry from the rv32i-analysis "what hurt" section. Already discussed in `kernel-language-extensions.md`; this doc adds engineering scope.

---

## C. `slice::<HI, LO>()` ergonomic wrapper — 2-4 days, ergonomic wrapper

### What already works today

Bit-field extraction is already supported via `(x >> LO).resize::<W>()`. The `resize` method (in `rhdl-bits/src/bits_impl.rs:171`) truncates the upper bits when shrinking; the AND-mask in the rv32i decoder's pattern is **redundant**. From the decoder:

```rust
// What rv32i decoder does today (redundant mask):
let rd: Bits<5> = ((instr >> 7) & bits::<32>(0x1F)).resize();

// What it could already do today (no mask needed):
let rd: Bits<5> = (instr >> 7).resize();

// The two forms produce byte-identical Verilog.
```

So bit-slicing is already a one-liner with `(x >> LO).resize::<W>()`. The bits-syntax-guide documents this idiom. The rv32i decoder uses the verbose form for historical reasons; new code should drop the mask.

### The fix — proc-macro spelling (stable Rust)

What's still pending: a `slice!()` proc-macro that takes the **HI** parameter explicitly so the call site reads like Verilog `instr[HI:LO]`:

```rust
let rd:     Bits<5> = slice!(instr, 11, 7);
let funct3: Bits<3> = slice!(instr, 14, 12);
let funct7: Bits<7> = slice!(instr, 31, 25);
```

**Why a proc-macro and not a method.** The natural method signature would be:

```rust
// ❌ requires feature(generic_const_exprs) — NIGHTLY only
pub const fn slice<const HI: usize, const LO: usize>(self) -> Bits<{HI - LO + 1}>
```

The return-type computation `Bits<{HI - LO + 1}>` requires `generic_const_exprs`, which is not on a stabilization path. Its planned replacement, `min_generic_const_args` (MGCA), is in prototype as of mid-2025 with no firm 2026 stabilization commitment. Targeting stable Rust today means not relying on either feature.

A proc-macro `slice!(x, HI, LO)` sidesteps the problem entirely: the macro computes `W = HI - LO + 1` at expansion time and emits `(x >> LO).resize::<W>()` — fully stable Rust, same generated Verilog.

### Effort and scope

2-4 days. Pure addition to `rhdl-macro` (proc-macro crate). No IR change. No const-generic gymnastics.

### Acceptance

- `slice!(instr, 11, 7)` on a `Bits<32>` returns `Bits<5>` and emits identical Verilog to `(instr >> 7).resize::<5>()`.
- Macro-time errors for `LO > HI`, `LO >= N`, `HI >= N` (computed in the macro body when it knows the literal values).
- Refactor the rv32i decoder's six field extracts as the canonical demonstration; line count drops by roughly half.

### Future direction (when MGCA stabilizes)

If `min_generic_const_args` ships in stable Rust someday, the method form `instr.slice::<11, 7>()` becomes implementable as a true method with the result type `Bits<{HI - LO + 1}>`. At that point the `slice!()` macro can be re-pointed at the method (or kept as a back-compat alias). Until then, the macro is the answer.

### Sequencing note

Same pattern as B: the underlying capability `(x >> LO).resize()` already works, so a useful pre-step is to refactor rv32i decoder.rs *today* to drop the redundant `& bits::<32>(MASK)`. That ships ~50% of the readability gain immediately. The `slice` method is then the polish.

### Acceptance

- `instr.slice::<11, 7>()` on a `Bits<32>` returns `Bits<5>` and Verilog-emits as `instr[11:7]` (post the prettier-Verilog work) or as the equivalent shift-and-mask.
- Type-level: `LO > HI`, `LO >= N`, `HI >= N` are compile errors.
- Refactor the rv32i decoder's six field extracts; lines reduce by ~60% on those lines.
- The same machinery extends to the Phase A `OperandSpec` decoder for the future VAX core (slide 4 of the Tier C plan).

### Composition with B

Once both ship, the decoder's R-type case becomes:

```rust
match instr.slice::<14, 12>() {
    0b000 => if instr.slice::<31, 25>() == 0b0100000 { AluOp::Sub } else { AluOp::Add },
    0b001 => AluOp::Sll,
    ...
}
```

That reads like a spec table.

---

## D. `concat!()` macro — ~1 week, pure addition

### The pain

The B-type and J-type immediate construction is the most painful code in `decoder.rs`:

```rust
#[kernel]
pub fn imm_b(instr: Bits<32>) -> Bits<32> {
    let bit12: Bits<32> = ((instr >> 31) & bits::<32>(1)) << 12;
    let bit11: Bits<32> = ((instr >> 7)  & bits::<32>(1)) << 11;
    let bits_10_5: Bits<32> = ((instr >> 25) & bits::<32>(0x3F)) << 5;
    let bits_4_1:  Bits<32> = ((instr >> 8)  & bits::<32>(0xF))  << 1;
    let combined: Bits<32> = bit12 | bit11 | bits_10_5 | bits_4_1;
    let sign_mask: Bits<32> = if (instr >> 31) & bits::<32>(1) != bits::<32>(0) {
        bits::<32>(0xFFFF_E000)
    } else {
        bits::<32>(0)
    };
    combined | sign_mask
}
```

Verilog programmers write:

```verilog
assign imm_b = {{19{instr[31]}}, instr[31], instr[7], instr[30:25], instr[11:8], 1'b0};
```

One line versus 12.

### The fix

A `concat!()` macro that takes positional bit fragments and produces a `Bits<N>` whose width is the sum of the fragments' widths. With a `repeat!()` (or `:N` postfix syntax) for replicated bits.

```rust
#[kernel]
pub fn imm_b(instr: Bits<32>) -> Bits<32> {
    concat!(
        repeat!(instr.slice::<31, 31>(), 19),  // sign extension
        instr.slice::<31, 31>(),               // imm[12]
        instr.slice::<7, 7>(),                 // imm[11]
        instr.slice::<30, 25>(),               // imm[10:5]
        instr.slice::<11, 8>(),                // imm[4:1]
        bits!(0b0_u1),                         // imm[0] = 0
    )
}
```

That's the structure. Six lines, each one a bit-field; reads top-to-bottom in the same order the spec specifies the immediate.

### Why surgical

Macro-only. `concat!()` lowers to a series of shifts and OR'd assignments — exactly what the user writes today by hand. `repeat!()` lowers to repeated concatenation. The downstream IR sees the desugared form.

### Composition with C

Without bit slicing, `concat!()` is awkward because every fragment needs explicit shift-and-mask. Ship C first, D second.

### Acceptance

- `concat!(a, b, c)` where `a: Bits<3>`, `b: Bits<2>`, `c: Bits<5>` returns `Bits<10>` with `a` in the top 3 bits.
- `repeat!(a, N)` where `a: Bits<W>` returns `Bits<N*W>` with `a` repeated `N` times. (Or use a sugar like `a:N` if the macro layer can parse it.)
- Type-level errors when widths can't be statically determined.
- Refactor `imm_b`, `imm_j`, and `imm_s` in the rv32i decoder; line count drops by ~40 lines.

---

## E. `Q`/`D` tuple ceiling — 2-3 weeks, modification

### The pain

The 5-stage pipelined CPU has many sub-circuit fields — `pc`, `if_id`, `id_ex`, `ex_mem`, `mem_wb`, `rf`, `csrs`. Seven fields, well within the 12-element auto-derived tuple ceiling, but only because the inter-stage register bundles are themselves `Digital` structs (`IfId`, `IdEx`, etc.) rather than independent DFFs. From the diary:

> "I hit the 12-element tuple ceiling on Q/D exactly once (narrowly avoided in the pipeline by keeping the IF/ID, ID/EX, EX/MEM, MEM/WB bundles as Digital structs rather than independent DFFs) and would have needed the §3.1 protocol-PHY pattern for a real-world CPU with more sub-circuits — that ceiling is going to bite the next person who tries to do anything ambitious."

The protocol-PHY pattern (CLAUDE.md §3.1, captured for CAN-RX in `notes/synchronous-tuple-ceiling-can-rx.md`) is the documented workaround: bundle multiple internal DFFs into one extras-DFF struct. This works but is genuinely awkward and doesn't compose with `#[derive(Fsm)]` cleanly.

### The fix

The `SynchronousDQ` derive currently generates `Q` and `D` as tuples (which inherit Rust's stdlib trait impls — `PartialEq`, `Hash`, etc. — that max out at 12 elements). Modify the derive to generate **named-field structs** instead:

```rust
// Today (auto-generated by SynchronousDQ derive — tuple form):
type Q = (FieldA, FieldB, FieldC, ...);  // breaks past 12
type D = (FieldA, FieldB, FieldC, ...);  // breaks past 12

// After (auto-generated as struct):
#[derive(Digital, Clone, Copy, PartialEq, Debug)]
pub struct PipelinedCpuQ {
    pub pc:      <dff::DFF<Bits<32>>      as SynchronousIO>::O,
    pub if_id:   <dff::DFF<IfId>          as SynchronousIO>::O,
    pub id_ex:   <dff::DFF<IdEx>          as SynchronousIO>::O,
    // ... no upper limit on field count
}
pub struct PipelinedCpuD { ... };
```

The user's kernel code is unchanged because it accesses fields by name (`q.if_id`, `d.if_id`) — already what the user writes today. The internal representation just becomes a struct instead of a tuple.

### Why this isn't trivial

Three subtleties:

1. **Backwards compatibility.** Existing widgets that opt into `dq_no_prefix` and access fields like `q.field` already work both ways. But the derive's emitted tuple type is referenced by user code in some test surfaces. An audit is needed.

2. **The `Digital` derive on the struct must work for arbitrary field count.** Today `Digital` is implemented for tuples up to 12 by the `derive(Digital)` macro. For a struct, `derive(Digital)` already handles arbitrary field counts. So the fix is "stop using tuples, use structs." But this requires the `SynchronousDQ` derive to emit a fresh struct type and its `Digital` derive — straightforward proc-macro work.

3. **The auto-prefix logic** (`q.<sub_widget>` vs `q.<field>` per `#[rhdl(dq_no_prefix)]`) must continue to work. This is the part that needs care.

### Why "modification" not "addition"

It changes the existing `SynchronousDQ` derive's emission. Existing widgets continue to work (their kernel code is unchanged) but the IR sees a different representation for the Q/D types. An expect_test snapshot regen across the entire widget corpus is required.

### Acceptance

- A `SynchronousDQ` widget with 20 sub-circuit fields compiles and behaves identically to the equivalent 12-bundled-into-extras-struct widget.
- All existing widgets re-emit byte-identical Verilog (audit via `cargo test --all` with `UPDATE_EXPECT=1`; review every diff).
- The protocol-PHY pattern from CLAUDE.md §3.1 becomes optional, not mandatory. Existing users of the pattern continue to work; new users can choose either form.
- A new test demonstrates a 20-DFF pipelined CPU compiling cleanly without the bundled-state workaround.

### Risk profile

Medium because of the corpus-wide snapshot regen. If the change inadvertently alters bit layout or field ordering, it ripples to every downstream test. Mitigation: a dedicated PR with one canary widget first, then the corpus migration.

---

## F. Kernel literal validation — ~1 week, addition

### The pain

From the diary:

> "`bits::<32>(!0x88u32 as u128)` compiled cleanly in Rust, lowered to RHIF, and then *crashed iverilog* with a parse error."

The expression is a valid Rust literal (it produces a `u128` value), it satisfies `bits::<32>` (the value fits in 32 bits after truncation), but the macro layer doesn't validate that the resulting Verilog literal will actually parse. The user wastes time debugging an iverilog crash for a kernel-level expressiveness issue.

### The fix

Add a validation pass in `rhdl-macro-core` (or `rhdl-vlog` if it's a Verilog-emission concern) that checks every `bits::<N>(literal)` and `signed::<N>(literal)` expression at macro-time:

1. The literal must reduce to a constant expression.
2. The reduced value must fit in `N` bits (or sign-extend cleanly for `SignedBits<N>`).
3. The expression must produce Verilog output that is parseable by all our supported simulators.

If any check fails, emit a span-precise miette diagnostic with the offending expression highlighted and a suggested replacement.

### Why surgical

A new validation function called from the existing kernel-macro pipeline. No new IR, no new opcodes. Pure addition.

### Acceptance

- The exact failing expression from the diary, `bits::<32>(!0x88u32 as u128)`, produces a compile error at macro-expansion time, not at iverilog time.
- The diagnostic suggests `bits::<32>(0xFFFF_FF77)` (or the equivalent rewrite) as the fix.
- A regression test in `crates/rhdl/tests/` enumerates 5-10 expressions known to crash iverilog and asserts that each produces a clear miette diagnostic at macro-expansion time.

### Low-stakes addition

Doesn't require existing snapshots to change; only adds new diagnostics. Easy to land in a single PR.

---

## G. `d.X` vs `q.X` confusion — 2-3 days + docs

### The pain

From the diary:

> "The d/q semantics — `d.csrs` is the *input* to the CSR child this cycle, combinationally, while `q.csrs` is its *output* this cycle, also combinationally, and the child's DFFs commit at the cycle edge — took me three CHANGELOG entries to articulate confidently, and I bet I'm still slightly wrong somewhere."

This isn't a compiler bug; it's a documentation gap. But the compiler can help by detecting common misuse patterns and producing diagnostics that explain.

### The fix — twofold

**Code part (~2 days):** Add a lint that fires when a kernel writes `q.<field> = ...` (which is reading from `q` so the assignment is semantically wrong — `q` is the read-only pre-firing snapshot). The lint's diagnostic explains: "`q.<field>` is the pre-firing snapshot of sub-circuit `<field>`'s output. To drive `<field>`'s input this cycle, write `d.<field> = ...` instead."

**Doc part (1 day):** Add a focused chapter or sub-section to the RHDL book at `doc/book/src/circuits/dq_semantics.md` that walks through the d/q pattern with a concrete worked example — a parent widget composing a child DFF. Include the timing diagram showing what `d`, `q`, and the child's internal register transition through one clock cycle. Reference the diary's exact quote as the motivating user feedback.

### Acceptance

- A kernel that writes `q.<field> = expr` produces the new diagnostic with span on the assignment.
- The book chapter is referenced from `doc/book/src/SUMMARY.md`.
- A CHANGELOG entry that names the diary excerpt as the trigger and links to the chapter.

### Why this matters

It's the smallest fix in the list but it removes a chronic onboarding tax. Every new kernel author hits this once.

---

## Sequencing recommendation

These can ship in any order with no dependencies among them, except as noted (D builds on C; B's value compounds with A and C). Suggested staging:

**Wave 1 (1 week, 4 PRs):**
- A. `bits!()` macro (1-2 days)
- C. Bit-slicing methods (3-5 days)
- G. d.X vs q.X diagnostic + doc chapter (2-3 days)
- F. Kernel literal validation (~1 week, can run in parallel)

After Wave 1 the rv32i decoder rewrites cleanly; literal-induced iverilog crashes are caught at compile time; the most chronic d/q confusion is addressed.

**Wave 2 (~2 weeks, 1 PR):**
- B. Numeric match patterns on `Bits<N>` (1-2 weeks)

This is the single highest-leverage change for decoder-shaped widgets. After it ships, the rv32i decoder's funct3 dispatch refactors from if-else-if cascades to clean match expressions. Every protocol PHY in Tier 3 of `widget-roadmap.md` benefits proportionally.

**Wave 3 (1-2 weeks, 1 PR):**
- D. `concat!()` macro (~1 week)

Builds on C's bit-slicing methods. After D ships, immediate-construction helpers in any decoder collapse from 10-15 lines to 5-7 lines.

**Wave 4 (2-3 weeks, 1 PR):**
- E. Q/D tuple ceiling fix (2-3 weeks)

Larger because the corpus snapshot regen. Worth doing carefully. Removes a real architectural ceiling that would otherwise bite Alto-Phase-3.5+ work, the VAX core, and any future protocol PHY that wants more than 12 sub-circuits.

**Cumulative:** roughly 5-8 weeks of focused engineering work, parallel-safe with all the active widget tracks. None of these changes touch the active hot paths (`rhdl-fpga`, `rhdl-rule`, `rhdl-alto`, `rhdl-rv32i`); they all live in `rhdl-macro`, `rhdl-macro-core`, `rhdl-bits`, and `rhdl-vlog`.

---

## Validation across the corpus

Each fix has the same validation pattern, scaled to scope:

1. **Unit test in the macro/lib crate** — exercises the new feature at the smallest scope.
2. **Integration test in `crates/rhdl/tests/`** — full kernel compilation + iverilog round-trip exercising the feature.
3. **Refactor at least one existing widget** to use the new feature; assert byte-identical Verilog before/after via expect_test snapshot.
4. **CHANGELOG entry** per CLAUDE.md §16.

For B, C, D, E specifically: refactor the rv32i decoder + the rv32i pipelined kernel as the canonical demonstration. The line-count delta is the headline metric.

For E: the corpus-wide snapshot regen is the validation. One canary widget first, then bulk migration with diff review.

---

## Cross-references

- `crates/rhdl-rv32i/rv32i-analysis.md` — the source-material analysis these fixes derive from.
- `crates/rhdl-rv32i/my_experience_05012026_by_claude.md` — the implementation diary; raw quotes used in this document.
- `kernel-language-extensions.md` — the design plan that contains the original (less-scoped) versions of A, B, C, D as language extensions. This document is the engineering-scoping follow-on.
- `notes/synchronous-tuple-ceiling-can-rx.md` — the existing analysis of pain point E. Extended here with a concrete fix.
- CLAUDE.md §11.1 — the compiler-change PR contract every one of these items must satisfy.
- CLAUDE.md §3.1 — the protocol-PHY bundled-state workaround that pain point E removes the need for.
