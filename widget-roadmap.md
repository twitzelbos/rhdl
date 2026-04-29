# RHDL Widget Roadmap

A prioritized list of widgets to add to `crates/rhdl-fpga`. Priorities follow one rule: **reusability ≈ depth in the dependency graph**. Build the deepest dependencies first; the higher tiers compose them almost for free.

This document is the strategic counterpart to `CLAUDE.md`. CLAUDE.md says *how* to build a widget; this document says *which* to build *next* and *why*.

---

## Already in the tree

`core::{constant, counter, delay, dff, option, slice}`, `core::ram::{synchronous, asynchronous, option_sync, option_async, pipe_sync}`, `cdc::{synchronizer, cross_counter}`, `gray::{encode, decode}`, `fifo::{synchronous, asynchronous}` (with internal `read_logic`/`write_logic` and a testing harness), `pipe::{map, filter, filter_map, chunked}`, `stream::{map, filter, filter_map, flatten, zip, tee, chunked, stream_buffer, fifo_to_stream, stream_to_fifo, pipe_wrapper, xfer}`, `axi4lite::{basic, channel, core, register, stream}`, `tristate::simple`, `dsp::lerp`, `rng::xorshift`, `reset::{conditioner, negating_conditioner, negation}`, `lid::carloni`.

## Confirmed missing (verified by grep)

UART, SPI, I2C, PWM, edge detector, debouncer, arbiter, integer divider, CRC engine, FIR/MAC, priority encoder, barrel shifter, rotator, full AXI4, Wishbone, Ethernet MAC, PLL/MMCM wrapper.

---

## Tier 0 — Foundation primitives

These are tiny — most are 30–100 LOC kernels — and they appear inside almost every higher-level widget. Build first. Each one unblocks several downstream widgets.

| # | Widget | Why it's foundational |
|---|---|---|
| 1 | ~~Edge detector~~ (rising / falling / any) — shipped: `crates/rhdl-fpga/src/core/edge_detector.rs` | Used by every protocol PHY, every trigger circuit, every debouncer. The simplest possible RHDL kernel; perfect first widget for the LLM-assisted workflow. |
| 2 | ~~Pulse stretcher / one-shot~~ (parameterized cycle count) — shipped: `crates/rhdl-fpga/src/core/pulse_stretcher.rs` | Used by debouncer, watchdog, timeout logic, blink-on-event. Composes a counter with a held flag. |
| 3 | ~~N-stage synchronizer chain~~ — shipped: `crates/rhdl-fpga/src/cdc/synchronizer_chain.rs` | Generalizes the existing single-bit `Sync1Bit` to depth `N`. Required by every CDC pattern. |
| 4 | ~~Multi-bit handshake bridge~~ (slow CDC for any `T: Digital`) — shipped: `crates/rhdl-fpga/src/cdc/slow_crosser.rs`. 4-phase req/ack with single-bit synchronizers; data is held stable in W and sampled by R. | Currently absent — the only multi-bit CDC is the gray-code `cross_counter` inside async FIFO. Required for config buses, status registers, control crossing slow domains. |
| 5 | ~~Priority encoder~~ (binary index of lowest/highest set bit) — shipped: `crates/rhdl-fpga/src/core/priority_encoder.rs` | Required by arbiters, interrupt controllers, instruction decoders, leading-zero count. |
| 6 | ~~Decoder / one-hot ↔ binary converters~~ — shipped: `crates/rhdl-fpga/src/core/one_hot.rs` | Required by register-file address decode, demuxes, every state-machine indicator. |

## Tier 1 — Combinational utilities

| # | Widget | Required by |
|---|---|---|
| 7 | ~~Barrel shifter~~ (parameterized over data width and shift-amount width) — shipped: `crates/rhdl-fpga/src/core/barrel_shifter.rs`. Five modes (LSL/LSR/ASR/ROL/ROR) selected by `ShiftOp` enum. | Variable shifts, rotators, bit-field extraction, DSP scaling. |
| 8 | ~~Population count (popcount)~~ — shipped: `crates/rhdl-fpga/src/core/popcount.rs` | ECC, hash-table sizing, normalization, ML inference. |
| 9 | ~~Leading-zero count~~ — shipped: `crates/rhdl-fpga/src/core/leading_zeros.rs` | Floating/fixed-point normalization, priority logic. |
| 10 | **Wide carry-chain comparator** | Wide-bus equality and magnitude (the built-in `Bits<N>` ops cover narrow cases). Lower priority than 7–9. |

## Tier 2 — Sequential building blocks

