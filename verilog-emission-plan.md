# Verilog Emission Plan — Making Generated Verilog Human-Readable

A plan for raising the readability of RHDL-emitted Verilog from "machine-generated SSA" to "looks like a competent engineer wrote it." The goal is not to make the Verilog *the* maintenance surface — RHDL kernels remain the source of truth — but to give customers, auditors, and FAEs the option to read, review, and (in extremis) hand-edit the emitted RTL without first decoding the lowering's name-mangling.

This is a tactical design plan focused on a single deliverable. Unlike the seven foundational design plans, it does not introduce new compiler features or shift the IR contract. It tightens the *output* of the existing pipeline so that the deliverable engineers actually receive matches the quality of the source they wrote.

---

## 1 — Motivation

RHDL's emitted Verilog is correct, simulator-faithful, and synthesizable. It is not, by current commercial standards, *readable*. The two are different concerns. A Verilog file that synthesizes identically can be either a transparent description of the design's intent or a 1,500-line wall of `r0 = arg_0; r1 = r0[0:0]; ...` that requires reverse-engineering before any human can review it.

The strategic context: enterprise FPGA customers — automotive, aerospace, defense, medical, financial — frequently need to inspect, audit, and occasionally *hand-modify* RTL before sign-off. Some procurement processes formally require it. The customer's view of an HDL is heavily shaped by the look of the emitted code; an HDL that produces opaque output is harder to sell into a design review meeting than one that produces something a senior Verilog engineer would write themselves. The same emitted Verilog is what gets submitted to lint tools, formal-verification flows, and FAE escalations. All of those benefit from human-readable output even when the engineer never intends to edit it.

