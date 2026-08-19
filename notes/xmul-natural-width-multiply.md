# Brief: `xmul` discards operand widths at Verilog emission (2026-08-19)

## One-line summary

`DynBits::xmul` is width-tracking arithmetic that **knows** its operand
widths and throws them away when emitting Verilog: it sign-extends both
operands to the product width and emits an equal-width multiply. Emitting
the multiply at natural operand widths instead would let vendor DSP
inference see an `18×14` product as `18×14` rather than as `32×32`.

**Status:** proposal, not implemented. Compiler-level, so per CLAUDE.md
§11.1 it is its own PR with its own Justification section. This brief is
the design input for that PR.

## How it was found

While parameterising `dsp::nco::sin_cos_linear_interp` over its widths
(PR #82 follow-up), the question came up of whether RHDL instantiates
Xilinx DSP48E1 primitives or relies on inference. It relies on inference —
see "Related finding" below — and that led to measuring what multiply is
actually emitted.

## Observed behaviour

Probe: two kernels multiplying an 18-bit by a 12-bit signed value.

**Route A — typed path, operands resized to a common width.** This is what
the NCO kernel did before the fix:

```rust
let a = i.a.resize::<48>();
let b = i.b.resize::<48>();
a * b
```

```verilog
reg signed [47:0] r2;  reg signed [47:0] r4;
r5 = r2 * r4;                      // 48 x 48
```

**Route B — `DynBits::xmul` at natural widths**, as `dsp::lerp::fixed`
uses:

```rust
let p = i.a.dyn_bits().xmul(i.b.dyn_bits());
```

```verilog
reg signed [17:0] r0;              // the 18-bit input
reg signed [11:0] r2;              // the 12-bit input
reg signed [29:0] r3;
reg signed [29:0] r4;              // <-- r0 sign-extended to 30
reg signed [29:0] r5;              // <-- r2 sign-extended to 30
r3 = r4 * r5;                      // 30 x 30
```

`xmul` correctly computes the *result* width as `18 + 12 = 30`. Then it
sign-extends **both operands** to 30 bits and emits a 30×30 multiply.

## Why this matters

A DSP48E1's multiplier is **18×25**, producing a 43-bit product (48 with
the accumulator). The operand widths, not the product width, decide how
many slices a multiply costs:

| emitted | DSP48E1 slices (no pruning) |
|---|---|
| 18 × 25 | 1 |
| 30 × 30 | 2 |
| 48 × 48 | 6–9 |

The example above is genuinely an 18×12 multiply. It fits one slice with
room to spare. Emitted as 30×30 it reads as a two-slice multiply.

Vivado's synthesiser does perform bit-range propagation and will often
recognise that `r4[29:18]` is a replication of `r4[17]` and prune back to
18×25. **But that is inference resting on inference**, it varies by tool
and version, and it is not something RHDL can assert or test. The whole
argument in `dsp::mixer`'s module docs — that a resource claim which
cannot be tested is not a resource claim — applies here.

Concrete instance: `sin_cos_linear_interp` picks `AMP_W = 18` *because*
it is the DSP48's native port width. Before the natural-width change the
kernel emitted a 48×48 multiply, so the reason for choosing 18 was not
expressed in the RTL at all. After the change it emits 32×32. It should be
able to emit 18×14.

## The proposal

Emit the multiply with operands at their **declared** widths and let the
assignment target carry the product width:

```verilog
// today
reg signed [29:0] r4;  reg signed [29:0] r5;
r3 = r4 * r5;

// proposed
reg signed [17:0] r0;  reg signed [11:0] r2;
r3 = $signed(r0) * $signed(r2);       // r3 is [29:0]
```

Verilog-2001 `*` on signed operands produces a result whose width is the
context width (here the 30-bit LHS), with operands sign-extended by the
language. So the semantics are unchanged — this is purely about **not
pre-extending in the IR**, letting the target width do the job the
language already does.

### Where the information already exists

This is the load-bearing point: **no new analysis is required.** `xmul`
computes the result width *from* the operand widths, so both are known at
the point the sign-extension is inserted. The change is to stop inserting
it, not to recover information that was lost.

### Scope

Likely touch points, to be confirmed by whoever takes the PR:

