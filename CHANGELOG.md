# RHDL Build Narrative

This file is the *story* of how RHDL — and especially its widget library in `crates/rhdl-fpga` — has evolved. It is not a `git log`. It is the record of what was built, **why**, what we learned along the way, and what followed up that wasn't obvious from the diff.

If `git log` answers *what changed and when*, this CHANGELOG answers *what we were trying to do and what we discovered*. New widgets, design pivots, gotchas hit during development, and follow-up debt all belong here. PRs and routine refactors that don't change a load-bearing decision do not.

## How to use this file

- **Adding an entry is mandatory.** See `CLAUDE.md` §16 — every widget, fix, or design pivot must land with a CHANGELOG entry in the same commit.
- Entries are grouped by date (newest at the top) and organized as discrete stories. One widget = one entry, even if it took several commits.
- Each entry follows the template below. Skip a section only if it's genuinely empty (e.g., no follow-ups), not because writing it is annoying.
- Be honest about workarounds. If you used `.skip(!0)`, hard-coded a constant to dodge a framework limit, or marked a test `#[ignore]`, say so and add it to the Follow-ups list in `widget-roadmap.md`.

### Entry template

```markdown
## YYYY-MM-DD — <Widget or change name>

**Path:** `<source path>` (and example/doc/test paths if relevant)

**Why this, why now:** <one paragraph — what unblocks, what motivates, what consumer is asking>

**Design decisions:** <bullet list — the choices made and what was rejected and why>

**Surprises and gotchas:** <bullet list — what didn't work the first time, what RHDL or the framework did that we didn't expect, what we'd warn the next builder about>

**Validation:** <one or two sentences — which tiers of the CLAUDE.md contract are met, and any honest deviations>

**Follow-ups:** <bullet list — anything deferred; cross-link to widget-roadmap.md "Follow-ups" section if added there>
```

---

## 2026-04-28 — Multi-bit handshake bridge (slow CDC)

**Path:** `crates/rhdl-fpga/src/cdc/slow_crosser.rs` (+ example, doc, vcd)

**Why this, why now:** Roadmap row #4 (Tier 0) — the only multi-bit CDC primitive currently in the tree is the Gray-coded `cross_counter` inside `AsyncFIFO`, which is specialized for monotonic counters. Anything else (config buses, status registers, command codes) had no path across clock domains. This is the textbook 4-phase req/ack handshake with single-bit synchronizers gating a stable W-domain data register sampled by R.

**Design decisions:**
- Hand-written `impl Circuit` (matching the `Sync1Bit` and `BitSyncChain` pattern) rather than a kernel-based composition. The data crossing primitive (W-domain wire sampled by R-domain register only after `req_sync_2` confirms stability) cannot be expressed with the framework's current type-system-enforced domain separation, so the widget directly implements `sim()` and `hdl()`.
- Two state machines, one per clock domain: source has `Idle / WaitForAck / WaitForAckClear`, destination has `Idle / WaitForReqClear`. Encoded as `Digital` enums (`SrcState`, `DstState`).
- `req` (W→R) and `ack` (R→W) each go through a 2-FF synchronizer chain in the destination domain. Documented; the metastability protection lives in those chains.
- Output struct carries signals from *both* domains (`data: Signal<T, R>`, `busy: Signal<bool, W>`) — verified that `#[derive(Timed, Digital)]` handles multi-domain output structs cleanly.
- `data_reg` is held stable from step 1 through step 5 of the handshake, so the destination samples it directly without per-bit synchronization. This is the standard CDC trick and saves `T::BITS` worth of flip-flops vs. naively chaining a sync per bit.