This is also a credibility play. The competitor HDLs that this project is positioned against — Chisel, SpinalHDL, Spade, Bluespec, Vitis HLS — each emit Verilog with their own readability profile. SpinalHDL's output is the gold standard among these (clean signal names, structural comments, recognizable always-blocks); Chisel's output is significantly worse than SpinalHDL but still better than RHDL's current emission; Bluespec's is famously opaque (BSV's compiler-output is the canonical example of "you need the source language"). RHDL today is closer to Bluespec on this axis than to SpinalHDL. The tactical move is to leapfrog Chisel's output quality and approach SpinalHDL's, without changing what RHDL fundamentally is.

---

## 2 — Current state

The Verilog emission pipeline is implemented in `rhdl-core/src/hdl/builder.rs` and `rhdl-core/src/ntl/hdl.rs`. The output flows through the `rhdl-vlog` AST and the `Pretty` formatter in `rhdl-vlog/src/formatter.rs`.

### 2.1 A worked example

The shipped test snapshot for a simple Delay widget (`crates/rhdl-fpga/src/core/delay.rs`) emits this:

```verilog
module top(input wire [1:0] clock_reset, input wire [4:0] i, output wire [4:0] o);
   wire [14:0] od;
   wire [9:0] d;
   wire [9:0] q;
   assign o = od[4:0];
   top_dffs c0(.clock_reset(clock_reset), .i(d[9:0]), .o(q[9:0]));
   assign d = od[14:5];
   assign od = kernel_delay(clock_reset, i, q);
   function [14:0] kernel_delay(input reg [1:0] arg_0, input reg [4:0] arg_1, input reg [9:0] arg_2);
         // d
         reg [9:0] r0;
         reg [4:0] r1;
         reg [9:0] r2;
         reg [4:0] r3;
         // d
         reg [9:0] r4;
         reg [4:0] r5;
         reg [14:0] r6;
         reg [1:0] r7;
         localparam l0 = 10'bXXXXXXXXXX;
         begin
            r7 = arg_0;
            r1 = arg_1;
            r2 = arg_2;
            r0 = l0;
            r0[4:0] = r1;
            r3 = r2[4:0];
            r4 = r0;
            r4[9:5] = r3;
            r5 = r2[9:5];
            r6 = {r4, r5};
            kernel_delay = r6;
         end
   endfunction
endmodule
```

The kernel that produced this is fewer than ten lines:

```rust
#[kernel]
pub fn delay<T: Digital, const N: usize>(_cr: ClockReset, i: T, q: Q<T, N>) -> (T, D<T, N>) {
    let mut d = D::<T, N>::dont_care();
    d.dffs[0] = i;
    for i in 1..N {
        d.dffs[i] = q.dffs[i - 1];
    }
    let o = q.dffs[N - 1];
    (o, d)
}
```

A reader who only sees the Verilog has no easy path back to the source intent. Every register is `rN`. The function arguments are `arg_0..arg_2` instead of `clock_reset`, `i`, `q`. The localparam carrying `dont_care()` is named `l0` instead of `dont_care_value` or similar. The bundling of output `o` with the `D` aggregate happens through a 15-bit unnamed `od` wire that is then sliced.

### 2.2 The catalog of readability problems

Cataloged precisely so each can be addressed:

1. **Generic register names `r0..rN`.** The SSA register identifier is what's emitted. The source variable names (`d`, `o`, `next_count`, etc.) are lost.
2. **Generic argument names `arg_0..arg_N`.** The kernel function's parameter names (`cr`, `i`, `q`) are lost; replaced with positional indices.
3. **Verilog `function`-based kernel translation.** The kernel becomes a Verilog `function` returning a packed bit vector, instantiated via `assign`. Most hand-written Verilog uses `always @(*)` blocks for combinational logic; engineers expect that style.
4. **Packed-bundle return wires.** The `(O, D)` tuple is packed into a single `od` wire (e.g., 15 bits = 5-bit `O` + 10-bit `D`), then sliced with `od[4:0]` and `od[14:5]`. The intermediate carries no semantic meaning.
5. **Dont-care literals as anonymous `localparam l0 = 10'bX...X;`.** Should be inline `'X` or named `dont_care_<context>`.
6. **Sparse or absent comments.** The emitted `// d` comment (sourced from the `op_alias`) appears where the user happens to assign to `d`, but doesn't propagate through subsequent register usage. There is no module-level header documenting what the module does.
7. **No alignment.** Same-purpose declarations (all the `reg [N:0] rX;` lines) aren't column-aligned; widths and names jitter line by line.
8. **No structural section markers.** No `// Reset handling`, `// State transitions`, `// Output computation`. The kernel's structure is invisible in the emitted form.
9. **Bit-range slices for single bits.** `r0[0:0]` instead of `r0[0]`, `r0[1:1]` instead of `r0[1]`. Cosmetic but pervasive.
10. **Hierarchical instances with auto-generated names.** `c0`, `c1`, `c2` for child sub-circuits instead of the user's struct field names (`count`, `read_logic`, `write_logic`).
11. **No source-line cross-references.** A `// rhdl: src/delay.rs:42` comment attached to a generated block would let an engineer trace generated code back to its source. Today there's nothing.
12. **Three nesting levels for a trivial operation.** The kernel wrapper module → kernel function → function body. A simple combinational widget has three layers of indirection in the Verilog, where one would do.
13. **Boolean width specifiers `[0:0]`.** A 1-bit signal is declared `wire [0:0] sig;` instead of `wire sig;`. The IEEE 1800 spec accepts both; engineers prefer the unwidth-prefixed form.
14. **No module-level header comment.** No `/* my_widget — purpose, parameters, timing, license */` header. Every commercial RTL block opens with one.
15. **`localparam` for static constants instead of `parameter`.** `localparam` is used for derived-from-parameter values; raw constants used internally to the body are fine as `localparam` but the convention isn't always observed.

### 2.3 What infrastructure already exists

Some lifting has been done. Worth knowing before designing more:

- **`rtl::Object::op_alias`** returns `Option<String>` for some operands; used today to attach a `#[doc = ...]` attribute to a register declaration, which the `Pretty` formatter renders as a `// alias` comment. The aliasing is sparse — only some operands get aliases — but the *mechanism* for propagating source names exists.
- **`SourceLocation`** is preserved through RHIF/RTL passes and is available at every emission site for diagnostic purposes; today only `miette` consumes it. The same span info could feed source-line cross-reference comments in emitted Verilog.
- **`rhdl-vlog`'s AST is rich.** Has explicit nodes for `Case`, `Sensitivity` (posedge/negedge), `Instance`, `Connection`, `Parameter`, all the binary/unary expression shapes. `Block`, `StmtList`, `Stmt::If`, `Stmt::For` (for synthesis-time loops). The shapes needed for a more idiomatic emission are present; the *emission code* is what doesn't use them well.
- **`Pretty` formatter** is a simple indent-and-newline printer. Indent unit is 3 spaces. No alignment, no column rules, no comment-aware spacing.

The ceiling on improvement is therefore higher than the current output suggests. The Verilog AST can already represent idiomatic Verilog; the emission code currently produces a degenerate subset of what the AST could express.

---

## 3 — Design principles

Five non-negotiable principles for the redesign.

**Correctness first, prettiness second.** The emitted Verilog must be functionally identical before and after each phase. Snapshot tests catch regressions. No change to the simulation behavior; no change to the synthesis result. Where an "improvement" affects functionality, it doesn't ship.

**Source-name propagation, not name-mangling.** The user wrote `d.write_address` and `next_count`; the Verilog should say so. Names flow through RHIF, RTL, and NTL via the existing symbol-table mechanism with new fields for source-name hints; the emission consumes those hints when present and falls back to generated names when not.

**Idiomatic Verilog, not synthesized Verilog.** Hand-written Verilog uses `always @(*)` for combinational logic, named instances, structural comments, aligned declarations. The emission targets the hand-written style, not the literal-translation-of-IR style.

**Two emission modes, eventually.** Compact (close to today, optimized for downstream tools) and verbose (heavy comments, source-line cross-references, optimized for human review). Engineers pick the mode that fits their downstream consumer.

**Snapshot-test everything.** Every existing widget has an emit-snapshot; every change to the emission pipeline must update snapshots and survive review of the diffs. The snapshots are the contract; reviewers verify them.

---

## 4 — Phased plan

Four phases, each independently shippable, each with measurable improvement to the snapshots.

| Phase | Theme | Effort | Risk |
|---|---|---|---|
| 1 | Cosmetic improvements (no IR changes) | ~2 weeks | low |
| 2 | Source-name propagation through RHIF/RTL/NTL | ~6 weeks | medium |
| 3 | Structural transforms (always_comb, inlining, named instances) | ~6 weeks | medium |
| 4 | Optional emission modes (compact / verbose / customer) | ~3 weeks | low |

Phase 1 is independent and ships first. Phase 2 is the prerequisite for the deepest Phase 3 transforms. Phase 4 can ship alongside or after Phase 3.

---

## 5 — Phase 1: Cosmetic improvements (no IR change)

The set of low-cost changes that improve readability without touching the IRs. All of these live in `rhdl-vlog/src/formatter.rs` and the emission code in `rhdl-core/src/hdl/builder.rs` and `rhdl-core/src/ntl/hdl.rs`.

### 5.1 Improvements

- **Module-level header comment.** Every emitted module gets a banner comment with: module name, source file path, RHDL version, brief description (taken from the widget's rustdoc summary if available), parameter list with brief annotations. ~10 lines per module.
- **Aligned declarations.** Group declarations by kind (input ports, output ports, internal wires, internal regs, parameters, instances). Within each group, column-align the type, width, and name. Standard Verilog style.
- **Eliminate `[0:0]` for single-bit signals.** A 1-bit signal becomes `wire sig;` not `wire [0:0] sig;`. Trivial AST-level transformation.
- **Eliminate single-bit-range slices.** `r0[0:0]` becomes `r0[0]`, `r0[1:1]` becomes `r0[1]`. Pure formatter rewrite.
- **Better `dont_care` literals.** `localparam l0 = 10'bXXXXXXXXXX;` becomes inline `'X` (Verilog 2001 supports it) or, if the localparam form is required, a named constant like `dont_care_<context>`.
- **Section comments.** The kernel's reset block emits `// Reset handling` before the `if (reset)` clause. The output computation emits `// Output computation`. The state-update emits `// State update`. These come from a small set of well-known kernel patterns the emitter recognizes.
- **Hierarchical-instance naming.** `top_dffs c0` becomes `top_dffs count` if the field is named `count`. Requires the field name to be available at emission time; today it is, just unused.
- **Docstring propagation.** Widget rustdoc → module-header comment. Field rustdoc → declaration `#[doc]` → Verilog comment on the relevant signal.
- **Source-line cross-references.** Every emitted `assign` and `always` block gets a `// rhdl: <file>:<line>` comment with the source line of the originating kernel statement. Uses the existing `SourceLocation` infrastructure.

### 5.2 What it doesn't fix

- Generic register names `r0..rN` — needs Phase 2.
- Generic argument names `arg_0..arg_N` — needs Phase 2.
- The `function`-based kernel translation — needs Phase 3.
- The packed `od` wire — needs Phase 3.

But the visual difference even from these cosmetic changes is substantial. The Delay-widget example above goes from "wall of `rN`" to "module with header, sectioned bodies, named instances, source links." Engineers reviewing the output will see the structure much faster.

### 5.3 Acceptance criteria

- Every existing widget snapshot is updated and reviewed; no functional behavior changes.
- A new test in `crates/rhdl-vlog/tests/` checks that the formatter never emits `[0:0]` widths or `[N:N]` single-bit slices.
- A worked example in the book shows the same widget before and after Phase 1, with reviewer-visible structural improvement.

---

## 6 — Phase 2: Source-name propagation

The deeper change. Today, register names are generated from SSA IDs; the source variable names are lost in the AST → MIR → RHIF lowering. Phase 2 makes them survive.

### 6.1 The mechanism

Each IR's symbol table grows a `name_hint: Option<String>` field per register. The hint is populated at three points:

- **RHIF**: when a register is assigned from a `let` binding, the binding's name becomes the hint. `let next_count = ...` produces a register with hint `"next_count"`.
- **RTL**: hints are preserved when RHIF lowers to RTL; new RTL registers introduced by the lowering inherit hints from their producing operation when meaningful (e.g., a width-cast register inherits the source register's name with a `_resized` suffix).
- **NTL**: same treatment as RTL.

When emission runs, the formatter prefers the hint over the generated `rN` ID. Where multiple registers in the same scope have the same hint (because the kernel rebinds `let next_count = ...` multiple times), the emitter disambiguates with a numeric suffix (`next_count_0`, `next_count_1`).

### 6.2 What this changes for the user

The earlier Delay-widget example becomes (sketched, post-Phase 2 only — Phase 3 fixes more):

```verilog
function [14:0] kernel_delay(input reg [1:0] cr, input reg [4:0] i, input reg [9:0] q);
      // d initial dont-care
      reg [9:0] d;
      reg [4:0] dffs_0;
      reg [9:0] d_after_dffs_0;
      reg [4:0] dffs_1;
      // ...
```

Argument names match the kernel signature. Internal registers carry the source-binding names. The `// d` comment is now redundant with the actual register name `d`.

### 6.3 What it doesn't fix

- Function-based kernel translation (Phase 3).
- The packed `od` return wire (Phase 3).
- The wrapper-module-and-function indirection (Phase 3).

### 6.4 Acceptance criteria

- Every register in every emitted widget has a name traceable to the kernel source.
- The hint propagation has unit tests that verify a `let foo = expr;` in a kernel produces a `reg foo;` (or `reg foo_<n>;` for re-binds) in the emitted Verilog.
- No existing widget snapshot regresses on functional behavior; all snapshots get re-blessed with the better names and the diffs are reviewer-audited.
- The rhif-formalization-plan.md spec documents the `name_hint` field and its propagation rules per pass (per the spec's pass-invariants section).

---

## 7 — Phase 3: Structural transforms

The biggest visual win. Replaces the function-based kernel translation with idiomatic always-block-based emission, eliminates the packed-bundle wire, gives sub-circuit instances meaningful names, and inlines simple kernels into the parent module rather than wrapping them in a Verilog `function`.

### 7.1 Replace `function` with `always @(*)`

Today every kernel becomes a Verilog `function` returning a packed bit vector, called from an `assign` in the parent module. The function is a 1990s-era idiom; modern hand-written Verilog uses `always @(*)` (or SystemVerilog's `always_comb`). The transform:

```verilog
// Before
function [14:0] kernel_delay(input reg [1:0] arg_0, ...);
   reg [9:0] r0; ...
   begin
      r0 = ...;
      kernel_delay = r6;
   end
endfunction
assign od = kernel_delay(clock_reset, i, q);

// After
always @(*) begin : delay_logic
   reg [9:0] d;
   d = ...;
   o = ...;
   d_out = d;
end
```

Same simulation, same synthesis. The `always @(*)` form reads like hand-written code; the function form reads like compiler output.

### 7.2 Eliminate the packed-bundle wire

The kernel returns `(O, D)` and today this becomes a single packed bit vector through an unnamed `od` wire that's then sliced. The cleaner emission has the kernel produce `o` and `d` as separate output registers of the always-block, with no packing needed. Phase 3 does this rewrite in the codegen.

### 7.3 Named hierarchical instances

A widget struct field `count: dff::DFF<Bits<N>>` should produce a Verilog instance `dff_BitsN count(...)`, not `top_dffs c0(...)`. The field name is available at emission time via the `Synchronous` / `Circuit` derive output; today the emitter discards it.

### 7.4 Inline trivial kernels

A kernel with fewer than N statements (where N is a tunable threshold; recommend 5) is inlined directly into the parent module's `always @(*)` block, skipping the wrapper-module indirection. For the smallest widgets (counter, edge-detector, similar), this collapses three Verilog modules into one.

### 7.5 Recognize and emit `case` statements

Today a kernel `match` lowers to a chain of `Select` ops, which become a chain of `assign x = cond ? ... : ...;` ternary expressions. Idiomatic Verilog uses a `case` statement. The codegen pattern-matches on the lowered IR and emits the `case` form when the match was on a discriminant.

### 7.6 What it doesn't fix

- Some inherently-RHDL idioms have no idiomatic Verilog equivalent (e.g., the `Wrap` opcode for `Option<T>` doesn't have a clean Verilog form; we emit it as a packed bit pattern with a comment explaining the layout).
- Vendor-primitive instantiations (per `vendor-primitive-architecture.md`) emit verbatim what the target says; we don't try to prettify those.

### 7.7 Acceptance criteria

- The Delay-widget example emits an `always @(*)` block with named registers, no packed bundle wire, named DFF instances, ~30 lines instead of ~50.
- Every existing widget snapshot is re-blessed; reviewer-audited.
- A worked example in the book shows the same widget before all four phases and after each phase, with side-by-side comparison.

---

## 8 — Phase 4: Optional emission modes

Three modes the user picks at codegen time:

```rust
descriptor.hdl_for_synth(&target).with_emit_mode(EmitMode::Compact)
descriptor.hdl_for_synth(&target).with_emit_mode(EmitMode::Verbose)
descriptor.hdl_for_synth(&target).with_emit_mode(EmitMode::Customer)
```

### 8.1 Compact mode

Optimized for downstream tools. No comments beyond what's necessary for synthesis directive (e.g., timing constraints). Aligned declarations. Idiomatic Verilog style. ~30% smaller than verbose mode.

### 8.2 Verbose mode (default)

Optimized for engineer review. Heavy comments. Source-line cross-references on every block. Module-level docstring. Parameter annotations. The output the customer reads first.

### 8.3 Customer mode

Optimized for hand-off. Verbose mode plus: redundant signal-name commentary (every internal signal documented), expanded ternary chains for clarity, signed/unsigned interpretation hints, and a module-level "this is generated; here's how to consume it" header. The output that goes into a customer deliverable folder.

### 8.4 Acceptance criteria

- Every existing widget snapshot has three variants — one per mode — committed.
- The emission flag is reachable from `cargo rhdl emit` (CLI subcommand) and from `Descriptor::hdl_for_synth().with_emit_mode()`.
- A worked example in the book shows the same widget under all three modes.

---

## 9 — Validation

Per CLAUDE.md §11.1, every phase is a compiler-level change:

- **Snapshot tests at every level.** Every existing widget has a Verilog snapshot today; every change to the emission re-blesses every snapshot. The diff is reviewer-audited.
- **Functional equivalence.** Pre-change and post-change Verilog must produce byte-identical iverilog simulation output for the existing iterator-based test inputs. This is the safety net.
- **Style-lint integration.** The formatter optionally runs the emitted Verilog through `verible-verilog-format` (an open-source Verilog formatter from Google) as a final pass. This catches drift between the generated style and the canonical-Verilog-style baseline.
- **Customer-side review.** A small panel of FPGA engineers (internal team plus 2-3 external reviewers) review post-Phase-3 output against representative widgets; their comments drive the Phase 4 emission-mode tuning.

---

## 10 — Risks and open questions

**Naming collisions.** Source-name hints can collide with Verilog reserved words (`reg`, `wire`, `module`, etc.) or with each other across nested scopes. The emitter must rename collisions with deterministic suffixes; the rules need to be documented.

**Performance.** The `Pretty` formatter is fast; the proposed alignment-and-comment-injection pipeline is slower. For very large designs (thousands of widgets) this may add seconds to compilation. Acceptable as long as the slowdown is measured and bounded; not acceptable if it's an order of magnitude.

**Snapshot churn during transition.** Every existing snapshot has to be re-blessed at each phase; the diff burden on reviewers is real. Mitigation: ship the phases as narrow, focused PRs with snapshots updated per-PR rather than en-masse at the end.

**Verbose mode line counts.** A widget that emits 100 lines today might emit 300 lines under verbose mode (heavy comments, source links, alignment). Some downstream tools choke on very large files. The compact mode is the safety valve.

**Verilog dialect drift.** "Idiomatic Verilog" is itself a contested topic. SpinalHDL's idiom is different from Vivado's IP Integrator's, which is different from what you'd see in a 2010-era ASIC RTL textbook. We pick one (probably the ASIC-RTL-textbook flavor; it's the most universally legible) and stick with it.

**Vendor-primitive collisions.** Per `vendor-primitive-architecture.md`, we emit vendor primitives verbatim. Their style won't match the rest of the file. The emitter should add a `/* begin vendor-primitive */ ... /* end */` comment fence around each and avoid trying to "fix" the style.

**SystemVerilog vs. Verilog.** Some readability improvements (`always_comb` instead of `always @(*)`, typed structs, nested arrays) require SystemVerilog. Need a config flag for "emit pure Verilog 2001" vs. "emit SystemVerilog 2017" — most modern tools accept the latter but some legacy synthesizers don't. Default to Verilog 2001 for safety; opt into SystemVerilog when known-safe.

**Comment drift with the source.** Source-line cross-references can become stale if the kernel source moves (unlikely within a single emission, but real over time across version-controlled snapshots). The cross-reference comment should include a content hash of the source line as well as the line number, so reviewers can detect drift; or the reference should be regenerated on every emission.

---

## 11 — Comparison with other HDLs

For grounding: how does this work in other Rust-or-Scala-or-Python HDLs?

**SpinalHDL.** Well-named registers, structural comments, hierarchical instance names, mostly idiomatic Verilog output. The gold standard among modern HDLs. Achieves this by preserving Scala variable names through the entire elaboration → backend pipeline; the names survive because the elaborator runs in JVM and Scala's reflection captures them.

**Chisel + FIRRTL.** FIRRTL preserves names better than RHDL's RHIF does today, but the emitted Verilog still has many SSA-derived register names. Better than RHDL today; worse than SpinalHDL.

**Bluespec.** The compiler output is famously opaque. Generated Verilog is essentially a dump of internal scheduler state. Bluespec users don't read the output; the source is the only viable maintenance surface.

**Vitis HLS.** Generated Verilog uses C-source-line cross-references aggressively (every emitted line has a comment pointing back to the C source). Names are pseudo-meaningful but heavily mangled. Heavy comments. Closer to what we want for verbose mode.

**Spade.** Spade's generated Verilog inherits the source's pipeline-stage labels and signal names. Modest output size. Style is workmanlike but not as polished as SpinalHDL.

**Amaranth (formerly nMigen).** Python-source names propagate cleanly into RTLIL (Yosys IR); the resulting Verilog is reasonable, though Yosys's pretty-printer is the default style and not particularly tailored. Comparable to Chisel.

The recommended target is SpinalHDL-equivalent output by Phase 3, with Phase 4's verbose mode adding Vitis-HLS-style cross-referencing for customer-facing builds.

---

## 12 — Sequencing recommendation

Phase 1 ships as one focused PR (~2 weeks) with the cosmetic changes. Reviewer can audit the snapshot diffs in a single sitting because no functional behavior changes.

Phase 2 ships as one larger PR (~6 weeks). The IR-level work is meaningful; the snapshot diffs are larger because every register name changes. Coordinate with `rhif-formalization-plan.md` Phase 1 (the prose spec) — this is exactly the kind of thing the spec should document, so the two PRs reinforce each other.

Phase 3 ships in two sub-phases: 3a (always_comb conversion + named instances + bundle elimination, ~4 weeks) and 3b (inline trivial kernels + case-statement recognition, ~2 weeks). Splitting reduces the per-PR review burden.

Phase 4 ships when there's customer demand for the verbose / customer modes. May not need all three modes; could be just compact + verbose. Customer mode is a marketing artifact more than an engineering one.

---

## 13 — References

[1] IEEE 1800-2017. *SystemVerilog Language Reference Manual.* — The standard for the SystemVerilog idioms (always_comb, structs, packed/unpacked arrays).

[2] IEEE 1364-2005. *Verilog HDL Language Reference Manual.* — The Verilog 2001 standard; the safer default for tool compatibility.

[3] Sutherland, S. *RTL Modeling with SystemVerilog for Simulation and Synthesis.* — The standard textbook on idiomatic RTL.

[4] Lattice Semiconductor / Open-Source Hardware Community. *verible* (https://github.com/chipsalliance/verible). — Google's open-source Verilog formatter and lint tool. Recommended as the optional final-pass formatter.

[5] Sutherland, S., Davidmann, S., Flake, P. *SystemVerilog for Design.* — Style guide for SystemVerilog hand-written code; what we target as "idiomatic."

[6] SpinalHDL Project. *Generated Verilog Examples.* https://spinalhdl.github.io/SpinalDoc-RTD/. — The gold standard for HDL-generated Verilog quality.

[7] Bachrach, J., et al. *Chisel: Constructing Hardware in a Scala Embedded Language.* DAC 2012. — For comparison with Chisel's output style.

[8] Pellauer, M., et al. *A-Ports: An Efficient Abstraction for Cycle-Accurate Performance Models on FPGAs.* FPGA 2008. — Adjacent precedent for HDL-as-source emission.

[9] AMD/Xilinx. *Vitis HLS User Guide* (UG1399). — For comparison with HLS output style; particularly the source-line cross-reference convention.

---

## 14 — Decisions captured

For the record (also reflected in `architecture.md` and `CLAUDE.md` once shipped):

- **Verilog readability is a deliverable, not an accident.** The emitted Verilog is what customers see; its quality is part of the project's commercial credibility.
- **The kernel remains the source of truth.** Generated Verilog is for review and audit, not for hand-maintenance. The pretty output should *enable* customer comprehension, not become the canonical form they edit.
- **Source-name propagation through RHIF/RTL/NTL is required.** The `name_hint` field is added to each IR's symbol table; passes preserve hints; emission consumes them. Documented in the rhif-formalization-plan.md spec.
- **Idiomatic Verilog is the target.** `always @(*)`, named instances, structural comments, aligned declarations. We do not try to make the output look like the kernel source.
- **Multiple emission modes.** Compact / verbose / customer. The user picks; the default is verbose.
- **SystemVerilog 2017 is opt-in, not default.** Pure Verilog 2001 is the default for tool compatibility.
- **Snapshot tests are the contract.** Every emission change updates every snapshot; reviewer audits the diffs.
- **Functional equivalence is the safety net.** No emission change ships without iverilog-round-trip confirmation that the new output produces the same waveform as the old.
- **Vendor primitives emit verbatim.** Per `vendor-primitive-architecture.md`; their style is fenced with comments and not "fixed."
- **The plan does not change the IR contract.** It changes only what falls out the bottom of NTL → Verilog. Other design plans (auto-pipelining, FSM, RCStream, rules, RHIF formalization, vendor primitives) compose with this without conflict.
