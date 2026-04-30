# Kernel-macro OOM — root cause, fix, and impact (2026-04-29)

> **Status: RESOLVED.** Root cause was an exponential macro
> expansion in `const_max!` (`rhdl-core/src/bitx/dyn_bit_manip.rs`).
> Fix: rewrite the macro to recurse linearly via a `const fn`
> helper.  Single-line file change (plus a 7-line const-fn
> helper).  All 740 existing tests pass; the OOM reproducer
> that previously consumed 7 GB and crashed rustc with SIGKILL
> after 170 seconds now compiles in 0.07 seconds.
>
> Supersedes the analysis in `notes/kernel-macro-oom.md`.  That
> note's working hypothesis ("wide enum × many construction
> sites in a kernel triggers exponential IR growth") was
> incorrect — the OOM has nothing to do with `#[kernel]` or with
> the construction site count.  The trigger is purely the
> Digital derive on a wide enum, which uses `const_max!` to
> compute `BITS`.

## TL;DR

`const_max!` in `crates/rhdl-core/src/bitx/dyn_bit_manip.rs` was
defined as a macro whose recursive call appeared **twice** on
the right-hand side:

```rust
macro_rules! const_max {
    ($x: expr) => ($x);
    ($x: expr, $($z: expr), +) => (
        if $x > const_max!($($z), +) {
            $x
        } else {
            const_max!($($z), +)        // ← recursive call appears twice
        }
    );
}
```

Each macro level doubles the recursive expansion, so
`const_max!(N args)` produces `2^(N-1)` leaf occurrences.  The
`Digital`-derive for enums uses `const_max!` over all variants
to compute the `BITS` constant:

```rust
// crates/rhdl-macro-core/src/digital_enum.rs:392
const BITS: usize = #width_bits + rhdl::const_max!(#(#variant_bits_mapping),*);
```

For a 22-variant enum, `const_max!` was invoked with 22
zero-payload arguments and expanded to **2,097,152 leaf
`0_usize` literals** wrapped in nested `if 0_usize > 0_usize`
chains.  The resulting expanded source for one such enum was
**632 MB** for a 35-line input file.  rustc OOMed at ~7 GB RSS
during type-checking.

Fix: rewrite the macro with a `const fn` helper so the
recursive call appears **once**:

```rust
pub const fn const_max_pair(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

macro_rules! const_max {
    ($x: expr) => ($x);
    ($x: expr, $($z: expr), +) => (
        $crate::bitx::dyn_bit_manip::const_max_pair($x, $crate::const_max!($($z), +))
    );
}
```

Total tokens emitted now linear in argument count.  The fix
landed in this commit; rustc memory for the same reproducer is
under 50 MB; compile time is under a second.

## Symptom recap (from the prior note)

The shipped MIDI parser hit SIGKILL during `cargo build
--package rhdl-fpga` on a 16 GB dev machine.  The pre-refactor
kernel used a 22-variant `MidiKind` enum as a struct field
constructed in 15+ places.  The shipped mitigation was to
replace the enum with `Bits<5>` codes, dropping the variant
count and resolving the OOM.

The prior note hypothesised that the *combination* of "wide
enum × struct-field × many-construction-sites × field-by-field
mutation" was structurally exponential.  This investigation
disproves that — the trigger is purely the wide enum *as a
type*, regardless of how many times it is referenced or
constructed.

## Investigation method

### Step 1 — minimal reproducer in an isolated crate

Built `/tmp/oom-experiment/` as a small standalone crate with a
22-variant `WideKind` enum, a 4-field `Msg` struct containing
it, and a `#[kernel]` function with 15 if-else assignments to
`msg.kind`.  Confirmed the OOM:

- Peak RSS: 7,074,742,272 bytes (≈ 7.07 GB)
- Wall time: 169.6 seconds
- Result: SIGKILL

### Step 2 — phase profiling with `-Z time-passes`

Installed nightly toolchain and re-ran with rustc's
`-Z time-passes`:

| phase | wall time | rss delta | rss after |
|---|---|---|---|
| `expand_crate` (proc-macro expansion) | 47.1s | +1916 MB | 1961 MB |
| `AST_validation` | 0.3s | +1180 MB | 3141 MB |
| `finalize_macro_resolutions` | 1.6s | +1180 MB | 4323 MB |
| `late_resolve_crate` | 0.5s | +549 MB | 4872 MB |
| `drop_ast` (transient) | 4.1s | +1302 MB peak | — |
| `type_check_crate` | 24.4s | (then SIGKILL) | — |

Two findings here:

1. **The proc-macro expansion phase alone consumed nearly 2 GB.**
   The macro's emitted token tree was huge.
2. **Every subsequent phase grew memory further.**  The full
   AST had to be carried through validation, resolution, and
   type-checking; each phase allocated additional structure on
   top of the already-large AST.

### Step 3 — dump the macro expansion to disk

Used `cargo +nightly rustc --lib -- -Zunpretty=expanded` to
dump the post-macro source.  Result:

```
$ wc -c /tmp/expanded.rs
632414773 /tmp/expanded.rs
```