**Surprises and gotchas:**
- **vlog pretty-printer drops the trailing `;` after `wire [0:0] src_send;` specifically.** I lost ~30 minutes to this. Renaming `src_send` → `send_in` (and the corresponding wire) made the issue go away. Other identifiers using the same `wire [0:0] <name>;` form (`src_clock`, `src_reset`, `dst_clock`, `dst_reset`) printed correctly. I do not yet know whether `src_send` is somehow keyword-adjacent in the vlog grammar or whether this is a printer bug; recorded as a follow-up to investigate.
- The async testbench iverilog limitation strikes again — same `.skip(!0)` workaround as `Sync1Bit` and `BitSyncChain`. Cross-link to the existing follow-up in `widget-roadmap.md`.
- **Pattern recap for hand-written multi-domain widgets:** state struct holds *current* and *next* values for every register, plus the last-seen clock for each domain (for edge detection). The `sim()` body has three logical stages per call: pre-edge computation (when each clock is low, compute next values), reset overrides (force next values to safe defaults if reset is asserted), edge-triggered latching (copy next → current on each rising edge). Hard to get right the first time; the `Sync1Bit` source is the canonical reference.

**Validation:** Tier 2 (`test_crossings_arrive_in_order` — sample R-domain output on each negative-edge of `dst_clock` and verify the four sent values appear in order); Tier 3 (HDL length sanity check at 2522 chars); Tier 4 (`iverilog` elaboration via `.skip(!0)`); Tier 5 (VCD digest). 4 tests, all passing.

**Follow-ups:**
- **Investigate the `wire [0:0] src_send;` vlog pretty-printer issue.** Reproducible: revert the rename and the generated Verilog drops the trailing `;`. Fix is either in the vlog parser, the pretty-printer, or both. May affect other widgets that use `_send`-suffixed identifiers.
- **`r_data` ready-to-consume strobe** — current API doesn't tell the destination *when* a new value arrived (`data` always presents the latest, but a one-cycle "fresh" pulse on the R side would let consumers chain on it). Recorded for v2.
- **Throughput** — every crossing takes ~6–8 source cycles + ~6–8 destination cycles. For higher throughput, use `AsyncFIFO`. Documented in the widget rustdoc.

---

## 2026-04-28 — Multiply-accumulate (MAC) unit (unsigned)

**Path:** `crates/rhdl-fpga/src/core/mac.rs` (+ example, doc, vcd)

**Why this, why now:** Roadmap row #15 — DSP foundation primitive. Required for any FIR/IIR filter, signal-processing pipeline, or integer-arithmetic neural-net inference. Companion to the divider just shipped.

**Design decisions:**
- Single-cycle multiply-and-accumulate (no pipeline registers between multiply and add). Throughput is one MAC per cycle. Considered a 2-stage pipelined variant — rejected for v1 because the single-stage form is simpler and the wider critical path is acceptable at the small `N` typical for early DSP work. The pipelined variant becomes natural once `auto-pipelining-plan.md` lands.
- Full-precision intermediate via `DynBits::xmul` (the same primitive `dsp::lerp::fixed::lerp_unsigned` uses). Two `Bits<N>` operands give a `2N`-bit product, then `.resize::<A_W>().as_bits()` widens to the accumulator width. `A_W >= 2N` is documented; if smaller, single products overflow.
- Interface: `(a, b, enable, clear)` mirrors the CRC engine's pattern. `clear` overrides `enable`. The accumulator output is always present; consumers gate on their own message-end signal.
- **Unsigned only** for v1. Signed MAC is one of the most-requested DSP primitives and will need either `xmul` on signed `DynBits` (already available — see `lerp_signed`) or a dedicated `SignedMacUnit<N, A_W>`. Recorded as follow-up.

**Surprises and gotchas:** None — `DynBits::xmul` was a well-blazed trail thanks to `lerp`. The `dyn_bits()` → `xmul()` → `resize::<A_W>().as_bits()` idiom is worth lifting into a documented kernel pattern.

**Validation:** All five tiers, 9 tests including a 7-pair stream test against the software reference (`Σ a_i * b_i`) and a max-product test (`0xFF × 0xFF` = `0xFE01`, fits in 24-bit accumulator). `iverilog` RTL+NTL clean.

