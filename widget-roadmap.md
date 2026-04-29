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
| 1 | **Edge detector** (rising / falling / any) | Used by every protocol PHY, every trigger circuit, every debouncer. The simplest possible RHDL kernel; perfect first widget for the LLM-assisted workflow. |
| 2 | **Pulse stretcher / one-shot** (parameterized cycle count) | Used by debouncer, watchdog, timeout logic, blink-on-event. Composes a counter with a held flag. |
| 3 | **N-stage synchronizer chain** | Generalizes the existing single-bit `Sync1Bit` to depth `N`. Required by every CDC pattern. |
| 4 | **Multi-bit handshake bridge** (slow CDC for any `T: Digital`) | Currently absent — the only multi-bit CDC is the gray-code `cross_counter` inside async FIFO. Required for config buses, status registers, control crossing slow domains. |
| 5 | **Priority encoder** (binary index of lowest/highest set bit) | Required by arbiters, interrupt controllers, instruction decoders, leading-zero count. |
| 6 | **Decoder / one-hot ↔ binary converters** | Required by register-file address decode, demuxes, every state-machine indicator. |

## Tier 1 — Combinational utilities

| # | Widget | Required by |
|---|---|---|
| 7 | **Barrel shifter** (parameterized over data width and shift-amount width) | Variable shifts, rotators, bit-field extraction, DSP scaling. |
| 8 | **Population count (popcount)** | ECC, hash-table sizing, normalization, ML inference. |
| 9 | **Leading-zero count** | Floating/fixed-point normalization, priority logic. |
| 10 | **Wide carry-chain comparator** | Wide-bus equality and magnitude (the built-in `Bits<N>` ops cover narrow cases). Lower priority than 7–9. |

## Tier 2 — Sequential building blocks

| # | Widget | Composes |
|---|---|---|
| 11 | **Debouncer** (parameterized sample period and signal type) | (1) edge detector + (2) pulse stretcher + counter. |
| 12 | **Round-robin arbiter** | (5) priority encoder + a rotation register. Required by multi-master AXI, switch fabrics, DMA channels. |
| 13 | **Strict-priority arbiter** | Trivial variant of (12). |
| 14 | **Integer divider** (shift-subtract, parameterized widths, signed/unsigned) | The Rust `/` operator does not synthesize in `#[kernel]`; you must instantiate this for any divide. Required by baud-rate generation, fixed-point math. |
| 15 | **Multiply-accumulate (MAC) unit** | FIR/IIR filters, DSP pipelines, ML inference. |
| 16 | **CRC engine** (parameterizable polynomial, width, init, reflect, xor-out) | UART, Ethernet MAC, SPI flash, USB, every packet validation. |
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

## Updating this document

When a widget ships and is merged with full contract compliance (per `CLAUDE.md`), strike its row from the table by changing `**Edge detector**` to `~~Edge detector~~` and add a brief note pointing at the source path. New widgets discovered along the way are added at the appropriate tier; if you're not sure which tier, default to "one tier higher than the deepest existing dependency."
