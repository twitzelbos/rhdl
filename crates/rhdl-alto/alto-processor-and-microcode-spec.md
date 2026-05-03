# Xerox Alto — Processor and Microcode Specification

*Captured 2026-05-02 by extracting from the canonical sources committed under `assets/bitsavers/`. Audience: engineers extending the `rhdl-alto` core or reading the original microcode. Not a tutorial — a reference. Organized around what an implementer needs to know.*

> **Source primacy.** This document is a digest. **The PDFs in `assets/bitsavers/` are the canonical authority.** When this digest disagrees with them, the PDFs win without exception — including any place where the implementation in `src/` happens to match the digest but contradicts the manual. Sections marked "✓ verified against AltoHW (Aug76) §X" have been read and reconciled against the PDF; sections without that tag are provisional until a verification pass reaches them.
>
> **Verification status (2026-05-02).** §1 (system architecture), §2 (microinstruction format), §3 (per-field detail: ALU/BS/F1/F2), §5 (16-task system), §6 (Emulator + interrupts + augmented instructions), §7 (constants ROM), §8 (disk subsystem incl. canonical task-spec codes), §9 (display subsystem), §10 (other peripherals), §11 (control RAM + SWMODE), §12 (microcode source format) are all verified against the Alto Hardware Manual (Aug 1976). §4 is partially verified (memory interface §4.4 verified; M/S registers §4.3 cross-referenced against AltoHW §8.7). Future verification passes should reconcile against `AltoHWRef.part1.pdf` / `.part2.pdf` (Alto II reference, ~1979) and `AltoSubsystems_Oct79.pdf` for any Alto II-specific updates that post-date Aug 1976.
>
> The originals are:
>
> - **`Alto Hardware Manual (Aug 1976)`** — `assets/bitsavers/Alto_Hardware_Manual_Aug76.pdf` — the canonical machine description. Sections referenced as "AltoHW §N".
> - **`Alto Hardware Reference (parts 1 & 2)`** — `AltoHWRef.part1.pdf` / `.part2.pdf` — Alto II hardware reference, 1979 timeframe.
> - **`Alto Subsystems (Oct 1979)`** — `AltoSubsystems_Oct79.pdf` — the disk, display, Ethernet, mouse, keyboard subsystems in detail.
> - **`AltoIICode3.mu.pdf`** + companion text `altoIIcode3.mu.txt` — the actual ROM microcode source (the Alto OS Release 19 vintage). Inline assembly defines the microinstruction encoding by example. ~2230 lines of microcode.
> - **`AltoCode24.mu.txt`** — earlier microcode revision (~2030 lines).
> - **`AltoConsts23.mu.pdf`** + companion text `altoconsts23.mu.txt` — the constants-ROM definitions and the canonical symbol table for bus sources, ALU functions, F1/F2 codes, magic memory addresses. Every microcode source `#include`s this file.
> - **`Alto_II_firmware/AltoPROMs_20070612/`** — the actual hardware PROMs (DISPL, MADR, 2KCTL, XM51 etc) — the silicon-side address-decoding and dispatch tables.
> - **ContrAlto** (`assets/contralto/Contralto2/`) — the Living Computers Museum's cycle-accurate emulator, used as the gold reference for our lockstep validation.
>
> Every field, code, and address in this document traces back to one of these. Where the value is encoded in a Rust enum or constant in `src/`, the spec cites the file and line.

---

## 1 — System architecture at a glance ✓ verified against AltoHW (Aug76) §1.0, §2.0

The Alto (1973) is a 16-bit microcoded computer. Everything — CPU instruction emulation, display refresh, disk I/O, Ethernet packet handling, mouse polling, keyboard scanning — runs on **one shared microengine**. The microengine executes a 32-bit horizontal microinstruction every **170 ns** (AltoHW §2.0: "the entire system is synchronous, with a clock interval of 170nsec"). Sixteen *hardware tasks* compete for cycles via priority arbitration; on each cycle the microengine fetches the highest-priority woken task's next microinstruction and executes it. Tasks that have no work pending stay dormant until external hardware wakes them.

```
                   ┌──────────────────────────────────────┐
                   │         16 HARDWARE TASKS            │
                   │  (each has its own MPC; one fires    │
                   │   per cycle by priority arbitration) │
                   └──────────────────┬───────────────────┘
                                      │ wakeup mux
                                      ▼
   ┌──────────────────────────────────────────────────────────┐
   │           UNIVERSAL MICROENGINE (170 ns cycle)           │
   │                                                          │
   │   MIF: fetch microinstruction[MPC] from microcode RAM    │
   │   MIE: execute (ALU, R-file, BUS, T, L, MD, MAR, NEXT)   │
   │                                                          │
   │   • R-register file: 32×16 (some tasks see banked S regs)│
   │   • T register, L register, M register                   │
   │   • Memory: MAR-addressed; MD = read/write                │
   │   • Bus: 16-bit shared signal driven by BS field         │
   │   • ALU: 16 functions on (BUS, T)                        │
   └──────────────────────────────────────────────────────────┘
```

Cycle time on real hardware: **170 ns** (≈5.88 MHz microcycle). One macroinstruction (Nova-style) typically takes 4–8 microcycles in the Emulator task; complete display-refresh tasks fire every horizontal-blanking interval; disk-word DMA fires once per 38.4 µs at the Diablo data rate.

**Word width.** All data paths are 16-bit. Memory addresses are 16-bit unsigned (64K 16-bit words = 128 KB address space). Per AltoHW §1.0: "**64K 16 bit words of 850ns semiconductor memory**." Addresses 177000B–177777B are reserved for I/O (AltoHW §3.1, Appendix B).

**Microcode memory.** Per AltoHW §2.4: "The microinstruction memory produces an instruction and the address of its successor NEXT[0-9]. … The amount of memory available for microinstructions is often extended by an additional 1K of control memory implemented with RAM. Because the MPC RAM produces 12 bits, enough are available (11) to address both the microinstruction ROM and RAM." So:
- **Microcode ROM:** 1K × 32 bits (always present)
- **Microcode RAM:** 1K × 32 bits (Alto II standard, optional on Alto I)
- **MPC width:** 12 bits in the MPC RAM (16 entries, one per task), of which 11 are used to address ROM+RAM (2K combined). NEXT field in the microinstruction is 10 bits, but combined with the 11th "RAM/ROM select" bit derived from the MPC the microengine addresses the full 2K.

**Standard system inventory.** Per AltoHW §1.0:
- 875-line television monitor with 606 × 808 displayable points, refreshed at **60 fields (30 frames) per second** (interlaced)
- Undecoded keyboard
- 3-button optical mouse + 5-finger keyset
- **Diablo Model 31 *or* Model 44** disk drive (the manual covers both)
- 3 Mbps Ethernet transceiver
- Optional: Diablo HyType printer
- 64K of 850 ns DRAM main memory
- 1K microinstruction RAM extending the 1K ROM

**Why this matters.** Every other contender for "first personal computer" had separate hardware for display, disk, network. The Alto folded all of them into the microengine — which is what makes it the canonical demonstration of microcoded heterogeneous compute. (See `tier-c-flagship-cores.md` §5 for the strategic framing.)

---

## 2 — The microinstruction ✓ verified against AltoHW (Aug76) §2.1

### 2.1 Bit layout (32-bit horizontal)

**Bit numbering convention (AltoHW §, Conventions and Notation):** "Bits in registers are numbered from the most significant bit (0) toward the least significant bit." So **bit 0 is the MSB**, bit 31 is the LSB. The fields below use Alto's own bit numbering:

```
 0    4 5    8 9  11 12  15 16  19 20 21 22                     31
┌──────┬──────┬─────┬─────┬─────┬──┬──┬───────────────────────────┐
│ RSEL │ ALUF │ BS  │ F1  │ F2  │L │T │           NEXT            │
│  5b  │  4b  │ 3b  │ 4b  │ 4b  │L │T │          10 bits          │
└──────┴──────┴─────┴─────┴─────┴──┴──┴───────────────────────────┘
  MSB                                                          LSB
```

The canonical field table from AltoHW §2.1:

| Bits   | Name    | Meaning |
|--------|---------|---------|
| 0–4    | RSELECT | R-register select — 5-bit index into the 32-entry R-register file (also forms the high 5 bits of the 8-bit constant-ROM address when constants are gated) |
| 5–8    | ALUF    | ALU Function — one of 16 functions (4-bit field) |
| 9–11   | BS      | Bus Data Source — selects which of 8 sources drives the processor bus |
| 12–15  | F1      | Function 1 — first auxiliary control |
| 16–19  | F2      | Function 2 — second auxiliary control |
| 20     | (Load L)| If 1: L ← ALU result at end of cycle |
| 21     | (Load T)| If 1: T ← BUS (or ALU output for ALUFs marked *) at end of cycle |
| 22–31  | NEXT    | Next microinstruction address (10 bits, subject to F1/F2 modifiers) |

**Note on bit numbering.** Alto's convention (MSB = bit 0) is the opposite of typical modern Verilog/Rust convention (LSB = bit 0). The implementation in `src/isa.rs::Microinstruction::pack` packs into a u32; the bit-field positions in that function are expressed in the modern LSB-first convention but encode the same field identities. When citing bit positions, this document uses Alto's convention exclusively to match the Hardware Manual and the .mu source files.

**RAM/ROM select bit.** Per AltoHW §2.4, the MPC RAM is 12 bits per task, but only 10 bits are addressable from NEXT. The 11th address bit (selecting ROM vs RAM) is held in the MPC RAM and updated by the SWMODE F1 function (see §3.3) on the Emulator task.

### 2.2 Decoded form (the implementation's preferred shape)

The packed 32-bit form is what the microcode RAM stores. Inside RHDL kernels we use the `Digital`-derived struct in `src/isa.rs:213-235`:

```rust
pub struct Microinstruction {
    pub rsel:   Bits<5>,        // R-register select
    pub aluf:   AluFunction,    // 16-variant enum (4-bit field)
    pub bs:     BusSource,      // 8-variant enum (3-bit field)
    pub f1:     F1Function,     // 16-variant enum (4-bit field)
    pub f2:     F2Function,     // 16-variant enum (4-bit field)
    pub t_load: bool,           // T ← BUS
    pub l_load: bool,           // L ← ALU
    pub next:   Bits<10>,       // next microinstruction address
}
```

`Microinstruction::pack` and `unpack` round-trip to the 32-bit binary form for loading microcode binaries from `.mb` files, while kernel code reads the typed struct.

### 2.3 Cycle structure (MIF / MIE)

The microengine is a 2-stage pipeline:

```
 Stage MIF (Microinstruction Fetch):
   inst ← microcode_RAM[MPC_of_winning_task]

 Stage MIE (Microinstruction Execute):
   bus      ← drive_bus(inst.bs, inst.rsel)
   alu_out  ← alu(inst.aluf, bus, T)
   if inst.t_load:    T ← bus               (at edge)
   if inst.l_load:    L ← alu_out           (at edge)
   if inst.bs == LOAD_R or various F1/F2 codes:
       R[inst.rsel] ← bus                   (at edge)
   memory / MAR / MD ← per F2 function      (at edge)
   f2_modifier_pending ← inst.next_modifier_bits  (at edge — see ↓)
   MPC_of_winning_task ← inst.next | f2_modifier_pending_FROM_PRIOR_CYCLE (at edge)
```

Every register write happens at the cycle edge. Reads see the previous cycle's values. A canonical microcode line like `T_ MD, EVENFIELD` translates to: `T ← MD` (bus comes from memory data, T_LOAD set, ALUF = 1 / pass-T which writes the bus value to T), and the `EVENFIELD` is an F2 modifier that sets a bit of NEXT. Multiple side effects per cycle is the whole point of horizontal microcode.

**F2 NEXT-modifier pipeline timing — DELAYED by one cycle.** This is the most important subtlety in the cycle structure and the one most likely to bite implementations. AltoHW §2.4 is loosely worded:

> "The microinstruction memory produces an instruction and the address of its successor NEXT[0-n]. This successor address may be modified by merging bits into it under control of the function fields of the **current microinstruction**."

"Current microinstruction" can be read as either (a) the one currently in MIR (= immediate apply: F2 in cycle K modifies cycle K's NEXT) or (b) the one that just completed (= delayed by one cycle: F2 in cycle K modifies cycle K+1's NEXT).  AltoHW does NOT disambiguate.  The May79 manual revision adds no new timing language either.

**Empirically, the hardware is DELAYED.** Two independent lines of evidence:

1. **Standard microcode behavior.**  The Emulator boot wait-loop iterates through MPC `0x130 → 0x14e → 0x150 → 0x151 → 0x152 → 0x153 → 0x154 → 0x131 → 0x14e → ... → 0x132 → ...` — each pass increments R[5]/R[6] via L-load chain, and the F2=BusToNext at MPC=0x153 causes the loop's exit MPC to advance by one each iteration.  This pattern only works if the modifier from cycle K (BusToNext at 0x153) is applied to cycle K+1's NEXT field (the unmodified `next=0x130` of MPC=0x154 → 0x130|1 = **0x131**).  Under immediate-apply semantics the loop would exit on its first non-zero iteration to MPC=0x155 instead, never reaching `0x131, 0x132, 0x133, ...`.  Captured live in the boot trace at CTR cycles 40-42:

   ```
   cycle 40: mpc=0x154 IR=0x0001 R5=0 R6=1   ← cycle running 0x153 (F2=BusToNext, BUS=←DISP=1)
   cycle 41: mpc=0x131 IR=0x0001 R5=0 R6=1   ← cycle running 0x154 (next=0x130, OR'd with deferred 1)
   cycle 42: mpc=0x14e IR=0x0001 R5=0 R6=2   ← cycle running 0x131 (= 0x130 | 1)
   ```

   Cycle 41's MPC is **0x131**, not 0x130 (no modifier) and not 0x155 (immediate).  The F2 modifier from cycle 40's microinstruction (= BUS bit 0 = 1) applied to cycle 41's NEXT field, exactly as the delayed model predicts.