**Follow-ups:**
- **Signed MAC unit** (`SignedMacUnit<N, A_W>`). Composes `SignedBits::xmul` and uses signed accumulator addition.
- **Pipelined multi-cycle variant** for high-throughput at large `N` once `auto-pipelining-plan.md` ships.

---

## 2026-04-28 — Integer divider (unsigned, shift-subtract)

**Path:** `crates/rhdl-fpga/src/core/divider.rs` (+ example, doc, vcd)

**Why this, why now:** Roadmap row #14 — the Rust `/` and `%` operators do not synthesize in `#[kernel]`, so any project that needs runtime division (baud-rate generation, fixed-point scaling, address calculation) must instantiate this widget. First multi-cycle widget in this batch — most prior widgets (popcount, leading-zero count, barrel shifter, strict arbiter) were single-cycle combinational.

**Design decisions:**
- Multi-cycle restoring shift-subtract algorithm. Computes `N`-bit ÷ `N`-bit in `N` cycles after `start`. The classic textbook approach; minimum hardware footprint at the cost of latency.
- **No `N+1`-bit arithmetic.** The standard formulation needs an extra carry bit on the partial remainder. I avoid it by capturing the would-be carry (`rem`'s old MSB before the left shift) into a separate `rem_msb` signal, computing the comparison `(carry || new_rem) >= divisor` as `carry==1 || new_rem >= divisor`, and exploiting `N`-bit wrapping subtraction — `(2^N + new_rem_low) - divisor mod 2^N == new_rem_low - divisor` with wrap. The kernel uses only `Bits<N>` operations; there's no need to invent a wider intermediate type.
- Interface: `(dividend, divisor, start)` in, `(quotient, remainder, busy)` out. `start` is ignored while `busy`. Result held until next `start`. Considered a richer ready/valid handshake; rejected because `busy` is sufficient and keeps the example simple.
- Divide-by-zero is *not* trapped — the algorithm naturally produces `quotient = 2^N - 1`, `remainder = dividend`. Documented; callers should gate `start` on `divisor != 0` if they care.
- **Signed division deferred** — the unsigned core is the building block; signed version composes it with operand-sign-extraction and result-sign-correction. Recorded as roadmap follow-up.

**Surprises and gotchas:**
- The "carry-bit-without-N+1-bits" trick is well known to hardware designers but worth restating in the kernel comments because the algebra is non-obvious to a reader the first time. The CHANGELOG-as-narrative format is the right place to record *why* the design looks the way it does.
- This widget would benefit from `auto-pipelining-plan.md` once that lands — the per-cycle critical path is `compare → conditional-subtract → shift`, which on wide `N` will dominate clock period. Recorded as a future improvement.

**Validation:** All five tiers, 6 tests including a 56-pair grid sweep against the software reference (`u128 / u128`) and an explicit divide-by-zero test. `iverilog` RTL+NTL clean with default options.

**Follow-ups:**
- **Signed integer divider** (sign-detect, divide unsigned magnitudes, sign-correct quotient and remainder). Deferred from #14.
- **Pipelined variant** to meet higher clock frequencies once `auto-pipelining-plan.md` ships. Per-bit critical path is the limiting factor.

---

## 2026-04-28 — Barrel shifter

**Path:** `crates/rhdl-fpga/src/core/barrel_shifter.rs` (+ example, doc, vcd)

**Why this, why now:** Roadmap row #7 — variable-amount shifter / rotator. The built-in `Bits<N>::<<` and `>>` cover logical shifts but not arithmetic right shift (sign-extend) or rotates, which are the operations that actually need a named widget. Foundation for variable shifts in DSP, bit-field extraction, and the rotation half of round-robin/CRC routines.

**Design decisions:**
- Single unified kernel function with a `ShiftOp` enum that selects one of five modes (`LogicalLeft`, `LogicalRight`, `ArithmeticRight`, `RotateLeft`, `RotateRight`). Considered providing five separate kernel functions; the enum approach keeps callers from having to dispatch in their own code and makes the synthesizer free to share intermediate logic across modes.
- `amount` documented as `[0, N)`. Out-of-range amounts trip the kernel VM at simulation time and are undefined in synthesis. Did not add automatic mod-by-N — that would force a divider into the critical path; callers that need it can pre-reduce.
- ASR sign-extension implemented as `LSR | sign_extend_mask`, where the mask covers the top `amount` bits when the input MSB is 1. Considered using `SignedBits<N>` directly; rejected because the widget should work on `Bits<N>` and let the caller decide how to interpret the result.

**Surprises and gotchas:**
- **`if/else` in kernels lowers to a combinational mux — *both branches always evaluate*.** I initially wrote `if amount == 0 { data } else { (data << amount) | (data >> n_minus_amount) }` to handle the `amount == 0` rotate case, where `n_minus_amount = N` would otherwise trip the kernel VM's `shift < N` check. The unit tests (which call the kernel as a Rust function) passed, but `test_kernel_vm_and_verilog_synchronous` failed with "Shift amount 8_b4 must be less than 8". The fix: clamp the shift amount itself, not just the result. Compute `let safe_n_minus = if is_zero { bits(0) } else { n_minus_amount };` and use `safe_n_minus` everywhere a shift might otherwise be `N`. The output mux still picks the logically-correct value; the always-evaluated branch's shift now always uses a safe amount.
- **Lesson, generalized:** any time a kernel does `if guard { ... } else { expr_with_potentially_invalid_arg }`, you must ensure `expr_with_potentially_invalid_arg` is *valid for all inputs*, not just inputs that satisfy the guard. The if/else is just a mux on the result; both inputs flow through the hardware. This is an extension of the "Reset semantics belong at the end of the kernel" rule in CLAUDE.md §12.
- **Rust direct call vs kernel VM diverge on shift bounds.** Rust's `Bits<N> << k` is permissive (it wraps gracefully); the kernel VM is strict. So Tier-1 unit tests (Rust direct) can mask this class of bug — only the VM cross-validation catches it. Worth adding `test_kernel_vm_and_verilog_synchronous` to every combinational kernel that uses variable shifts.

**Validation:** All five tiers, 11 tests including a 280-input cross-validation sweep (7 data values × 8 amounts × 5 modes) through Verilog. Tier-1 unit tests cover identity at amount=0, swap-nibbles at amount=4 (rotate ROL/ROR symmetry), ROL∘ROR=identity round-trip, and exhaustive 8-bit × 8-amount sweeps for LSL and LSR against `u8::wrapping_shl/shr`.

**Follow-ups:**
- The if/else-evaluates-both-branches behavior should probably be called out explicitly in CLAUDE.md §4 ("The Subset of Rust That `#[kernel]` Accepts") so future agents don't have to rediscover it. Recorded as a documentation follow-up.

---

## 2026-04-28 — Leading-zero count

**Path:** `crates/rhdl-fpga/src/core/leading_zeros.rs` (+ example, doc, vcd)

**Why this, why now:** Roadmap row #9 — foundational primitive for fixed/floating-point normalization, dynamic-range estimation in DSP, and integer-to-float conversion. Companion to popcount (just shipped) and a thin variant of `priority_encoder_msb`.

**Design decisions:**
- Implemented inline rather than as a wrapper around `priority_encoder_msb`. Wrapping would have required a runtime `if input == 0 { N } else { N - 1 - msb }` post-step plus an `Option`-to-`Bits<W>` conversion in the kernel; doing it inline keeps the all-zeros special case cheap and the synthesized adder tree bounded.
- Pure `#[kernel]` function. Same parameterization shape as `popcount`: separate `N` (input width) and `W` (output width), user picks `W >= ceil(log2(N+1))`.

**Surprises and gotchas:** None — same loop pattern as priority encoder (MSB-first scan with `mut found` + `mut clz` accumulator). Validated exhaustively against `u8::leading_zeros()` (kernel) and `test_kernel_vm_and_verilog_synchronous` (Verilog), both 256-input sweeps.

**Validation:** All five tiers, 7 tests, Verilog cross-validation clean.

**Follow-ups:** None.

---

## 2026-04-28 — Population count (popcount)

**Path:** `crates/rhdl-fpga/src/core/popcount.rs` (+ example, doc, vcd)

**Why this, why now:** Roadmap row #8 — combinational primitive used by ECC syndrome weighting, hash-table sizing, normalization, and ML inference (binary neural net activation counts). Independent enough from other widgets to ship in any order, picked here because it's a one-screen kernel and good warmup for the longer combinational utilities (barrel shifter, leading-zero count) coming next.

**Design decisions:**
- Pure `#[kernel]` function — no struct, no Synchronous wrapper. Users that need it as a participating subcore can wrap with `Func` (see the example) or call it inline from another kernel.
- Two const generics: `N` (input width) and `W` (output width). The user picks `W >= ceil(log2(N+1))` so the maximum count is representable. Documented; not asserted (no compile-time arithmetic in stable const generics).
- Implemented as the unrolled "test-each-bit, conditional `+= 1`" loop. The synthesizer turns this into an adder tree. Rejected: pre-baked Wallace/Dadda trees — would have required hand-coded reduction tables and offered no advantage at the small input widths typical for RHDL kernels.

**Surprises and gotchas:**
- The `let one_w: Bits<W> = if bit_i != bits(0) { bits(1) } else { bits(0) };` cast pattern is the cleanest way to widen a 1-bit AND result to the accumulator width inside a kernel — `.resize::<W>()` and similar Rust-side methods are not (yet?) kernel-compatible for this kind of conditional widen-and-mux.
- Validated against `u8::count_ones()` exhaustively (256 inputs) at the kernel level *and* via `test_kernel_vm_and_verilog_synchronous` (256 inputs through Verilog).

**Validation:** All five tiers, 7 tests, Verilog cross-validation clean over the entire 8-bit input space.

**Follow-ups:** None.

---

## 2026-04-28 — Strict-priority arbiter

**Path:** `crates/rhdl-fpga/src/core/strict_priority_arbiter.rs` (+ example, doc, vcd)

**Why this, why now:** Roadmap row #13 — the trivial variant of the round-robin arbiter, useful as a fixed-priority interrupt controller, exception-ranking primitive, and a deliberately *unfair* baseline against which to test fair arbiters. Shipped immediately after `RoundRobinArbiter` so the two have matching I/O signatures and are drop-in swappable.

**Design decisions:**
- I/O signature (`Bits<N>` → `Option<Bits<W>>`) deliberately mirrors `RoundRobinArbiter` so the two are interchangeable.
- Implemented as an *empty-struct* Synchronous widget with no DFFs and no subcores. The kernel just calls `priority_encoder_lsb` and returns its result. This was a small experiment: I wanted to know whether an empty Synchronous struct would derive correctly. It does — `#[derive(Synchronous, SynchronousDQ)]` on a struct with zero fields produces `Q { } / D { }`, the kernel takes `_q: Q<N, W>` and returns `D::<N, W>::dont_care()`, and the framework synthesizes the expected zero-state Verilog. This is a useful pattern for any combinational widget that needs to participate as a Synchronous subcore in a higher-level composition.
- Kept rejected: making this a pure `#[kernel]` function (would have required users to add their own `Func` wrapper at every use site, breaking the swappability with `RoundRobinArbiter`).

**Surprises and gotchas:** Empty-struct Synchronous widgets work cleanly. Tier 4 (`iverilog` RTL+NTL) passes with the default test bench options — no `.skip(...)` workaround needed because there's no DFF, no non-zero reset value, and no async domain crossing.

**Validation:** All five tiers, 7 tests, `iverilog` RTL+NTL clean. Tier 2 includes a starvation test (constant `0b0101` for 16 cycles → bit 2 *never* gets a grant) that doubles as the load-bearing demonstration of why round-robin exists.

**Follow-ups:** None.

---

## 2026-04-28 — First eight widgets (`feat/widget-roadmap-batch-1`)

A single-day batch advancing through the recommended-first-eight list in `widget-roadmap.md` (rows 1–8 of "Recommended first eight"). The batch was a deliberate AI-assisted shakedown of the full CLAUDE.md contract: every widget got rustdoc with schematic + internals diagrams, runnable example, committed waveform, and Tier 1–5 tests. The lib test count grew from **149 → 224 passing** with **0 regressions**.

### Cross-cutting observations from the batch

These showed up in multiple widgets and shape what we'd do differently next time:

- **`q.<subcore>` semantics depend on whether the subcore has internal state.** For purely combinational subcores (e.g. `Constant`), `q.<field>` reflects same-cycle output; for pipelined subcores (e.g. `DFF`), `q.<field>` is one cycle behind `d.<field>`. The debouncer composition initially failed because the composer assumed `q.settle` (a `PulseStretcher` output, which sits behind a DFF) was same-cycle — see the debouncer entry below for the fix.
- **Async testbench cycle alignment is a real framework limitation.** Hand-written multi-domain widgets (`Sync1Bit`, `BitSyncChain`) cannot use the default `TestBench::rtl(...)` per-sample comparison — the codebase convention is `.skip(!0)`, which gets you elaboration coverage but not functional ground-truth. Recorded as a follow-up.
- **Verilog `initial begin` ≠ Rust `dont_care`.** When a DFF's reset value is non-zero, the Verilog `initial` block sets the reg at time 0 but the Rust simulator's initial state is `dont_care` (which prints as 0). They agree after the first clock edge. `core::crc` hits this and uses `.skip(2)`; `core::counter` doesn't because its reset value is 0. Recorded as a follow-up.
- **Const-generic loops in kernels.** `for i in 0..N` with const-bound `N` is unrolled at compile time; `i` is constant *per iteration*. `bits(i as u128)` works. `Bits<N> >> usize` does **not** compile — use `>> (i as u128)`. `bits::<N>(1) << index` (where `index: Bits<W>`) also works for shift-by-runtime.
- **For pure combinational kernels, `test_kernel_vm_and_verilog_synchronous`** is the right Tier 3+4 cover — it compiles to Verilog, runs both Rust VM and iverilog, and compares per input. Used by `core::priority_encoder` and `core::one_hot`.

### Per-widget notes

#### Edge detector — `crates/rhdl-fpga/src/core/edge_detector.rs`

**Why:** Roadmap #1 — the simplest possible RHDL kernel and the canonical reference for the AI-assisted-build workflow.

**Design:** Single `DFF<bool>` for `prev`, three combinational outputs (`rising`, `falling`, `any`) packed into an `Edges` struct. Reset zeroes `prev` and forces all three outputs low. Outputs use `dont_care()` + per-field assignment per the template.

**Surprises:** None — pattern lifted cleanly from `core::counter` and `fifo::write_logic`.

**Validation:** All five tiers pass. `iverilog` round-trip RTL+NTL clean. 9 tests.

#### Pulse stretcher — `crates/rhdl-fpga/src/core/pulse_stretcher.rs`

**Why:** Roadmap #2 — used by debouncer, watchdog, blink-on-event. Composes a counter with a held flag.

**Design:** Bit-width `N` parameterizes the counter; runtime `stretch` value supplied via a `Constant<Bits<N>>` subcore. The kernel reads `q.stretch` and re-arms the counter to `q.stretch` on every high input cycle, decrements otherwise. Output is `q.counter != 0`.

**Surprises:** First widget where I needed a runtime-configurable value inside the kernel. The pattern is to hold it in a `Constant<T>` subcore and read it as `q.<field>`. Same idiom shows up in `axi4lite::register::rom`.

**Validation:** All five tiers, 11 tests, `iverilog` round-trip clean.

#### N-stage synchronizer chain — `crates/rhdl-fpga/src/cdc/synchronizer_chain.rs`

**Why:** Roadmap #3 — generalizes the existing 2-stage `Sync1Bit` to depth `N`. Required by every CDC pattern.

**Design:** Hand-written `impl Circuit` (matching `Sync1Bit`'s style), since `#[kernel]` widgets can't currently express clock-domain-crossing primitives. State holds `[bool; N]` for next/current. HDL is generated programmatically with `quote!` repetition inside `parse_quote!{ ... }` — `#(#reg_decls)*` works for vlog token streams the same way it does for syn.

**Surprises:**
- I emitted the chain without an `initial begin` and got `iverilog` `Expected 0, got x` — non-blocking assignments on undeclared regs start as `X`. Adding `initial begin reg_i = 1'b0; end` for each stage fixed it. `Sync1Bit` doesn't have `initial`s but also uses `.skip(!0)` so it never sees the divergence.
- After fixing initial, hit a different `iverilog` mismatch (`Expected 1 got 0`) under the async testbench. Confirmed via `cross_counter` that this is a framework-level issue with per-event comparison vs `posedge`-driven Verilog updates. Followed prior-art convention of `.skip(!0)` and documented honestly.

**Validation:** Tier 1 N/A (widget is hand-written, no kernel to unit-test directly), Tier 2 (Rust glitch_check), Tier 3 (HDL snapshot for both N=2 and N=4), Tier 4 (`iverilog` elaboration via `.skip(!0)` — see follow-up), Tier 5 (VCD digest).

#### Priority encoder — `crates/rhdl-fpga/src/core/priority_encoder.rs`

**Why:** Roadmap #4. Foundation for arbiters, interrupt controllers, leading-zero count.

**Design:** Pure `#[kernel]` functions (`priority_encoder_lsb`, `priority_encoder_msb`), no struct. Constant-bounded loop, mut `idx` accumulator + mut `found` flag. Returns `Option<Bits<W>>` (per-CLAUDE.md kernels support Option natively).

**Surprises:**
- `Bits<N> >> usize` doesn't compile — only `>> u128`, `>> Bits<M>`, `>> DynBits` exist. Cast loop index: `input >> (i as u128)`.
- For `test_kernel_vm_and_verilog_synchronous` the `K` type-parameter must be the *fully concretized* function instance: `priority_encoder_lsb::<8, 3>` (not `priority_encoder_lsb`). The error message is misleading.

**Validation:** Tier 1 (10 unit tests including exhaustive 8-bit sweep against `u128::trailing_zeros`/`leading_zeros`), Tier 3+4 via `test_kernel_vm_and_verilog_synchronous` for both lsb and msb (256-input sweep), Tier 5 VCD via a `Func` wrapper.

#### One-hot ↔ binary — `crates/rhdl-fpga/src/core/one_hot.rs`

**Why:** Roadmap #5 — pair to priority encoder.

**Design:** Two `#[kernel]` functions. `binary_to_one_hot<W, N>` is a single shift `bits::<N>(1) << index`. `one_hot_to_binary<N, W>` unrolls the same loop pattern as the priority encoder but unconditionally OR-accumulates indices (so multi-hot input gives the OR of indices — documented as unspecified contract).

**Surprises:** None new. The `bits::<N>(1) << Bits<W>` shift just works thanks to the existing `Shl<Bits<M>>` impl on `Bits<N>`.

**Validation:** Tier 1 (8 tests including round-trip `one_hot_to_binary . binary_to_one_hot == id`), Tier 3+4 cross-validation against Verilog for both functions over their full input space, Tier 5 VCD.

#### Debouncer — `crates/rhdl-fpga/src/core/debouncer.rs`

**Why:** Roadmap #6 — first widget to *compose* multiple existing widgets (edge detector + pulse stretcher + DFF). The composition demo.

**Design:** Three subcores; kernel routes their inputs/outputs. The "stable" condition gates whether the input is latched into the output DFF.

**Surprises (and a real bug caught by Tier 2):**
- First draft used `let stable = !q.settle;` which let the very first transition leak through to the output. The bug: `q.settle` is the `PulseStretcher`'s output, which sits behind its internal DFF and so reflects the *previous* cycle's value. On the cycle the input transitions, `q.settle` is still false (the stretcher hasn't been armed yet), so the kernel decided the input was stable and latched the new value.
- Fix: `let stable = !q.settle && !q.edge.any;` — also gate on the edge detector's same-cycle output (`q.edge.any` is `EdgeDetector`'s combinational output, available same-cycle). The Tier 2 `test_short_glitch_rejected` test caught this immediately and is now load-bearing regression coverage. Comment in the kernel calls out *why* the `&& !q.edge.any` term is required.
- The takeaway is general: **when composing widgets, distinguish subcores whose `q.<field>` is same-cycle (combinational outputs of `Constant`, `EdgeDetector`-style logic) from those whose `q.<field>` is delayed (anything fronted by a DFF).** The kernel's mental model has to match.

**Validation:** All five tiers, 10 tests, `iverilog` RTL+NTL clean. Tier 3 uses an HDL-length proxy snapshot (5066 chars) rather than a full text snapshot — see follow-up.

#### Round-robin arbiter — `crates/rhdl-fpga/src/core/round_robin_arbiter.rs`

**Why:** Roadmap #7 — required by multi-master AXI, switch fabrics, DMA channels.

**Design:** Mask-and-rotate variant. Two-DFF state: `last_granted: Bits<W>` and `valid: bool`. The kernel walks all N positions in rotated order starting from `last_granted + 1`, picks the first set request bit. `Bits<W>` arithmetic wraps mod `2^W = N`, so the de-rotated index falls out for free *if* `N = 2^W`. That constraint is documented.

**Surprises:** None — the design works first try once you accept `N = 2^W` as a precondition. Non-power-of-2 `N` would need an explicit modulo, which is more work.

**Validation:** All five tiers, 10 tests including a fairness sweep (32 cycles, all four requesters constantly asking → grants exactly cycle in `0,1,2,3,0,1,2,3,...` order), `iverilog` clean.

#### CRC engine — `crates/rhdl-fpga/src/core/crc.rs`

**Why:** Roadmap #8 — unblocks UART, Ethernet, SPI flash. Last in the first-eight batch on purpose: the dependencies (no protocol PHYs need it yet) make it the rightmost leaf in the build order.

**Design:** Bit-serial, MSB-first shift-register CRC. Polynomial and init are runtime-configurable (each lives in a `Constant<Bits<W>>` subcore). Input struct carries `bit`, `enable`, and a `clear` strobe (which reloads init without needing a global reset).

**Reflection / xor-out are deliberately omitted** — these are message-boundary post-processing steps that vary by use site. A wrapper widget can add them when a specific protocol PHY needs them.

**Surprises:**
- `iverilog` Tier 4 hit the "non-zero DFF reset vs Verilog `initial` block" issue. The DFF resets to `0xFFFF` (CRC-16-CCITT init); Verilog's `initial begin o = 0xFFFF` runs at time 0; Rust sim's state starts as `dont_care` (prints as 0). They agree after the first clock edge. Used `.skip(2)` to bypass the pre-edge sample window. Recorded as follow-up.
- Validated against the well-known `123456789` → `0x29B1` for CRC-16-CCITT (no reflection variant), and against an in-house Rust reference for back-to-back messages via `clear`.

**Validation:** All five tiers, 9 tests, `iverilog` clean (with documented `.skip(2)`). Tier 3 uses HDL-length proxy.
