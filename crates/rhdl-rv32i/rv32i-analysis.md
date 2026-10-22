# RV32I Implementation Review — Expressiveness Assessment

*Captured 2026-04-30. Origin: code review of the v0.1/v0.2/v0.3 RV32I implementation against RHDL's expressiveness goals. Scope: every file under `crates/rhdl-rv32i/src/`.*

---

## Verdict

The code is **good — better than expected for a v0.1**, but it's also a clear demonstration of where RHDL's language-level pain points still live. The structural choices are right; the syntactic ergonomics aren't there yet, and the rough spots cluster in exactly the places the kernel-language-extensions plan was meant to address. That's actually encouraging because it means the design plan is on target.

Total LOC: roughly 2567 across 11 files, of which approximately 1800 is actual core logic (excluding the 440-line compliance harness). For comparison: Sodor's RV32I in Chisel is ~2200 LOC for similar scope; picorv32 in Verilog is ~3000 LOC and famously dense. RHDL's RV32I is competitive on lines and substantially clearer on intent.

---

## File-by-file walkthrough

### `isa.rs` (228 lines) — excellent

The `DecodedInstruction` struct as a "control word" is the right abstraction — every signal the executor needs in one place, no per-opcode dispatch in the executor itself. The choice to have separate small enums for `Opcode`, `AluOp`, `BranchOp`, `MemOp`, `AluSrc`, `WritebackSrc`, `CsrOp`, `SystemOp` rather than one mega-enum is correct: each one matches exactly one piece of the decoder's responsibility, each is small enough that an exhaustive match reads as a spec table.

The `AluOp::Pass` pseudo-op for LUI is elegant — keeps the ALU's match exhaustive without a special case, and the comment explains intent. This is the level of detail-orientation the code-review-readability bar expects.

### `alu.rs` (52 lines) — genuinely beautiful

A 32-bit RV32I ALU in 25 lines of actual logic. The match statement reads almost exactly like the RV32I spec table:

```rust
match op {
    AluOp::Add  => a + b,
    AluOp::Sub  => a - b,
    AluOp::Sll  => a << shamt,
    AluOp::Slt  => if a_signed < b_signed { bits::<32>(1) } else { bits::<32>(0) },
    AluOp::Sltu => if a < b { bits::<32>(1) } else { bits::<32>(0) },
    AluOp::Xor  => a ^ b,
    AluOp::Srl  => a >> shamt,
    AluOp::Sra  => (a_signed >> shamt).as_unsigned(),
    AluOp::Or   => a | b,
    AluOp::And  => a & b,
    AluOp::Pass => b,
}
```

`(a_signed >> shamt).as_unsigned()` for SRA is exactly right. This is the level of clarity that makes the AI-assist wedge concrete — an LLM looking at this kernel and adapting it to RV32IM is a 5-minute job because every operation maps to one match arm.

### `reg_file.rs` (109 lines) — architecturally right