2. **ContrAlto cross-check.**  ContrAlto's `Tasks/Task.cs` implements delayed semantics explicitly:

   ```csharp
   // Task.cs:173 — at start of each cycle:
   nextModifier = _nextModifier;
   _nextModifier = 0;
   // ... F1/F2 handling sets _nextModifier |= ... (cumulative for THIS cycle's modifier) ...
   // Task.cs:549 — at end of each cycle:
   _mpc = (ushort)(instruction.NEXT | nextModifier);   // <-- uses last cycle's modifier
   ```

   ContrAlto and the standard microcode are consistent.  Spec digest (this document) and ContrAlto **agree** that F2 NEXT-modifier timing is delayed by exactly one cycle.

**RHDL implementation status.**  `src/microengine.rs` implements the delayed pipeline via the `next_modifier_pending: dff::DFF<Bits<10>>` field — at end of cycle K the kernel latches `next_modifier_this_cycle` into the DFF; at cycle K+1's start, the DFF's value is OR'd into K+1's NEXT field.  Anchored by the regression test `tests/f2_next_modifier_pipeline.rs` (chip-level, runs the minimal scenario from the boot trace).

This subsection is the **normative disambiguation** of AltoHW §2.4's loose prose.  Future contributors writing kernels that emit F2 NEXT modifiers (BusToNext, BusEqZero, ShiftLessThanZero, ShiftEqZero, AluCarryToNext, IDispatch, ACSOURCE, BUSODD, IR←, plus the per-task disk codes INIT/RWC/RECNO/XFRDAT/SWRNRDY/NFER/STROBON) MUST assume delayed semantics; tests that observe `next_mpc` for these opcodes MUST anchor against this rule.

**Test-writing note.**  The standalone `Microengine` simulator's combinational-settle loop (rhdl-macro Synchronous derive, `for _ in 0..MAX_ITERS`) makes per-cycle DFF observations *unreliable* for tests that read `q.X` after writing `d.X` within the same cycle — the settle loop converges to a fixed-point where `q.X` reflects the just-latched `d.X`, masking the cycle boundary.  This is **not** how real hardware behaves.  Multi-cycle delayed-pipeline behavior MUST be tested at the **chip level** (where DFF outputs propagate through the full clock cycle and settle cleanly), not at the standalone-microengine level.  See the regression test in `tests/f2_next_modifier_pipeline.rs` for the canonical pattern.

---

## 3 — Per-field detail

### 3.1 ALU functions (ALUF, 4 bits, 16 codes) ✓ verified against AltoHW (Aug76) §2.1, ALU Functions table

The ALU is a SN74181-type, restricted to 16 of its 48 possible operations. Per AltoHW §2.1: "The ALU output feeds the L and MAR registers. T may also be loaded from the ALU output under certain conditions" — specifically, when T_LOAD is set on an instruction with one of the **starred** ALUFs, T receives the ALU output instead of the bus. This is critical for accumulator-style patterns.

| ALUF (octal) | Mnemonic       | Result          | T←ALU* |
|--------------|----------------|-----------------|--------|
| 0            | `BUS`          | A (BUS)         |    |
| 1            | `T`            | B (T)           |    |
| 2            | `BUS OR T*`    | A + B           | * — T loads from ALU output if T_LOAD set |
| 3            | `BUS AND T`    | A · B           |    |
| 4            | `BUS XOR T`    | A XOR B         |    |
| 5            | `BUS + 1*`     | A PLUS 1        | * |
| 6            | `BUS − 1*`     | A MINUS 1       | * |
| 7            | `BUS + T`      | A PLUS B        |    |
| 10B (8)      | `BUS − T`      | A MINUS B       |    |
| 11B (9)      | `BUS − T − 1`  | A MINUS B MINUS 1 |  |
| 12B (10)     | `BUS + T + 1*` | A PLUS B PLUS 1 | * |
| 13B (11)     | `BUS + SKIP`   | A PLUS 1 (when SKIP asserted) / A otherwise |  |
| 14B (12)     | `BUS·T (AND)*` | A · B           | * — bitwise AND, with the * meaning T also loads from ALU output |
| 15B (13)     | `BUS AND NOT T`| A · ¬B          |    |
| 16B–17B      | `UNDEFINED`    | — (avoid)       |    |

The asterisk encoding from AltoHW §2.1 footnote: "If T is loaded during an instruction which specifies this function, it will be loaded from the ALU output rather than from the bus." This is how patterns like `T← BUS OR T` work: the bus drives BUS, ALU computes BUS|T, T_LOAD is set, and T receives the OR result (not the BUS itself).

The ALU's S3/S2/S1/S0/M/C inputs to the SN74181 are documented in the same table; see AltoHW §2.1 for the exact PROM mapping if implementing the ALU at the gate level.

**ALU carry-out** is captured by an F2 modifier (F2=5 `ALUCY`); the carry used is "that produced by the ALU function which last loaded the L register" (AltoHW §2.1 footnote on F2 BUS). So the carry is sticky-from-last-L-load, not from the current cycle.

### 3.2 Bus sources (BS, 3 bits, 8 codes) ✓ verified against AltoHW (Aug76) §2.1, Bus Sources table

| Value | Mnemonic   | Source                                                            | Task-specific? |
|-------|------------|-------------------------------------------------------------------|----------------|
| 0     | `←RName`   | Read R[rsel]                                                      | universal |
| 1     | `RName←`   | Load R\*: not logically a source, but R is gated to the bus during R-load; `R←` forces BUS to 0 so `T← ALUFunction(0,T)` may be executed simultaneously | universal |
| 2     |  (—)       | Nothing — bus reads as `−1` (all-ones, no source asserting)        | universal |
| 3     | `←KSTAT`   | Disk control status bits (canonical for Disk Sector / Disk Word tasks) | task-spec\*\* |
| 4     | `←KDATA`   | 16 bits of disk data (canonical for Disk Sector / Disk Word tasks)     | task-spec\*\* |
| 5     | `←MD`      | Memory data (read result of previous MAR access)                  | universal |
| 6     | `←MOUSE`   | Mouse data (4 bits of mouse state, remainder of word is 1)        | universal |
| 7     | `←DISP`    | Low-order 8 bits of IR, **sign extended**                         | universal (in Emulator); task-specific elsewhere |

*\*Per AltoHW §2.1: "R is gated to the bus during both reading and writing, ... Load R forces the BUS to 0, so that `T← ALUFunction(0,T)` may be executed simultaneously."*

*\*\*Per AltoHW §2.1: "By convention, these bus sources are task specific, i.e., their meaning depends on the currently active task. `←KSTAT` and `←KDATA` are the interpretations used during the disk sector and word tasks."*

**Wired-AND constants masking (AltoHW §2.2).** When two sources are gated onto the bus simultaneously, "the processor bus ANDs if more than one source is gated to it". The constants ROM gates to the bus when **F1=7 OR F2=7 OR BS≥4**, and "the intent in enabling constants with BS≥4 is to provide a masking facility, particularly for the `←MOUSE` and `←DISP` bus source. ... Up to 32 such mask constants can be provided for each of the 4 bus sources ≥4." So a microinstruction with BS=6 (`←MOUSE`) and a constants-ROM-resident mask in the RSELECT-indexed slot reads the AND of the mouse data and the mask, in one cycle.

In RHDL: `src/isa.rs::BusSource:81-104` enumerates the eight BS codes; the constants-ROM masking is on the Phase 4 follow-up list.

### 3.3 F1 functions (4 bits, 16 codes) ✓ verified against AltoHW (Aug76) §2.1 (universal F1) and §2.4 (TASK/BLOCK semantics)

Per AltoHW §2.1: "The first eight conditions specified by each field are interpreted identically by all tasks (except BLOCK), but the interpretation of the second eight depends on the active task." So **F1 codes 0–7 are universal**; **F1 codes 8–15 are per-task**.

**Universal F1 codes (0–7), from AltoHW §2.1:**

| F1 | Mnemonic     | Effect |
|----|--------------|--------|
| 0  | (—)          | No Activity |
| 1  | `MAR←`       | Load MAR from ALU output; start main memory reference (see §2.3) |
| 2  | `TASK`       | Switch tasks if higher priority wakeup is pending |
| 3  | `BLOCK`      | Disable current task until re-enabled by hardware-generated condition. *Hardware convention: not actually performed by the microprocessor, but by the individual device interfaces (AltoHW §2.4).* |
| 4  | `←L LSH 1`   | Left shift L one place\* |
| 5  | `←L RSH 1`   | Right shift L one place\* |
| 6  | `←L LCY 8`   | Cycle L (rotate left 8 places)\* |
| 7  | `←CONSTANT`  | Put on the bus the constant from the ROM location addressed by `{RSELECT, BS}` (8-bit concatenation) |

*\*Codes 4–6 are modified by the DNS (Do Nova Shift) function and the MAGIC function — see §3.4 F2 task-specific codes for the Emulator.*

**Task-specific F1 codes (8–15)** vary entirely by which task is running. The canonical assignments (from `altoconsts23.mu.txt:35-42` and `src/isa.rs::F1Function:110-154`, cross-referenced with AltoHW §3 Emulator and §6 Disk):

| F1 | Mnemonic        | Task         | Effect |
|----|-----------------|--------------|--------|
| 10 | `SWMODE`        | Emulator     | Switch microcode mode (ROM↔RAM); takes effect at next NEXT-mediated dispatch |
| 11 | `WRTRAM`        | RAM-related  | Write microcode RAM |
| 12 | `RDRAM`         | RAM-related  | Read microcode RAM |
| 13 | `RMR` / `SRB`   | Emulator / others | Reset Mode Reg (Emulator) / Set Register Bank (others) |
| 14 | (varies)        | per-task     | E.g., Disk Sector: `KCWA←` (KCWA ← BUS, DMA dest address) |
| 15 | (varies)        | per-task     | E.g., Disk Sector: `KCOM←` (KCOM ← BUS, command word) |
| 16 | (varies)        | per-task     | E.g., Disk Sector: `KADR←` (cyl/head/sector); Emulator: `RSNF` (read host number) |
| 17 | (varies)        | per-task     | E.g., Disk Sector: `KDATA←`; Emulator: `STARTF` (start I/O) |

*Octal codes 10–17 = decimal 8–15 (4-bit field).*

The Emulator-task F1=10 (`SWMODE`) switches between ROM and RAM microcode, which is how user-loadable microcode worked. The Disk Sector task's F1 codes 14–17 write the disk-controller hardware registers. Per-task F1 dispatch is on the Phase 5 polish list — current RHDL implementation uses universal dispatch.

### 3.4 F2 functions (4 bits, 16 codes) ✓ verified against AltoHW (Aug76) §2.1 (universal F2)

Same 8/8 split as F1: **F2 codes 0–7 are universal**; **F2 codes 8–15 are per-task**.

**Universal F2 codes (0–7), from AltoHW §2.1:**

| F2 | Mnemonic     | Effect |
|----|--------------|--------|
| 0  | (—)          | No Activity |
| 1  | `BUS=0`      | NEXT ← NEXT or (1 if BUS == 0 else 0) — sets NEXT[low bit] when bus is zero |
| 2  | `SH<0`       | NEXT ← NEXT or (1 if SHIFTER OUTPUT < 0 else 0) — sets bit on shifted-L sign |
| 3  | `SH=0`       | NEXT ← NEXT or (1 if SHIFTER OUTPUT == 0 else 0) |
| 4  | `BUS`        | NEXT ← NEXT or BUS(6,15) — OR low 10 bits of BUS into NEXT (computed branch) |
| 5  | `ALUCY`      | NEXT ← NEXT or LastALUC0\* — OR last-loaded-L's ALU carry-out into NEXT[low bit] |
| 6  | `MD←`        | Deliver BUS data to memory (memory[MAR] ← BUS, see §2.3) |
| 7  | `←CONSTANT`  | Same as F1=7 (gates constants ROM onto bus) |

*\*Per AltoHW §2.1 footnote: "The carry used is that produced by the ALU function which last loaded the L register."* So `ALUCY` reflects sticky-from-last-L-load carry, not current cycle.

**Task-specific F2 codes (8–15)** depend on the running task. Canonical assignments (from `altoconsts23.mu.txt:44-50`, `src/isa.rs::F2Function:160-208`, and AltoHW §3 Emulator + §6 Disk):

| F2  | Mnemonic                   | Task              | Effect |
|-----|----------------------------|-------------------|--------|
| 10  | `BUSODD` / `EVENFIELD` / `DDR` | Emulator / DVT,DHT / DWT | Emulator: NEXT or= BUS[15] (parity/odd-bit test); DVT,DHT: even-field flag; DWT: display data register write |
| 11  | `MAGIC` / `SETMODE`        | Emulator / DHT    | Emulator: shift-magic (alternate L-shift bit fed in from MAGIC source); DHT: set display half/full-resolution mode |
| 12  | `DNS`                      | Emulator          | Do Nova Shift — applies the Nova carry-and-shift discipline to the result |
| 13  | `ACDEST`                   | Emulator          | RSEL ← IR(DestAC), routing the destination accumulator address into RSEL |
| 14  | `←MD` (load) / `IR←`       | (depends)         | Some tasks: IR ← MD (Emulator: latch fetched instruction into IR) |
| 15  | `IDISP`                    | Emulator          | NEXT or= 8-way dispatch on IR opcode bits — routes to per-opcode handler |
| 16  | `←ACSOURCE`                | Emulator          | RSEL ← IR(SrcAC), routing the source accumulator address into RSEL |
| 17  | (per-task)                 | per-task          | E.g., Disk Sector/Word: `KFER` / `STROBE` for disk transfer control |

