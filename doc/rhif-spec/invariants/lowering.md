# Lowering Invariants — RHIF → RTL → NTL → Verilog

> Normative reference for the semantic correspondence between RHIF and the downstream IRs (RTL and NTL), and ultimately Verilog. Lowering is "semantics-preserving on observables" — the run of an RHIF kernel and the run of its lowered RTL produce the same output for every input.

## Levels of lowering

```
RHIF ──┬── lower_rhif_to_rtl ───→ RTL (untyped SSA)
       │                              │
       │                              │
       │                              ├── stage3 passes (NTL passes) ───→ NTL (netlist)
       │                              │                                        │
       │                              │                                        │
       │                              │                              rhdl-vlog AST → Verilog
```

Each arrow is a function from one IR to another; the property in this document is that each function is observation-equivalent on its domain.

## What "observation-equivalent" means

Two compilers `f` and `g` (i.e., two ways of executing a kernel) are **observation-equivalent** if, for every well-typed input `x`:

```
f(x) = g(x)
```

For RHDL, `f` is "execute as RHIF" (the VM in `rhif/vm.rs`) and `g` is "lower to RTL, execute as RTL" (the VM in `rtl/vm.rs`). The equivalence asserts that lowering produces the same outputs as the source — not the same intermediate values, not the same sequence of operations, just the same observable result.

For Verilog: `g` is "lower all the way to Verilog and run iverilog." The kernel's TestBench wraps both forms and asserts that they produce the same VCD waveform on every cycle. The Tier-4 round-trip tests (per CLAUDE.md §5) operationalise this.

## RHIF → RTL invariants

`lower_rhif_to_rtl` produces an RTL `Object` that is observation-equivalent to the RHIF input. Specifically:

- **Outputs match.** For every input, the RTL output bits equal the RHIF output bits.
- **Well-typed RHIF lowers to well-formed RTL.** RTL has its own well-formedness conditions (defined in `crates/rhdl-core/src/rtl/spec.rs`); the lowering produces only valid RTL.
- **Each RHIF opcode has a finite lowering.** Per opcode:

| RHIF op       | Typical RTL lowering |
|---|---|
| `Noop`         | dropped (no RTL emitted) |
| `Binary(op)`   | `RTL::Binary(op, …)` (same flavour) |
| `Unary(op)`    | `RTL::Unary(op, …)` |
| `Select`       | `RTL::Select` (a 2:1 mux) |
| `Index` (static path) | `RTL::Index` (wire selection — no logic) |
| `Index` (DynamicIndex) | `RTL::Mux` (N:1 multiplexer keyed on the dynamic slot) |
| `Assign`       | `RTL::Concat` of single source — typically optimised out |
| `Splice` (static) | `RTL::Splice` (bit-range write) |
| `Splice` (DynamicIndex) | `RTL::Mux` controlling per-element write enables |
| `Repeat`       | `RTL::Concat` of N copies |
| `Struct` / `Enum` | `RTL::Concat` of fields, with discriminant inlined for `Enum` |
| `Tuple`        | `RTL::Concat` |
| `Case`         | chain of `RTL::Select`s, equality-priority encoded |
| `Exec`         | either inlined (more `RTL::*` ops in the caller) or a Verilog-module instance |
| `Array`        | `RTL::Concat` of elements |
| `AsBits` / `AsSigned` / `Resize` | `RTL` width-change ops |
| `Retime`       | wire pass-through (the colour wrapper has no runtime cost) |
| `Wrap`         | `RTL::Concat` building the discriminant + payload of `Option`/`Result` |

- **Sub-operation count is bounded.** The lowering of one RHIF opcode produces a number of RTL opcodes bounded by the kind sizes (e.g., a `Binary(Mul)` on `Bits<N>` produces an N×N multiplier; not unbounded).

- **No semantic surprises.** Width-extension behaviour, sign-extension behaviour, and `X` propagation all match between RHIF and RTL.

## RTL → NTL invariants

`stage3` passes lower RTL to NTL. NTL is the "netlist" form — closer to gates, no high-level constructs. Invariants:

- **Outputs match.** RTL and NTL produce the same observable values.
- **NTL is "flat".** No more arithmetic ops in their abstract form; everything is bit-level operations on individual wires.
- **Resource accounting is exposed.** Each gate, mux, and flip-flop in NTL corresponds to a real synthesizable element. The compiler can give honest reports of FF count, LUT count, and combinational depth from NTL.

## NTL → Verilog invariants

The Verilog emitter (driven by `rhdl-vlog`) is the final stage. Invariants:

- **Outputs match.** The iverilog-simulated Verilog produces the same VCD as the NTL VM.
- **Verilog is human-readable.** Generated module names match RHDL's structure; signal names match RHDL's slot names where possible; comments preserve traceability to the original kernel.
- **No string templating.** All Verilog flows through the `rhdl-vlog` AST and pretty-printer. (Per `architecture.md` §3 — "Verilog through the AST, never strings.")

## Reset and clock semantics across lowering

RHIF kernels are pure functions; clocks and resets live one level up. The `Synchronous` and `Circuit` traits wrap a kernel with the clock-distribution and reset-fanout logic. At lowering time:

- **Clocks** are generated by the wrapping `Synchronous`/`Circuit` machinery, *not* by RHIF. The `cr.clock` field of `ClockReset` is read inside the kernel as an opaque `Clock` value that the kernel typically does nothing with (the synchronous semantics are imposed by the surrounding `dff::DFF` sub-circuits, which translate to flip-flops at lowering).
- **Resets** are similarly wrapped; the kernel's `cr.reset.any()` becomes an `Any`-reduction over the reset bits, which lowers to a wire that the surrounding flip-flops use as their reset input.

See [`reset-clock.md`](../reset-clock.md) for the full model.

## What lowering does *not* preserve

- **Intermediate slot values.** A pass might fold `s1 ← Add(a, b); s2 ← Mul(s1, c)` into `s2 ← Mul(Add(a, b), c)` (one fused op); the intermediate `s1` value is no longer materialised. Observation-equivalence is on outputs, not on intermediate slots.
- **Operation counts and order.** The lowering is allowed to reorder commutative ops, share common subexpressions, and parallelise; only the final outputs must match.
- **Source-position metadata.** Where lowering produces multiple RTL ops from one RHIF op, the source-location annotations may be the same on all of them. This is fine for diagnostics; not for fine-grained debugging.

## How lowering correctness is tested

- **Tier-3 HDL emission snapshots** (per CLAUDE.md §5.3): every widget pins its emitted Verilog as an `expect_test` snapshot. A change in lowering moves every widget's snapshot; reviewers audit the diff.
- **Tier-4 iverilog round-trip** (per CLAUDE.md §5.4): every widget runs both the RHDL simulator and iverilog on the same input stream; the compiler asserts byte-for-byte VCD equivalence at every cycle.
- **Tier-5 VCD digest** (per CLAUDE.md §5.5): a SHA256 of the VCD output is pinned; any subtle ordering change (which functional tests don't catch) flips the digest.

These three together approximate the "observation-equivalence" property at the engineering level, pending the formal-VM property tests of Phase 2 of this plan.

## Cross-references

- `crates/rhdl-core/src/rhif/vm.rs` — the RHIF VM.
- `crates/rhdl-core/src/rtl/vm.rs` — the RTL VM.
- `crates/rhdl-core/src/compiler/lower_rhif_to_rtl.rs` — the lowering function.
- `crates/rhdl-core/src/compiler/stage3.rs` — RTL → NTL passes.
- `crates/rhdl-vlog/` — Verilog AST and emission.
- `architecture.md` §3 — the multi-IR architecture.