| # | Widget | Composes |
|---|---|---|
| 11 | ~~Debouncer~~ (parameterized sample period and signal type) — shipped: `crates/rhdl-fpga/src/core/debouncer.rs` | (1) edge detector + (2) pulse stretcher + counter. |
| 12 | ~~Round-robin arbiter~~ — shipped: `crates/rhdl-fpga/src/core/round_robin_arbiter.rs` | (5) priority encoder + a rotation register. Required by multi-master AXI, switch fabrics, DMA channels. |
| 13 | ~~Strict-priority arbiter~~ — shipped: `crates/rhdl-fpga/src/core/strict_priority_arbiter.rs` | Trivial variant of (12). |
| 14 | ~~Integer divider~~ (shift-subtract, **unsigned only**) — shipped: `crates/rhdl-fpga/src/core/divider.rs`. Signed-divide variant deferred. | The Rust `/` operator does not synthesize in `#[kernel]`; you must instantiate this for any divide. Required by baud-rate generation, fixed-point math. |
| 15 | ~~Multiply-accumulate (MAC) unit~~ — shipped: `crates/rhdl-fpga/src/core/mac.rs`. Single-cycle, unsigned, full-precision intermediate via `DynBits::xmul`. Signed variant deferred. | FIR/IIR filters, DSP pipelines, ML inference. |
| 16 | ~~CRC engine~~ (bit-serial; parameterizable polynomial, width, init) — shipped: `crates/rhdl-fpga/src/core/crc.rs`. Reflect / xor-out deferred — apply in software at message boundary, or add a wrapper widget. | UART, Ethernet MAC, SPI flash, USB, every packet validation. |
| 17 | **Generic memory-mapped register file** (decoupled from AXI4-Lite) | Every peripheral. The existing `axi4lite::register` is bus-coupled; this is a strict generalization that any bus adapter can wrap. |

## Tier 3 — First-class protocol PHYs

These are what users *want*. They become straightforward once Tiers 0–2 exist.

| # | Widget |
|---|---|
| 18 | **UART TX**, **UART RX**, full-duplex **UART** with FIFOs |
| 19 | **SPI master** (parameterized CPOL/CPHA/bit-order/word-width) |
| 20 | **SPI slave** |
| 21 | **I2C master** (exercises the existing `tristate` widget end-to-end via SDA) |
| 22 | **PWM generator** (parameterized resolution, duty-cycle width) |

## Tier 4 — Larger systems

| # | Widget | Notes |
|---|---|---|
| 23 | **Full AXI4** (read + write channels with bursting) | Required for DDR, PCIe, anything bandwidth-heavy. |
| 24 | **Wishbone classic + pipelined** | Alternative open bus. |
| 25 | **DMA engine** | Composes (12) round-robin arbiter, (17) register file, (23) AXI4. |
| 26 | **Ethernet MAC frontend** | The killer demo. Composes (16) CRC32, framing logic, CDC. |

---

## Recommended first eight (two-week scope)

In order. Each widget unblocks the next.

1. **Edge detector** — pick this first. ~20-line kernel. Perfect AI-workflow shakedown and the canonical reference implementation that other widget agents can pattern-match against.
2. **Pulse stretcher / one-shot** — same scale, complements (1).
3. **N-stage synchronizer chain** — generalizes `Sync1Bit`. Tiny.
4. **Priority encoder** — pure combinational, parameterized, foundation for (6) and (12).
5. **Decoder / one-hot ↔ binary** — pair with (4).
6. **Debouncer** — first widget that *composes* (1)+(2)+a counter. Demonstrates Tier-2 composition.
7. **Round-robin arbiter** — textbook design with rich test-coverage requirements; ideal LLM showcase.
8. **CRC engine** — last in this batch because it unblocks UART, Ethernet, SPI flash. Parameterizable CRC is ~150 LOC and has well-known reference values for testing.

After this batch, you have everything needed to write UART, SPI, debouncer-fronted GPIO, and a generic register-file peripheral with backpressure and arbitration. That set unlocks essentially every introductory FPGA tutorial — which is the leverage point for showcasing AI-assisted RHDL development.

## Why not start with UART

UART is what users *want*; what they *need* is the dependency stack underneath it. Starting with UART means building the edge detector, pulse stretcher, baud-rate divider, and shift register inline — none of them reusable, all entangled with UART-specific concerns. Two weeks later when someone wants SPI you rewrite the same pieces. Pay the depth-first cost up front; ship the protocols third.

## A concrete reusability dividend

If Tiers 0–2 are complete, the *combined* implementation cost of UART, SPI master, I2C master, and PWM is roughly the cost of UART alone done from scratch. That's the lever.

---

## Parallel work streams

The widget library is one of three independent tracks. The other two unblock language-level and compiler-level capabilities that reshape how widgets get written:

- **`auto-pipelining-plan.md`** — design plan for letting the RHDL compiler automatically insert pipeline registers to meet a target clock frequency. Phase 1 covers pure combinational kernels; Phase 2 stateful kernels with hazard analysis; Phase 3 loop pipelining with II analysis. Lives at NTL level, after the Stage-3 optimization passes. Once shipped, widgets with long combinational paths (integer divider, MAC unit, wide CRC, AXI4 burst logic) become substantially easier to express because the timing-closure work moves from the user's source into the compiler.