**632 MB of expanded source for a 35-line input file.**  Visual
inspection showed deeply nested chains of `if 0_usize > 0_usize
{ 0_usize } else { 0_usize }` — this is `const_max!` expanding
over 22 zero-payload-bit arguments.

```
$ grep -c "0_usize" /tmp/expanded.rs
3145727
```

3.1 million `0_usize` tokens in a single file.

### Step 4 — isolate the trigger

Reduced the reproducer to **just the enum derive** (no
`#[kernel]`, no struct, no kernel body):

```rust
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum WideKind {
    V0, V1, V2, ..., V21,    // 22 unit variants
}
```

Result: still SIGKILL.  This proves the OOM has nothing to do
with the `#[kernel]` macro, the struct field, the construction
site count, or the field-by-field mutation pattern from the
prior note.  The OOM is purely from `#[derive(Digital)]` on a
wide enum.

### Step 5 — locate the exponential expansion

`grep -rn 'macro_rules! const_max' crates/` found the macro at
`crates/rhdl-core/src/bitx/dyn_bit_manip.rs:146`.  Inspection
revealed the recursive call appearing twice on the RHS — every
level of recursion doubles the number of recursive instances.

For N arguments, total leaf instances = 2^(N-1):

| N (variant count) | leaf count | scale |
|---|---|---|
| 5 | 16 | trivial |
| 10 | 512 | trivial |
| 15 | 16,384 | manageable |
| 20 | 524,288 | painful (~150 MB expanded) |
| 22 | 2,097,152 | OOM (~632 MB expanded) |
| 25 | 16,777,216 | impossible |

Each leaf in our reproducer is `0_usize` (because all variants
are unit so `variant_bits_mapping` returns `0`).  In a real
widget where some variants have payloads, the leaf is a more
complex `<T as Digital>::BITS` expression — making the absolute
size larger but the exponential scaling unchanged.

### Step 6 — verify the fix

Patched `const_max!` to use a `const fn` helper for the
pairwise max, eliminating the duplicate recursive call:

```rust
pub const fn const_max_pair(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

macro_rules! const_max {
    ($x: expr) => ($x);
    ($x: expr, $($z: expr), +) => (
        $crate::bitx::dyn_bit_manip::const_max_pair($x, $crate::const_max!($($z), +))
    );
}
```

Each macro level now adds exactly one `const_max_pair(...)`
call.  Total tokens after expansion: linear in argument count.

Re-ran the reproducer:

| metric | before | after |
|---|---|---|
| Wall time | 169.6 s | 0.07 s |
| Peak RSS | 7.07 GB | 45 MB |
| Status | SIGKILL | OK |

Re-ran the original MIDI OOM reproducer
(`notes/kernel-macro-oom-repro.rs.txt`) by copying it over the
shipped `midi_parser.rs`:

| metric | before | after |
|---|---|---|
| Status | SIGKILL after ~30 s | Clean compile in 6.6 s |

### Step 7 — non-regression

Restored the shipped `midi_parser.rs` and ran the full
`rhdl-fpga` lib test suite:

```
$ cargo test --package rhdl-fpga --lib
test result: ok. 740 passed; 0 failed; 1 ignored; 0 measured;
0 filtered out; finished in 142.30s
```

Plus the targeted const_max test:

```
$ cargo test --package rhdl-core --lib const_max
test bitx::dyn_bit_manip::tests::test_const_max_macro ... ok
```

No regressions.

## Root cause analysis

The bug is **textbook exponential macro recursion**.  Rust's
declarative-macro system does not memoise: when a macro's
recursive call appears twice on the RHS, both instances are
expanded fully.  For a recursion depth of N, this produces
2^N leaf expansions.

The `const_max!` author likely reasoned about the macro as if
it were an if-then-else expression that evaluates the
recursion once.  At runtime that is what `if a > b { a } else
{ b }` does — but at macro-expansion time, both branches are
fully expanded into the source before any evaluation can
occur.

The fix collapses the duplicated recursive call into a single
call to a `const fn`.  The const-fn body has the same `if a >
b` shape, but it is evaluated once at compile time on a single
expanded call site.  The macro emission becomes linear in
argument count.

This pattern (macro recursion with the recursive call duplicated
across both branches of an if/else) is a classic Rust
declarative-macro footgun.  See for example [the RFC discussion
on declarative macro hygiene](https://rust-lang.github.io/rfcs/3086-macro-metavar-expr.html)
which calls out that "macros that appear simple may have
non-obvious quadratic or exponential expansion costs."

## Why the prior diagnosis was wrong

The prior note attributed the OOM to a four-factor combination:

1. A wide enum.
2. The enum used as a struct field.
3. The struct constructed in many code paths within one kernel.
4. Field-by-field assignment style.

Steps 4–6 of this investigation eliminate factors 2–4: the OOM
reproduces with **just** `#[derive(Digital)]` on the wide enum,
no struct, no kernel, no construction sites.  Only factor 1 was
load-bearing.

The prior note also speculated:

> "The crash happened during macro expansion (the `#[kernel]`
> proc-macro in `rhdl-macro-core`), not during type checking or
> codegen."