NEXT-modify F2 codes are how branches happen in microcode. Real Alto microcode is full of patterns like `L_ SLC -1, BUS=0` followed by `:DHT0` — this means "compute L = SLC - 1, modify NEXT bit 0 if BUS == 0 (here BUS = SLC), and the next-target label resolves the conditional branch."

The `IDISP` F2 (code 15, Emulator only) is the centerpiece of the Nova-emulator inner loop. After the Emulator fetches an instruction into IR (via `IR← MD`), the next microinstruction fires `IDISP`, which ORs IR's opcode bits into NEXT, routing execution to the per-opcode microcode handler for that Nova instruction.

---

## 4 — Registers

### 4.1 Per-cycle pipeline registers

| Register | Width | Role |
|----------|-------|------|
| BUS      | 16    | Tri-state bus; driven once per cycle by BS-selected source. Combinational (not edge-clocked). |
| T        | 16    | ALU operand B. Loaded via `T_LOAD` from BUS at cycle edge. |
| L        | 16    | ALU result staging + shift register. Loaded via `L_LOAD` from ALU output at cycle edge. F1 codes 4/5/6 also rotate/shift L. |
| M        | 16    | Auxiliary register; alias of L in some contexts (see `altoconsts23.mu.txt:74-76`: `$M $R40`). On Alto II is a separate physical register. |

### 4.2 R-register file

32 entries × 16 bits, addressed by the 5-bit RSEL field. Read-once, write-once per cycle (single-port). Some R-registers are conventionally named (from `altoIIcode3.mu.txt:40-56` and `altoconsts23.mu.txt`):

| Index   | Name      | Convention |
|---------|-----------|-----------|
| R4      | `NWW`     | State of interrupt system |
| R20     | `CURX`    | Cursor X coordinate |
| R21     | `CURDATA` | Cursor data |
| R22     | `CBA`     | Display Control Block Address |
| R23     | `AECL`    | Display task auxiliary |
| R24     | `SLC`     | Scan Line Count |
| R25     | `MTEMP`   | Public temporary (MRT-shared) |
| R26     | `HTAB`    | Horizontal tabulation reg |
| R27     | `YPOS`    | Y position |
| R30     | `DWA`     | Display Word Address |
| R37     | `R37`     | MRT, interval timer, EIA |
| R40     | `M`       | M register alias |

**Unlike RV32I's hardwired-zero `x0`, R0 is a normal R/W R-register.** This is documented in `src/regfile.rs`.

### 4.3 M and S registers (control RAM optional, AltoHW §8.7)

The **control RAM card** is optional on Alto I, standard on Alto II. It provides:

- 1024 × 32 bit fast (90 ns) read/write memory used as additional microinstruction storage (extending the 1K microinstruction ROM)
- An even faster 32 × 16 bit (40 ns) memory containing the **M register and 31 S registers**

Per AltoHW §8.7: "The control RAM card also includes an M register and 31 S registers. The M register is the analog of the basic Alto's L register. It provides data for the S registers, which are analogous to the basic Alto's R registers. These additional registers were provided to ease the tight constraint on R register availability."

**Critical asymmetries from the L/R analog:**

- The M register is loaded from the ALU output **only when the highest-priority RAM-related task is executing** and L_LOAD is set. So M only updates in RAM-related tasks (typically just the Emulator on standard Alto).
- The data path from M to the S registers contains **no shifter** (unlike L→R, where the shifter is part of the L→R path).
- ACSOURCE and ACDEST F2's have **no effect on S register addressing** (they only affect R selection).
- When reading S registers onto the bus, **RSELECT=0 returns the current value of M**, not S0. This is why there are only 31 useful S-registers (numbered S1–S31).

**S registers are bank-switched.** Per `altoconsts23.mu.txt:62-64` and ContrAlto, the SRB (Set Register Bank, F1 task-specific) cycles through 8 banks of 32 S-registers each = 256 total, addressed by RSELECT after a per-task bank-select operation. ESRB (Emulator's Set Register Bank, F1=15B in Emulator) sets the Emulator's bank.

The Emulator uses S-banks for opcode-handler scratch; BITBLT uses S-registers for source/destination block parameters; user-loadable microcode uses them for arbitrary working storage.

**RAM-related tasks (AltoHW §8.1).** Per AltoHW §8.1: "A RAM-related task is defined as one during whose execution the control RAM card will respond to F1 and BS fields of microinstructions. ... The standard Alto is wired so that the **emulator task is the only RAM-related task**. At most two other tasks can be made RAM-related by a simple backpanel wiring change."

So on a standard Alto, only the Emulator (task 0) sees S-registers, RDRAM, WRTRAM, SWMODE, etc.; other tasks see those F1 codes as no-ops or different per-task functions.

### 4.4 Memory interface ✓ verified against AltoHW (Aug76) §2.3

| Register | Width | Role |
|----------|-------|------|
| MAR      | 16    | Memory Address Register. Loaded by **F1=1 (`MAR←`)** from the ALU output; this initiates a main memory reference. |
| MD       | 16    | Memory Data Register. Read via **BS=5 (`←MD`)** for fetch-result. Written via **F2=6 (`MD←`)** to deliver BUS data to memory. |
| IR       | 16    | Instruction Register (Emulator-task only). Loaded via Emulator F2 code 14 (`IR← MD`). Drives `IDISP` (F2=15). |

**Memory timing rules (universal, per AltoHW §2.3):**

a) "A minimum of one microinstruction must intervene between the initiation of a memory reference and an `MD←` or `←MD`."

b) "Both machines [Alto I and Alto II] share the property that the processor will suspend execution of microinstructions if an `←MD` or `MD←` is executed before the memory interface is prepared to deliver or accept data." So memory references stall the pipeline if the timing isn't met.

c) "The memory checks parity on all fetches, unless the cycle is a refresh cycle or the address is between 177000B and 177777B inclusive, in which case an I/O device is being referenced. Parity errors result in activation of the highest-priority task (task number 15) whose purpose is to deal with the error."

d) "If RSELECT = 37B during the instruction which starts the memory, a refresh cycle is assumed and all memory cards are activated. This is used by the refresh task." So **MRT does DRAM refresh by issuing `MAR←` with RSELECT=37B**, not by any dedicated refresh instruction.

e) "`MAR←` cannot be invoked in the same instruction as `←MD` of a previous access."

**Alto I vs Alto II timing differences:**

- **Alto I:** Store happens in the **fourth cycle** after `MAR←` (if `MD←` is issued no later than the fourth). Read result delivered on `←MD` in the fourth cycle (single word) or fourth+fifth (doubleword, MAR even-aligned).
- **Alto II:** Store happens in the **third cycle** after `MAR←`. Read result available in cycle four; *because Alto II latches memory contents,* `←MD` can be executed any time after the fourth cycle and still obtain the read result. This permits a "double-word exchange" idiom.
- **Alto II XMAR (extended MAR):** F1=1 + F2=6 simultaneously loads extended MAR for >64K addressing on Alto II configurations with XM (extended memory) hardware.

The RHDL implementation models the Alto II timing (3-cycle store latency, 4-cycle load latency) per `src/memory.rs`. Phase 3 uses a 256-word stub; Phase 3.5 swaps to `rhdl_fpga::core::ram::SyncBRAM` for the full address space.

---

## 5 — The 16-task system ✓ verified against AltoHW (Aug76) §2.4 (Microprocessor Control)

The Alto's defining architectural feature. Sixteen hardware tasks; on every microcycle, the priority encoder picks the highest-priority woken task and the microengine executes that task's next microinstruction.

### 5.1 Task numbering and priority

Per AltoHW §2.4: "Control of the Alto microprocessor is shared among 16 'tasks' arranged in a priority order. The tasks are numbered 0 to 15: **0 is the lowest priority task and 15 is the highest**. The lowest priority task is the emulator task which fetches instructions and executes them."

Per AltoHW §2.3 footnote (c): **"Parity errors result in activation of the highest-priority task (task number 15) whose purpose is to deal with the error."** The HW manual treats task 15 as canonical for parity in *generic* terms — but real Alto II microcode (`altoIIcode3.mu`) places `PART` at **task 13** and leaves task 15 unused, with `KWDX` (Disk Word, the hard-realtime task) as the highest-priority *active* task.  ContrAlto's `CPU.cs` enum agrees: `Parity = 13, DiskWord = 14`.  This implementation follows the real-microcode convention — see §5.2 below.

**Reset entry points (AltoHW §2.4 *Initialization*).** Each task starts at MPC = its task number:

> "This presents an initialization problem which is solved by having each task start at the location which is its task number (thus the emulator task finds its first instruction to execute at MPC=0). Task numbers are written into the MPC RAM during a reset cycle, which may be initiated manually or by a CPU instruction (see SIO instruction in section 3.3)."

So the Emulator task starts at **MPC=0**, KSEC at **MPC=4**, MRT at **MPC=8**, PART at **MPC=13**, KWDX at **MPC=14**. The microcode labels (`NOVEM`, `KSEC`, `MRT`, etc.) are *Mu-assembler labels* that resolve to those addresses by being placed at those positions in the assembled output via the `!17,20,...` directive in altoIIcode3.mu.

### 5.2 Task table (canonical numbering — matches real altoIIcode3.mu)

From `altoIIcode3.mu.txt:25` (the `!17,20,NOVEM,,,,KSEC,,,EREST,MRT,DWT,CURT,DHT,DVT,PART,KWDX,;` reset-vector directive) and cross-referenced with ContrAlto's `CPU.cs` enum:

| Index | Mnemonic   | Reset MPC | Purpose                                                | Priority |
|-------|------------|-----------|--------------------------------------------------------|----------|
| 0     | `EMU` / `NOVEM` | 0    | Nova-instruction emulator (CPU)                        | **lowest** — always requesting wakeup; runs whenever no other task is woken |
| 1     | (unused)   | 1         | reserved                                               | — |
| 2     | (unused)   | 2         | reserved                                               | — |
| 3     | (unused)   | 3         | reserved                                               | — |
| 4     | `KSEC`     | 4         | Disk Sector — runs once per sector mark; sets up DMA   | medium |
| 5     | (unused)   | 5         | reserved                                               | — |
| 6     | (unused)   | 6         | reserved                                               | — |
| 7     | `EREST`    | 7         | Ethernet — packet RX/TX                                | medium |
| 8     | `MRT`      | 8         | Memory Refresh Task — DRAM refresh + interval timer + EIA | high |
| 9     | `DWT`      | 9         | Display Word Task — per-word display DMA                | high |
| 10    | `CURT`     | 10        | Cursor task — overlay mouse cursor on framebuffer       | high |
| 11    | `DHT`      | 11        | Display Horizontal Task — end-of-line housekeeping     | high |
| 12    | `DVT`      | 12        | Display Vertical Task — end-of-frame, vertical retrace | high |
| 13    | `PART`     | 13        | **Parity error task** — handles memory parity errors   | very high |
| 14    | `KWDX`     | 14        | Disk Word Task — per-word disk DMA                      | **highest active** |
| 15    | (unused)   | 15        | reserved (HW manual reserves task 15 for parity in generic terms; real Alto II microcode does not use this slot) | — |

Higher-numbered task = higher priority.  The Emulator (task 0) runs as the "background" task; KWDX (task 14) preempts almost everything to keep up with disk word strobes; PART (task 13) handles parity-error interrupts.  Task 15 is reserved by the hardware spec but unused by the as-shipped Alto II microcode binary.

### 5.3 Wakeup signals

Each task has a hardware-generated `wakeup` input. Per AltoHW §2.4: "The 'wakeup signals' which drive the priority encoder are hardware-generated and are not accessible to the microprogram."

- **DVT, DHT, DWT, CURT** wake from the display-controller hardware (vertical/horizontal retrace timing, per-word output FIFO empty, cursor scan-line reached).
- **KSEC, KWDX** wake from the disk-controller hardware (sector mark detected, word strobe).
- **EREST** wakes when an Ethernet packet starts arriving or transmission needed.
- **MRT** wakes periodically (every ~38 µs) to refresh DRAM and tick the 38 µs interval timer; also services EIA serial receive.
- **PART** wakes on memory-parity errors detected during fetch.
- **EMU** (task 0) is **always requesting wakeup** (per AltoHW §2.4: "The lowest priority task is the CPU emulator, which is always requesting wakeup") — it runs whenever no higher-priority task is woken.

### 5.4 Task switching semantics (AltoHW §2.4)

Per AltoHW §2.4: "If the processor executes the TASK function (F1=2) during an instruction, the current task register is loaded (at the end of the instruction) with the number of the current highest priority task as determined by the priority encoder. This causes the next instruction to be fetched from the ROM location specified by the saved task's MPC. **One additional instruction is executed before the switch becomes effective.**"

So `TASK` is delayed by one cycle:

```
Instruction          Instruction       Address Stored
Being Executed       Being Fetched     MPC at End of Cycle
A                    B                 C
B                    C                 D
C *                  D                 E      ; C uses TASK; switch announced
D                    J                 K      ; D still runs; J fetched from new task
J **                 K                 L      ; J does no address modification
K ***                L                 M
L                    E                 F      ; back in original task
E                    F                 G

* = task-switching instruction (TASK)
** = first instruction of new task (must do no NEXT modification, since modification would affect the OTHER task's MPC)
*** = task-switching instruction in new task
```