- **RTL/NTL lowering** of the multiply opcode, wherever the operand
  sign-extension is currently materialised as separate registers. The
  probe shows `r4`/`r5` as distinct regs, so the extension is explicit in
  the IR rather than an artefact of the printer.
- **`rhdl-vlog`** AST/printer, if a mixed-width binary op is not currently
  representable. Per `architecture.md` this must go through the AST — not
  string templating.
- The typed path (`SignedBits<N> * SignedBits<N>`) requires equal widths
  at the *Rust* type level, so it is unaffected. This proposal is about
  `DynBits`, which is the mechanism that already expresses mixed widths.

### Open questions for the implementing PR

1. Is the operand extension inserted at RHIF→RTL, RTL→NTL, or in the
   Verilog printer? That decides which spec page and which pass tests are
   in scope.
2. Does NTL, being a netlist IR, have a natural place for a mixed-width
   multiply, or does bit-blasting make the question moot at that level?
   If NTL bit-blasts, the win may only be available on the RTL path, which
   would be worth stating explicitly rather than discovering later.
3. Does any existing pass assume multiply operands are equal-width? A
   grep for the multiply opcode's construction sites would answer it.
4. Should the same treatment apply to `xadd`/`xsub`? The DSP argument is
   specific to multiplies, but the asymmetry would be odd, and adders are
   where the pre-extension costs fabric rather than slices.

## Alternatives considered

- **Leave it to Vivado.** The status quo. Defensible, and probably works
  in practice today. Rejected as the *documented* answer because it cannot
  be tested, and because the gap between "the widths say 18 bits" and
  "the RTL says 48 bits" already misled this codebase once.
- **Instantiate DSP48E1 primitives directly.** The
  `vendor-primitive-architecture.md` route. Strictly more powerful and
  strictly more work: it needs the `Target` trait, `primitive!`, and a
  per-vendor primitive library, none of which exist yet. Natural-width
  emission is complementary and much cheaper — it improves inference on
  *every* target rather than adding a primitive for one.
- **Narrow at the widget level.** What the NCO now does: use `xmul`
  instead of resizing to `INT_W`. Gets 48×48 down to 32×32 and is
  available today, but cannot reach 18×14 because the limit is in
  emission. Already taken; this proposal is the remaining half.

## What a validation would look like

Per §11.1, tests at every level the change touches:

1. **Pass-level** — `expect_test` snapshot of the IR before/after,
   showing the operand extension no longer materialised.
2. **Lowering** — a minimal mixed-width multiply lowered through each IR
   with snapshots at each level.
3. **Kernel integration** — a kernel in `crates/rhdl/tests/` doing an
   18×12 `xmul`, `iverilog` round-tripped, asserting the emitted operand
   widths are 18 and 12.
4. **Widget regression** — `cargo test --all` without `UPDATE_EXPECT`.
   Expect HDL snapshots to move anywhere `xmul` is used
   (`dsp::lerp::fixed`, `dsp::nco::sin_cos_linear_interp`) and **no VCD
   digest to move at all**, since the arithmetic is unchanged. A digest
   that moves means the semantics shifted and the PR is wrong.
5. **Negative** — a case where operand and target widths are inconsistent
   should still be rejected with a useful diagnostic.

`dsp::nco::sin_cos_linear_interp`'s
`emitted_multiply_operands_are_natural_width` test already asserts the
operand widths of the emitted multiply, so it will tighten from 32 to 18
when this lands — a ready-made acceptance check.

## Related finding: no vendor primitive is instantiated anywhere

Recorded here because it is the context the question arose in, and because
it is independently worth knowing:

- The emitted Verilog uses behavioural operators throughout
  (`r45 = r44 * r43;`). There is no DSP48, MULT18X18, or any other vendor
  primitive instantiated.
- `grep -rn "DSP48\|MULT18X18\|primitive!"` across `crates/` returns only
  prose in comments.
- There is no `trait Target` and no `hdl_for` in `rhdl-core`.
  `vendor-primitive-architecture.md` is a design document that has not
  shipped.
- `rhdl-bsp` contains `constraints/` and `drivers/` only — no primitive
  library.

So all DSP mapping today is vendor inference from behavioural RTL. That is
a reasonable place to be, and it makes natural-width emission more
valuable rather than less: while inference is the only mechanism, the
quality of what is handed to the inferencer is the only lever available.
