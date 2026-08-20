# Zero-width `Digital` types miscompile — two root causes (2026-08-20)

## Context

Found while making `dsp::mixer::complex::ComplexMixer` generic over its
framing type `F`, so that a `SyncMark`-framed chain and an unframed one
are the same widget.  `F = ()` is the unframed instantiation and is what
every existing caller uses, so it has to keep working.

Three separate failures showed up.  On first write-up they were filed as
three independent bugs at three layers.  That was wrong, and the
correction matters for how they get fixed: **two of the three are the
same defect**, differing only in whether an existing check happens to
catch it.

Nothing here is specific to `()` as a type.  Any `Digital` type of width
zero should reproduce all of it.

---

## Root cause A — a zero-bit value renders as the illegal literal `0'b`  **[FIXED 2026-08-21]**

> **Fixed**, but *not* the way this note first proposed. The proposal
> below was to **reject** a zero-width value. That was tried and broke
> `rcstream::util::split` and `combine`, which carry their framing type
> through a `Constant<F>` field precisely because `PhantomData` has no
> HDL — at `F = ()` that is a `Constant<()>`, and rejecting it would
> have made an unframed `RCStream` unrepresentable.
>
> **A zero-width sub-circuit is a deliberate idiom in this tree, not a
> mistake to diagnose.** What landed instead splits the two halves: the
> `LitVerilog` conversions are `TryFrom` and still reject zero width, so
> nothing emits `0'b` by accident; and `signal_literal` is a single
> opt-in that substitutes a one-bit zero where a signal must be driven,
> matching the one-bit port the emitter already declares. Declaration
> and literal now agree.
>
> **Root cause B below is untouched and is the more serious of the two.**


**Where.** The `TypedBits -> vlog::LitVerilog` conversion.  Measured
directly:

```
bool literal : [1'b0]
unit literal : [0'b]      <- width zero, and no value digits
```

`0'b` is not legal Verilog: a sized literal needs at least one digit.

**Symptom.** `dff::DFF<()>` fails the `ModuleList::checked()` syntax
gate:

```
top.v:7:  syntax error / Malformed statement
top.v:11: syntax error / Malformed statement
```

Those two lines are the reset-value interpolations in
`core/dff.rs::hdl`, which uses `#init` twice:

```rust
let init: vlog::LitVerilog = self.reset.typed_bits().into();
// initial begin o = #init; end
// always @(posedge clock) if (reset) o <= #init; else o <= i;
```

so the emitted text is `o = 0'b;` and `o <= 0'b;`.  Confirmed by
dumping the `DFF<bool>` module and reading off lines 7 and 11, which are
exactly those two statements.

`Constant<T>::hdl` builds its literal the same way and **is** affected
identically — verified, and it is the cleanest demonstration of the bug
because the whole module is three lines:

```verilog
module top(input wire [1:0] clock_reset, output wire [0:0] o);
   assign o = 0'b;        // Constant<()>;  Constant<bool> gives 1'b0
endmodule
```

Note also `output wire [0:0] o` — a zero-bit type declared one bit wide.
That widening is harmless on its own but is the same
zero-width-is-not-really-handled theme as root cause B.

**It is not caught at that point.** `Constant::descriptor()` returns
`Ok`, malformed Verilog and all, because it populates `hdl` without
running the syntax gate. The gate lives in `Descriptor::hdl()`
(`rhdl-core/src/circuit/descriptor.rs:62`), so the error surfaces when
something asks a *parent* descriptor for its HDL. In practice that is
every test that emits Verilog, so it does not escape to synthesis — but
"the descriptor built fine" is not evidence that the Verilog is legal.

**Severity: low.** Caught by the syntax gate before any Verilog is used,
and confined to one conversion that has no guard for the empty case:

```rust
// rhdl-core/src/hdl/builder.rs:26, From<&TypedBits> for vlog::LitVerilog
let base = if tb.kind().is_signed() { "sb" } else { "b" };
let bits = base.chars()
    .chain(tb.iter().rev().map(...))   // zero iterations at zero width
    .collect::<String>();              // so `bits` is just "b"
vlog::lit_verilog(tb.len() as u32, &bits)   // and tb.len() is 0  ->  0'b
```