The "no address modification on first instruction of new task" constraint is a real microcode hazard the assembler enforces.

Per AltoHW §2.4: "The TASK function should be executed only at times when the current task has no state in L or T, and has no main memory operations in progress, since there is no provision in the hardware for saving this information." So tasks must reach quiescence (no in-flight memory access, no values in L or T they care about) before yielding.

### 5.5 BLOCK semantics (AltoHW §2.4)

Per AltoHW §2.4: "The BLOCK function (F1=3) is used, by convention, to signal a hardware device associated with the currently running task to remove its wakeup signal. This function is *not* accomplished by the Alto microprocessor, but rather by the individual device interfaces."

So BLOCK is a *convention* — the microengine doesn't clear the wakeup itself; the device hardware monitors the F1 lines and clears its own wakeup when it sees its task asserting F1=3. This is a critical design point for any RHDL widget that integrates with a task: the *device* widget must snoop F1 and gate its wakeup output on (~~F1==BLOCK while this task active~~).

In RHDL, the 16-task system is the canonical `rhdl-rule` use case (see `src/task_system.rs` and the README's Phase 2 overview).

---

## 6 — The Emulator task and Nova-instruction dispatch ✓ verified against AltoHW (Aug76) §3.0, §3.1

Task 0 (`EMU`) emulates a Nova-derived 16-bit instruction set. Per AltoHW §3.0: "The standard microcode on the Alto contains an 'emulator' as the lowest-priority task. This code fetches, decodes, and executes instructions resident in the Alto memory whose encoding resembles that of the Data General Nova computers. This 'standard' emulator can be replaced by changing the microcode that is executed as the lowest priority task, often by executing special emulator microcode in the microcode RAM."

The Emulator's reset entry is at **MPC=0** (per §5.1). The Mu-assembler label `NOVEM:` (or equivalents in different microcode revisions) is placed at that address by the assembler.

### 6.1 Differences from Data General Nova (AltoHW §3.1)

The Alto's Nova-derived instruction set differs from the Nova in four ways:

1. **16-bit addresses** (Nova has 15). "Multi-level indirection is not possible, and all 16 bits of a register used for indexing are significant."
2. **No auto-index locations** (the Nova reserves locations 16–31 for auto-increment indirect).
3. **The interrupt system is entirely different** (see AltoHW §3.2).
4. **The I/O class of instructions is not implemented**; instead, the Alto has an "augmented instruction set" (see AltoHW §3.3 for the Alto-specific ops).

### 6.2 Emulator state (AltoHW §3.1)

| Register | Width | Purpose |
|----------|-------|---------|
| PC       | 16    | Program counter — address of next instruction |
| AC0..AC3 | 16 ea | Four accumulators |
| C        | 1     | Carry bit, modified by most arithmetic |
| Memory   | 16-bit words, 0..176777B (0x000000..0x00FE00 logical) | 64K words; addresses 177000B..177777B reserved for I/O (see Appendix B) |

### 6.3 Nova instruction formats (AltoHW §3.1, Figure 3)

Four instruction groups, distinguished by the top 3 bits:

```
M-Group  (LDA, STA):     [0|MFunc|DestAC|I|X|       DISP       ]   bits 0..15
J-Group  (JMP,JSR,ISZ,DSZ):  [0|0|0|JFunc|I|X|       DISP       ]
A-Group  (COM..AND):     [1|SrcAC|DestAC|AFunc|SH|CY|NL|  SK   ]
S-Group  (Alto-specific): [0|1|1|         ...                  ]  (augmented)
```

Effective-address calculation for M-group and J-group (AltoHW §3.1, paraphrased):

```
SExtend(x)  =  if x ≥ 200B then x | 177400B else x      // 8-bit sign extension

E()  =  let E1 = (
            if X = 0 then DISP                          // page 0 addressing
            else if X = 1 then SExtend(DISP) + PC       // PC-relative
            else if X = 2 then SExtend(DISP) + AC2      // base register AC2
            else if X = 3 then SExtend(DISP) + AC3      // base register AC3
        ) in
        if I = 1 then memory[E1] else E1                // single-level indirection
```

### 6.4 Operation groups

- **M-Group:** `LDA DestAC, E` (`AC[DestAC] ← M[E]`) and `STA DestAC, E` (`M[E] ← AC[DestAC]`).
- **J-Group:** `JMP E` (`PC ← E`); `JSR E` (`AC3 ← PC+1; PC ← E`); `ISZ E` (`M[E] ← M[E]+1; if M[E]==0 then PC++` — does **not** alter C); `DSZ E` (`M[E] ← M[E]-1; if M[E]==0 then PC++` — does **not** alter C).
- **A-Group:** binary ALU op on (SrcAC, DestAC) into DestAC, with optional carry override (CY: 0=use C, 1=Z=zero, 2=O=one, 3=C=complement-of-C), shift (SH: 0=none, 1=L=rotate-left-17-bit, 2=R=rotate-right-17-bit, 3=S=byte-swap), no-load (NL: 1 means skip the destination write), and skip (SK: 0..7 — skip-on-result conditions). Eight ALU functions: COM, NEG, MOV, INC, ADC, SUB, ADD, AND.
- **S-Group:** Alto-specific augmented opcodes (BITBLT, JMPRAM, SIO, BLT, BLKS, MUL, DIV, RCLK, SIT, etc.).

### 6.5 The IDISP dispatch sub-table

The dispatch table is in `altoIIcode3.mu.txt:38`:

```
;Cycle dispatch table:
!37,20,L0,L1,L2,L3,L4,L5,L6,L7,L8,R7,R6,R5,R4,R3X,R2X,R1X;
```

Each label corresponds to a microcode entry point for one of the Nova-style opcode classes. The `IDISP` F2 ORs IR's high opcode bits into NEXT, routing to one of these 16 handlers.

### 6.6 Emulator hardware (AltoHW §3.5)

Per AltoHW §3.5: "There is a small amount of special hardware which is used exclusively by the emulator. This hardware is controlled by the task specific F2's, and by the `←DISP` bus source."

**IR register (instruction register).** Loaded by `IR←` (F2=14B). "IR← also merges bus bits 0,5,6 and 7 into NEXT, which does a first level instruction dispatch. The high order bits of IR cannot be directly read, but the displacement field of IR (8 low order bits, sign extended), may be read with the `←DISP` bus source."

So `IR←` has a *side effect of OR'ing IR[0,5,6,7] into NEXT* — that's the first-level dispatch (M-Group / J-Group / A-Group / S-Group decode).

**IDISP (F2=15B): 16-way dispatch.** Per AltoHW §3.5, "The IDISP function (F2=15B) does a 16 way dispatch under control of a 256x4 PROM. The values are tabulated below:"

| Conditions               | OR'ed onto NEXT       |
|--------------------------|-----------------------|
| if `IR[0] = 1`           | `3 - IR[8-9]`         |
| elseif `IR[1-2] = 0`     | `IR[3-4]`             |
| elseif `IR[1-2] = 1`     | 4                     |
| elseif `IR[1-2] = 2`     | 5                     |
| elseif `IR[4-7] = 0`     | 1                     |
| elseif `IR[4-7] = 1`     | 0                     |
| elseif `IR[4-7] = 6`     | 16B                   |
| elseif `IR[4-7] = 16B`   | 6                     |
| else                     | `IR[4-7]`             |

(Recall MSB=0 numbering: IR[1-2] are the two highest bits selecting M/J/A/S group; IR[4-7] is the AFunc field for A-group.)

**ACSOURCE (F2=16B): two roles.** Per AltoHW §3.5: "`←ACSOURCE` (F2=16B) has two roles. First, it replaces the two low order bits of the R select field with the complement of the SrcAC field of IR, (IR[1-2] XOR 3), allowing the emulator to address its accumulators (which are assigned to R0-R3). Second, a dispatch is performed":

| Conditions               | OR'ed onto NEXT |
|--------------------------|-----------------|
| if `IR[0]=1`             | `IR[8-9] XOR 3` (the complement of the SH field of IR) |
| elseif `IR[1-2] = 3`     | `IR[5]`         (the indirect bit of IR) |
| elseif `IR[3-7] = 0`     | 2               |
| elseif `IR[3-7] = 1`     | 5               |
| elseif `IR[3-7] = 2`     | 3               |
| elseif `IR[3-7] = 3`     | 6               |
| elseif `IR[3-7] = 4`     | 7               |
| elseif `IR[3-7] = 11B`   | 4               |
| elseif `IR[3-7] = 12B`   | 4               |
| elseif `IR[3-7] = 16B`   | 1               |
| elseif `IR[3-7] = 37B`   | 17B             |
| else                     | 16B             |

**ACDEST (F2=13B).** Per AltoHW §3.5: "F2=13B, ACDEST, causes (IR[3-4] XOR 3) to be used as the low order two bits of the RSELECT field. This addresses the accumulators from the destination field of the instruction. The selected register may be loaded or read."

**DNS (F2=12B): Do Nova Shift.** Per AltoHW §3.5: "The emulator has two additional bits of state, the SKIP and CARRY flip flops. CARRY is identical to the Nova carry bit, and is set or cleared as appropriate when the DNS← (do Nova shifts) function is executed. DNS also addresses R from (IR[3-4] XOR 3), and sets the SKIP flip flop if appropriate. The PC is incremented by 1 at the beginning of the next emulated instruction if SKIP is set, using ALUF 13B. IR← clears SKIP."

So DNS+ACDEST work together: DNS sets up shift+carry+skip per the Nova A-group rules, ACDEST routes the result back to the destination AC.

**Note on RSELECT vs constants ROM addressing:** "Note that the functions which replace the low bits of RSELECT with IR affect only the selection of R; they do not affect the address supplied to the constant ROM."

**BUSODD and MAGIC (F2=10B and F2=11B in Emulator).** Per AltoHW §3.5: "The two additional emulator specific functions, BUSODD and MAGIC, are not peculiar to Nova emulation, but are included for their general usefulness. **BUSODD merges BUS[15] into NEXT[9]**, and **MAGIC is applied in conjunction with LSH and RSH to allow double length shifts. It shifts the high order bit of T into the low order bit of R on left shifts, and shifts the low order bit of T into the high order bit of R on right shifts.**" So MAGIC turns L-shift/R-shift into a 32-bit (R:T) shift operation.

**STARTF (F1=17B).** Per AltoHW §3.5: "The STARTF function (F1=17B) is used by the SIO instruction, and is used to define commands for I/O hardware, including the Ethernet."

### 6.7 Interrupt system (AltoHW §3.2)

Per AltoHW §3.2: "The emulator microcode implements an interrupt structure which allows both I/O devices and programs to interrupt the main program. The interrupt system provides **15 channels of vectored interrupts with adjustable priority**: the lowest-priority channel is numbered 1; the highest is numbered 15."

The interrupt system uses one R-register (`NWW`, "new wakeups waiting") and four reserved page-1 locations:

| Address | Symbol     | Purpose |
|---------|------------|---------|
| (R4)    | `NWW`      | Interrupt pending mask in R-register; bit 0 of NWW disables interrupts when set; bits 1–15 hold pending channels |
| 452B    | `WW`       | Wakeups Waiting — channels with pending interrupts (OR'ed into NWW between instructions) |
| 453B    | `ACTIVE`   | Active channels — bit *n* set if channel *n* is currently active. Bit 0 unused. |
| 500B    | `PCLOC`    | Saved PC when an interrupt is initiated |
| 501B–514B | `INTVEC` to `INTVEC+13B` | Service routine pointers, indexed by `15 - channel`. INTVEC = highest priority (channel 15); INTVEC+14 = lowest (channel 1). |

**Inner-loop interrupt check.** Per AltoHW §3.2: "The main loop of the emulator checks NWW during the fetch of each emulated instruction. If NWW is greater than zero, the microcode computes (NWW OR WW) AND ACTIVE. If this quantity is nonzero, an interrupt is caused. If not, NWW OR WW is stored in WW, NWW is cleared, and the instruction is restarted."

**Service entry sequence.** "If the interrupt is caused, the microcode stores the program counter in PCLOC, sets bit 0 of NWW to disable further interrupts, clears the bit in NWW corresponding to the interrupt channel about to occur, and loads the PC with rv(INTVEC+15-CHANNEL)."

**Channel 15 (highest) is permanently assigned to the parity error task** (per §10.6) — when task 15 detects a parity error, it ORs into NWW bit 15 to trigger an emulator-level parity-error interrupt.

**Three macroinstructions control interrupts:**

| Mnemonic | Octal   | Effect |
|----------|---------|--------|
| `DIR`    | 61000B  | Disable interrupts: sets NWW bit 0 |
| `EIR`    | 61001B  | Enable interrupts: clears NWW bit 0; ORs WW into NWW |
| `DIRS`   | 61013B  | Disable interrupts and skip if interrupts were enabled |

I/O device microcode posts interrupts by ORing into NWW or WW. Each device has a dedicated page-1 location (e.g., display interrupt mask at DASTART+1 = 421B; Ethernet interrupt at EBLOC = 601B) where the program writes a bit-mask of channels to interrupt on completion.

### 6.8 Augmented instruction set (AltoHW §3.3, partial)

Selected Alto-specific instructions (see `altoIIcode3.mu.txt:35` for the complete sub-table):

| Mnemonic    | Octal   | Effect |
|-------------|---------|--------|
| DREAD       | 61015B  | (Alto II only) Double-word read: AC0 ← rv(AC3); AC1 ← rv(AC3 XOR 1) |
| DWRITE      | 61016B  | (Alto II only) Double-word write: rv(AC3) ← AC0; rv(AC3 XOR 1) ← AC1 |
| DEXCH       | 61017B  | (Alto II only) Double-word exchange |
| DIAGNOSE1/2 | 61022B/61023B | (Alto II only) Diagnostic instructions for Hamming-code memory |
| BITBLT      | 61024B  | Bit-boundary block transfer — the Alto's signature graphics primitive |
| MUL/DIV     | (varies)| 16×16 multiply / 32÷16 divide (microcoded) |
| RCLK / SIT  | (varies)| Read clock / Set interval timer |
| SIO         | (varies)| Start I/O — used to invoke booting and Ethernet operations |
| JMPRAM/RDRM/WTRM | (varies) | Microcode RAM control for user-loadable microcode |

**BITBLT** deserves special mention: it's an interruptible block-bit-transfer instruction parameterized by a 16-word "BBTable" pointer in AC2. It implements one of {Replace, Paint (OR), Invert (XOR), Erase (AND-NOT)} between a Source block and a Destination block, where the Source can be: a bit-map block, the complement of a bit-map block, the AND of a block and a "gray block," or just the gray block. A typical 8×14 character takes ~1500 cycles. BITBLT requires the microcode RAM to be present (it uses S-registers); on RAM-less Altos, BITBLT traps.

---

## 7 — Constants ROM ✓ verified against AltoHW (Aug76) §2.2 (Constant Memory)

Per AltoHW §2.2: "The constant memory is a **256 × 16 PROM** which holds arbitrary constants. The constant memory is gated to the bus by **F1=7, F2=7, or BS≥4**. The constant memory is addressed by the (8 bit) **concatenation of RSELECT and BS**."

So the gating conditions are wider than just F1=7:
- **F1=7** (`←CONSTANT`) — explicit constant load
- **F2=7** (`←CONSTANT`) — explicit constant load (same effect as F1=7)
- **BS≥4** (any of `←KDATA`, `←MD`, `←MOUSE`, `←DISP`) — *implicit constant gating*, used as a **wired-AND mask**

The 8-bit constants ROM address is `{RSELECT[0:4], BS[0:2]}` — RSELECT contributes the high 5 bits, BS the low 3 bits. So the 256 PROM entries are organized as 32 RSELECT-rows × 8 BS-columns. Of those 256 slots, only 4 BS-columns (BS=4,5,6,7) are used for the masking facility — that's "up to 32 such mask constants for each of the 4 bus sources ≥4" (32 × 4 = 128 mask constants).

Per AltoHW §2.2: "The intent in enabling constants with BS≥4 is to provide a masking facility, particularly for the `←MOUSE` and `←DISP` bus source. This works because the processor bus ANDs if more than one source is gated to it."

**Alto I limitation (AltoHW §2.2):** "It is not possible to use a constant other than −1 with the `←MD` bus source, because memory parity is calculated on the bus, and a parity error will result if bits are marked off in a word fetched from memory." So Alto I cannot mask `←MD` reads with constants other than all-ones.

**Constants ROM contents.** Some indices are pre-assigned in `altoconsts23.mu.txt:110-220` to canonical values (`$0`, `$ALLONES`, `$BIAS`, etc.) and used pervasively in microcode.

The constants ROM is also where commonly-referenced page-1 magic addresses appear — `$MOUSELOC = $424` (mouse-data block), `$CURLOC = $426` (cursor block), `$DASTART = $420` (display header) — though these are address values used as constants, not the masking constants per se.

### 7.1 Memory map and reserved I/O addresses

Memory-mapped IO ranges live above 0x177777 - many of:

| Address range | Purpose |
|---------------|---------|
| 0x000000–0x000077 | Page 0 (special locations — see §8) |
| 0x000420 (`DASTART`) | Display header pointer |
| 0x000424 (`MOUSELOC`) | Mouse data block |
| 0x000426 (`CURLOC`) | Cursor block |
| 0x000430 (`CLOCKLOC`) | Real-time clock |
| 0x000452 (`WWLOC`) | Wakeup-waiting word in page 1 |
| 0x000460 (`MASKTAB`) | Mask Table for "convert" instruction |
| 0x000500 (`PCLOC`) | PC vector in page 1 |
| 0x000526 (`TRAPDISP`) | Trap dispatch |
| 0x000527 (`TRAPPC`) | Trap PC |
| 0x177024 (`ERRADDR`) | Memory Error Address Register (Alto II) |
| 0x177025 (`ERRSTAT`) | Memory Error Status Register |
| 0x177026 (`ERRCTRL`) | Memory Error Control Register |
| 0x177701 (`EIALOC`) | EIA serial input |

Full mapping in `altoconsts23.mu.txt:144-360`.

---

## 8 — Disk subsystem ✓ verified against AltoHW (Aug76) §6.0 (Disk and Controller)

Per AltoHW §6.0: "The disk controller is designed to accommodate one of a variety of DIABLO disk drives, **including models 31 and 44**. Each drive accommodates one or two disks. Each disk has two heads, one per side. Information is recorded on each disk in a **12-sector format** on each of up to 406 (depending on the disk model) radial track positions."

### 8.1 Disk geometry (AltoHW §6.0, Figure 7)

| Parameter            | Diablo 31              | Diablo 44              |
|----------------------|------------------------|------------------------|
| Drives/Alto          | 1 or 2                 | 1                      |
| Packs/drive          | 1 removable            | 1 removable + 1 fixed  |
| Cylinders            | 203                    | 406                    |
| Tracks/cylinder/pack | 2 (one per head)       | same                   |
| Sectors/track        | 12                     | 12                     |
| Words/sector         | 2 header + 8 label + 256 data | same            |
| Data words/track     | 3072                   | 3072                   |
| Sectors/pack         | 4872                   | 9744                   |
| Rotation             | 40 ms                  | 25 ms                  |
| Seek (avg)           | 70 ms                  | 30 ms                  |
| Transfer rate (avg)  | 1.22 MHz / 13 ns/word  | 1.9 MHz / 7-8 ns/word  |

### 8.2 Per-sector recording structure (AltoHW §6.0)

Each sector contains **three independent recording blocks**:

| Block   | Words | Purpose |
|---------|-------|---------|
| Header  | 2     | Address of the recording position (cylinder/head/sector identification) |
| Label   | 8     | Software-defined label (filesystem metadata in Alto OS / BCPL world) |
| Data    | 256   | Sector payload |

Each block "may be independently read, written, or checked, except that writing, once begun, must continue until the end of the recording position." Block-checking is a hardware comparator: "information on the disk is compared word for word with a specified block of main memory. ... When a memory word containing 0 is encountered, the matching word read from the disk is stored in its place and does not take part in the check" — a partial-write-with-verify mechanism.

### 8.3 The KBLK chain (AltoHW §6.0)

The Alto program communicates with the disk controller via a **chain of disk command blocks**. The chain head is at:

| Address | Symbol     | Purpose |
|---------|------------|---------|
| 521B    | `KBLK`     | Pointer to first disk command block (DCB), or 0 if controller idle |
| 522B    | `KBLK+1`   | Status at beginning of current sector |
| 523B    | `KBLK+2`   | Disk address of most-recently started disk command |
| 524B    | `KBLK+3`   | Sector interrupt bit mask |

Each disk command block is **10 words**:

| Offset | Field | Purpose |
|--------|-------|---------|
| DCB+0  | (link)         | Pointer to next DCB, or 0 |
| DCB+1  | Status         | Filled in by controller on completion |
| DCB+2  | Command        | What to do (see §8.4) |
| DCB+3  | Header pointer | Memory address of header block |
| DCB+4  | Label pointer  | Memory address of label block |
| DCB+5  | Data pointer   | Memory address of data block |
| DCB+6  | Done-no-error interrupt mask | Channels to interrupt on success |
| DCB+7  | Done-error interrupt mask | Channels to interrupt on error |
| DCB+8  | (unused)       | Reserved / available to program |
| DCB+9  | Disk address   | Cylinder/head/sector (see §8.4) |

Storing -1 (or any "illegal" disk address like all-ones) in DCB+9 forces a hardware "restore" (seek to track 0) at the start of the operation.

### 8.4 Disk address word, command word, status word (AltoHW §6.0)

**Disk address word A** (DCB+9):

| Field    | Range                         | Significance |
|----------|-------------------------------|--------------|
| A[0–3]   | 0–13B                         | Sector number |
| A[4–12]  | 0–625B (Model 44) / 0–312B (Model 31) | Track number |
| A[13]    | 0/1                           | Head number |
| A[14]    | 0/1                           | Disk number (XOR'd with C[15] to yield hardware disk number) |
| A[15]    | 0/1                           | 0 normally; 1 = restore to track 0 via hardware "restore" |

**Disk command word C** (DCB+2):

| Field      | Significance |
|------------|--------------|
| C[0–7]     | Must be 110B (sentinel — verifies a valid disk command) |
| C[8–9]     | 0=read header, 1=check header, 2/3=write header |
| C[10–11]   | 0=read label, 1=check label, 2/3=write label |
| C[12–13]   | 0=read data, 1=check data, 2/3=write data |
| C[14]      | 1 = terminate after seek complete (don't transfer data) |
| C[15]      | XOR'd with A[14] to yield hardware disk number |

**Disk status word S** (DCB+1):

| Field    | Bit-meaning |
|----------|--------------|
| S[0–3]   | Current sector number |
| S[4–7]   | 17B sentinel (set to 0 by software, controller writes 17B to indicate status valid) |
| S[8]     | 1 = seek failed (illegal track address) |
| S[9]     | 1 = seek in progress |
| S[10]    | 1 = disk unit not ready |
| S[11]    | 1 = data or sector processing was late |
| S[12]    | 1 = disk interface not transferring data last sector |
| S[13]    | 1 = checksum error (command allowed to proceed) |
| S[14–15] | 0=correctly completed, 1=hardware error, 2=check error, 3=illegal sector |

### 8.5 Disk-task microcode interface ✓ verified against AltoHW (Aug76) §6.0 (per-task F1/F2/BS tables)

Per AltoHW §6.0, the disk controller communicates with the microprocessor via "task wakeup signals for the sector and word tasks; ... five task-specific F2's which modify the next microinstruction address; ... seven task-specific F1's, four of which activate bus destination registers, and the remaining three of which provide useful pulses; and ... two task-specific BS's."

**Task-specific F1 codes (in KSEC and KWDX):**

| F1   | Mnemonic   | Effect |
|------|------------|--------|
| 11B  | `STROBE`   | Initiates a disk seek operation. KDATA must be loaded previously, and SENDADR bit of KCOM register set to 1. |
| 12B  | `KSTAT←`   | KSTAT[12-15] ← BUS[12-15]. (BUS[13] is OR'ed into KSTAT[13].) Lets microcode write conditions into status. |
| 13B  | `INCRECNO` | Advances the shift registers holding the KADR register so that they present the number and read/write/check status of the next record. |
| 14B  | `CLRSTAT`  | Causes all error latches in the disk controller to reset; clears KSTAT[13]. |
| 15B  | `KCOM←`    | KCOM ← BUS[1-5]. KCOM bits decoded as: (1) XFEROFF — inhibits data transmission; (2) WDINHIB — prevents Disk Word Task wakeup; (3) BCLKSRC — bit clock source; (4) WFFO — bit counter control; (5) SENDADR — transmit KDATA[4-12,15] as track address |
| 16B  | `KADR←`    | KADR ← BUS[8-14]. Has format of word C (§8.4). Also causes head address bit to be loaded from KDATA[13]. |
| 17B  | `KDATA←`   | KDATA ← BUS[0-15]. Data output register; also holds disk address during KADR← and seek commands. |

**Task-specific F2 codes (in KSEC and KWDX):**

| F2   | Mnemonic   | Effect |
|------|------------|--------|
| 10B  | `INIT`     | NEXT ← NEXT or (37B if WDTASKACT AND WDINIT, else 0) |
| 11B  | `RWC`      | NEXT ← NEXT or (3 if write current record / 2 if check / 0 otherwise) |
| 12B  | `RECNO`    | NEXT ← NEXT or MAP(record-number) where MAP(0)=0, MAP(1)=2, MAP(2)=3, MAP(3)=1 |
| 13B  | `XFRDAT`   | NEXT ← NEXT or (1 if current command wants data transfer, else 0) |
| 14B  | `SWRNRDY`  | NEXT ← NEXT or (1 if disk not ready for command, else 0) |
| 15B  | `NFER`     | NEXT ← NEXT or (0 if fatal error in latches, else 1) |
| 16B  | `STROBON`  | NEXT ← NEXT or (1 if seek strobe still on, else 0) |

**Task-specific BS codes (in KSEC and KWDX):**

| BS   | Mnemonic   | Effect |
|------|------------|--------|
| 3    | `←KSTAT`   | KSTAT register on bus. Has format of disk status word (§8.4). |
| 4    | `←KDATA`   | Disk input data register on bus. |

A diagnostic note from AltoHW §6.0: "If one reads the disk input data register while writing, what should appear is delayed written data correctly aligned on word boundaries. This is a painless way of checking most of the data paths in the disk controller hardware."

### 8.6 Disk task DMA flow

1. **KSEC** (priority 4) wakes when the disk drive asserts a sector-mark pulse. It reads KSTAT to verify alignment, dispatches via the KBLK chain, sets up KCOM/KADR/KCWA from the current DCB, then `BLOCK`s.
2. **KWDX** (priority 14, very high) wakes once per word strobed by the disk's serial-to-parallel converter. The task reads from KDATA into memory at KCWA (or vice versa for writes), increments KCWA, decrements an internal word counter, and `TASK`s. When the counter reaches zero, the transfer is complete and KSTAT's done bit asserts; on completion the controller stores status into the DCB and follows the link to the next DCB (or idles if link=0).

### 8.7 Boot sequence (AltoHW §3.4)

Per AltoHW §3.4 (*Bootstrapping*): "A 'boot,' which is invoked either by pressing the small button at the rear of the keyboard or by executing an appropriate SIO instruction (see section 3.3), simply resets all micro-PC's to fixed initial values determined by their task numbers. Unless the Reset Mode Register specifies otherwise (see section 8.4), the emulator task is started in the PROM and performs a number of operations:"

1. PC is stored in memory location 0 (the accumulators are not altered).
2. The display is cleared (`rv(420B) ← 0`, i.e. DASTART set to 0).
3. Interrupts are disabled.
4. The first keyboard word (`KBDAD = 177034B`) is read to determine the boot type:

   - **Disk Boot** (BS key NOT depressed): The microcode interprets any depressed keys reported in this keyboard word as a real disk address. If no keys are depressed, this results in a real disk address of 0. The single disk sector at the given address is read: the **256 data words are read into locations 1 to 400B inclusive**; the **label is read into locations 402B to 411B inclusive**. When the transfer is complete, PC ← 1, and the emulator is started. The disk status is stored in location 2, so the bootstrapping code must skip this location.

   - **Ether Boot** (BS key depressed): The Ethernet hardware is set up to read any packet with destination Alto number 377B into locations 1 to 400B inclusive. If a packet arrives with good status and with memory location 2 (the second word of the packet) equal to 602B (a "Breath-of-Life" packet), PC ← 3, and the emulator is started.

So the disk boot loads sector 0 of cylinder 0, head 0 (or the keyboard-encoded address) into the bottom 256 words of memory and jumps to PC=1. The boot constants are in `altoconsts23.mu.txt:236-245` (`$BDAD`, `$KBLKADR`, `$KBLKADR2`, `$KBLKADR3`).

---

## 9 — Display subsystem ✓ verified against AltoHW (Aug76) §4.0–§4.4

Per AltoHW §4.1: "The CRT is a standard **875 line raster-scanned TV monitor, refreshed at 60 fields per second** from a bit map in main memory. The CRT contains **606 points horizontally, and 808 points vertically, or 489,648 points total**."

### 9.1 Display geometry

- **606 pixels horizontal × 808 pixels vertical** (portrait orientation; total 489,648 displayable points)
- **875-line raster TV monitor** at **60 fields/second** (interlaced — 30 frames/second)
- **38 16-bit words per scan line** (38 × 16 = 608 horizontal bits; the extra 2 are blanking)
- **30,704 16-bit words to fill the screen** (38 words × 808 lines)
- **1 bit per pixel** (monochrome, on/off)

### 9.2 Display Control Blocks (DCBs) — AltoHW §4.1

Display memory is organized as a linked list of DCBs starting at `DASTART = 420B` in page 1. **DCBs must be located at even addresses** in memory.

| Address     | Symbol       | Purpose |
|-------------|--------------|---------|
| 420B        | `DASTART`    | Pointer to first DCB (top of screen), or 0 if display off |
| 421B        | `DASTART+1`  | Vertical-field-interrupt bit mask. Every 1/60 sec, this word is OR'ed into NWW to cause interrupts. |

Each DCB is **4 words**:

| Offset | Field    | Layout                                               |
|--------|----------|------------------------------------------------------|
| DCB+0  | (link)   | Pointer to next DCB, or 0 if last                    |
| DCB+1  | (mode)   | Bit 0: 0=high resolution, 1=low resolution; Bit 1: 0=black-on-white, 1=white-on-black; Bits 2–7 (`HTAB`): leading 16·HTAB bit-times of zeros to wait at start of each scan line; Bits 8–15 (`NWRDS`): number of 16-bit words per scan line of this block (must be even; 0 to skip space) |
| DCB+2  | `SA`     | Bit map starting address (must be even)              |
| DCB+3  | `SLC`    | Scan line count — block defines `2*SLC` scan lines (SLC per field, interlaced) |

Per AltoHW §4.1: "At normal resolution, the first scan line of the first (even) field of a block is taken from location SA to SA+NWRDS−1, the first scan line of the odd field is taken from locations SA+NWRDS to SA+2*NWRDS−1. During each field, the bit map address is incremented by NWRDS between each scan line. Thus, although the display is interlaced, its representation in memory is not. In low resolution mode, the video is generated at half speed, and each scan line is displayed twice (once in each field)."

### 9.3 Display controller hardware (AltoHW §4.2, Figure 5)

The display controller consists of:

- **Sync generator** — produces vertical/horizontal sync at 60 Hz field rate; provides the asynchronous bit clock to the shift register.
- **Bit clock** — disabled during blanking; rate set by **SETMODE (F2=11B in DHT)** to either 50 ns/bit (high-res) or 100 ns/bit (low-res). SETMODE examines the two high-order bits of the bus: bit 0=1 sets the clock to 100ns and merges 1 into NEXT[9]; SETMODE also latches bit 1 of the bus to set video output polarity.
- **16-word buffer** — loaded by `DDR← BUS` (F2=10B in DWT). Synchronizes data transfer between the 170 ns master clock and the asynchronous bit clock.
- **1-word intermediate buffer** — sits between the 16-word buffer and the display shift register.
- **Display shift register** — clocked at the bit-clock rate; shifts out one bit per pixel.
- **Cursor shift register** — 16-bit, mixed with the display data via a digital mixer to produce the final video signal.

### 9.4 Display tasks (AltoHW §4.2, §4.3)

The display microcode is divided into three tasks; **DVT > DHT > DWT** in priority order (per the priority numbering — DVT is task 12, DHT is 11, DWT is 9).

The display controller hardware generates wakeup requests:
- **DVT** is awakened **once per field** (at the beginning of vertical retrace).
- **DHT** is awakened once per field at start, and thereafter whenever DWT blocks. DHT can block itself, in which case neither it nor DWT can be awakened until the start of the next field.
- **DWT** wakeup is controlled by the state of the 16-word buffer: if DWT has not executed BLOCK, if DHT is not blocked, and if the buffer is not full, DWT wakeups are generated. Hardware sets the buffer empty and clears the DWT block flip-flop at the beginning of horizontal retrace for every scan line.

### 9.5 Display registers (AltoHW §4.3)

The display controller microcode uses 6 R-registers:

| R-name   | Holds |
|----------|-------|
| `CBA`    | Address of the currently active DCB+1 |
| `AECL`   | Address of the end of the currently active scan line's bit map in main memory |
| `SLC`    | Number of scan lines remaining in the currently active DCB |
| `HTAB`   | Number of tab words remaining on the current scan line |
| `DWA`    | Address of the bit map doubleword currently being fetched for transmission to the hardware buffer |
| `MTEMP`  | Temporary cell |

### 9.6 Per-task responsibilities (AltoHW §4.3)

**DVT** (Display Vertical Task):
- Initializes controller by setting SLC to 0 and CBA to DASTART+1
- Merges the contents of DASTART+1 into NWW (interrupt request word) — causes an interrupt if the specified channel is active
- Sets up cursor information (see §9.7)
- TASKs and goes inactive until next field

**DHT** (Display Horizontal Task):
- Initiates fetch of word addressed by CBA
- If SLC == 0: controller is finished with current DCB; fetches link word; if non-zero, replaces CBA and processes new block; if zero, BLOCKs until next field
- If SLC > 0: decrements SLC, fetches second DCB word; sets display rate and polarity; extracts tab count into HTAB; extracts NWRDS to increment DWA and AECL appropriately based on mode/field
- All registers required by DWT have now been set up; DHT TASKs and becomes inactive until DWT BLOCKs
- For new DCBs, DHT fetches all 4 words; for subsequent scan lines of the same DCB, DHT only accesses the first doubleword

**DWT** (Display Word Task):
- "Has the sole task of transferring words from memory to the hardware"
- On wakeup during horizontal retrace, checks HTAB; if non-zero, outputs HTAB zeros to the display
- When HTAB == 0, fetches a doubleword from DWA, compares DWA with AECL — if equal, BLOCKs until next scan line; if not equal, increments DWA by 2 and continues to supply words to the buffer

### 9.7 Cursor (AltoHW §4.4)

"Because of the difficulty of inserting a cursor at the appropriate place in the display bit map at reasonable speed, **a hardware cursor** is included in the Alto. The cursor consists of an arbitrary **16×16 bit patch**, which is merged with the video at the appropriate time."

- Cursor bit map: 16 words starting at `CURMAP = 431B`
- Cursor x,y coordinates: `CURLOC = 426B` (x), `CURLOC+1 = 427B` (y)
- Origin: upper-left corner of the screen
- The cursor may be removed from view by setting x to −1 (most efficient method)

The cursor hardware consists of a 16-bit shift register and an x-coordinate counter clocked by the bit clock. The hardware is loaded during horizontal retrace by the **CURT (cursor task) microcode**, which simply copies x and the bit map segment from R-memory into the hardware.

The MRT (memory refresh task) also touches cursor state: it checks the current y position of the display, and if in range, fetches the appropriate bit map segment from CURMAP to set up R-memory for CURT. When the cursor y position is exceeded, MRT sets a flag to disable further processing.

### 9.8 Display task summary table

| Task | Rate                              | Role |
|------|-----------------------------------|------|
| DVT  | 60 Hz (per field)                 | Initialize DCB walk; trigger vertical interrupt; refresh cursor |
| DHT  | ~50 kHz (per scan line)           | End-of-line housekeeping; advance to next DCB if SLC exhausted |
| DWT  | depends on mode (~1.6 MHz hi-res) | Output one buffer-load of words per fetch via `DDR←` (F2=10B) |
| CURT | per scan line where cursor visible| Overlay 16×16 cursor patch on the scan |

`DDR←` (Display Data Register write, F2=10B in DWT) loads the 16-word buffer in one cycle. `EVENFIELD` (also F2=10B but in DHT/DVT) merges 1 into NEXT[9] if the display is in the even field. `SETMODE` (F2=11B in DHT) sets the bit-clock rate and video polarity.

---

## 10 — Other subsystems ✓ verified against AltoHW (Aug76) §5.0–§5.6

Per AltoHW §5.0: "The Alto can have a number of slow peripherals which appear to programs as memory locations in the range **177000–177777B**. ... In the reserved memory locations associated with keyboard, mouse, keyset and Diablo printer input, **a more positive logic value reads as a 1**" (i.e., the I/O is "low-true" — pressed key = 0, idle = 1).

### 10.1 Ethernet ✓ verified against AltoHW (Aug76) §7.0–§7.2

Per AltoHW §7.0: "The Ethernet is the principal means of communications between an Alto and the outside world. It is a broadcast, multi-drop, packet-switching, bit serial, digital communications network. ... To connect up to 256 nodes, separated by as much as 1 kilometer, with a **2.94 megabits/sec channel**." (Note: nominal 3 Mbps; canonical figure 2.94 Mbps.)

Ethernets come in three pieces: **transceiver** (taps into the cable), **interface** (Alto-side hardware), and **microcode** (running in the Ether task with priority 7). The interface contains: an interface buffer, output shift register + phase encoder, clock recovery circuit, input shift register, CRC register, and one microcode task.

**Reserved page-1 locations (programmer-visible):**

| Address | Symbol     | Purpose |
|---------|------------|---------|
| 600B    | `EPLOC`    | Post location: status posted here when a command completes |
| 601B    | `EBLOC`    | Interrupt bit mask: OR'ed into NWW when command completes |
| 602B    | `EELOC`    | End count: words remaining in main-memory buffer at completion |
| 603B    | `ELLOC`    | Load location: random retransmission-interval mask |
| 604B    | `EICLOC`   | Input count: input buffer size in words |
| 605B    | `EIPLOC`   | Input pointer: address of input buffer |
| 606B    | `EOCLOC`   | Output count: output buffer size in words (≤256 by convention) |
| 607B    | `EOPLOC`   | Output pointer: address of output buffer |
| 610B    | `EHLOC`    | Host address: zero in left byte; this Alto's address (1B–377B) in right byte |

**SIO commands** (AC0[14:15]):
- 0 = no-op; 1 = start transmitter; 2 = start receiver; 3 = reset interface and microcode

**Emulator-side F1 codes for Ethernet support** (per AltoHW §3.5 and §7):
- `RSNF` (F1=16B) — reads the Alto's host number set by backplane wires
- `STARTF` (F1=17B) — used by SIO; sets the ICmd/OCmd flip-flops in the Ethernet interface from BUS[14:15], causing the Ethernet task to wake up

**Ethernet task R-registers (AltoHW §7.3):**

| R-name | Holds |
|--------|-------|
| `ECntr` | Number of words remaining in the buffer |
| `EPntr` | Points at the word prior to that next to be processed |

**Task-specific F1 codes (in EREST task):**

| F1   | Mnemonic | Effect |
|------|----------|--------|
| 13B  | `EILFct` | Input Look — gates interface buffer to BUS[0:15] without advancing read pointer |
| 14B  | `EPFct`  | Post Function — gates interface status to BUS[8:15]; resets interface at end of cycle |
| 15B  | `EWFct`  | Countdown Wakeup — sets a flip-flop causing wakeup on next SWAKMRT (must be issued in instruction after a TASK) |

**Task-specific F2 codes (in EREST task):**

| F2   | Mnemonic | Effect |
|------|----------|--------|
| 10B  | `EODFct` | Output Data — loads interface buffer from BUS[0:15]; increments write pointer |
| 11B  | `EOSFct` | Output Start — sets OBusy flip-flop in interface; starts data wakeups for output |
| 12B  | `ERBFct` | Reset Branch — merges ICmd and OCmd flip-flops into NEXT[6:7] |
| 13B  | `EEFct`  | End-of-transmission — disables further data wakeups when output buffer fully transferred |
| 14B  | `EBFct`  | Branch — ORs 1 into NEXT[7] if input data late, SIO with AC0[14:15] non-zero, or transmit/receive done; ORs 1 into NEXT[6] if collision |
| 15B  | `ECBFct` | Countdown Branch — ORs 1 into NEXT[7] if interface buffer not empty |
| 16B  | `EISFct` | Input Start — sets IBusy flip-flop; interface hunts for packet start (silence + transition) |

**Task-specific BS code (in EREST task):**

| BS   | Mnemonic | Effect |
|------|----------|--------|
| 4    | `EIDFct` | Input Data Function — gates interface buffer to BUS[0:15]; increments read pointer |

**Packet structure (convention):** First word = destination Alto number (left byte) + source Alto number (right byte). Destination 0 = broadcast. Second word = packet type (e.g., 1000B for Pup protocol). After the data words: 16-bit CRC trailer.

The Ethernet uses a single task (EREST) shared between input and output — when an SIO is issued, the Ethernet microcode dispatches on whether it's input/output/reset by reading the ICmd/OCmd flip-flops via `ERBFct`. Up to 16 retransmissions on collision, with random intervals scaled by current collision count.

### 10.2 Keyboard (AltoHW §5.1)

61-key keyboard. Appears as **four 16-bit words** at `KBDAD = 177034B`. Depressed keys read as 0; idle keys as 1.

| Word          | Examples |
|---------------|---------|
| KBDAD         | 5, 4, 6, E, 7, D, U, V, 0, K, -, P, /, \, LF, BS |
| KBDAD+1       | 3, 2, W, Q, S, A, 9, I, X, O, L, ;, ], LF/FL2/etc. |
| KBDAD+2       | 1, ESC, TAB, F, CTRL, C, J, B, Z, ⟨shift-left⟩, :, RETURN, ←, DEL |
| KBDAD+3       | R, T, G, Y, H, 8, N, M, LOCK, SPACE, [, +, ⟨shift-right⟩ |

Alto II keyboard adds function keys (FR1–FR4, FL1–FL5, BW) at the high bits of KBDAD+2/+3.

### 10.3 Mouse (AltoHW §5.2)

Three-button optical mouse. Buttons read at low-order bits of `UTILIN = 177030B`:

| Bit | Button |
|-----|--------|
| 13  | Top or Left button |
| 14  | Bottom or Right button |
| 15  | Middle button |

**Mouse position is maintained by MRT microcode** in main memory at `MOUSELOC = 424B` (x) and `MOUSELOC+1 = 425B` (y) in page one. **Coordinates are relative**, i.e., the hardware only increments and decrements them; the OS resets them to absolute values periodically. Resolution: ~100 points per inch.

### 10.4 Keyset (AltoHW §5.3)

Five-finger keyset (chord keyboard) at bits 8–12 of `UTILIN = 177030B`:

| Bit | Key |
|-----|-----|
| 8   | Key 0 (left-most) |
| 9   | Key 1 |
| 10  | Key 2 |
| 11  | Key 3 |
| 12  | Key 4 (right-most) |

### 10.5 Diablo printer (AltoHW §5.4)

Optional Diablo HyType printer, controlled via two memory-mapped locations: `UTILIN = 177030B` (status, mainly the 7 low bits — paper ready, daisy ready, carriage ready, etc.) and `UTILOUT = 177016B` (control — strobed bits to print, scroll, position carriage, set ribbon).

A 1 in UTILOUT writes "as a more negative logic value." Output operations are *toggled*: set bit to 1, then back to 0. Printable character codes go in bits 9–15 of UTILOUT (codes < 40B are interpreted as lowercase "w").

### 10.6 Parity error detection (AltoHW §5.6)

Per AltoHW §5.6: "The detection and reporting of parity errors is accomplished somewhat differently on Alto I and Alto II. In both machines, the processing of errors is undertaken by the **highest priority microtask** [task 15], which is invoked very soon after an error occurs. The microtask reports a parity error by causing an interrupt on the highest-priority emulator interrupt channel, i.e. by oring into NWW bit 15."

When a parity error happens, the parity task stores R-register snapshots into reserved locations 614B–621B (DCBR, KNMAR, DWA, CBA, PC, SAD).

**Alto II only** has Hamming-code single-bit error correction and double-bit error detection. Three memory-mapped registers control it:

| Address | Symbol | Purpose |
|---------|--------|---------|
| 177024B | MEAR   | Memory Error Address Register (first error since last read) |
| 177025B | MESR   | Memory Error Status Register (first error specifics, low-true) |
| 177026B | MECR   | Memory Error Control Register (low-true: enables single/double-bit interrupts) |

### 10.7 Memory refresh + interval timer

The MRT task (task 8, high priority) wakes periodically. It services three jobs in turn:

1. **DRAM refresh**: walk a row counter through main memory at 38 µs/row to satisfy DRAM refresh requirements. (FPGA implementation: BRAM doesn't need refresh; this becomes a no-op.) Per AltoHW §2.3 (d), refresh is invoked by issuing `MAR←` with **RSELECT = 37B**, which "activates all memory cards" (a refresh cycle, not a real fetch).
2. **Interval timer**: maintain the 38-µs-period interval timer, used for software clocks and sleep timeouts. Stored at `CLOCKLOC = 0x000430`.
3. **EIA serial RX**: poll the serial port for incoming bytes, buffer them.
4. **Cursor maintenance**: as documented in §9.7, MRT also fetches the cursor bit-map segment for the current scan line.

MRT uses R37 (`$R37` per the constants) as its working register and shares MTEMP (R25) with anyone else who needs scratch space.

---

## 11 — Control RAM and the SWMODE mechanism ✓ verified against AltoHW (Aug76) §8.0–§8.8

Per AltoHW §8.0: "The control RAM is an optional logic card containing a fast (90 nsec.) **1024-word by 32-bit read/write memory**, an even faster (40 nsec.) **32-word by 16-bit read/write memory** [the M and S registers], and logic to interface those memories to the Alto's microinstruction bus, processor bus, and ALU output. **Unlike other memories in the Alto, the larger memory of the control RAM can hold microinstructions and/or data, and may be used exactly as the memory of a von Neumann computer.**"

### 11.1 ROM/RAM dispatch via the MPC's PC0 bit (AltoHW §8.3)

Per AltoHW §8.3: "The PC0 bit of the micro-program counter (MPC) of each Alto task specifies whether that task is currently executing microinstructions from the control ROM or the control RAM. The next microinstruction address field of a microinstruction is not wide enough to specify a transfer from ROM to RAM or vice-versa. **A special transfer mechanism exists only for RAM-related tasks, in the form of SWMODE, a RAM-related F1.** SWMODE inverts the PC0 bit of the running task, taking effect after the microinstruction following that in which the SWMODE appears."

So the 12-bit MPC has a high "RAM/ROM select" bit. Standard tasks **cannot switch between ROM and RAM** — only the Emulator (and possibly two other tasks if backpanel-wired RAM-related) can issue SWMODE.

### 11.2 Control-RAM microinstruction encoding (AltoHW §8.3)

Per AltoHW §8.3, the control RAM stores microinstructions with this bit-mapping:

| Halfword     | Bit of ALU output | Field |
|--------------|-------------------|-------|
| High         | 0–4               | RSELECT |
| High         | 5–8               | ALU Function Select |
| High         | 9–11              | Bus Data Source |
| High         | 12–15\*            | Function 1 |
| Low          | 0–3\*              | Function 2 |
| Low          | 4                 | Load T |
| Low          | 5\*                | Load L |
| Low          | 6–15              | Next micro address |

*\*Fields denoted with \* are represented with their **high-order bit inverted**; this is an artifact of hardware microinstruction decoding.* So the actual hardware-level encoding has F1[0], F2[0], LoadL all inverted compared to the field's logical value. The Mu microassembler and any microcode loader must do this transformation; reading microcode RAM directly via RDRAM yields the inverted encoding.

### 11.3 Control RAM access — RDRAM, WRTRAM (AltoHW §8.2)

Per AltoHW §8.2: "Loading the control RAM from the ALU output and reading the control RAM onto the processor bus is slightly clumsy. ... For both reading and writing, the control RAM address is specified by the **control RAM address register**, which is loaded from the ALU output whenever T is loaded from its source. This load may take place as late as the microinstruction in which WRTRAM or RDRAM is asserted."

So the protocol is: **load T (which side-effect-loads the control RAM address register), then issue WRTRAM or RDRAM in the same or subsequent instruction**.

Address bit decode (in the ALU output):
- Bits 0–3: ignored
- Bit 4: RAM/ROM (0 = R/W RAM, 1 = read ROM with caveats, see AltoHW §8.8)
- Bit 5: HALFSEL (0 = low-order 16 bits, 1 = high-order 16 bits) — ignored on writing
- Bits 6–15: word address (0–1023)

Reading: **only one halfword per RDRAM**. To read a full 32-bit microinstruction, microcode must load the address twice (once with HALFSEL=0, once with HALFSEL=1) and issue RDRAM twice. Read data appears AND'ed onto the bus during the microinstruction following RDRAM.

Writing: a single WRTRAM writes both halves at once. The **M register** holds the high-order half (loaded from the ALU output the instruction before WRTRAM); the ALU output during the WRTRAM-bearing instruction is written into the low-order half. This protocol "mates well with doubleword main memory reads" — typical sequence:

```
MAR← ramAddrSrc;              ; start memory fetch of µinstruction high+low
NOP;                          ; required for memory timing (Alto II)
L← MD;        T← FIE;         ; high half → L (saved); FIE = saved RAM address
T← MD;        TEMP← L, L← T;  ; low half → T; high half → TEMP
T← FIE, WRTRAM;               ; T-load also loads RAM address register; WRTRAM writes
                              ; (M = ALU result one µi ago, low = ALU result now)
L← TEMP;                      ; complete write into the RAM
```

### 11.4 Reset Mode Register (AltoHW §8.4)

The RAM-related F1 `RMR←` (F1=11B in RAM-related tasks) loads the **Reset Mode Register** from the processor bus. This 16-bit register supplies the initial value of each task's PC0 bit during the next reset (boot) operation:

> Bit *i* of the bus specifies the initial mode of task *i* (low bit = task 0 = Emulator; high bit = task 15 = parity). A task starts in the control ROM if its RMR bit is 1; otherwise it starts in the control RAM.

On power-up, RMR is automatically set to all 1s — all tasks start in ROM. After loading RMR with a different value, a subsequent boot will start the specified tasks in RAM. This is how loadable-microcode systems (like Smalltalk's per-task accelerator microcode) install themselves.

### 11.5 Standard emulator access (AltoHW §8.5)

Three macroinstructions provide minimum access from emulated programs:

| Mnemonic    | Octal   | Effect |
|-------------|---------|--------|
| `RDRAM`     | 61011B  | Read control RAM halfword addressed by AC1 into AC0 |
| `WRTRAM`    | 61012B  | Write AC0 (high) and AC3 (low) into control RAM word at AC1 |
| `JMPRAM`    | 61010B  | Sends control of the emulator task to control RAM at AC1 mod 1024 |

Per AltoHW §8.5: "[`JMPRAM`] is fraught with peril. If done in error it is the one of the few emulator instructions which can cause the machine to plunge completely off the deep end. If the RAM is not installed, control will go to the ROM location in AC1 (mod 1024)." This is in fact how a clever programmer could test for the presence of control RAM — though the AltoHW explicitly recommends using WRTRAM/RDRAM instead.

### 11.6 Emulator trap interpretation (AltoHW §8.6)

All unused opcodes except 77400B–77777B (which is used by Swat, the Alto debugger) and 61xxxB (where xxx ∈ [0, 377B]) transfer control to microlocation `RAMTRAP` with:
- The instruction in L
- The instruction cycled by 8 bits in R-register XREG
- The emulator's R-register PC counted one beyond the trapping instruction

```
RAMTRAP:    SWMODE, :TRAP;      ; switch into RAM, jump to RAM-side TRAP1 entry
...
TRAP:       ..., :TRAP1;        ; user-defined trap handler
```

If the machine has a control RAM, these instructions enter it at TRAP1 (= 37B in ROM microcode). If no RAM is present, the unimplemented opcode is handled per AltoHW §3.3 (raises an emulator-level exception).

### 11.7 Restrictions and caveats (AltoHW §8.8)

**Both RDRAM and WRTRAM cause the microprocessor's system clock to stop for one cycle.** This may yield unspecified results if the system clock is also stopped for some other reason (e.g. waiting for memory data). As a general rule, the system clock should run without hesitation during the microinstruction following a RDRAM or WRTRAM, except for the effect of the RDRAM or WRTRAM itself.

**Alto I phantom parity errors.** On Alto I, a memory reference followed too closely by a WRTRAM can cause a "phantom" parity error, because some Alto I memories cannot keep memory data good for two microinstruction times when the system clock suspends. Workaround: insert a NOP between the MAR← and the first MD reference.

**BUS=0 timing on RAM-related tasks.** "One cannot reliably test BUS=0 in the first instruction after a task switch into a RAM-related task when the bus data being tested is coming from the M register or one of the S registers." This is a timing hazard arising from the late determination of whether a RAM-related task is running.

---

## 12 — Microcode source format (`.mu` files) ✓ verified against AltoHW (Aug76) §9.1

Per AltoHW §9.1: "The microassembler which assembles microcode for the Alto is called **Mu**. By convention, microcode source files have the extension .MU, and binary files have the extension .MB. Standard Alto I ROM microcode versions will be called `AltoCodex.MU`; those for Alto II will be called `AltoIICodex.MU`."

Source lines look like:

```
DVT:    MAR_ L_ DASTART+1;
        CBA_ L, L_ 0;
        T_ MD;          CAUSE A VERTICAL FIELD INTERRUPT
        L_ NWW OR T;
```

Each line is one microinstruction (one cycle). The `_` is the assignment operator (`L_ DASTART+1` means `L ← DASTART+1`). Multiple comma-separated clauses produce side-effects in the same cycle: `MAR_ L_ DASTART+1` means MAR ← BUS, L ← BUS, BUS = DASTART+1 (a constant via constants-ROM lookup) — three side-effects on one cycle.

The `:LABEL` postfix selects a NEXT branch target. Labels declared with `!N,M,...` create jump tables (see the `;Cycle dispatch table:` block in `altoIIcode3.mu.txt:38`).

### 12.1 Standard fixed addresses (AltoHW §9.1)

For ROM/RAM compatibility, the following addresses are guaranteed in all standard Alto I microcode versions after 20, and all standard Alto II microcode versions:

| Address | Label    | Semantics |
|---------|----------|-----------|
| 20B     | `START`  | Beginning of emulator's main loop; starts a new emulated instruction |
| 37B     | `TRAP1`  | RAM location to which unfamiliar traps are sent; ROM location which implements trap sequence |
| 22B     | `RAMCYCX`| Fast cyclic shift subroutine |
| 105B    | `BLT`    | Block transfer subroutine |
| 106B    | `BLKS`   | Block store subroutine |
| 120B    | `MUL`    | Multiply subroutine |
| 121B    | `DIV`    | Divide subroutine |
| 124B    | `BITBLT` | BITBLT subroutine |
| 160B    | `L0`     | Cyclic shift dispatch table |

So **microcode RAM that interoperates with the standard ROM** must agree with the ROM at these addresses (i.e., RAM versions placed at these addresses must implement compatible behavior).

### 12.2 Magic-number microinstruction encoding in declarations

The `$NAME $LXXXXXX,YYYYYY,ZZZZZZ` syntax in `altoconsts23.mu.txt` encodes a base microinstruction template via three octal numbers. The format is documented inline. For the implementer: this is the original-format encoding the Mu assembler consumed. The RHDL implementation works against the unpacked struct directly; the Mu encoding is only relevant when consuming pre-assembled `.mb` binaries.

### 12.3 Loading microcode into RHDL

`src/microcode_loader.rs` parses the `.mu.txt` source files (or pre-packed `.mb` binaries from the bitsavers PROM dumps) and produces a `Vec<Microinstruction>` indexable by 10-bit MPC. The microcode RAM `src/microcode_rom.rs` is BRAM-shaped per the prettier-Verilog plan. The `assets/bitsavers/Alto_II_firmware/AltoPROMs_20070612/` directory has the raw PROM dumps for the Alto II hardware decode tables (DISPL, MADR, 2KCTL, XM51).

---

## 13 — Cross-reference: what's encoded where in `src/`

| Spec section | Source file | Implementation status |
|--------------|------------|------------------------|
| §2.1 Microinstruction layout | `src/isa.rs:1-235` | Phase 1 ✓ |
| §3.1 ALU functions | `src/isa.rs::AluFunction:34-74` + `src/alu.rs` (kernel) | Phase 1 ✓ (asterisk T-load semantics need check) |
| §3.2 Bus sources | `src/isa.rs::BusSource:81-104` | Phase 1 ✓ |
| §3.3 F1 functions (universal) | `src/isa.rs::F1Function:110-154` | Phase 1 ✓ |
| §3.3 F1 functions (per-task) | (not yet wired per-task) | Phase 5 |
| §3.4 F2 functions (universal) | `src/isa.rs::F2Function:160-208` | Phase 1 ✓ |
| §3.4 F2 functions (per-task) | (not yet wired per-task) | Phase 5 |
| §4.2 R-register file | `src/regfile.rs` | Phase 1 ✓ |
| §4.3 M and S registers | (not yet implemented) | Phase 5 (control RAM) |
| §4.4 Memory interface | `src/memory.rs` (256-word stub for Phase 3) | Phase 3 (full size: Phase 3.5) |
| §5 16-task system | `src/task_system.rs` (rhdl-rule kernel) | Phase 2 ✓ |
| §6 Emulator + IDISP | (not yet implemented) | Phase 5+ |
| §6.7 Interrupt system | (not yet implemented) | Phase 5+ |
| §7 Constants ROM | `src/constant_rom.rs` | Phase 1 (returns 0; full lookup Phase 4) |
| §8.2 Disk controller registers | `src/disk_controller.rs` | Phase 3 ✓ |
| §8.1 Diablo 31/44 disk model | `src/diablo_disk.rs` | Phase 3 ✓ (Diablo 31 only) |
| §8.4 Boot block loader | `src/disk_image_loader.rs` | Phase 3 ✓ |
| §8.5 Disk task-spec F1/F2/BS | (collapsed to `DiskWordTransfer` F2=8) | Phase 3.5 simplification; full per-task: Phase 5 |
| §9 Display subsystem | (framebuffer + DCB walk; no video timing) | Phase 4 partial; HDMI/VGA: Phase 6 |
| §10.1 Ethernet | (not yet implemented) | Phase 6 / optional |
| §10.7 MRT | partial in `src/task_system.rs` | Phase 5+ for full timer + cursor |
| §11 Control RAM (RDRAM/WRTRAM/SWMODE) | (not yet implemented) | Phase 5 |
| §11.4 Reset Mode Register | (not yet implemented) | Phase 5 |
| Microengine pipeline | `src/microengine.rs` (single-task) + `src/microcycle.rs` (shared kernel) | Phase 1 ✓ |
| Top-level chip composition | `src/alto_chip.rs` | Phase 3 ✓ |
| Microcode loader | `src/microcode_loader.rs` + `src/microcode_rom.rs` | Phase 1 ✓ |

The current implementation status (Phase 1+2+3.5 per the README) covers everything in §2 through §5 plus disk subsystem foundations. The Emulator task body (§6), the interrupt system (§6.7), the M/S registers + control RAM (§4.3, §11), and per-task F1/F2 dispatch (§3.3, §3.4) are the largest remaining pieces for the Phase 5 milestone.

---

## 14 — Implementation simplifications and divergences

The Alto is **legendarily** intricate. Several places where the RHDL implementation deliberately simplifies for tractability, with a note on what would be needed to match real hardware exactly:

**Per-task F1/F2 dispatch.** Real Alto hardware decodes F1 and F2 differently per running task — code 12 in DVT means something different than code 12 in KSEC. The RHDL implementation currently uses universal F1/F2 dispatch; per-task dispatch is on the Phase 5 polish list (per `tier-c-flagship-cores.md` §5.5 Phase 5). The canonical per-task tables are now documented in §3.3, §3.4, §8.5, §10.1.

**Disk word-DMA collapsed to single F2 code.** Real Alto's DMA spans multiple cycles via STROBE / KFER / STROBE2 codes (per §8.5); RHDL collapses this to one F2 (`DiskWordTransfer`, F2=8) for Phase 3.5 simplicity. Documented in `src/isa.rs:183-190`.

**Constants ROM is "drive zero" in Phase 1.** Real Alto's F1=`CONSTANT` indexes the constants ROM via RSEL+BS to pick a 16-bit value (and BS≥4 implicitly gates the constants ROM as a wired-AND mask, per §7). RHDL Phase 1 returns 0 from this code path; the full constants-ROM lookup is Phase 4-adjacent work (the binary table is in `assets/bitsavers/altoconsts23.mu.txt`).

**Memory size: 256 words for Phase 3.** Real Alto has 64 KW (Alto I) or 128 KW (Alto II). The RHDL implementation uses 256 words to keep iverilog testbench compilation tractable (per the CHANGELOG entry for PR #44 — the 2KW DFF array failed iverilog scanner buffers). Phase 3.5 swaps to `rhdl_fpga::core::ram::SyncBRAM` for proper Verilog memory emission and parameterized sizing.

**Display output buffer is a memory region, not a real video timing generator.** Real Alto has a video output stage with HSYNC/VSYNC, 50/100 ns bit clock, 16-word DDR buffer, etc. (per §9.3). RHDL stops at "framebuffer + DCB chain". The HDMI/VGA output is FPGA-target work (Phase 6+).

**Ethernet is not implemented.** EREST task is reserved but its body is on the Phase 6 / optional list. Real Alto Ethernet at 2.94 Mbps with the canonical task-specific F1/F2/BS codes (per §10.1) is interesting historical hardware but adds significant scope.

**M and S registers / control RAM** (per §4.3 and §11) **not implemented.** Phase 1 only models the 32-entry R-register file. Adding M+S-registers, the SWMODE / RDRAM / WRTRAM mechanism, and the Reset Mode Register is Phase 5 work alongside per-task F1/F2 dispatch.

**Bit numbering convention.** Real Alto uses MSB=0 numbering; the RHDL implementation in `src/isa.rs::Microinstruction::pack` uses Rust's natural LSB=0 packing. The two are isomorphic — the field identities are preserved — but anyone reading microcode source files (`.mu.txt`) needs to be aware that Alto's `IR[1-2]` corresponds to bits 13-14 in modern Rust convention, not bits 1-2. The microcode loader must perform this conversion.

**Hardware-encoded inverted bits.** Per §11.2, the actual hardware encoding of the microinstruction has F1[0], F2[0], LoadL all inverted relative to the field's logical value. The RHDL `pack` function implements the logical layout, not the hardware-inverted layout; if/when implementing RDRAM at the silicon-faithful level, the inversion must be applied on the microinstruction-bus side.

These simplifications are tracked in the CHANGELOG and are explicitly **documented** rather than silent — so the implementation behavior diverges in known places, not in surprising ones. The lockstep harness against ContrAlto (the gold reference) catches any unintended divergence.

---

## 15 — References

### Primary sources (in this repository)

- `assets/bitsavers/Alto_Hardware_Manual_Aug76.pdf` — canonical Alto Hardware Manual, 1976 edition
- `assets/bitsavers/AltoHWRef.part1.pdf` and `.part2.pdf` — Alto II Hardware Reference, ~1979
- `assets/bitsavers/AltoSubsystems_Oct79.pdf` — comprehensive subsystems reference
- `assets/bitsavers/AltoIICode3.mu.pdf` + `altoIIcode3.mu.txt` — Alto II microcode source (2230 lines)
- `assets/bitsavers/altocode24.mu.txt` — earlier Alto microcode (2030 lines)
- `assets/bitsavers/AltoConsts23.mu.pdf` + `altoconsts23.mu.txt` — definitive symbol table (370 lines)
- `assets/bitsavers/Alto_II_firmware/AltoPROMs_20070612/` — actual PROM binaries (DISPL, MADR, 2KCTL, XM51 etc.)
- `assets/contralto/Contralto2/` — ContrAlto cycle-accurate emulator source

### External references

- Lampson, Butler W. *Alto: A Personal Computer.* Computer Structures: Principles and Examples, 1979. — the canonical Alto paper
- Thacker, Charles P. et al. *Alto: A Personal Computer System Hardware Manual.* Xerox PARC, 1976. — the source of `Alto_Hardware_Manual_Aug76.pdf`
- Ken Shirriff's blog series on the Alto — http://www.righto.com/ — modern technical analysis with gate-level detail
- *Smalltalk-80: Bits of History, Words of Advice* (Krasner, ed.), 1983 — Smalltalk-side context
- Living Computers Museum / ContrAlto — https://github.com/livingcomputers/ContrAlto — gold-reference emulator

### Strategic context (in repo root)

- `tier-c-flagship-cores.md` §5 — the Tier-C plan section that scoped the rhdl-alto implementation
- `rule-architecture.md` — design plan for `rhdl-rule`, used heavily by `src/task_system.rs`
- `CHANGELOG.md` — PRs #43, #44 (and follow-ups) document the Phase 1+2+3 implementation milestones

---

## 16 — Maintenance

When extending this document:

1. **Cite the source file and page/section.** A claim without a citation is folklore. The bitsavers files in `assets/bitsavers/` are the ground truth; cite them by filename and section number from the original document. Tag verified sections with `✓ verified against AltoHW (Aug76) §X` so future agents can tell what's been reconciled.
2. **PDFs are the canonical authority.** When this digest disagrees with the original PDFs, the PDFs win — without exception, including any place where the implementation in `src/` happens to match the digest but contradicts the manual.
3. **Diff against the implementation.** When you write "F1 code 12 does X", check `src/isa.rs::F1Function` and confirm the implementation matches. If they disagree, fix the implementation (real hardware wins) or fix this doc (real hardware wins, but maybe phase X documented the simplification).
4. **Update §13 cross-references** if you add a new module to `src/`.
5. **Add a CHANGELOG entry** describing what new spec material landed and which sections were updated. Per CLAUDE.md §16, every documentation change to a load-bearing spec gets an entry.

This document is intended to be the single source of truth for "what is the Alto, in enough detail to implement it." It is not the place for implementation strategy (that's `tier-c-flagship-cores.md`) or test-result reports (that's the CHANGELOG). It is the canonical *specification* the code answers to.