- **`kernel-language-extensions.md`** — design spec for expanding the subset of Rust accepted inside `#[kernel]`. Phase 1 (pattern desugarings: `let-else`, or-patterns, range patterns, match guards, `@` bindings, array destructuring, `for x in array`, compile-time `assert`) unblocks readable state machines for UART/SPI/I2C. Phase 2 (`Bits<N>` method library: `count_ones`, `leading_zeros`, `reverse_bits`, saturating arithmetic) is a direct dependency of several roadmap widgets — popcount for ECC, leading-zero count for floating-point normalization, reverse-bits for serial protocols and CRC reflect. Phase 3 (`?` on `Option`/`Result`) and Phase 4 (custom traits, const-generic arithmetic) follow.

The roadmap, the auto-pipelining plan, and the kernel-language extensions are best worked in parallel, not in sequence. A widget agent picking up an item from this list should consult both companion documents before deciding how to express the widget — many widgets that are hard today are easy with one or two language extensions in place, and noting that explicitly in a Tier-2 widget's design lets the work be re-prioritized cleanly.

---

## Updating this document

When a widget ships and is merged with full contract compliance (per `CLAUDE.md`), strike its row from the table by changing `**Edge detector**` to `~~Edge detector~~` and add a brief note pointing at the source path. New widgets discovered along the way are added at the appropriate tier; if you're not sure which tier, default to "one tier higher than the deepest existing dependency."

---

## Follow-ups (deferred from the first eight)

Recorded honestly per CLAUDE.md §15 — these widgets shipped with workarounds that should be tightened up when the underlying gaps are closed.

- **Async testbench cycle-alignment** — `cdc::synchronizer_chain::BitSyncChain` Tier-4 test uses `.skip(!0)`, which disables iverilog per-sample comparison. The same workaround already exists in `cdc::synchronizer::Sync1Bit`. Root cause: the asynchronous testbench framework (`rhdl-core/src/sim/testbench/asynchronous.rs`) compares DUT output against the Rust simulator at every event in the merged input stream, which doesn't align cycle-for-cycle with iverilog's `always @(posedge clock)` semantics for hand-written multi-domain widgets. Functional correctness is currently covered by Rust glitch_check + VCD digest. A fix would either (a) teach the testbench to sample only at clock edges of the destination domain, or (b) add a per-domain "settle" hook so Rust and iverilog agree on the observation moment. Affects all hand-written async widgets.

- **Verilog-snapshot length proxies** — `core::debouncer`, `core::round_robin_arbiter`, and `core::crc` `test_vlog_generation` tests check the *length* of the emitted Verilog rather than the full text. Length is a cheap regression canary but won't catch semantic-preserving changes. Replace with full `expect_test` snapshots once the codegen output stabilizes (the snapshots will be ~2-7 KB each and need re-blessing whenever any compiler pass changes).

- **Non-zero DFF reset value vs iverilog `initial`** — `core::crc::CrcEngine` Tier-4 test uses `.skip(2)` to bypass a one-cycle mismatch on the very first sample: the DFF resets to `0xFFFF`, which Verilog's `initial begin` block applies at time 0 but the Rust simulator only applies after the first rising edge (initial state is `dont_care`). After the first clock edge they agree. A clean fix would have the Rust simulator's `init()` for DFF use the configured reset value rather than `dont_care` — this is a one-line change to `core/dff.rs::Synchronous::init` but will affect every widget that uses a non-default DFF reset.

- **Pre-existing failures noted but not fixed** — `faulty_reducer::test_no_combinatorial_paths` was failing before any of the first-eight work; `cargo clippy --all -- -D warnings` produces ~34 errors in `rhdl-core` from a newer clippy version (mostly `collapsible_if`). Both are out of scope for the widget builds and are flagged here so they aren't lost.

- **vlog pretty-printer drops `;` after `wire [0:0] src_send;`** (and presumably other identifiers — exact trigger TBD). Discovered while building `cdc::slow_crosser`; worked around by renaming the wire to `send_in`. Reproducible: revert the rename and the generated Verilog from `slow_crosser::SlowCrosser::hdl()` drops the trailing semicolon, breaking iverilog. Investigation should look at `rhdl-vlog`'s parser/pretty-printer for any name-based special-casing.

- **`if/else`-evaluates-both-branches discoverability** — captured in CLAUDE.md §4 "Subtle semantics to internalize" so future agents don't have to rediscover it. Cross-validation via `test_kernel_vm_and_verilog_synchronous` is the test convention that exposes this class of bug; widgets that use variable shifts should always include such a test.