`From<&BitString> for vlog::LitVerilog`
(`rhdl-core/src/types/bit_string.rs:18`) has the same shape and the same
gap.

It only bites when a *generic* widget is instantiated at a zero-width
parameter — nobody writes `DFF<()>` on purpose — which is exactly the
case that matters here.

**Fix shape.** Either render a zero-bit value as something legal, or
refuse to emit a zero-bit register at all (see root cause B's fix
discussion — erasure would make this unreachable too).

---

## Root cause B — RHIF→RTL lowering emits a register that is never written

This is the real one, and it accounts for **both** of the other two
symptoms.

**It is not an emitter bug.** An earlier version of this note placed it
in Verilog emission, because that is where the symptom shows up. It is
two layers earlier: the lowering produces a malformed RTL object, and
the emitter renders that faithfully.

### The evidence

The kernel is `a.frame != b.frame` over `Item<b8, F>`. Compile it at
both instantiations and dump each IR.

**RHIF is correct for this kernel.** At `F = ()`:

```
Reg r2 : ()          // zero-width, well-typed
Reg r3 : ()
Reg r4 : b1
r2 <- r0.frame       // defined
r3 <- r1.frame       // defined
r4 <- r2 != r3
```

Every register is written before it is read. There is nothing wrong
with a zero-width value at this level, and the type system needs it.

Do not over-read this into "RHIF is always fine" — symptom B1 below is a
zero-width slot that RHIF *does* leave undefined, in a different
construct. The asymmetry that matters is not which layer produces the
gap, but which layer checks for it: RHIF has
`partial_initialization_check.rs`, so its gaps are loud. RTL has no
equivalent, so its gaps are silent.

**RTL at `F = bool`** — three instructions, everything defined:

```
reg r0 : b1
reg r1 : b9
r0 <- r1[8..9]       // extract .frame
r2 <- r3[8..9]       // extract .frame
r4 <- r0 != r2
```

**RTL at `F = ()`** — one instruction:

```
reg r1 : b0          // allocated ...
reg r2 : b0          // ... and never written
r0 <- r1 != r2
```

The two extraction instructions are **gone**. Extracting zero bits is a
no-op, so the lowering correctly emits nothing — but it had already
allocated the destination registers. The result is an RTL object
containing registers that are read and never written.

That is malformed independently of Verilog. The `x` downstream is just
Verilog rendering a register nobody drove:

```verilog
reg [0:0] r5;     // the b0 registers, widened to one bit by the emitter
reg [0:0] r6;
...
r4 = r5 != r6;    // x != x  ->  x
```

### Why nothing caught it

`rhif_passes/` contains `partial_initialization_check.rs` and
`check_rhif_flow.rs`. `rtl_passes/` contains **no** equivalent.

Here the RHIF is well-formed, so the RHIF check passes — correctly. The
lowering then drops the defining instruction while keeping the register,
and nothing re-checks at RTL. B1 and B2 are therefore the same gap
landing on opposite sides of the only check that exists.

### This is a missed case in an existing policy, not an unconsidered corner

`rtl_passes/` already treats zero-width RTL operands as a known hazard:

| pass | what it does with zero width |
|---|---|
| `strip_empty_args_from_concat.rs` | filters zero-bit args out of a concat |
| `remove_empty_function_arguments.rs` | drops zero-bit function args |
| `check_no_zero_resize.rs` | **rejects** a zero-length cast as an ICE |

So the established policy is: strip where harmless, reject where
meaningless. **Binary operations were simply never covered** — a
comparison on zero-width operands is neither stripped, nor rejected, nor
folded. Fixing this completes an existing decision rather than making a
new one.

### Symptom B1 — the partial-initialisation checker catches it (an ICE)

```
RHDL Internal Compile Error
  ╰─▶ Slot sr3 is read before being written
```

Reproduced with a `Constant<F>` read in a kernel at `F = ()`:

```rust
pub struct ConstOnly<F: Digital + Default> {
    c: Constant<F>,
    keep: dff::DFF<b8>,      // keeps the widget non-trivial
}
// kernel: let mut f = q.c; if let Some(x) = i { f = x; }
```

**This is the checker working, not a bug in the checker.**  The slot
genuinely is read before being written, because a zero-width value never
got a defining instruction.  The message is accurate; it just points at
the consumer rather than at the missing definition, which is why it took
a while to place.