Bundled-state DFF pattern correctly applied — and the comment explains *why* (the 12-element auto-tuple ceiling, the `Bits<1024>` workaround that doesn't work). This is a worked example of CLAUDE.md §3.1 paying for itself.

The "no same-cycle write-read bypass" comment is the kind of design decision a future contributor will need to read; it's exactly the right comment in exactly the right place.

**One note for follow-up:** the bypass *could* exist in single-cycle without forming a loop — the loop fear in the comment is `addi x1, x1, 1`-style, but in single-cycle the read happens combinationally on `q.regs` and the write goes to `d.regs`, so the same-cycle bypass would just be a mux on the read output checking `i.waddr == i.raddr1`. Worth re-examining; either the comment's analysis is wrong or there's a subtler issue not yet identified. The combinational reachability matrix from `combinational-reachability-and-loop-detection.md` should be able to confirm or refute this claim mechanically once that work ships — that's exactly the kind of analysis it provides. If the matrix shows no loop forms, the comment is stale and the bypass can be added.

### `cpu.rs` (328 lines) — credible reference implementation

The single-cycle CPU kernel reads top-to-bottom in roughly the order a software person would describe the datapath: decode → read regs → pick ALU operands → ALU → memory ports → branch decision → CSR access → trap detection → writeback selection → register-file drive → CSR drive → next-PC commit. That's exactly what an HDL kernel should read like.

The kernel is dense but not overlong; the reset-comes-last discipline is followed; sub-circuit composition (`d.rf = RegIn { ... }`, `d.csrs = CsrIn { ... }`) is uniform. **This is a credible reference implementation** that compares favorably against Sodor's Scala or picorv32's hand-tuned Verilog.

### `decoder.rs` (365 lines) — where the language pain becomes visible

The structural correctness is fine — every RV32I opcode is covered, the `funct3` dispatch is exhaustive, illegal encodings produce `illegal: true`. But the syntactic expressiveness is rough:

```rust
if funct3 == f3_zero {
    d.alu_op = if funct7 == f7_alt { AluOp::Sub } else { AluOp::Add };
} else if funct3 == f3_one {
    d.alu_op = AluOp::Sll;
} else if funct3 == f3_two {
    d.alu_op = AluOp::Slt;
} else if funct3 == f3_three {
    d.alu_op = AluOp::Sltu;
} ...
```

This is the code that *cries out* for `match` on a `Bits<3>` with numeric literal patterns:

```rust
match funct3 {
    0b000 => if funct7 == 0b0100000 { AluOp::Sub } else { AluOp::Add },
    0b001 => AluOp::Sll,
    0b010 => AluOp::Slt,
    0b011 => AluOp::Sltu,
    0b100 => AluOp::Xor,
    0b101 => if funct7 == 0b0100000 { AluOp::Sra } else { AluOp::Srl },
    0b110 => AluOp::Or,
    0b111 => AluOp::And,
}
```

That's roughly half the lines and reads like a spec table. The need for the manual `let f3_zero: Bits<3> = bits::<3>(0)` ceremony is a serious source of verbosity, and the workaround pattern (declaring named constants before the if-chain) actively works against readability — by the time you reach `if funct3 == f3_seven` you've forgotten what value `f3_seven` actually represents.

The decoder is hitting three distinct syntactic patterns that aren't well-supported today; each is detailed in the "Language gaps" section below.

### `pipelined.rs` (516 lines) — kernel-size pain

Each pipeline stage is computed in a single sweep of the kernel, in reverse order (W → M → E → D → F), with hazard logic inlined. The structure is correct; it's just a lot of code in one function. The hazard helpers (`detect_load_use_stall`, `forward_select`, `writes_back`) are well-extracted, but the body of the kernel is unavoidably long because every pipeline-stage's logic is there.

**`rhdl-rule` Phases 1 and 2 have shipped** (in `crates/rhdl-rule` and `crates/rhdl-rule-core`, with full annotation support — `urgent_before`, `mutually_exclusive`, `conflict_free` — pilot widget tests, and emission-snapshot regression tests). The pipelined CPU is **not waiting for `rhdl-rule` — it's waiting for someone to rewrite it using `rhdl-rule`.**

Each pipeline stage is structurally a rule — "given the current state of EX/MEM, compute MEM/WB-next." The rules are scheduled by the Carloni interlock plus the stall/squash signals. Expressed as `#[rule]`-attributed methods, the pipelined CPU could plausibly be five rules of ~50 LOC each (Fetch, Decode, Execute, Memory, Writeback), with inter-stage ordering managed by the rule scheduler instead of by the current reverse-order kernel layout. That cuts the kernel size by maybe 60% and dramatically improves AI-assistability, because each rule is independently understandable.

The fact that `pipelined.rs` was written without using `rhdl-rule` despite Phase 2 being complete is itself diagnostic: either the rule pilot widgets haven't yet covered enough of the pipeline-stage shape to make the rewrite obvious, or the author wrote `pipelined.rs` against an earlier baseline before Phase 2 landed. Either way, **the rewrite is the right next polish step** — and it's a concrete deliverable, not a speculative one.

Note the pipelined CPU does **not** use `#[derive(Fsm)]`, despite having clear stall/squash/normal-flow state at each stage. Per CLAUDE.md §12.14, FSM-shaped widgets benefit from the derive — but the pipeline's "state" is implicit in the `valid`/`stall`/`squash` flags rather than in an explicit enum. Defensible choice; arguable that an explicit `PipelineState` enum per stage would surface the FSM diagrams the way every other major core in the library does. Worth thinking about for the v0.3 polish pass.

---

## Strengths confirmed

These are properties of the code that demonstrate RHDL is delivering on its architectural promises:

**The `#[kernel]` discipline produces code that reads naturally as a hardware datapath description.** Walk through `cpu_kernel` top-to-bottom and you read it the way you'd describe the CPU on a whiteboard. This is non-trivial; it's the property Verilog has and most HLS tools have lost.

**Sub-circuit composition through `Q`/`D` bundles works cleanly.** The regfile, csrfile, and pipeline registers all integrate uniformly. Each shows up as a struct field on the parent widget, gets driven via `d.field = ...`, and exposes its outputs via `q.field`. There's no special-case handling for "this is a memory" vs. "this is a register" — composition is uniform.

**The bundled-state-DFF pattern from CLAUDE.md §3.1 was load-bearing for the regfile.** Without it, 32 entries would have hit the 12-element ceiling on auto-derived Q/D tuples. This is exactly the case the pattern was documented for; the regfile validates the pattern's necessity.

**Pure helper kernels compose cleanly.** `imm_i`, `imm_s`, `imm_b`, `imm_u`, `imm_j`, `branch_taken`, `load_format`, `forward_select`, `forward_value` are all tiny pure-functional kernels invoked from the larger kernels. The architecture's promise of "kernels are pure functions; you compose by calling them" pays off in practice.

**Documentation is excellent throughout.** Every public item has a doc comment. The module-level `//!` blocks explain what's in scope, what's deferred, and what the rationale is. The "no same-cycle write-read bypass" comment is the right shape and depth for a future contributor.

**Module decomposition is right.** Five immediate-extraction helpers as separate kernels means each can be unit-tested in isolation, which is exactly the Tier-1 validation discipline CLAUDE.md §5 wants.

---

## Weaknesses confirmed — and what they imply for pending work

These are language-level pain points the code surfaces. Each maps to a specific entry that should land in `kernel-language-extensions.md` or to a sequencing implication for an existing plan.

### 1. Match on numeric values of `Bits<N>`

Currently match arms accept enum variants but not numeric literals on bit values. The funct3 dispatch becomes an if-else-if-else cascade. **Worth elevating to the front of the kernel-language-extensions queue.**

Tier 1 desugaring; the macro layer can rewrite into existing `if-else-if` cascade equivalent. Probably 1 week of compiler work. Highest immediate payoff for the decoder's readability.

### 2. Bit slicing syntax

Every field extraction is `(instr >> N) & bits::<32>(MASK)`. A Verilog programmer writes `instr[11:7]`. Even a method-style call like `instr.slice::<11, 7>()` or `instr.bits(11..=7)` would be a transformative readability improvement. **This is the single biggest visual contributor to the decoder's verbosity.**

This is **not currently captured in the kernel-language-extensions plan**, and after seeing the decoder, it should be — it would benefit every bit-manipulation widget in the library, not just the RV32I core. Every protocol PHY in Tier 3 of the widget roadmap is doing instruction-decoder-style bit-field extraction; every one of them suffers the same verbosity tax.

### 3. Bit concatenation syntax

The B-type and J-type immediate construction is the most painful code in the file:

```rust
let bit12: Bits<32> = ((instr >> 31) & bits::<32>(1)) << 12;
let bit11: Bits<32> = ((instr >> 7) & bits::<32>(1)) << 11;
let bits_10_5: Bits<32> = ((instr >> 25) & bits::<32>(0x3F)) << 5;
let bits_4_1: Bits<32> = ((instr >> 8) & bits::<32>(0xF)) << 1;
let combined: Bits<32> = bit12 | bit11 | bits_10_5 | bits_4_1;
let sign_mask: Bits<32> = if (instr >> 31) & bits::<32>(1) != bits::<32>(0) {
    bits::<32>(0xFFFF_E000)
} else {
    bits::<32>(0)
};
combined | sign_mask
```

A Verilog programmer writes:

```verilog
assign imm_b = {{19{instr[31]}}, instr[31], instr[7], instr[30:25], instr[11:8], 1'b0};
```

That's one line versus 12. RHDL's verbosity tax here is steep enough that a `concat!()` macro or a builder API for assembling bit-fragments-with-positions is a genuine ergonomic gap. The signed-shift trick used in `imm_i` (`(raw >> 20).as_unsigned()`) is elegant but only works for the trailing-zeros-only case; it doesn't help for J-type or B-type.

### 4. The `bits::<N>(literal)` ceremony

Almost every line has it. A `bits!(0x1F)` macro, or method-chained `0x1F_u32.bits::<32>()`, or numeric-literal type inference that picks `Bits<32>` from context — any of these would produce material readability gains. Smallest-possible change, biggest gain for "lines per match arm."

### 5. Pipelined-CPU kernel size

`pipelined.rs` is 516 lines and the kernel itself is the bulk of that. Not because the code is bad — because each pipeline stage's logic has to be expressed somewhere, and the *current* implementation chose the single-kernel-with-reverse-order approach. **`rhdl-rule` Phases 1 and 2 have already shipped**, so the cleaner approach (each pipeline stage as a `#[rule]`-attributed method) is available today; the pipelined CPU is a concrete near-term rewrite candidate, not a hypothetical one.

### 6. No FSM derive on the pipelined CPU

The pipeline has natural per-stage state (normal / stall / squash) but it's encoded as `valid`/`stall`/`squash` flags rather than as an explicit FSM enum. This costs the FSM-derive-generated diagrams that the rest of the library benefits from. Worth considering an explicit `PipelineState` per stage during the v0.3 polish pass.

---

## What this implies for the design plans

### kernel-language-extensions.md

Pick up four high-priority entries surfaced by this code review:

1. **Numeric match patterns** on `Bits<N>` — match arms accepting `0b001`, `0xA`, etc. with bit-width inferred from the matched type. Tier 1 desugaring; macro-layer rewrite. ~1 week.
2. **Bit slicing syntax** — method-style (`instr.bits::<11, 7>()`) or range-style (`instr[11..=7]`). Method-style is Tier 1; range-indexing-syntax needs compiler support.
3. **Bit concatenation syntax** — `concat!(...)` macro accepting a list of bit fragments with positions. Tier 1 macro.
4. **`bits!()` macro / improved literal inference** — `bits!(0x1F)` resolving to `Bits<N>` from context.

These four together would compress the decoder by approximately 40% and make the decoder code read like a spec table rather than like a series of bit manipulations. The downstream effect on every protocol PHY in the widget roadmap is even larger because they all do similar field-extraction work.

### rule-architecture.md

`rhdl-rule` Phases 1 and 2 are shipped. The 5-stage pipelined CPU should be the **next pilot rewrite** under whatever follow-on Phase 2-polish or pre-Phase-3 effort is in flight — adding it to the existing pilot list demonstrates that `rhdl-rule` handles a real production-shape complexity (a pipelined RISC-V core), not just academic toy widgets. The rewrite would cut the kernel size dramatically and produce a publishable case study.

### combinational-reachability-and-loop-detection.md

The `reg_file.rs` "no same-cycle write-read bypass" comment is a test case. The matrix should be able to confirm or refute mechanically whether the bypass actually creates a loop. If the matrix shows no loop forms, the comment is stale and the bypass can be added (improving single-cycle CPI). This is a concrete validation opportunity for the matrix once it ships.

### tier-c-flagship-cores.md

The Tier C-1 task (#70 in the queue) marks RV32I as "first among Tier C cores; lowest risk, highest immediate strategic value." This implementation validates that assessment — it's a credible flagship in its current form, and the gaps that remain are language-level (kernel-language-extensions surface) rather than core-level.

The marketing artifact for Tier C-1 is now real: a 1500-line pure RHDL RV32I core that compiles cleanly, lockstep-validates against Spike (when the harness lands), and reads cleanly enough to use as a teaching reference. That's a concrete demo for the strategic plays in `chisel-strategy.md` (compile-time + Verilog-output-quality + AI-assist wedges) and `bsv-strategy.md` (clean composition + good diagnostics).

---

## Bottom line for the strategy

The code is **good enough today** to credibly demonstrate RHDL on stage, in a paper, or in a recruiting demo. It is **not yet what it could be** — and the gap between "today" and "what it could be" is precisely the kernel-language-extensions and `rhdl-rule` work that's already in the design-plan queue.

The forcing function for prioritizing those plans just got a lot more concrete: every line of the decoder's funct3 dispatch is an argument for shipping numeric match patterns and bit slicing.

Recommended next steps, in priority order:

1. **Add the four kernel-language-extensions entries** (numeric match patterns, bit slicing, bit concatenation, literal ceremony) to `kernel-language-extensions.md` as concrete §2.x entries with worked-example before-and-after code drawn from the decoder. Each becomes a Phase-1-sized implementation task.
2. **Rewrite the pipelined CPU using `rhdl-rule`** (Phases 1 and 2 are already shipped). The rewrite is the marquee case study demonstrating `rhdl-rule` on a real production-shape widget. Add it to `rule-architecture.md`'s pilot list as a completed-or-in-flight item.
3. **Validate the regfile bypass claim** with the combinational reachability matrix once Phase 1 of that work lands. Either fix the comment or add the bypass.
4. **Run the decoder through a hypothetical "with bit slicing + numeric match patterns" rewrite** as a worked example. Compute the actual line-count delta. That number is the headline for the "kernel-language-extensions ROI" pitch.

The implementation team should be told: this code is publishable. Polish passes (FSM derive, RCStream memory ports, language-extension migrations) come after the lockstep-with-Spike validation lands. Don't block on language work; the language work blocks on this code surfacing the actual pain points, which it now has.