The `-Z time-passes` data shows this was partly right and
partly wrong: the proc-macro phase consumes ~2 GB, but that is
not where the crash happens.  Subsequent phases (AST validation,
resolution, type checking) each allocate more on top of the
already-large AST, eventually exhausting memory in
`type_check_crate`.  The proc-macro is the *cause* (it emits
2 GB of tokens that everything downstream then has to process)
but not the *site* of the OOM.

The shipped mitigation in MIDI (`Bits<5>` codes instead of an
enum) worked because it eliminated the wide enum entirely —
`Bits<5>` is a primitive type whose `Digital` impl does not go
through `const_max!`.  The mitigation was correct but hid the
actual root cause.

## Impact on the deferred widgets

The widgets paused on this issue can now proceed with their
full intended scope:

- **Full Classical CAN 2.0A node** (the immediate trigger for
  this investigation).  21-variant FSM enum + 30-field state
  struct in one kernel — should now compile.
- **SCSI Parallel target/initiator** (per
  `notes/scsi-parallel-deferred.md`).  The IR-explosion concern
  in that note was citing the same OOM mechanism — that concern
  is resolved.
- **Modbus RTU slave + master extension** (per
  `notes/kernel-language-constraints-modbus.md` Constraint 4).
  Same — IR-explosion concern resolved.
- **Future widgets with > 12 FSM states** generally.

The other constraints called out in those notes remain (helper
kernel `&` arguments, runtime-array-indexing on DFF arrays).
Those are real but smaller in scope; the widgets are no longer
blocked structurally.

## Recommended follow-up actions

### Mandatory

- [x] **Fix `const_max!`** in
  `crates/rhdl-core/src/bitx/dyn_bit_manip.rs`.  Done in this
  commit.

### Strongly recommended

- [ ] **Add a regression test for wide-enum `Digital` derives.**
  A simple test that derives `Digital` on a 32-variant enum
  would catch any future regression of this issue.  Belongs in
  `crates/rhdl-core/src/bitx/dyn_bit_manip.rs` next to the
  existing `test_const_max_macro`, or as a compile-time test
  in a `tests/` directory.
- [ ] **Audit other macros in the workspace for the same
  pattern.**  Specifically: any `macro_rules!` that recurses
  on a `$($z:expr),+` repetition where the recursive call
  appears more than once on the RHS.  A grep across `crates/`
  would surface candidates quickly.
- [ ] **Update `notes/kernel-macro-oom.md`.**  Add a header
  pointing to this resolution note, and explicitly mark the
  prior diagnosis as superseded so the next reader is not
  misled.
- [ ] **Update `notes/scsi-parallel-deferred.md` and
  `notes/kernel-language-constraints-modbus.md`** to reflect
  that the IR-explosion concern (Constraint 4 / Constraint
  block) is resolved.

### Nice-to-have

- [ ] **Re-attempt the full CAN node**.  The blocker that made
  me restore the original `can_master.rs` is gone.  The drafted
  unified node (TX + RX + extended IDs + bit-timing resync +
  TEC/REC + bus-off + error frames + acceptance filter) should
  now compile.
- [ ] **Documentation**: add a short note in `architecture.md`
  about the macro-expansion footgun, so future macro authors
  in this codebase know to evaluate recursive calls only once.

### Out of scope

- The kernel macro itself (the `#[kernel]` proc-macro in
  `rhdl-macro-core`) was *not* the cause.  Its emission is
  linear in source size.  No changes needed there.
- The `#[derive(Digital)]` macro for enums was the *site* but
  not the *cause* — the bug is in the helper macro it relies
  on.  No changes needed in `digital_enum.rs`.

## Reproduction

The minimal isolated reproducer lives at `/tmp/oom-experiment/`
during this session.  To re-create:

```bash
mkdir -p /tmp/oom-experiment
cd /tmp/oom-experiment
cat > Cargo.toml <<'EOF'
[package]
name = "oom-experiment"
version = "0.1.0"
edition = "2021"

[dependencies]
rhdl = { path = "/path/to/rhdl/crates/rhdl" }

[lib]
path = "lib.rs"
EOF
cat > lib.rs <<'EOF'
use rhdl::prelude::*;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum WideKind {
    #[default]
    V0,
    V1, V2, V3, V4, V5, V6, V7, V8, V9, V10,
    V11, V12, V13, V14, V15, V16, V17, V18, V19, V20, V21,
}
EOF
cargo check
```

To compare before/after, revert the `const_max!` macro to its
prior form (preserved in the commit history of this fix) and
re-run.  Add `RUSTC_BOOTSTRAP=1` and `--release` if you want to
measure type-checking peak memory; for the basic OOM
demonstration, `cargo check` on stable suffices.

## Validation summary

| validation | result |
|---|---|
| `cargo test --package rhdl-core --lib const_max` | pass |
| `cargo test --package rhdl-fpga --lib` (740 tests) | pass |
| `cargo check` on the standalone reproducer | 0.07 s, 45 MB RSS |
| `cargo check` on the original MIDI OOM repro | 6.6 s, normal memory |
| Full workspace build | clean (no widget snapshot regressions) |