Note it only fires for the `let mut x = …; if let … { x = … }` shape.
Binding in both arms of a `match` — the `rcstream::zip` idiom — produces
a different RHIF shape that does not put the undefined slot where the
checker looks:

```rust
// trips it at F = ()
let mut af = q.idle.frame;
if let Some(x) = i.a { af = x.frame; }

// does not
let (have_a, item_a) = match i.a {
    Some(it) => (true, it),
    None     => (false, Item::<Iq<A_W>, F> { data: zero_a, frame: q.idle.frame }),
};
```

That difference is **not** a fix.  It is the same missing definition
going unnoticed.

### Symptom B2 — the checker does not catch it, and `x` reaches Verilog

Same missing definition, on a path the checker does not flag.  It
compiles, passes every Rust-side test tier, and diverges only in
simulation:

```
TESTBENCH FAILED: Expected 00011111101111111010000000010110100010000
                  got     0x011111101111111010000000010110100010000
Test 6 at time 151
```

The Rust simulator evaluates `() != ()` as a defined `false`; `iverilog`
evaluates the undriven regs as `x`.  Because the guard `have_a &&
have_b` is *true* on the cycles that matter, the `x` is not masked and
propagates to the output bundle.

**Severity: high.**  Not for blast radius — few widgets are generic over
a `Digital` parameter today — but because of the failure mode.  It is a
silent cross-simulator divergence: no diagnostic, no ICE, every Rust
tier green, and only the Tier-4 round-trip catches it.  It is a concrete
instance of the thing CLAUDE.md section 12 rule 3 asserts in the
abstract, that Tier 4 is the only tier checking the emitted hardware.
In synthesis this is an `x`, not an error.

### When it actually bites

A zero-width value is harmless while it **stays** zero-width: it
contributes no bits, so an undriven one cannot corrupt anything
downstream.  It bites only when an operation turns zero-width operands
into a **non-zero-width result**.

Comparison is the obvious such operation — two 0-bit inputs, one 1-bit
output — and is the only one hit so far.  Anything else that reduces a
value to a flag would qualify.

This is what makes the workaround in the tree principled rather than a
hack.  Padding keeps the operands out of the one shape where the missing
definition can escape:

```rust
// dsp/mixer/complex.rs
let pad = bits::<1>(0);
d.mismatch = have_a && have_b && ((af, pad) != (bf, pad));
```

The pads are equal on both sides, so the pair differs exactly when the
markers do, and the compared type is at least one bit wide for every
`F`.

---

## Proposed fix

### The principle

A `Digital` type with zero bits has **exactly one inhabitant**. It
carries no information, so every operation over zero-width operands has
a compile-time-known result, and nothing about it needs to exist at
runtime.

Two consequences, and they pull in different directions depending on
the layer:

- **In RHIF, keep it.** The type system needs `()`; `Item<T, ()>` being
  36 rather than 37 bits is a documented promise of `rcstream::bus`, and
  generic code needs *some* type meaning "no framing" that does not cost
  a wire. RHIF can still leave a zero-width slot undefined (symptom B1),
  but it has a check that catches that, so those failures are loud.
- **In RTL and below, erase it.** A register with no bits is not a
  useful thing to have allocated. Every current symptom traces to one
  being allocated and then not driven.

So the boundary is the RHIF→RTL lowering. That is where zero-width
values should stop existing.

### The change, in priority order

**1. Add an RTL well-formedness check: every register that is read must
be written.**

The RTL analogue of `rhif_passes/partial_initialization_check.rs`, which
has no counterpart in `rtl_passes/`.

Listed first deliberately. It fixes nothing by itself, and it is still
the highest-value item, because **it is not about zero width at all**.
Any lowering bug that drops a defining instruction is currently a silent
`x` in the emitted Verilog, passing every Rust-side tier. This class of
bug has now cost one afternoon of bisecting a testbench diff; the check
turns the whole class into a compile error.

It should be written so it fails on today's tree at `F = ()` — that is
its first test case.

**2. Constant-fold binary operations on zero-width operands during
lowering.**

`!=` becomes literal `false`, `==` becomes literal `true`, because a
one-inhabitant type admits nothing else. This is the correctness fix,
and it makes the Verilog agree with the Rust simulator, which already
returns a defined `false`.

This completes the policy `strip_empty_args_from_concat`,
`remove_empty_function_arguments` and `check_no_zero_resize` already
established for other opcodes. Enumerating which opcodes can actually
receive zero-width operands is part of the work — comparison is the only
one observed to bite, and the enumeration should be in the PR's
Justification section rather than assumed.

**3. Do not allocate RTL registers for zero-width values.**

With nothing reading them, nothing needs them. This makes both root
causes *unreachable* rather than fixed: no literal is rendered for a
value that does not exist, and no register goes undriven that was never
allocated.

**4. Remove the silent width coercions in the emitter.**  *(Partly
addressed 2026-08-21 — see the banner on root cause A. The literal no
longer disagrees with the declaration; the two `saturating_sub`
coercions themselves are still there.)*

Three sites currently turn zero width into something else, quietly, and
they disagree with one another:

| site | zero width becomes |
|---|---|
| `Kind → SignedWidth`, `hdl/builder.rs` — `len.saturating_sub(1)` | `[0:0]`, one bit |
| `&Range<usize> → BitRange`, `rhdl-vlog/atoms.rs:157` — same | `[0:0]`, one bit |
| `&TypedBits → LitVerilog`, `hdl/builder.rs:26` | `0'b`, no digits |

Declarations say one bit, literals say zero. That disagreement is why
the symptoms look so strange. After 1–3 these become unreachable, so
this drops from "the fix" to defence in depth — but it is worth doing,
because `saturating_sub` is precisely what turns "nobody thought about
zero width here" into "emits a plausible-looking one-bit signal".
Making the conversions fallible means a future miss is loud.

Note this is also the *only* part that addresses root cause A, since
`DFF` and `Constant` hand-write their Verilog and never pass through
RHIF or RTL at all.

### Alternatives considered and rejected

**Legalise instead of erase** — keep the one-bit declaration and just
ensure it is always driven to `0`. Much smaller, and comparisons would
then be correct (`0 != 0` is `false`). Rejected: it forces the invariant
"a slot's declared width is not the value's width", because a
zero-width field must still contribute zero bits to struct layout and
concatenation. That is a subtle rule every future pass has to remember,
and forgetting it yields wrong widths rather than an error. Erasure
needs no such rule.

**Fix only the literal** (`0'b` → something legal) — the minimal patch
for root cause A alone. Leaves B, the serious one, entirely untouched.
Not worth a PR by itself.

**Ban zero-width `Digital` types** — would work, and deletes a
documented, load-bearing feature. `rcstream::bus` promises `F = ()` adds
no wire bits; the generic mixer depends on it. Wrong direction.

### Tests that must ship with it

- A kernel comparing a zero-width generic parameter, round-tripped
  through `iverilog`, asserting a **defined** result. This is the test
  that would have caught B2 when the generic was first written, and it
  does not exist today.
- The new RTL check failing on the pre-fix tree, so it is demonstrably
  able to fail.
- A negative test that a zero-width declaration or literal is rejected
  rather than coerced (part 4).
- `ComplexMixer<(), ...>` continuing to work, with the padding
  workaround in `dsp/mixer/complex.rs` **removed** — the padding exists
  only because of this bug, and the fix landing should retire it.

### Why it is not in this branch

Per CLAUDE.md §11.1 a compiler change is one feature per PR, with tests
at every IR level it touches and a Justification section answering the
five questions. Parts 1–3 are arguably one feature ("zero-width values
are erased at the RHIF→RTL boundary") spanning layers, which §11.1
permits; part 4 is separable. None of it is the framing work this branch
is about, so it is filed here and in `widget-roadmap.md`.

## Reproducing

The probe widgets were throwaway.  The shapes above are complete; put
each in its own `mod` inside a `tests/` file, because
`#[rhdl(dq_no_prefix)]` emits `Q`/`D` at module scope and two widgets in
one module collide.

To see emitted Verilog for a case that fails the syntax gate, read
`descriptor.hdl` directly — it is a public field — instead of calling
`Descriptor::hdl()`, which runs `ModuleList::checked()` and returns the
iverilog error before you can see the text.
