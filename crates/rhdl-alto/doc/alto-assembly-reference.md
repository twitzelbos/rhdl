# Alto Assembly Languages — A Verbose Tutorial Reference

*A practical reference for the two assembly languages of the Xerox Alto: **Mu**, the microassembler, and **MA** (a.k.a. **ALTOASM**), the Nova-derived macroassembler. Drawn from the canonical 1973–1979 PARC archive (under `crates/rhdl-alto/assets/`), reconciled against authentic period source code, and structured for engineers writing or reading Alto code today — particularly those extending the `rhdl-alto` crate.*

> **Source primacy.** This reference is a digest of the PARC documents in `assets/`. Where this digest disagrees with the primary documents, the primary documents win. Pagination cited as e.g. "ALTOASM p.3" refers to PDF page numbers in `assets/bitsavers/ALTOASM_Oct74.pdf`. The PARC documents in `assets/` are the canonical authority.

---

## Table of Contents

- [Part I — Introduction](#part-i--introduction)
  - [1.1 Two assembly languages, two layers](#11-two-assembly-languages-two-layers)
  - [1.2 The Alto's place in the abstraction stack](#12-the-altos-place-in-the-abstraction-stack)
  - [1.3 How this reference is organized](#13-how-this-reference-is-organized)
  - [1.4 Sources](#14-sources)
- [Part II — The Alto microengine in twelve pages](#part-ii--the-alto-microengine-in-twelve-pages)
  - [2.1 What sits below assembly](#21-what-sits-below-assembly)
  - [2.2 The 32-bit horizontal microinstruction](#22-the-32-bit-horizontal-microinstruction)
  - [2.3 The sixteen-task system](#23-the-sixteen-task-system)
  - [2.4 The Nova-derived ISA the Emulator runs](#24-the-nova-derived-isa-the-emulator-runs)
  - [2.5 How Mu and MA layer onto this](#25-how-mu-and-ma-layer-onto-this)
- [Part III — Mu, the microassembler](#part-iii--mu-the-microassembler)
  - [3.1 Source file format](#31-source-file-format-mu)
  - [3.2 The microinstruction word and its fields, in Mu syntax](#32-the-microinstruction-word-in-mu-syntax)
  - [3.3 Mu statements: assignments, side effects, and the comma](#33-mu-statements)
  - [3.4 Constants and the constants ROM](#34-constants-and-the-constants-rom)
  - [3.5 Symbols, labels, and the dollar-sign convention](#35-symbols-labels-and-)
  - [3.6 Branching, dispatch tables, and the `!N,M,…` idiom](#36-branching-dispatch-tables)
  - [3.7 Per-task vs universal F1 / F2 codes in Mu](#37-per-task-vs-universal-codes-in-mu)
  - [3.8 Worked example 1 — a sector-tick counter task](#38-mu-worked-example-1)
  - [3.9 Worked example 2 — a 4-state finite-state machine](#39-mu-worked-example-2)
  - [3.10 Worked example 3 — the Disk Sector body skeleton](#310-mu-worked-example-3)
  - [3.11 Magic-number microinstruction encoding](#311-magic-number-encoding)
  - [3.12 Loading microcode: the boot path and SIO](#312-loading-microcode)
  - [3.13 Common Mu idioms and patterns](#313-common-mu-idioms)
- [Part IV — MA, the macroassembler (ALTOASM)](#part-iv--ma-the-macroassembler-altoasm)
  - [4.1 What MA is, and is not](#41-what-ma-is-and-is-not)
  - [4.2 The Nova-derived instruction set, refresher](#42-the-nova-derived-instruction-set)
  - [4.3 Instruction formats and addressing modes](#43-instruction-formats-and-addressing)
  - [4.4 Pseudo-operations summary](#44-pseudo-operations-summary)
  - [4.5 Assembly regions: `.NREL`, `.SREL`, `.ZREL`](#45-assembly-regions)
  - [4.6 Externals: `.ENT`, `.EXTN`, `.EXTD`, `.BEXT`, `.BEXTZ`](#46-externals)
  - [4.7 File inclusion: `.GET` and `.GETNOLIST`](#47-file-inclusion)
  - [4.8 Text mode: `.TXTM`](#48-text-mode)
  - [4.9 Expressions and operators](#49-expressions-and-operators)
  - [4.10 The skip-pseudo-op family](#410-the-skip-pseudo-op-family)
  - [4.11 Alto-specific augmented opcodes](#411-alto-specific-augmented-opcodes)
  - [4.12 Worked example 1 — hello world with LDA/STA](#412-ma-worked-example-1)
  - [4.13 Worked example 2 — a loop with skip](#413-ma-worked-example-2)
  - [4.14 Worked example 3 — calling BITBLT](#414-ma-worked-example-3)
  - [4.15 Worked example 4 — SIO-driven disk transfer](#415-ma-worked-example-4)
  - [4.16 Reading the canonical sample: LevBcpl.asm](#416-reading-levbcpl-asm)
- [Part V — Where Mu and MA meet](#part-v--where-mu-and-ma-meet)
  - [5.1 IDISP: routing MA opcodes to Mu handlers](#51-idisp)
  - [5.2 JMPRAM, RDRAM, WRTRAM: MA reaching into microcode RAM](#52-jmpram-rdram-wrtram)
  - [5.3 Standard fixed addresses](#53-standard-fixed-addresses)
  - [5.4 Worked example — implementing a custom opcode end-to-end](#54-custom-opcode-end-to-end)
- [Part VI — BCPL and the broader toolchain](#part-vi--bcpl-and-the-broader-toolchain)
  - [6.1 The BCPL compilation pipeline](#61-bcpl-compilation-pipeline)
  - [6.2 The `.BR` relocatable binary format](#62-br-format)
  - [6.3 BLDR: loading `.BR` files](#63-bldr)
  - [6.4 BCPL ↔ MA calling convention](#64-bcpl-ma-calling-convention)
  - [6.5 Stack-frame layout from LevBcpl.asm](#65-stack-frame-layout)
- [Appendices](#appendices)
  - [A — Mu directive quick reference](#appendix-a--mu-directive-quick-reference)
  - [B — MA opcode quick reference (universal Nova ops)](#appendix-b--ma-opcode-quick-reference-universal-nova-ops)
  - [C — MA Alto-specific opcodes quick reference](#appendix-c--ma-alto-specific-opcodes-quick-reference)
  - [D — MA skip-pseudo-op cheatsheet](#appendix-d--ma-skip-pseudo-op-cheatsheet)
  - [E — Reserved page-1 locations](#appendix-e--reserved-page-1-locations)
  - [F — Sources and citations](#appendix-f--sources-and-citations)

---

# Part I — Introduction

## 1.1 Two assembly languages, two layers

The Xerox Alto has two assembly languages, used at two different layers of the machine.

**Mu** is the microassembler. It targets the 32-bit horizontal microinstruction word that the Alto's microengine fetches and executes every 170 ns. A Mu source file (`.mu`) declares constant-ROM values, symbol-table aliases, and a sequence of microinstructions — each line in the body is one microinstruction, one cycle, multiple side effects in parallel. The Alto's standard ROM microcode (`altoIIcode3.mu.txt`, 2 230 lines), the constants ROM source (`altoconsts23.mu.txt`, 370 lines), and replacement microcode for special tasks (e.g. `MesaROM.mu`, the Mesa accelerator microcode) are all written in Mu. Mu source files share one global namespace via the `#include` directive (called `.GET` in MA terms), with a canonical "constants" file every other source includes by convention.

**MA** is the macroassembler — formally **ALTOASM**, written by Ed McCreight in 1974 and documented in `ALTOASM_Oct74.pdf`. It targets the 16-bit Nova-style macroinstructions that run on the Emulator task — the lowest-priority hardware task in the microengine, which fetches one Nova-style instruction from main memory and executes it via 4 to 8 microinstructions of the Emulator microcode. MA is BCPL-compatible and produces `.BR` (relocatable binary) files that link with the BCPL loader (BLDR). The instruction-set semantics are documented in the **Alto Hardware Manual** §3 (`Alto_Hardware_Manual_Aug76.pdf`).

The two languages are not alternatives. They sit on top of each other:

```
  ┌──────────────────────────────────────────────────────────────┐
  │  BCPL programs (.3 files)         ←──  the systems language  │
  │     ↓                                                        │
  │  BCPL compiler                                               │
  │     ↓                                                        │
  │  .BR files                        ←──  relocatable binary    │
  │     ↑                                                        │
  │  MA hand-written .ASM (or .SR)    ←──  inline assembly       │
  │     ↑                                  here too              │
  │  Universal microengine                                       │
  │     ↑                                                        │
  │  Mu microcode .mu files           ←──  what makes the Nova   │
  │                                        ISA actually work     │
  └──────────────────────────────────────────────────────────────┘
```

A typical Alto application is BCPL with hand-rolled MA in the inner loops where you need Alto-specific opcodes (BITBLT, MUL, SIO) or the BCPL compiler's code is too slow. The whole stack runs on top of microcode that you, in principle, can also rewrite in Mu and load into the writable control RAM.

## 1.2 The Alto's place in the abstraction stack

The Alto (1973) is the canonical microcoded heterogeneous compute architecture. One ALU. One bus. One register file. Sixteen hardware tasks share that engine — display, disk, Ethernet, mouse, the Nova-style CPU emulator — each running in its own microcode loop, time-multiplexed by a priority encoder at the bottom of the microcycle. The two assembly languages reflect that layering:

- **Mu** is what you write to add a new hardware task, write a new opcode handler in the Emulator, implement BITBLT, change the disk sector microcode, or replace the Emulator wholesale (as the Mesa system did).
- **MA** is what you write to do anything *with* the existing Nova-derived instruction set: load and store memory, branch, call subroutines, talk to peripheral hardware via SIO, manipulate bitmaps via BITBLT.

A reader who knows only modern x86, ARM, or RISC-V will find both layers familiar in shape but different in detail. Mu is unusually horizontal — a single instruction can perform an ALU operation, write a register, modify the next-instruction address, initiate a memory fetch, and yield the cycle to a higher-priority task, all in the same 170 ns. MA looks like Nova because it *is* Nova, with a small set of Alto-specific extensions; the Nova was Data General's DOS minicomputer ISA, and the Alto's ALTOASM is "patterned after that of the Data General NOVA DOS assembler" (ALTOASM p.1) with deletions, additions, and some incompatibilities.

## 1.3 How this reference is organized

Six parts plus six appendices.

- **Part II** is a twelve-page refresher on the Alto microengine. It is short by design — the canonical architectural reference is `alto-processor-and-microcode-spec.md` in this same crate. The summary here is calibrated to what an assembly-language author needs to know, no more.
- **Part III** is the Mu reference. Source-file format, the microinstruction word, statements, constants, labels, dispatch tables, per-task codes, magic-number encoding, three worked examples, and the canonical Mu idioms.
- **Part IV** is the MA reference. The instruction-set refresher, instruction formats, every pseudo-op in ALTOASM, the assembly-region model, externals, file inclusion, expressions, the skip-pseudo-op family, the Alto-specific augmented opcodes, four worked examples, and a guided read of the canonical period source `LevBcpl.asm`.
- **Part V** is the meeting point. Three mechanisms cross between Mu and MA: `IDISP` routes MA opcode bits into Mu microcode dispatch; `JMPRAM` / `RDRAM` / `WRTRAM` let MA programs reach into microcode RAM; and a small set of fixed microcode addresses (`START`, `TRAP1`, `BLT`, `BITBLT`, …) is the shared interface contract.
- **Part VI** is BCPL and the larger toolchain — the language MA most often cooperates with, the `.BR` relocatable binary format both produce, the BLDR loader, the BCPL ↔ MA calling convention, and the stack-frame layout you see in real period code.
- **Appendices** are quick-reference tables: every Mu directive, every MA opcode, every skip pseudo-op, the canonical reserved page-1 locations, and a citations index.

If you are writing your first Mu file, jump to §3.1 and read in order through §3.13. If you are writing your first MA file, §4.1 through §4.16. If you are extending the Alto's instruction set with a new opcode, §5 is the load-bearing chapter. If you are modernizing BCPL code or trying to read period source, Part VI.

## 1.4 Sources

This reference is built from the documents and source files in `crates/rhdl-alto/assets/`. Each citation in the body refers to one of:

- **AltoHW** — `assets/bitsavers/Alto_Hardware_Manual_Aug76.pdf`. Charles Thacker, Ed McCreight, Butler Lampson, Robert Sproull, Dave Boggs et al., *Alto Hardware Manual*, Aug 1976. The canonical instruction-set and microengine reference. Cited as "AltoHW §X" with section numbers from the manual itself.
- **AltoHW79** — `assets/bitsavers/Alto_Hardware_Manual_May79.pdf`. The May 1979 revision. Adds Alto II details. Cited only where it differs from the 1976 edition.
- **AltoHWRef** — `assets/bitsavers/AltoHWRef.part1.pdf` and `.part2.pdf`. Alto II Hardware Reference, ~1979.
- **ALTOASM** — `assets/bitsavers/ALTOASM_Oct74.pdf`. Ed McCreight, *ALTOASM*, Oct 1974. The canonical MA reference. 6 pages. Cited as "ALTOASM p.N".
- **ASM.TTY** — `assets/altosource/asm.tty`. Plain-text mirror of the same ALTOASM manual, useful for grep.
- **AltoSubsystems** — `assets/bitsavers/AltoSubsystems_Oct79.pdf`. Subsystems reference (disk, display, Ethernet, mouse, keyboard).
- **AltoOS** — `assets/bitsavers/Alto_Operating_System_Reference_Manual_Jun75.pdf`. Alto OS reference, June 1975.
- **BCPL Ref** — `assets/bitsavers/BCPL_Reference_Manual_Sep79.pdf`. James Curry & PARC staff, *BCPL Reference Manual*, Sep 1979.
- **CompBCPL** — `assets/bitsavers/Compiling_BCPL_Programs_Aug73.pdf`. James Curry, *Compiling BCPL Programs*, Aug 1973. Five-page memo on the BCPL toolchain.
- **AltoUsers** — `assets/bitsavers/Alto_Users_Handbook_Sep79.pdf`. *Alto Users' Handbook*, Sep 1979.
- **CSL-79-11** — `assets/bitsavers/CSL-79-11_Alto_A_Personal_Computer.pdf`. Thacker, McCreight, Lampson, Sproull, Boggs, *Alto: A Personal Computer*, CSL Tech Report 79-11.
- **AltoIICode3** — `assets/bitsavers/altoIIcode3.mu.txt` (and `AltoIICode3.mu.pdf`). The canonical Alto II ROM microcode — 2 230 lines of real Mu source.
- **AltoCode24** — `assets/bitsavers/altocode24.mu.txt`. Earlier Alto microcode revision.
- **AltoConsts** — `assets/bitsavers/altoconsts23.mu.txt` (and `AltoConsts23.mu.pdf`). The canonical 370-line constants and symbols file.
- **LevBcpl** — `assets/altosource/LevBcpl.asm.html`. The BCPL runtime hand-coded in MA, 1979 vintage. ~440 lines of authentic period MA source.
- **rhdl-alto code** — `crates/rhdl-alto/src/`. The RHDL Rust implementation, particularly `isa.rs`, `microcode_loader.rs`, `microengine.rs`, `task_system.rs`, `disk_controller.rs`, `diablo_disk.rs`, `alto_chip.rs`.

---

# Part II — The Alto microengine in twelve pages

This part is a compressed refresher. The full architectural reference is `crates/rhdl-alto/alto-processor-and-microcode-spec.md` (about 50 pages). What follows is the minimum a Mu or MA author needs.

## 2.1 What sits below assembly

The Alto's compute fabric is one **universal microengine** running at 170 ns per cycle (≈5.88 MHz). Per AltoHW §1.0 and §2.0:

- One **16-bit ALU** (an SN74181, of which 16 of the 48 functions are used) takes operands `(BUS, T)` and produces a 16-bit result `L`.
- One **shared 16-bit BUS**. In any cycle the BUS is driven by exactly one of eight bus-source codes — but, because the bus is wired-AND, multiple sources can in fact be gated on simultaneously, with the result being their bitwise AND. This is the basis of the constants-ROM masking facility (AltoHW §2.2).
- One **R-register file** of 32 × 16 bits, addressed by a 5-bit RSEL field.
- Optional **M register and 31 S registers** (control RAM card). M is the analog of L; the 31 S registers are the analog of R, but only available to RAM-related tasks. AltoHW §8.7.
- Two pipeline-staging registers, **T and L**. T is the ALU's "B" operand, loaded from the bus or, for certain ALU functions, from the ALU output. L is the ALU's result register.
- Memory: **64K 16-bit words of 850 ns DRAM**. Accessed via MAR (memory address register) and MD (memory data register). One-cycle minimum between issuing `MAR ← BUS` and reading `MD`.
- A **constants ROM** — 256 × 16 — gated onto the bus by F1=7, F2=7, or any BS≥4. Doubles as a wired-AND mask provider for the `←MOUSE` and `←DISP` bus sources.

The instruction set the microengine executes is the 32-bit horizontal microinstruction (§2.2). The control flow that decides *which* microinstruction to execute next on each cycle is the 16-task arbiter (§2.3). The set of macroinstructions runnable on top of the microengine is the Nova-derived ISA (§2.4).

## 2.2 The 32-bit horizontal microinstruction

Per AltoHW §2.1, every 170 ns the microengine fetches a 32-bit microinstruction and executes it. The bit numbering follows the Alto convention of MSB = bit 0:

```
 0    4 5    8 9  11 12  15 16  19 20 21 22                     31
┌──────┬──────┬─────┬─────┬─────┬──┬──┬───────────────────────────┐
│ RSEL │ ALUF │ BS  │ F1  │ F2  │L │T │           NEXT            │
│  5b  │  4b  │ 3b  │ 4b  │ 4b  │L │T │          10 bits          │
└──────┴──────┴─────┴─────┴─────┴──┴──┴───────────────────────────┘
  MSB                                                          LSB
```

Eight fields:

- **RSEL[0:4]** — R-register select; also forms the high 5 bits of the 8-bit constants-ROM address when constants are gated.
- **ALUF[5:8]** — ALU function; one of 16 codes (out of 48 the SN74181 can do).
- **BS[9:11]** — Bus Data Source; one of 8 sources.
- **F1[12:15]** — auxiliary function 1.
- **F2[16:19]** — auxiliary function 2.
- **LL[20]** — Load L: if 1, L ← ALU result at cycle edge.
- **LT[21]** — Load T: if 1, T ← BUS (or ALU output for "starred" ALU functions) at cycle edge.
- **NEXT[22:31]** — 10-bit next-microinstruction address (subject to F1/F2 modifiers).

Each microinstruction is one cycle and produces *several side effects in parallel*: the ALU computes, the bus is driven, T or L may be loaded, R[RSEL] may be loaded, MAR or memory may be written, NEXT may be modified by F2 dispatch. This horizontal-encoded parallelism is what makes Mu different in feel from a typical RISC: every line of Mu source is doing four to seven things at once.

The full per-field tables (ALU functions; bus sources; F1; F2; the universal-vs-per-task split) are in §3.2.

## 2.3 The sixteen-task system

Per AltoHW §2.4: "Control of the Alto microprocessor is shared among 16 'tasks' arranged in a priority order. The tasks are numbered 0 to 15: 0 is the lowest priority task and 15 is the highest."

Each task has its own MPC (microprogram counter) — there are 16 MPCs, one per task, each 12 bits. Other state (T, L, R, M, MD, MAR, etc.) is *shared* between tasks, which means a task that yields the engine must do so at a quiescent point — no in-flight memory access, nothing in T or L it cares about preserving. The arbiter is a priority encoder over the 16 wakeup signals; on each microcycle, the highest-priority woken task whose MPC is at a quiescent point gets to execute its next microinstruction.

The standard Alto II task assignments (see `task_system.rs` and AltoHW §3.0):

| # | Mnemonic | Purpose | Priority |
|---|---|---|---|
| 0 | `EMU` | Nova ISA emulator | lowest |
| 4 | `KSEC` | Disk Sector — runs once per disk sector mark | medium |
| 7 | `EREST` | Ethernet receive/transmit | medium |
| 8 | `MRT` | Memory Refresh (DRAM refresh + 38 µs interval timer + EIA UART) | high |
| 9 | `DWT` | Display Word Task — once per word output to display | high |
| 10 | `CURT` | Cursor task | high |
| 11 | `DHT` | Display Horizontal — once per scan line | high |
| 12 | `DVT` | Display Vertical — once per field | high |
| 14 | `KWDX` | Disk Word — once per disk word strobe during transfer | very high |
| 15 | `PART` | Parity error handler | highest |

The non-listed slots (1, 2, 3, 5, 6, 13) are reserved.

Two F1 codes are central to the task system: **F1=2 (`TASK`)** yields the engine — the microengine looks at the wakeup vector and switches to the highest-priority woken task. **F1=3 (`BLOCK`)** *signals the device interface associated with the running task to drop its wakeup* — note that BLOCK is a convention implemented by each device interface, not by the microengine itself (AltoHW §2.4: "*This function is not accomplished by the Alto microprocessor, but rather by the individual device interfaces*").

## 2.4 The Nova-derived ISA the Emulator runs

The Emulator task (task 0) executes a Nova-derived 16-bit instruction set (AltoHW §3.0). It differs from the Data General Nova in four ways (AltoHW §3.1):

1. **16-bit addresses** (Nova has 15). Multi-level indirection is not possible. All 16 bits of an indexing register are significant.
2. **No auto-index locations.** The Nova reserves locations 16–31 for auto-increment indirect; the Alto does not.
3. **Different interrupt system.** AltoHW §3.2 covers the Alto's NWW/WW/INTVEC mechanism (15 vectored channels).
4. **No I/O class instructions.** Nova's DOA/IOP/DIO/etc. are *not implemented*. Instead the Alto has an "augmented instruction set" — BITBLT, MUL, DIV, BLT, BLKS, SIO, JMPRAM, RDRAM, WTRM, etc. — which use otherwise-unused opcodes to access Alto-specific microcode handlers.

Emulator state: PC (16-bit program counter), AC0 / AC1 / AC2 / AC3 (four 16-bit accumulators), C (1-bit carry).

Four instruction-format groups (AltoHW §3.1, Figure 3):

- **M-Group** — `LDA` (load accumulator), `STA` (store accumulator). Format: `[0|MFunc|DestAC|I|X|DISP]`.
- **J-Group** — `JMP`, `JSR` (jump and save return), `ISZ` (increment-and-skip-if-zero), `DSZ`. Format: `[000|JFunc|I|X|DISP]`.
- **A-Group** — eight ALU operations (COM, NEG, MOV, INC, ADC, SUB, ADD, AND) on (SrcAC, DestAC) with optional shift, carry override, no-load, and skip. Format: `[1|SrcAC|DestAC|AFunc|SH|CY|NL|SK]`.
- **S-Group** — Alto-specific augmented opcodes. Format: `[011|...]`.

Effective-address calculation (M-group, J-group):

```
SExtend(x)  =  if x ≥ 200B then x | 177400B else x      // 8-bit sign extension

E()  =  let E1 = (
            if X = 0 then DISP                          // page 0 addressing
            elseif X = 1 then SExtend(DISP) + PC        // PC-relative
            elseif X = 2 then SExtend(DISP) + AC2       // base register AC2
            elseif X = 3 then SExtend(DISP) + AC3       // base register AC3
        ) in
        if I = 1 then memory[E1] else E1                // single-level indirection
```

The Emulator microcode that fetches, decodes, and executes one of these instructions per macroinstruction lives in `altoIIcode3.mu.txt` starting at the `START` label (microcode address 20B).

## 2.5 How Mu and MA layer onto this

A short tour of who speaks to whom.

**MA → microengine, indirectly.** When you write `LDA 0 FOO` in MA, the assembler emits a 16-bit Nova LDA instruction. At runtime the Emulator microcode (Mu) fetches that instruction word from main memory, decodes it, computes the effective address, performs the memory load, and writes AC0. From MA's perspective the operation is atomic; the multi-cycle microcode underneath is invisible. This is the principal use of MA: *write Nova-style code without thinking about microcode*.

**Mu → microengine, directly.** A Mu source file is loaded into the microcode ROM (or RAM, on Alto II), and the microengine fetches its instructions directly. Mu authors write the Emulator's instruction-decode loop, the Disk Sector task body, the Display Word task body, and so on. *This is the only way to add new hardware tasks or modify existing ones.*

**MA → microcode RAM.** Three macroinstructions in MA's augmented set let the Emulator program reach into microcode RAM: `RDRAM` reads, `WRTRAM` writes, `JMPRAM` transfers control of the Emulator task to a microcode-RAM location. This is how user-loadable microcode (e.g. Mesa's accelerator microcode) is installed.

**Mu → MA's instruction stream.** The Emulator microcode contains an `IDISP` (Instruction Dispatch) F2 code that ORs the high bits of IR (the just-fetched MA instruction) into NEXT, routing to the per-opcode microcode handler. This is the mechanism by which "the Emulator implements the Nova-derived ISA": it's a 16-way dispatch over a 256×4 PROM (AltoHW §3.5) that picks the microcode handler for whatever MA opcode just got fetched.

Part V covers these crossing points in more detail. For now: Mu is what makes the Nova-style ISA work at all; MA is what you write *given* that the ISA works.

---

# Part III — Mu, the microassembler

## 3.1 Source file format `.mu`

A Mu source file is plain ASCII text with no formal grammar imposed by syntax rules — the Mu assembler is permissive about whitespace, line breaks, and comments. Conventionally, a Mu file consists of:

1. **A title comment** — first line, beginning with `;`.
2. **A `#include` of the canonical constants file** — typically `altoconsts23.mu`. Per AltoHW §9.1: "the file `AltoConstsx.MU` will be maintained (the `x` corresponding to the latest AltoCode`x`) containing definitions and constants for both Alto I and Alto II. These can be logically incorporated into other microcode assemblies via the 'include' feature of Mu (`#ALTOCONSTSx.MU;`)."
3. **Symbol declarations.** R-register names, constant names, predeclared microinstruction templates (the magic-number form, see §3.11).
4. **The microcode body** — one microinstruction per line, terminated by `;`.
5. **Label declarations** for external entry points.

A minimal example (in synthetic form, not period-correct, for orientation):

```mu
;Demo task body for documentation
#ALTOCONSTSX.MU;

$MyTaskStart  $L060000,000000,000000;     ; predefined task entry template
$MyState      $R20;                        ; an R register alias

MyTaskStart:
  L_ MyState+1, MyState_ L, TASK, :MyTaskStart;
```

Real Mu source — like `altoIIcode3.mu.txt` — has thousands of lines structured this way. Comments use `;` to end-of-line. Whitespace is liberal; the only mandatory delimiters are `,` (within a microinstruction) and `;` (between microinstructions or after a declaration).

**File extensions.** `.mu` (or `.MU`) for Mu source. `.MB` for assembled binary. The repository copies in `assets/bitsavers/` use the `.mu.txt` extension because they are plain-text mirrors of the canonical `.MU` files (which on the Alto file system used the bare `.mu` extension).

## 3.2 The microinstruction word in Mu syntax

A Mu microinstruction line maps to the 32-bit word of §2.2. The Mu assembler does not, in general, expect you to write all eight fields explicitly — instead it provides a syntactic shortcut for each field, and uses a small table-driven decoder to combine them into the 32-bit word.

The eight fields, in Mu syntax:

| Field | Bit positions | Mu syntactic form |
|---|---|---|
| RSEL | 0–4 (5 b) | The R-register name (e.g. `T`, `L`, `MAR`, or any user-named register — see §3.5). Or, when not loading or reading an R, the assembler sets RSEL to a constants-ROM index used by the BS/F1/F2 codes. |
| ALUF | 5–8 (4 b) | Implicit from the right-hand side of an `L_` (L-load) assignment: e.g. `L_ T+R37` selects ALUF = "BUS+T". |
| BS | 9–11 (3 b) | Implicit from the right-hand side of any non-L assignment, or from the LHS source of an L-load. E.g. `L_ MD` selects `BS=5 (←MD)`. |
| F1 | 12–15 (4 b) | Either implicit (e.g. `TASK` is F1=2; `MAR_` is F1=1) or explicit (e.g. `LRSH1` for left-shift). |
| F2 | 16–19 (4 b) | Either implicit (e.g. `MD_` is F2=6) or explicit (e.g. `BUS=0` is F2=1). |
| LL (Load L) | 20 (1 b) | Set whenever a microinstruction has an `L_` clause. |
| LT (Load T) | 21 (1 b) | Set whenever a microinstruction has a `T_` clause. |
| NEXT | 22–31 (10 b) | The label after `:` at the end of the line, e.g. `:MYLOOP`. |

So a microinstruction like:

```
L_ T+R37, T_ L, MAR_ R37, :NEXT;
```

decodes to: ALUF computes `T+R37`, BS is the implicit register read from R37, F1 is `MAR←` (F1=1), LL=1 (the `L_` prefix), LT=1 (the `T_` prefix), RSEL points at R37, NEXT points at the address of label `NEXT`.

The Mu assembler's job is to pack these clauses into the 32-bit word. **Conflicts** — e.g. trying to load L from two different ALU computations in the same line, or asking for two F1 codes at once — are detected and reported as assembly errors. **Implicit-conflict resolution** handles common cases: you can write `L_ MD` and `L_ MD, MAR_ R37` and the Mu assembler combines them coherently; you cannot write `L_ MD, L_ R37` because they pick incompatible BS values.

Within a single line, **clauses are comma-separated** and can appear in any order. Whitespace around commas is optional. `:LABEL` is always the last clause; it sets NEXT.

## 3.3 Mu statements

A Mu statement (microinstruction) is a sequence of **clauses** separated by commas, terminated by `;`. Each clause sets one or more bits of the 32-bit microinstruction word. Common clause forms:

| Clause form | What it sets | Example |
|---|---|---|
| `L_ <expr>` | LL=1, ALUF and BS implicit from `<expr>` | `L_ T+R37` |
| `T_ <bus>` | LT=1, BS picks the bus source | `T_ MD` |
| `MAR_ <expr>` | F1=1 (MAR←), expr drives the bus | `MAR_ L` |
| `MD_ <expr>` | F2=6 (MD←), expr drives the bus | `MD_ T` |
| `<R>_ <bus>` | RSEL=<R>, BS=LOAD R←, value goes into R[<R>] | `R37_ L` |
| `TASK` | F1=2 | `TASK` |
| `BLOCK` | F1=3 | `BLOCK` |
| `LLSH1` / `LRSH1` / `LLCY8` | F1=4/5/6 | `L_ L LLSH1` |
| `BUS=0`, `SH=0`, `SH<0`, `BUS` | F2=1/3/2/4 — NEXT-modifying tests | `L_ T-1, BUS=0` |
| `:LABEL` | NEXT = address-of-label | `:MYLOOP` |
| `←MD`, `←MOUSE`, `←KSTAT`, `←KDATA`, `←DISP`, `←CONSTANT` | BS source | `T_ ←MD` (often abbreviated `T_ MD`) |
| `task-specific F1/F2 code` | depends on task — see §3.7 | `KFER` (F2=15B in disk tasks) |

The crucial idea: **multiple clauses execute in parallel within a single cycle.** The famous example from `altoIIcode3.mu.txt`:

```mu
DVT:    MAR_ L_ DASTART+1;
        CBA_ L, L_ 0;
        T_ MD;          ;CAUSE A VERTICAL FIELD INTERRUPT
        L_ NWW OR T;
        ...
```

Read line 1 carefully: `MAR_ L_ DASTART+1` is *three* simultaneous side effects:

1. The bus is driven from the constants ROM with the value of `DASTART+1` (because `DASTART+1` is an alias for a constant-ROM cell).
2. F1=1 (`MAR←`) latches the bus into MAR, initiating a memory reference.
3. LL=1, with ALUF set to "pass-bus", latches the bus into L.

So in 170 ns, the cycle starts a memory fetch from address `DASTART+1`, *and* leaves L holding `DASTART+1` for the next cycle to use. That is what "horizontal microcode" means: the parallelism is encoded in the instruction word itself.

## 3.4 Constants and the constants ROM

The constants ROM is a 256 × 16 PROM gated onto the bus by F1=7, F2=7, or BS≥4 (AltoHW §2.2). The 8-bit constants-ROM address is `{RSELECT[0:4], BS[0:2]}` — RSEL contributes the high 5 bits, BS the low 3 bits.

In Mu source, constants are declared in `altoconsts23.mu.txt` with the `$NAME $value;` form:

```mu
$ALLONES   $L177777,000000,000000;       ; predeclared as a magic-number form
$DASTART   $420;                          ; a pure value
$MOUSELOC  $424;
$CURLOC    $426;
$WWLOC     $452;
$NWW       $R4;                           ; an R-register alias
$T0        $R30;                          ; another R-register alias
```

The `$L...` form (three octal numbers) is the magic-number microinstruction encoding (§3.11). The `$<value>` form is a plain integer constant (default octal).

Inside a microinstruction, naming a constant (e.g. `DASTART+1`) causes the Mu assembler to:

1. Allocate (or reuse) a constants-ROM slot containing that 16-bit value.
2. Emit a microinstruction in which BS is set to point into the constants ROM (typically by setting BS≥4 to a well-known constants-only value, or by setting F1=7 explicitly), and RSEL is set to the constants-ROM-row index.

This means constants are *not* free — they consume one of the 256 PROM slots. In practice, the canonical constants file (`altoconsts23.mu.txt`) pre-allocates the most commonly used values (`$0`, `$1`, `$ALLONES`, `$BIAS`, `$MASK4`, etc.) so that user microcode rarely needs to add new constants.

**Wired-AND masking** (AltoHW §2.2) is the most subtle constants-ROM feature. When BS is set to one of the BS≥4 sources (`←MD`, `←MOUSE`, `←DISP`, etc.), the constants ROM *also* gates onto the bus simultaneously, and the bus performs a wired-AND. So `T_ MOUSE AND MASK4` (where `MASK4` is a constant-ROM value at the right index) reads the mouse register, AND-s it with the mask, and stores the result in T — all in one cycle, using zero ALU operations. This is the dominant pattern for extracting bit-fields from `←MOUSE` and `←DISP`.

## 3.5 Symbols, labels, and `$`

Mu has three name-spaces:

- **Symbols starting with `$`** — declarations, e.g. `$NAME $value`. These declare a constant, an R-register alias, or a microinstruction template. They go into the constants ROM, the symbol table, or the prefab-microinstruction table at assembly time.
- **Plain identifiers used as labels** — followed by `:`. These bind a microinstruction address (in the microcode ROM/RAM) to the identifier. Labels are referenced via `:LABEL` clauses in the body of microinstructions, which set NEXT.
- **Predefined names** — opcodes (`MAR_`, `MD_`, `TASK`, etc.), R-register names (`L`, `T`, `M`, `R0` … `R37`), and the special directive markers (`!`, `#`, etc.).

Conventional R-register aliases (from `altoconsts23.mu.txt`):

| Alias | R index | Used by |
|---|---|---|
| `NWW` | R4 | Interrupt system |
| `CURX` | R20 | Display cursor X |
| `CURDATA` | R21 | Display cursor data |
| `CBA` | R22 | Display Control Block Address |
| `AECL` | R23 | Display task auxiliary |
| `SLC` | R24 | Display Scan Line Count |
| `MTEMP` | R25 | Public temporary |
| `HTAB` | R26 | Display horizontal-tab register |
| `YPOS` | R27 | Display Y position |
| `DWA` | R30 | Display Word Address |
| `R37` | R37 | MRT, interval timer, EIA |
| `M` | R40 | M register alias |

These aliases are part of the canonical constants file. *Always include the canonical constants file in your Mu source* — by convention all of `altoIIcode3.mu.txt` and equivalent files do `#ALTOCONSTSX.MU;` at the top.

## 3.6 Branching, dispatch tables, and the `!N,M,…` idiom

Mu has no `if`/`else` syntax. Branching happens by *modifying NEXT* — the 10-bit address of the next microinstruction — via F2 codes that test conditions and OR a 1 into one of NEXT's low bits. Per AltoHW §2.1:

| F2 | Effect on NEXT |
|---|---|
| 0 | (no modification) |
| 1 | `BUS=0`: NEXT or= 1 if BUS == 0 |
| 2 | `SH<0`: NEXT or= 1 if shifter output < 0 |
| 3 | `SH=0`: NEXT or= 1 if shifter output == 0 |
| 4 | `BUS`: NEXT or= BUS[6:15] (a computed branch on low 10 bits of the bus) |
| 5 | `ALUCY`: NEXT or= last-loaded-L's ALU carry-out |
| 6 | `MD←` (memory write — not a NEXT modifier) |
| 7 | `←CONSTANT` — constants gating |

So a simple "if BUS == 0 goto LABEL_A else goto LABEL_B" is encoded as: pick LABEL_A and LABEL_B at addresses that differ only in the low bit; set NEXT to LABEL_A; assert F2=BUS=0; the cycle either keeps NEXT pointing at LABEL_A or OR-s a 1 to make it LABEL_B.

The Mu assembler typically handles this for you via the **`!N,M,…` jump-table directive**. Per `altoIIcode3.mu.txt:38`:

```mu
;Cycle dispatch table:
!37,20,L0,L1,L2,L3,L4,L5,L6,L7,L8,R7,R6,R5,R4,R3X,R2X,R1X;
```

The `!` directive declares a list of labels at fixed offsets from a base address. The 16 labels here are placed at address pattern `xxxx00000`, `xxxx00001`, …, `xxxx01111` (with consecutive low 4 bits and shared high bits). The Emulator's IDISP F2 code then OR-s 4 bits of IR into NEXT to dispatch to one of these handlers in one cycle.

For more complex dispatches, several `!N,M,…` directives can be stacked, providing 32-, 64-, or 256-way dispatches. The IDISP PROM (256×4) gives the Emulator a 16-way dispatch (see AltoHW §3.5 and §5.1 of this reference).

**A simple if/else worked by hand:**

```mu
; Test if T == 0; if so go to ZERO_HANDLER, else FALL_THROUGH.
!1,2,FALL_THROUGH,ZERO_HANDLER;       ; declares two labels at .+0 and .+1
       L_ T, BUS=0, :FALL_THROUGH;     ; tests BUS (= T), sets NEXT[0] if BUS==0
FALL_THROUGH:
       ...                              ; reached if T != 0
ZERO_HANDLER:
       ...                              ; reached if T == 0
```

The `!1,...` declares the two labels at addresses .+0 and .+1; the microinstruction sets NEXT to FALL_THROUGH and asserts F2=BUS=0; the cycle's runtime evaluation either leaves NEXT at FALL_THROUGH or OR-s in a 1, jumping to ZERO_HANDLER.

This branching style is unusual, but it composes well: a single microinstruction can test a condition *and* update state *and* dispatch to a multi-way next-instruction, all in 170 ns.

## 3.7 Per-task vs universal F1 / F2 codes in Mu

Per AltoHW §2.1: "The first eight conditions specified by each field are interpreted identically by all tasks (except BLOCK), but the interpretation of the second eight depends on the active task."

So **F1 codes 0–7 and F2 codes 0–7 are universal**; **F1 codes 10B–17B and F2 codes 10B–17B are per-task**. The Mu assembler does not validate per-task usage — it is the *programmer's* responsibility to use only the codes valid for the running task.

**Universal F1** (codes 0–7):

| F1 | Mnemonic | Effect |
|---|---|---|
| 0 | (no-op) | nothing |
| 1 | `MAR←` | MAR ← ALU; start memory reference |
| 2 | `TASK` | yield to higher-priority task |
| 3 | `BLOCK` | device clears its own wakeup |
| 4 | `←L LSH 1` | L ← L << 1 |
| 5 | `←L RSH 1` | L ← L >> 1 |
| 6 | `←L LCY 8` | L ← L rotated left 8 bits |
| 7 | `←CONSTANT` | gate constants ROM onto bus |

**Universal F2** (codes 0–7):

| F2 | Mnemonic | Effect |
|---|---|---|
| 0 | (no-op) | nothing |
| 1 | `BUS=0` | NEXT or= 1 if bus == 0 |
| 2 | `SH<0` | NEXT or= 1 if shifter output < 0 |
| 3 | `SH=0` | NEXT or= 1 if shifter output == 0 |
| 4 | `BUS` | NEXT or= BUS[6:15] |
| 5 | `ALUCY` | NEXT or= last-loaded-L's ALU carry-out |
| 6 | `MD←` | memory[MAR] ← BUS |
| 7 | `←CONSTANT` | same as F1=7 |

**Per-task F1** (codes 10B–17B): meaning depends on running task. Examples — Emulator: `SWMODE` (10B, switch RAM/ROM mode), `WRTRAM` (12B, write microcode RAM), `RDRAM` (13B, read microcode RAM), `RMR` (14B, reset mode register), `STARTF` (17B, set Ethernet/IO ICmd/OCmd flip-flops). Disk Sector & Word: `STROBE` (11B), `KSTAT←` (12B), `INCRECNO` (13B), `CLRSTAT` (14B), `KCOM←` (15B), `KADR←` (16B), `KDATA←` (17B). Ethernet: `EILFct` (13B), `EPFct` (14B), `EWFct` (15B).

**Per-task F2** (codes 10B–17B): similar pattern. Examples — Emulator: `BUSODD` (10B), `MAGIC` (11B), `DNS` (12B), `ACDEST` (13B), `IR←` (14B), `IDISP` (15B), `←ACSOURCE` (16B). Display tasks: `EVENFIELD` (10B in DVT/DHT), `SETMODE` (11B in DHT), `←DDR` (10B in DWT). Disk: `INIT` (10B), `RWC` (11B), `RECNO` (12B), `XFRDAT` (13B), `SWRNRDY` (14B), `NFER` (15B), `STROBON` (16B). Ethernet: `EODFct` (10B), `EOSFct` (11B), `ERBFct` (12B), `EEFct` (13B), `EBFct` (14B), `ECBFct` (15B), `EISFct` (16B).

The complete per-task tables are in `alto-processor-and-microcode-spec.md` §3.3 / §3.4, with citations to AltoHW §3.5, §6.0, §7.3, etc.

## 3.8 Mu worked example 1

A trivial sector-tick counter task. Wakes once per sector mark, increments a counter in R-memory, blocks until the next mark. Synthetic but representative.

```mu
;Demo Sector Tick task — increments a counter every sector mark.
#ALTOCONSTSX.MU;

$TickCount   $R20;           ; alias R20 as our tick counter

;Reset entry point. Each task's initial MPC = task number, so this
;label must be placed at MPC=4 (the Disk Sector task slot).
!1,1,STInit;                 ; place STInit at base+0
STInit:
  L_ 0, TickCount_ L, BLOCK, :STInit;     ; zero the counter, BLOCK to wait
                                          ; for the device wakeup; on wakeup,
                                          ; come back here

; When the device wakes us (sector mark fired), STInit re-runs:
;   L_ 0           — load 0 into L
;   TickCount_ L   — store L into R[TickCount] (R20)
;   BLOCK          — clear the disk's wakeup latch
;   :STInit        — set NEXT = STInit (so we come back next time)

; In a real task we would increment instead of zero:
STBody:
  L_ TickCount+1, TickCount_ L, BLOCK, :STBody;
```

What happens cycle by cycle:

1. Reset / boot: MPC = 4 (the Disk Sector task index). The first microinstruction at address 4 is `STBody`.
2. Sector mark fires; the disk hardware sets the Disk Sector task's wakeup latch. The arbiter picks the Disk Sector task on the next cycle (since it's high priority and now woken).
3. `L_ TickCount+1` reads R[TickCount] = R[R20], adds 1, latches the result in L.
4. `TickCount_ L` writes L back into R[R20].
5. `BLOCK` (F1=3) signals the disk hardware to clear the Disk Sector task's wakeup latch.
6. `:STBody` sets NEXT = STBody (this address itself, looping).
7. The cycle ends; the arbiter looks at wakeups again. With the disk-sector wakeup just cleared, lower-priority tasks may run.
8. On the next sector mark, the wakeup fires again, the arbiter picks Disk Sector, microcode resumes at STBody, and the counter increments again.

That entire task is one microinstruction. 32 bits. One cycle of work per sector boundary, plus the cycle of arbitration that picks the task.

This is the canonical shape for a "wake on hardware event, do constant work, BLOCK" task. The Disk Sector, Disk Word, MRT, and Cursor tasks all follow this pattern, just with longer bodies between BLOCK calls.

## 3.9 Mu worked example 2

A 4-state finite-state machine. Reads a single bit from main memory, classifies into Idle/Start/Data/End, and increments different counters per state.

```mu
;Demo 4-state FSM
#ALTOCONSTSX.MU;

$State        $R20;        ; state index 0..3
$IdleCnt      $R21;
$StartCnt     $R22;
$DataCnt      $R23;
$EndCnt       $R24;
$DataAddr     $R25;        ; address of input bit in memory

; State table — 4 entries, one per state. The dispatch is a
; 4-way branch on State[0:1].
!3,4,FSMIdle,FSMStart,FSMData,FSMEnd;

FSMEntry:
  ; Read State, set NEXT to dispatch.
  L_ State, :FSMDispatch;

FSMDispatch:
  ; 4-way dispatch on L[8:9] (low 2 bits of the State value).
  ; F2=BUS OR's L's low 10 bits into NEXT, restricted to 2 bits
  ; by aligning the dispatch table at .+0..+3.
  L_ State, BUS, :FSMIdle;        ; F2=BUS dispatches; NEXT=FSMIdle (base)

FSMIdle:
  L_ IdleCnt+1, IdleCnt_ L;
  ; Transition to Start. Update State.
  L_ 1, State_ L, :FSMEntry;

FSMStart:
  L_ StartCnt+1, StartCnt_ L;
  L_ 2, State_ L, :FSMEntry;

FSMData:
  L_ DataCnt+1, DataCnt_ L;
  ; Transition to End.
  L_ 3, State_ L, :FSMEntry;

FSMEnd:
  L_ EndCnt+1, EndCnt_ L;
  ; Loop back to Idle.
  L_ 0, State_ L, :FSMEntry;
```

What's worth noticing:

- The dispatch table `!3,4,...` declares four labels at addresses `.+0, .+1, .+2, .+3` — the low 2 bits of NEXT distinguish them.
- The dispatch microinstruction `L_ State, BUS, :FSMIdle` does *three* things: loads `State` into L, F2=BUS asserts which OR-s the bus's low 10 bits into NEXT, and sets the base NEXT to FSMIdle. Net result: NEXT becomes `FSMIdle | (State[0:1])` — a 4-way branch in one cycle.
- Each state's body is two microinstructions: increment its counter, then update State and jump back to FSMEntry. Real Alto microcode often compresses this further by combining the counter increment with the State update in a single microinstruction.

## 3.10 Mu worked example 3

The Disk Sector body skeleton. This is closer to real period code — it's a sketch of what `KSEC` (the canonical disk-sector microcode label) looks like in `altoIIcode3.mu.txt`, simplified to the essential structure.

```mu
;Sketch of Disk Sector microcode body
#ALTOCONSTSX.MU;

$KBLK        $521;            ; head of the disk-command-block chain
$XFEROFF     $L100000,000000,000000;   ; KCOM bit indicating "no transfer"

; Reset entry: MPC = 4 (KSEC).
KSEC:
  ; Step 1: read the disk's status. KSTAT comes from the disk
  ; controller via BS=3 (KSTAT, task-specific in KSEC).
  T_ ←KSTAT;

  ; Step 2: dispatch on disk status — if XFEROFF, bypass setup.
  L_ T XOR XFEROFF, BUS=0, :KSECNoXfer;

KSECDoXfer:
  ; Step 3: read the next disk command from main memory.
  MAR_ KBLK;            ; start memory fetch of the chain head
  ; (1-cycle latency — interleave other work)
  T_ ←MD;               ; latch the loaded value into T
  ; ...read further DCB words via additional MAR_/MD reads...
  ; (in real microcode this is several dozen instructions)

  ; Step 4: program the disk controller registers.
  KCOM_ T;              ; F1=15B (KCOM←) — set command word
  ; ...similar KADR_, KCWA_ setups...

  ; Step 5: yield. The disk word task (KWDX) will fire repeatedly
  ; for the next 256 cycles to actually move the data.
  TASK, BLOCK, :KSEC;

KSECNoXfer:
  ; Nothing to do this sector. BLOCK and wait for the next mark.
  TASK, BLOCK, :KSEC;
```

What this code shows:

- **The sector-task pattern**: read status, decide what to do, optionally program the controller, yield via `TASK` and clear our wakeup via `BLOCK`.
- **Memory access timing**: `MAR_ X` initiates a fetch; the result becomes available on `←MD` after at least one intervening cycle (AltoHW §2.3). Real microcode interleaves MD reads with other useful work to absorb the memory latency.
- **Per-task F1**: `KCOM_` is `F1=15B` (`KCOM←`) in the disk-sector task. The same F1 code in the Emulator task means something completely different (`ESRB` — set register bank in Alto II Emulator).
- **TASK and BLOCK in the same instruction**: yielding the engine *and* clearing our wakeup. Once we BLOCK, the disk-sector task is dormant until the next sector-mark pulse from hardware.

Real `KSEC` in `altoIIcode3.mu.txt` is about 70 lines of Mu, with several conditional branches handling read vs. write, error-status checks, and DCB-chain walking. The skeleton above is the structural spine.

## 3.11 Magic-number microinstruction encoding

Some constants in `altoconsts23.mu.txt` use a peculiar three-octal-number form:

```mu
$ALLONES    $L177777,000000,000000;
$LDC        $L060000,000010,037777;
```

This is **the magic-number microinstruction encoding**: a way to declare a *predefined microinstruction template* that user code can later branch to or invoke. The three octal numbers correspond to fixed bit-patterns of the 32-bit microinstruction word, with specific bits left for the user to fill in via subsequent declarations.

Per AltoHW §8.3, the actual hardware encoding has F1[0], F2[0], and Load-L bits *inverted* relative to their logical meaning — an artifact of how the microinstruction PROM is decoded. The three-octal-number form bakes in this inversion. The Mu assembler handles the conversion when packing real microinstructions; the `$L...,...,...;` form is for declaring template patterns that will live in the constants ROM as if they were 16-bit values, often used for jumps into named entry points.

For the implementer of a microcode loader (`microcode_loader.rs`), this matters because the binary `.MB` files store microinstructions in the inverted-bit hardware form. The loader must invert F1[0], F2[0], LL when unpacking. The RHDL implementation in `src/isa.rs::Microinstruction::pack` handles this inversion explicitly.

For the Mu programmer, this matters mainly when reading `altoconsts23.mu.txt` and seeing the magic three-number form — it is a microinstruction template, not an arbitrary 48-bit value.

## 3.12 Loading microcode

There are three ways microcode gets into the microengine:

1. **Boot-time PROM**: the standard ROM microcode is burned into the microcode PROMs. The Alto II's combined microcode address space is 11 bits (1K ROM + 1K RAM), with bit 11 of the MPC selecting between them.
2. **SIO instruction**: the SIO MA instruction (see §4.11 and AltoHW §3.3) can trigger a reset cycle that re-initializes all task MPCs to their task numbers. Combined with prior microcode-RAM loading, this is how new microcode "takes effect".
3. **WRTRAM macroinstruction**: from MA, a program can write microcode RAM word by word using the `WRTRAM` opcode (see §5.2 and AltoHW §8.5). Once the RAM contains the desired microcode, executing `JMPRAM` or asserting `SWMODE` from a RAM-related task transfers control to it.

The boot path is the standard route: the Alto's reset button (or an SIO with reset) re-loads all 16 task MPCs to their task numbers, the Emulator starts at MPC=0 (which is the `NOVEM` label in the standard microcode, which begins the Nova-instruction-fetch loop), and execution proceeds.

## 3.13 Common Mu idioms

A short catalogue of patterns that recur throughout `altoIIcode3.mu.txt` and should be in any Mu writer's vocabulary.

**Pattern: load a constant into an R-register in one cycle.**

```mu
   L_ MyConstant, MyReg_ L, TASK;
```

ALU passes the constant; L latches; the R-register write happens at cycle edge. Plus we yield. One microinstruction.

**Pattern: increment-and-skip-if-zero.**

```mu
   L_ MyCounter+1, MyCounter_ L, BUS=0, :SkipPath;
```

ALU computes counter+1; L latches; counter is updated; F2=BUS=0 sets NEXT[0]=1 if (counter+1) is zero. Note the test fires on the *new* value of the counter, not the old. Two adjacent labels at NEXT and NEXT+1 give you the if/else.

**Pattern: read memory, mask, store.**

```mu
   MAR_ TargetAddr;          ; cycle 1: start fetch
   T_ MD AND Mask;           ; cycle 2: read MD AND'd with constants-ROM mask
   MD_ T, MAR_ DestAddr;     ; cycle 3: write T to memory at DestAddr
```

The first cycle initiates the fetch; the second uses the wired-AND constants gating to mask the result; the third stores. Three cycles of useful work per memory read-modify-write.

**Pattern: cooperative dispatch on multi-bit value.**

```mu
   !3,4,Branch0,Branch1,Branch2,Branch3;  ; declare 4 labels at adjacent addresses
   L_ State, BUS, :Branch0;                ; dispatch on L[8:9] OR'd into NEXT
```

The `!` directive declares the labels at consecutive addresses; the `BUS` F2 ORs L's low bits into NEXT.

**Pattern: TASK plus BLOCK.**

```mu
   TASK, BLOCK, :MyResetLabel;
```

Yield the engine *and* clear my wakeup latch *and* set NEXT to my reset label. Used at the end of every task body to put the task back to sleep until its next hardware wakeup.

These patterns plus the §3.7 per-task code tables cover ~80% of what real Mu code looks like. The remaining 20% is task-specific details — e.g. the Display Word Task's per-cycle DDR-write idiom, the Disk Sector Task's KCOM-bit-decode, the Emulator's IDISP routing.

---

# Part IV — MA, the macroassembler (ALTOASM)

## 4.1 What MA is, and is not

ALTOASM is "patterned after that of the Data General NOVA DOS assembler" (ALTOASM p.1). It is a 1-pass-with-deferred-fixups assembler that produces BCPL-compatible relocatable binary (`.BR`) files for input to the BCPL loader BLDR. The source language is Nova-DOS-flavoured assembly with several specific deletions, additions, and incompatibilities (ALTOASM p.1–4).

**What MA is:**

- A line-oriented assembler. One source line ≈ one machine instruction, plus optional label and comment.
- A symbolic assembler with externals. Symbols can be declared `.ENT` (entry, exported) and `.EXTN`/`.EXTD` (external, imported), plus `.BEXT`/`.BEXTZ` (BCPL-compatible externals).
- A relocatable assembler. Output is `.BR` (relocatable binary), which BLDR then links and loads.
- A macro assembler — though "macro" here means "BCPL-compatible relocatable binary output" rather than full M4-style text macros. Actual text-macro support is limited to `.GET` file inclusion (§4.7).
- BCPL-compatible. Strings follow BCPL conventions; BCPL operators are accepted in expressions; the output file format is what BCPL programs link against.

**What MA is *not*:**

- *Not* a full Nova DOS assembler. Specific Nova directives have been omitted (see §4.4).
- *Not* a microassembler. To write microcode, use Mu.
- *Not* a high-level language. To write higher-level code, use BCPL (or, later, Mesa or Smalltalk).
- *Not* a linker. MA produces `.BR`; BLDR does the linking.

**File extensions** (ALTOASM p.3): the canonical source extension is **`.SR`** (in 1974). By 1979, the convention had drifted toward `.ASM` (we see this in `LevBcpl.asm`). Both work — `ALTOASM FOO.SR` and `ALTOASM FOO.asm` are both legal. The output is `.BR`. The listing file is `.BT` per BCPL convention.

## 4.2 The Nova-derived instruction set

The Alto Emulator runs a 16-bit Nova-derived ISA. AltoHW §3 is the canonical reference. The condensed picture for the MA author:

**State.** PC (16-bit), AC0..AC3 (four 16-bit accumulators), C (1-bit carry).

**Memory.** 64K 16-bit words at addresses 0..177777B. Addresses 177000B–177777B reserved for I/O.

**Instruction groups.** Four formats, distinguished by the high 3 bits of the opcode:

| Group | High bits | Mnemonics |
|---|---|---|
| M-Group | `0|MFunc` | LDA, STA |
| J-Group | `000|JFunc` | JMP, JSR, ISZ, DSZ |
| A-Group | `1|...` | COM, NEG, MOV, INC, ADC, SUB, ADD, AND |
| S-Group | `011|...` | Alto-specific augmented opcodes |

A typical MA program is a mix of M-group memory ops, J-group control flow, A-group ALU operations, plus S-group Alto-specific instructions for BITBLT, MUL, SIO, etc.

## 4.3 Instruction formats and addressing modes

**M-Group (LDA, STA).** Format: `[0|MFunc(2)|DestAC(2)|I|X(2)|DISP(8)]`.

- `MFunc` distinguishes LDA (1) from STA (2).
- `DestAC` is the accumulator (0..3) being loaded or stored.
- `I` is the indirect bit; if 1, the effective address is `*memory[E]` rather than `E`.
- `X` is the index mode:
  - `0` — page 0 addressing: E = DISP (a 0..255 byte address into page 0).
  - `1` — PC-relative: E = SExtend(DISP) + PC.
  - `2` — base register AC2: E = SExtend(DISP) + AC2.
  - `3` — base register AC3: E = SExtend(DISP) + AC3.
- `DISP` is the 8-bit displacement, sign-extended for X=1,2,3.

**J-Group (JMP, JSR, ISZ, DSZ).** Format: `[000|JFunc(2)|I|X(2)|DISP(8)]`. Same addressing-mode encoding as M-group. JFunc distinguishes JMP (0), JSR (1), ISZ (2), DSZ (3).

**A-Group (eight ALU ops).** Format: `[1|SrcAC(2)|DestAC(2)|AFunc(3)|SH(2)|CY(2)|NL(1)|SK(3)]`.

- `SrcAC`, `DestAC`: source and destination accumulator (0..3).
- `AFunc` selects: 0=COM, 1=NEG, 2=MOV, 3=INC, 4=ADC, 5=SUB, 6=ADD, 7=AND.
- `SH` selects shift mode: 0=none, 1=L (rotate left 17 bits inc. carry), 2=R (rotate right), 3=S (byte swap).
- `CY` selects carry override: 0=use C, 1=Z (force 0), 2=O (force 1), 3=C (complement of C).
- `NL` is "no-load": if 1, the destination AC is NOT updated (useful for skip-only instructions).
- `SK` is the skip condition: 0=never, 1=always, 2=carry-zero, 3=carry-nonzero, 4=result-zero, 5=result-nonzero, 6=either-zero, 7=both-nonzero.

**S-Group (Alto-specific).** Various encodings, all starting with `[011|...]`. See §4.11.

In MA source, you write A-group instructions with the operation mnemonic followed by source AC, destination AC, and optional skip/shift/carry suffixes:

```
ADD 1 0          ; AC0 ← AC0 + AC1
ADDZL 1 0 SZC    ; AC0 ← (AC0 + AC1) << 1 with carry-zero override; skip if no carry
SUB# 1 0 SNR     ; just compute (AC0 - AC1), don't store; skip if non-zero
```

The `#` suffix on a mnemonic adds the no-load (NL) bit. The skip suffix (`SZC`, `SNR`, etc.) is one of the 8 skip conditions. The shift suffix (`L`, `R`, `S`) is appended to the mnemonic (`ADDZL` = ADD with zero-carry and left-shift).

## 4.4 Pseudo-operations summary

ALTOASM supports a subset of the Nova DOS assembler pseudo-ops, plus a few Alto-specific additions. Per ALTOASM p.1 and p.2:

**Omitted from Nova DOS** (these will not work):

- `.TXTO`, `.TXTE`, `.TXTF` — text modes.
- `.XPNG`, `.IFE`, `.IFN`, `.ENDC` — conditional assembly.
- `.EOT` — end-of-tape marker.
- Floating-point input format and operators.
- `DOA`, `IOP`, `.DIO`, `.DIOA`, `.DIAC` — Nova I/O instructions and directives. (Alto has its own SIO mechanism, see §4.11.)

**Inherited from Nova DOS:**

- `.TXTM B` / `.TXTM L` — text mode (BCPL strings vs long strings).
- `.ENT` — entry point declaration.
- `.EXTN`, `.EXTD` — external declarations.
- `.BLK` — reserve a block of words.
- `.LOC` — set the current location counter.
- `=` — symbol value assignment, e.g. `K3 = 3`.
- `.END` — end of source.
- `.TITL` — set the module title (also written `.titl`).

**Alto-specific additions:**

- `.NREL`, `.SREL`, `.ZREL` — assembly regions (§4.5).
- `.GET "name"` — file inclusion (§4.7).
- `.GETNOLIST "name"` — file inclusion, exclude from listing.
- `.BEXT`, `.BEXTZ` — BCPL-style externals (§4.6).

## 4.5 Assembly regions

Per ALTOASM p.1 and p.2: "This assembler can assemble into three regions: two static regions (one in page 0) and one code region. The directives `.NREL`, `.SREL`, and `.ZREL` cause the assembler to begin placing code in the code region, the non-page-0 static region, and the page 0 static region, respectively."

The three regions:

- **`.NREL`** — non-relocatable code. Goes into the code segment of the `.BR` file. Holds executable instructions, constant tables in code space.
- **`.SREL`** — non-page-0 static. Goes into the non-page-0 statics segment. Holds globals, persistent variables, BCPL OWN sections.
- **`.ZREL`** — page-0 static. Goes into the page-0 statics segment. Holds page-0 globals and the small set of BCPL externals that benefit from page-0 addressing.

**Restrictions enforced by BLDR** (ALTOASM p.1): the code area may not contain pointers into the code area; the first word of the code area may not point to a static area; no static area may contain pointers to a static area. The only externals are statics. Arithmetic is not allowed on symbols denoting statics. The symbol `.` (the current location) is undefined in `.SREL` and `.ZREL`.

A typical MA file uses all three regions:

```asm
        .titl   MyModule

        .ent    MyMain          ; export MyMain as an entry point
        .bext   MyGlobal        ; declare MyGlobal as a non-page-0 static
        .bextz  MyZGlobal       ; declare MyZGlobal as a page-0 static

;-- non-page-0 statics --
        .srel
MyGlobal:       0               ; one-word zero-initialized global
MyArray:        .blk    16.     ; 16 words (decimal)

;-- page-0 statics --
        .zrel
MyZGlobal:      0

;-- code --
        .nrel
MyMain: lda     0 K1            ; AC0 ← memory[K1]
        sta     0 MyGlobal      ; *MyGlobal ← AC0
        jmp     @retAddr        ; return via indirect jump
K1:     1
retAddr:        0
        .end
```

The `.titl`, `.ent`, `.bext`, `.bextz` directives are administrative — declarations that do not generate code. The `.srel`, `.zrel`, `.nrel` directives switch the current region. Within each region, lines either declare data (with a label and a value, e.g. `MyGlobal: 0`) or generate code (e.g. `lda 0 K1`).

## 4.6 Externals

Five external-declaration directives, each with a slightly different semantic:

- **`.ENT name`** — *export* `name` as an entry point. Other modules can `.EXTN name` and reference it.
- **`.EXTN name`** — *import* `name` as an external normal-region static. Use when you want to reference a symbol defined in another module's `.SREL` section.
- **`.EXTD name`** — *import* `name` as an external deferred-resolution static. The classical Nova DOS form.
- **`.BEXT name`** — *import* `name` as an external non-page-0 static, BCPL-compatible (ALTOASM p.2). Use this for symbols defined in BCPL modules' OWN sections or `.SREL` regions.
- **`.BEXTZ name`** — *import* `name` as an external page-0 static, BCPL-compatible. Use for BCPL externals declared in page-0.

The crucial distinction is BCPL compatibility. When MA code wants to call into BCPL code or share globals with BCPL, it must use `.BEXT` / `.BEXTZ` rather than the older Nova `.EXTN` / `.EXTD`. The BCPL loader enforces this: BCPL-style externals link with BCPL-style externals.

In practice, `.BEXT` and `.BEXTZ` are what you'll see in any modern (post-1974) MA file. Pure Nova `.EXTN`/`.EXTD` are vestigial.

## 4.7 File inclusion

`.GET "filename"` (ALTOASM p.2) inserts the named file into the source text at that point. The included file is parsed as if it were typed inline. Useful primarily for canned declaration lists (lots of `.BEXT` decls for a library) and for canned entry/exit sequences.

`.GET` can be nested up to two levels deep. So `A.SR` can `.GET "B.SR"`, and `B.SR` can `.GET "C.SR"`, but a third level is not allowed.

`.GETNOLIST "filename"` (ALTOASM p.2) is the same as `.GET` but suppresses the included file from the assembly listing. Useful for very large declaration files where you don't want the listing flooded with imports.

A typical use:

```asm
        .titl   MyModule
        .get    "MyDecls.SR"            ; lots of .bext, .bextz declarations
        .get    "MyMacros.SR"           ; common code fragments

        .nrel
MyMain: ...
```

## 4.8 Text mode

Two text modes (ALTOASM p.2):

- **`.TXTM B`** — BCPL string format. A null-terminated string with a length byte at the start. Default.
- **`.TXTM L`** — long string format. A full word holding the length, followed by character pairs (two characters per word).

Used when emitting string literals via `"text"` syntax in source. Example:

```asm
        .txtm B
greeting:       "Hello, Alto!"
```

## 4.9 Expressions and operators

Per ALTOASM p.2:

- **Parentheses are supported** for grouping. *Operator precedence is not* — without parentheses, evaluation is strictly left-to-right.
- **Constants:**
  - Decimal: trailing period, e.g. `17.` is decimal 17.
  - Octal: no period, e.g. `17` is octal 17 (decimal 15). All numbers without explicit form are octal.
  - Character: `"K` is the ASCII code for 'K'.
  - Power-of-2 shift: `5B10` means `5 << 10 = 5000B`. Note: the Nova convention `5B10` = `5 shifted left by 10 bits = 5000B` is what ALTOASM uses, *not* BCPL's convention. ALTOASM p.2: "The construct 1B10 means 40(octal), following the NOVA assembler's convention rather than BCPL's. I am willing to be convinced on this point." (McCreight's joking footnote.)
  - Multiplier: `$*N` is the current location counter times N.
  - String chars: `"H"e"l"l"o"` produces multiple character constants.
- **Operators** — most BCPL and Nova DOS assembler operators are accepted. Common ones: `+`, `-`, `*`, `/`, `%` (mod), `&` (and), `!` (or), `\` (xor), `>>`, `<<`, `=` (equals), unary `-`, unary `\` (complement).

Expressions evaluate to either an absolute value, a code-relative reference, a static reference, or undefined. The kind of value affects which region it can be placed in.

## 4.10 The skip-pseudo-op family

Nova DOS used numeric skip-condition codes (SK 0..7). Dan Ingalls contributed a set of pseudo-mnemonic skip names (ALTOASM p.3). All of these expand to A-group instructions with the appropriate SK field. Two operands:

| Mnemonic | Meaning |
|---|---|
| `SZE` | Skip if zero (signed result == 0) |
| `SZ` | Skip if zero (synonym) |
| `SNZ` | Skip if non-zero |
| `SP` | Skip if positive |
| `SGZ` | Skip if greater than zero (synonym for SP non-zero) |
| `SN` | Skip if negative |
| `SEQ` | Skip if equal |
| `SE` | Skip if equal (synonym) |
| `SNE` | Skip if not equal |
| `SLT` | Skip if less than (signed) |
| `SLE` | Skip if less than or equal (signed) |
| `SGT` | Skip if greater than (signed) |
| `SGE` | Skip if greater than or equal (signed) |
| `SGTU` | Skip if greater than (unsigned) |
| `SLEU` | Skip if less than or equal (unsigned) |
| `SGEU` | Skip if greater than or equal (unsigned) |
| `SLTU` | Skip if less than (unsigned) |
| `SODD` | Skip if odd |
| `SKEVEN` | Skip if even |
| `SNIL` | Skip if NIL (i.e., -1) |
| `SNNIL` | Skip if not NIL |
| `MKZERO` | Set destination to 0 |
| `MKONE` | Set destination to 1 |
| `MKNIL` | Set destination to NIL (-1) |
| `MKMINUSONE` | Synonym for MKNIL |

Zero-operand variants (predefined in the symbol table):

| Mnemonic | Meaning |
|---|---|
| `NOP` | No operation |
| `SKIP` | Unconditional skip |

Per ALTOASM p.3: "It should be explained that U stands for unsigned, and that Dan thinks of NIL as -1."

These skip mnemonics are *idiomatic* in real Alto MA. Compare hand-coded C-style:

```asm
; if (a < b) goto less; else goto notLess;
        sub#    1 0 snc          ; non-destructively compute AC0-AC1, skip if no-carry
        jmp     less
        jmp     notLess
```

vs. the Ingalls-style:

```asm
        sltu    1 0
        jmp     less
        jmp     notLess
```

The latter is what real period code uses.

## 4.11 Alto-specific augmented opcodes

Per AltoHW §3.3 and the ALTOASM p.3 predefined symbol list, the Alto adds the following macroinstruction mnemonics not present in stock Nova:

| Mnemonic | Octal | Effect |
|---|---|---|
| `JSRII` | varies | Indirect-indirect JSR; convenient for vectored calls |
| `JSRIS` | varies | Indirect-static JSR |
| `CYCLE` | varies | Cyclic shift (rotate) |
| `CONVERT` | varies | Character-set conversion via 256-entry table |
| `DIR` | 61000B | Disable interrupts |
| `EIR` | 61001B | Enable interrupts |
| `BRI` | varies | Branch and restore interrupts |
| `RCLK` | varies | Read clock |
| `SIO` | varies | Start I/O — sets ICmd/OCmd flags via STARTF (F1=17B in Emulator) |
| `BLT` | 105B (microcode addr) | Block transfer — copies a region of memory |
| `BLKS` | 106B | Block store — fills a region with a constant |
| `SIT` | varies | Set interval timer |
| `RDRM` | 61011B | Read microcode RAM |
| `WTRM` | 61012B | Write microcode RAM |
| `JMPRM` | 61010B | Jump into microcode RAM (also `JMPRAM`) |
| `MUL` | 120B (microcode addr) | 16×16 → 32-bit multiply |
| `DIV` | 121B (microcode addr) | 32÷16 → 16+remainder divide |
| `BITBLT` | 124B (microcode addr) | Bit-boundary block transfer (the canonical graphics primitive) |
| `DIRS` | 61013B | Disable interrupts and skip if previously enabled |
| `VERS` | 61014B | Version query |
| `DREAD` | 61015B | Double-word read (Alto II only) |
| `DWRITE` | 61016B | Double-word write (Alto II only) |
| `DEXCH` | 61017B | Double-word exchange (Alto II only) |
| `DIAGNOSE1` | 61022B | Diagnostic instruction (Alto II only) |
| `DIAGNOSE2` | 61023B | Diagnostic instruction (Alto II only) |

For BLT, BLKS, MUL, DIV, BITBLT, the canonical microcode handler lives at a fixed standard address (105B, 106B, 120B, 121B, 124B respectively — see §5.3). The MA mnemonic is sugar for `JSRII <microcode-addr>`.

## 4.12 MA worked example 1

Hello-world style: load two words from page 0, add them, store the result.

```asm
        .titl   AddDemo
        .ent    AddDemo

        .zrel
A:      0                      ; first input (page-0 static)
B:      0                      ; second input
Sum:    0                      ; output

        .nrel
AddDemo:
        lda     0 A             ; AC0 ← *A
        lda     1 B             ; AC1 ← *B
        add     1 0             ; AC0 ← AC0 + AC1
        sta     0 Sum           ; *Sum ← AC0
        jmp     0 3             ; return: PC ← AC3 (caller's saved PC)
        .end
```

What's happening:

- `.titl AddDemo` names the module.
- `.ent AddDemo` declares `AddDemo` as an entry point so callers can find it.
- `.zrel` switches to the page-0 static region; `A`, `B`, `Sum` are page-0 globals.
- `.nrel` switches to code; everything from there is in the code region.
- `lda 0 A` is M-group: load AC0 from the page-0 address `A` (X=0, page-0 addressing).
- `add 1 0` is A-group: AC0 ← AC0 + AC1.
- `jmp 0 3` is J-group with X=3 base-AC3 addressing, displacement 0 — i.e. `PC ← 0 + AC3`. This is the canonical Alto subroutine return: the caller `JSR`-ed and saved the return PC in AC3, the callee jumps to AC3 to return.

This is a typical hand-written MA inner loop body — short, lots of register-level manipulation, no high-level abstractions.

## 4.13 MA worked example 2

A loop using the skip family. Sums an array of 16 words pointed to by AC2 into AC0.

```asm
        .titl   ArraySum
        .ent    ArraySum

        .nrel
ArraySum:
        sub     0 0             ; AC0 ← 0 (clear sum)
        lda     1 K16           ; AC1 ← 16 (count)
ASLoop: lda     3 0 2           ; AC3 ← *(AC2)
        add     3 0             ; AC0 ← AC0 + AC3
        inc     2 2             ; AC2 ← AC2 + 1
        dsz     ,K1             ; (decrement K1 — used as scratch counter — and skip if zero)
        jmp     ASLoop          ; loop
        jmp     0 3             ; return

K16:    16.                     ; decimal 16
K1:     1
        .end
```

Several patterns here:

- `sub 0 0` is a Nova idiom for "AC0 ← 0". A-group SUB with Src=Dst=AC0 sets the result to zero.
- `lda 1 K16` loads the count from a code-relative constant (X=0, page-0; K16 happens to be in page 0 by virtue of being in `.NREL` region — not strictly true, see notes on page-0 addressing).
- `lda 3 0 2` is M-group with X=2 base-AC2 addressing, displacement 0 — i.e. AC3 ← *(0 + AC2).
- `inc 2 2` is A-group INC with Src=Dst=AC2 — i.e. AC2 ← AC2 + 1.
- `dsz` is J-group: decrement memory[K1] by 1; skip if the result is zero.
- The pattern `dsz ... ; jmp loop ; jmp exit` is the canonical Nova "loop until counter zero" idiom — a counter is kept in memory, decremented each iteration, and the loop exits via the skipped instruction.

A more idiomatic MA approach using Ingalls' skips:

```asm
ASLoop: lda     3 0 2           ; AC3 ← *(AC2)
        add     3 0             ; AC0 ← AC0 + AC3
        inc     2 2             ; AC2++
        sub#    0 0 skp         ; (no-load, always skip)
        ...
```

But the pure DSZ idiom is more common in real period code.

## 4.14 MA worked example 3

Calling BITBLT. BITBLT (61024B) is the Alto's bit-boundary block-transfer instruction. It takes a pointer to a "BBTable" — a 16-word struct in memory — in AC2, and AC1=0 to invoke (per AltoHW §3.3).

```asm
        .titl   BBDemo
        .ent    BBDemo
        .bext   src, dst, gray0, gray1, gray2, gray3

        .nrel
BBDemo:
        sta     3 retAddr       ; save return AC3
        sub     1 1             ; AC1 ← 0 (required for BITBLT call)
        lda     2 BBTabAddr     ; AC2 ← &BBTab
        bitblt                  ; do the transfer (returns to PC+1)
        lda     3 retAddr       ; restore caller's return PC
        jmp     0 3             ; return

retAddr:        0
BBTabAddr:      BBTab

;-- BITBLT-table struct (per AltoHW §3.3) --
        .srel
BBTab:  funcCode                ; word 0: function = operation + sourcetype
        0                       ; word 1: unused
DBCA:   dst                     ; word 2: destination bca (extern)
DBMR:   38.                     ; word 3: destination bmr (38 words/scan-line, decimal)
DLX:    0                       ; word 4: destination left-x
DTY:    0                       ; word 5: destination top-y
DW:     608.                    ; word 6: destination width (bits)
DH:     16.                     ; word 7: destination height (lines)
SBCA:   src                     ; word 8: source bca
SBMR:   1                       ; word 9: source bmr
SLX:    0                       ; word 10: source left-x
STY:    0                       ; word 11: source top-y
GRAY0:  gray0                   ; word 12: gray-block pattern[0]
GRAY1:  gray1                   ; word 13: gray-block pattern[1]
GRAY2:  gray2                   ; word 14: gray-block pattern[2]
GRAY3:  gray3                   ; word 15: gray-block pattern[3]

funcCode = 0                    ; operation=0 (Replace), sourcetype=0 (block)
        .end
```

What this shows:

- `bitblt` is the BITBLT macro-mnemonic, which assembles to a JSRII at the BITBLT microcode entry.
- The BBTab struct is in `.SREL` so it can be modified at runtime.
- `funcCode = 0` is a symbol-value assignment; the operation and sourcetype combine via addition (per AltoHW §3.3, the function field is `operation + sourcetype`).
- The destination width is 608 bits (the Alto's full screen width is 606 bits, padded to 608).

In actual production code, the BBTab is often allocated dynamically and filled in just before the BITBLT call, since most callers want to vary the source/destination/dimensions at runtime. The static-allocation form shown here is for clarity.

## 4.15 MA worked example 4

Starting an Ethernet transmit operation via SIO. Per AltoHW §7.1, SIO takes a command code in AC0[14:15]: 0=no-op, 1=start transmitter, 2=start receiver, 3=reset.

```asm
        .titl   EtherTX
        .ent    EtherTX
        .bext   EPLOC, EBLOC, EOCLOC, EOPLOC

;-- Ethernet reserved page-1 locations (canonical) --
        EPLOC = 600
        EBLOC = 601
        EOCLOC = 606
        EOPLOC = 607

        .nrel
EtherTX:                        ; Caller has put packet length in AC0,
                                ; packet pointer in AC1, IRQ mask in AC2.
        sta     3 saveR3        ; save return
        sta     0 EOCLOC        ; output count
        sta     1 EOPLOC        ; output pointer
        sta     2 EBLOC         ; interrupt bit mask
        sub     0 0             ; AC0 ← 0 (about to set bits)
        lda     0 K1            ; AC0 ← 1 (= start-transmitter command)
        sio                     ; start the Ethernet
        lda     3 saveR3        ; restore return
        jmp     0 3             ; return

K1:     1
saveR3: 0
        .end
```

Notes:

- `EPLOC`, `EBLOC`, `EOCLOC`, `EOPLOC` are the canonical Ethernet reserved-page-1 locations from AltoHW §7.1. In real code these would be `.GET` from the standard `Ether.SR` declaration file.
- `sio` is the Alto's "start I/O" instruction. It triggers `STARTF` (F1=17B in the Emulator), which sets the ICmd/OCmd flip-flops in the Ethernet hardware, which in turn wakes up the Ether task to actually do the work.
- The pattern of "save AC3, set up state, do work, restore AC3, return" is the canonical Alto MA function shape. The save-restore is needed because the called subroutine often clobbers AC3 internally.

## 4.16 Reading LevBcpl.asm

`LevBcpl.asm` (in `assets/altosource/LevBcpl.asm.html`) is the canonical period MA source we have: the BCPL runtime hand-coded in MA, ~440 lines, dated December 1978 by David Boggs. It demonstrates every major MA idiom in real production-quality code. Worth reading in full, but here is a tour of the major sections.

**Top of file — module declarations:**

```asm
; LevBCPL.asm - Bcpl runtime machine code
; Copyright Xerox Corporation 1979
; Last modified December 10, 1978  12:30 AM by Boggs
        .titl LevBCPL

        .ent LevBcpl
        .bext StackOverflow

        .srel
LevBcpl:        .LevBcpl
StackOverflow:  StkOfl   ; so swat can print a reasonable name.

        .bext OsFinish

; Error codes passed to the procedure called on a "finish";
;  If no error, pass 0
ecOfl   = 1000.   ; Stack overflow (common)
ecNYI   = 1001.   ; Not yet implemented
swat    = 77400   ; Debugger trap instruction
```

The `=` form is the symbol-value assignment. `ecOfl = 1000.` means "the symbol `ecOfl` is a constant with value 1000 decimal".

**Page-0 cyclic-shift dispatch table:**

```asm
        .zrel
; Right shift constant amount; LQ0
; ...
LQ0:    movzr 0 0 skp
LQ1:    movzr 1 1 skp
        movzr 0 0 skp
        movzr 1 1 skp
        ...
```

Each `movzr 0 0 skp` is an A-group MOV with zero-carry-override and right-shift, then unconditional skip. This is a pre-computed dispatch — calling `jsr 314 - 2*n` (where n is 1..7) lands you 2n entries into the table, executes the right number of right-shift instructions, then skips out via the trailing `jmp 0 3`. A genius hand-rolled "rotate by N" implemented as a table of unrolled shifts.

**The frame-allocation prelude:**

```asm
GETFRM: sta 3 fTemp 2           ; save return in old frame
        mov 2 3                 ; compute new frame
        lda 2 @fTemp 3          ; fetch frame size
GETFRMALT:
        inc 2 2
        com 2 2
        add 3 2                 ; AC2 → new frame
        sta 0 -1 3              ; save AC0 below current frame
        lda 0 ZSMAX             ; stack max
        subz# 0 2 snc           ; check for overflow
         jmp GFOFL              ; overflow
        ...
```

Several MA idioms on display:

- `sta 3 fTemp 2` — store AC3 at offset `fTemp` from AC2 (i.e. into the current frame's temp slot).
- `mov 2 3` — A-group MOV: AC3 ← AC2.
- `lda 2 @fTemp 3` — load AC2 from `*(memory[fTemp + AC3])` (the `@` is indirect).
- `inc 2 2`, `com 2 2` — increment-then-complement is "negate":  `AC2 ← -AC2`.
- `subz# 0 2 snc` — A-group SUB with zero-carry-override (so the carry doesn't get involved), no-load (`#`), skip-if-no-carry. Together: "non-destructively compute AC2-AC0 with carry forced 0; if there's no carry out, skip the next instruction." This implements signed compare.
- The error path is `jmp GFOFL`; the success path falls through.

**Calling convention:**

The frame-layout symbols are declared near the top:

```asm
fLast = 0       ; The layout of a BCPL frame
fRet  = 1
fTemp = 2
fArgx = 3
fArg1 = 4
fArg2 = 5
fArg3 = 6
```

So a BCPL stack frame at offset AC2 has: `fLast` (link to caller's frame), `fRet` (return PC), `fTemp` (temp scratch), `fArgx` (extra arg pointer for >3 args), `fArg1`/`fArg2`/`fArg3` (first three args). This is the universal calling convention BCPL programs use; MA code that wants to interop with BCPL must respect it.

The full file is worth reading. It's the most concentrated dose of authentic-period MA available.

---

# Part V — Where Mu and MA meet

## 5.1 IDISP

The Emulator microcode's central trick is the **IDISP** (Instruction Dispatch) F2 code, which routes the just-fetched MA opcode bits into the next-instruction NEXT field. Per AltoHW §3.5: "The IDISP function (F2=15B) does a 16 way dispatch under control of a 256x4 PROM."

The canonical IDISP table (AltoHW §3.5):

```
Conditions             OR'ed onto NEXT
if   IR[1-2] = 0       then IR[3-4]
elseif IR[1-2] = 1     then 4
elseif IR[1-2] = 2     then 5
elseif IR[4-7] = 0     then 1
elseif IR[4-7] = 1     then 0
elseif IR[4-7] = 6     then 16B
else                   IR[4-7]
```

Reading the high bits of IR (the just-fetched instruction word, MSB=0 numbering):

- `IR[1-2] = 0` is the M-group prefix (LDA, STA), so the dispatch picks one of 4 handlers based on IR[3-4] (which is MFunc for M-group).
- `IR[1-2] = 1` is the A-group prefix (top bit set), dispatch to the A-group handler.
- `IR[1-2] = 2` (i.e. high bits = `010`) is the J-group, dispatch to handler 5.
- The fallthrough case checks IR[4-7] (the instruction sub-opcode) and dispatches based on that for the various S-group augmented opcodes.

The Emulator microcode at the dispatch table (`!37,20,L0,L1,L2,L3,...,R1X` in `altoIIcode3.mu.txt:38`) is the array of handlers IDISP routes to. So the flow for a Nova LDA is:

1. **MA author writes:** `lda 0 FOO`.
2. **MA assembles to:** the 16-bit Nova LDA instruction, e.g. `0|0|01|00|00|00|0010` for "LDA 0 from page-0 address 002".
3. **Emulator microcode fetches** the instruction into IR via `IR← MD` (F2=14B in Emulator).
4. **IDISP fires** (F2=15B), reading IR's high bits and OR-ing them into NEXT, dispatching to the LDA handler.
5. **LDA handler microcode** computes the effective address, issues `MAR_`, waits a cycle, reads `MD`, and stores it in AC0.
6. **The Emulator returns** to the fetch loop for the next instruction.

This entire pipeline is invisible to the MA programmer. From the MA side, `lda 0 FOO` is one instruction. Underneath, the microengine spends 4 to 8 cycles executing it.

## 5.2 JMPRAM, RDRAM, WRTRAM

The MA augmented-opcode set includes three instructions that cross the Mu / MA boundary in the other direction:

- **`RDRAM`** (61011B) — Read microcode RAM. Reads the halfword addressed by AC1 into AC0. Microcode addressing: bit 4 selects ROM/RAM, bit 5 selects high/low halfword, bits 6–15 are the word address (AltoHW §8.2).
- **`WRTRAM`** (61012B) — Write microcode RAM. Writes AC0 (high half) and AC3 (low half) into the microcode RAM word addressed by AC1. Both halves write at once.
- **`JMPRAM`** (61010B) — Jump into microcode RAM. Sends control of the Emulator task to microcode-RAM location AC1 (mod 1024). Per AltoHW §8.5: "This operation is fraught with peril. If done in error it is the one of the few emulator instructions which can cause the machine to plunge completely off the deep end."

These three together let an MA program install custom microcode and transfer execution to it. The pattern:

```asm
; Load microcode word addr=20 into AC1, write a microinstruction
LoadCustomMicro:
        lda 1 K20                ; AC1 = 20 (target microcode address)
        lda 0 HighHalf           ; AC0 = high 16 bits
        lda 3 LowHalf            ; AC3 = low 16 bits
        wtrm                     ; WRTRAM: writes both halves
        ; ... continue loading more microinstructions ...

; Once the microcode is loaded, jump into it:
RunCustom:
        lda 1 K20
        jmprm                    ; JMPRAM: Emulator executes microcode at RAM[20]
```

Per AltoHW §8.5, an alternative way to test for the presence of microcode RAM is to use WRTRAM + RDRAM and read back what was written. JMPRAM "is fraught with peril" because, if the destination doesn't actually contain valid microcode, the machine can hang.

This is how Mesa's accelerator microcode and Smalltalk's BitBlt-extending microcode get loaded.

## 5.3 Standard fixed addresses

Per AltoHW §9.1, certain microcode addresses are guaranteed by all standard ROM microcode versions and are the stable interface contract MA code can rely on:

| Address | Label | Semantics |
|---|---|---|
| 20B | `START` | Beginning of Emulator's main loop |
| 22B | `RAMCYCX` | Fast cyclic shift subroutine |
| 37B | `TRAP1` | RAM location for unfamiliar traps; ROM trap-sequence implementation |
| 105B | `BLT` | Block transfer subroutine |
| 106B | `BLKS` | Block store subroutine |
| 120B | `MUL` | Multiply subroutine |
| 121B | `DIV` | Divide subroutine |
| 124B | `BITBLT` | BITBLT subroutine |
| 160B | `L0` | Cyclic shift dispatch table |

So `MUL` in MA assembles to a JSRII to address 120B; the microcode at 120B is the canonical multiply implementation; control returns via the standard subroutine return.

This is also the contract user-loaded RAM microcode must respect: if you replace `BITBLT` (e.g. for a custom blitting variant), your replacement must live at address 124B in your microcode RAM and obey the standard calling convention.

## 5.4 Custom opcode end-to-end

A worked example: implement a custom "popcount" opcode that counts 1-bits in AC0 and stores the count in AC1.

**Step 1: choose an opcode.** Pick an unused S-group encoding. Per AltoHW §8.6, opcodes in 77400B–77777B are reserved for Swat (the Alto debugger), and 61000B–61377B for Alto-defined SIO-class instructions. Pick something else, e.g. 61400B.

**Step 2: write the Mu microcode.** A simple popcount loop:

```mu
;Custom POPCNT microcode
;Counts 1-bits in AC0, returns in AC1.
;
;Place at fixed microcode-RAM address PCNT (e.g. 200B).

PCNT:
  T_ AC0;                         ; T = AC0 (input)
  L_ 0, AC1_ L;                   ; AC1 = 0 (clear count)
  ; (loop body - 16 unrolled iterations testing bit 15 of T)
PCNTLoop:
  L_ T AND 1, BUS=0, :PCNTBit0;   ; test bit 0 of T
PCNTBit0:                          ; bit was 0, fall through
  L_ T LRSH1, T_ L;               ; shift T right
  ; ... continue for 16 iterations ...
  TASK, :START;                   ; return to Emulator main loop
```

In real microcode this would be an unrolled 16-iteration loop (one per bit), branching on each bit and incrementing AC1 if the bit is 1. Real Alto microcode uses tricks like cyclic-shift-by-N to compress the loop further.

**Step 3: choose an MA mnemonic.** Add it to a `.GET` declaration file:

```asm
; popcnt.SR
POPCNT  =  61400B
```

**Step 4: load the microcode.** From an MA program, write the popcount microcode word by word into RAM:

```asm
        .titl LoadPopcnt
        .ent LoadPopcnt

        .nrel
LoadPopcnt:
        ; ... load each microinstruction word via WRTRAM ...
        ; (this is the tedious part; in practice you'd assemble Mu source
        ;  with a microcode-RAM-loader tool)
        ; ...
        jmp 0 3
```

**Step 5: invoke from MA.** Use the new opcode:

```asm
        lda 0 SomeWord
        61400B                  ; POPCNT
        sta 1 BitCount          ; store result
```

**Step 6: bind opcode to handler.** The IDISP PROM and the Emulator's initialization microcode together decide where opcode 61400B routes. For a fully custom opcode, you typically modify the dispatch sub-table (`!37,20,...` in altoIIcode3.mu.txt) so opcode 61400B routes to PCNT.

In practice, period code didn't usually go to this much trouble — the Alto's 1K of microcode RAM was scarce, and most "custom opcodes" were small variations on the standard set. But the mechanism exists, and Mesa and Smalltalk both used it for substantial microcode replacement.

---

# Part VI — BCPL and the broader toolchain

## 6.1 The BCPL compilation pipeline

Per CompBCPL p.1: "The BCPL compiler consists of six files, normally called `BCPL.SV`, `BCPL.YL`, `BCPL.YC`, `BCPL.YS`, `BCPL.YT`, and `BCPL.YG`. The .SV file is the main program; the .Y* files contain the code for the five passes of the compiler."

The five passes (CompBCPL p.3):

- **LEX** — lexical analyzer. Tokens.
- **CAE** — parser. Abstract syntax tree.
- **SAE** — symbol-table generator. Symbol resolution.
- **TRN** — Ocode generator. BCPL's intermediate-code form.
- **NCG** — code generator (passes 1 and 2). Final relocatable binary.

The output is a `.BR` file (relocatable binary) that BLDR will link.

Source extension: `.3`. Output: `.BR`. Listing: `.BT` (text output).

**Invocation** (CompBCPL p.1):

```
BCPL PROG.3
```

or with switches:

```
BCPL/U/A PROG.3              ; /U = uppercase mapping, /A = assembly listing
```

The compilation pipeline mirrors many later compilers: lexer → parser → semantic analysis → IR → backend. BCPL was unusual in its time for being implemented in BCPL itself (a self-hosted compiler).

## 6.2 `.BR` format

The relocatable binary (`.BR`) format is what both BCPL's compiler and ALTOASM produce. It is a multi-section format:

- **Code section** — the `.NREL` region's contents.
- **Statics sections** — `.SREL` (non-page-0) and `.ZREL` (page-0).
- **Symbol table** — entries (`.ENT`) and externals (`.EXTN`/`.EXTD`/`.BEXT`/`.BEXTZ`).
- **Relocation entries** — flagging which words in the code/statics need address fix-up at load time.

The format is inherited from the Nova DOS `.SV` format and shares its general structure. BLDR concatenates `.BR` files, resolves externals, and produces a `.RUN` file (executable).

## 6.3 BLDR

BLDR is the BCPL loader. It takes a list of `.BR` files, resolves externals between them, and produces a runnable image. Key points:

- BLDR distinguishes external names by case by default. The `/U` switch maps all external names to upper-case (use this when mixing upper-case and lower-case `.BR` files).
- BLDR produces a `.RUN` file (executable image).
- BLDR understands the Alto's three regions (code, srel-static, zrel-static) and packs each appropriately.

Typical BLDR invocation:

```
BLDR/U PROG MYLIB IO1 IO2
```

This loads `PROG.BR`, `MYLIB.BR`, `IO1.BR`, `IO2.BR`, resolving externals, mapping case via `/U`, and producing `PROG.RUN`.

## 6.4 BCPL ↔ MA calling convention

The BCPL calling convention (from `LevBcpl.asm`):

- **AC2** is the frame pointer.
- **AC3** is the return address (ABI-saved).
- Args are passed in the *callee's frame* at offsets `fArg1=4`, `fArg2=5`, `fArg3=6`. For >3 args, the caller passes a pointer at `fArgx=3`.
- Frame layout: `fLast=0` (link), `fRet=1` (return PC, redundant with AC3 for first call), `fTemp=2` (scratch), `fArgx=3` (extra-args pointer or count), `fArg1..fArg3` (first three args).

To call BCPL from MA:

```asm
        ; Load args into AC0, AC1 (first two)
        lda 0 ARG1
        lda 1 ARG2
        ; Save current AC3 (return address)
        sta 3 SAVEDR3
        ; Jump to BCPL function
        jsr @bcplFunc          ; AC3 ← PC+1; PC ← *(bcplFunc)
        ; AC0 holds return value on return
        lda 3 SAVEDR3
        jmp 0 3
```

To call MA from BCPL: the BCPL compiler emits the standard BCPL calling convention, which the MA function must respect. Specifically, the MA function is entered with the frame pointer in AC2 and args at fArg1, fArg2, fArg3 in the new frame (allocated by GETFRM, see LevBcpl.asm). The MA function returns by jumping through fRet.

The BCPL ↔ MA boundary is one of the things that makes MA non-trivial: you can't just write "any old assembly" — you have to follow BCPL's frame and register conventions.

## 6.5 Stack-frame layout

From `LevBcpl.asm`, the canonical BCPL frame layout:

```
   AC2 → ┌─────────────────┐
         │  fLast    (-2)  │  link to caller's frame
         │  fRet     (1)   │  return PC (also in AC3 at entry)
         │  fTemp    (2)   │  scratch
         │  fArgx    (3)   │  pointer to args >3 (or count)
         │  fArg1    (4)   │  first arg
         │  fArg2    (5)   │  second arg
         │  fArg3    (6)   │  third arg
         │  ...            │
         │  locals         │  function's local variables
         └─────────────────┘
```

The frame grows downward (toward lower addresses). The frame size is determined at compile time and stored as a literal that GETFRM reads to compute the new frame's location. Stack overflow is detected by GETFRM comparing against ZSMAX and trapping to `StkOfl` if exceeded.

This is all 1970s-era discipline — no MMU, no hardware stack protection, just a software-checked stack limit. But the Alto's microcode-implemented `JSR` and the BCPL runtime's GETFRM cooperate to make it reliable for typical workloads.

---

# Appendices

## Appendix A — Mu directive quick reference

Standard Mu directives, drawn from `altoIIcode3.mu.txt` and AltoHW §9.1:

| Directive | Purpose |
|---|---|
| `;...` | Comment to end of line |
| `#filename;` | Include another Mu source file |
| `$NAME $value;` | Declare a constant or symbol alias |
| `$NAME $LXXX,YYY,ZZZ;` | Declare a magic-number microinstruction template (3-octal form) |
| `$NAME $RN;` | Declare an R-register alias (e.g. `$NWW $R4`) |
| `LABEL:` | Declare a microcode address label |
| `!N,M,L1,L2,...;` | Declare a jump table at consecutive addresses |
| `(microinstruction);` | One microinstruction terminated by `;` |

Per-microinstruction clause syntax is in §3.3.

## Appendix B — MA opcode quick reference (universal Nova ops)

Universal Nova-derived instructions (M-group, J-group, A-group):

| Mnemonic | Group | Format | Effect |
|---|---|---|---|
| `LDA d X disp` | M | `LDA dst X disp` | AC[d] ← memory[E] |
| `STA s X disp` | M | `STA src X disp` | memory[E] ← AC[s] |
| `JMP X disp` | J | `JMP X disp` | PC ← E |
| `JSR X disp` | J | `JSR X disp` | AC3 ← PC+1; PC ← E |
| `ISZ X disp` | J | `ISZ X disp` | memory[E] ← memory[E]+1; if 0, PC++ |
| `DSZ X disp` | J | `DSZ X disp` | memory[E] ← memory[E]-1; if 0, PC++ |
| `COM s d` | A | `COM s d` | AC[d] ← ¬AC[s] |
| `NEG s d` | A | `NEG s d` | AC[d] ← -AC[s] |
| `MOV s d` | A | `MOV s d` | AC[d] ← AC[s] |
| `INC s d` | A | `INC s d` | AC[d] ← AC[s]+1 |
| `ADC s d` | A | `ADC s d` | AC[d] ← AC[d] + ¬AC[s] |
| `SUB s d` | A | `SUB s d` | AC[d] ← AC[d] - AC[s] |
| `ADD s d` | A | `ADD s d` | AC[d] ← AC[d] + AC[s] |
| `AND s d` | A | `AND s d` | AC[d] ← AC[d] ∧ AC[s] |

Suffixes (combinable): `L` (rotate left), `R` (rotate right), `S` (byte swap); carry mod `Z`/`O`/`C`; `#` (no-load); skip mnemonic at end (see Appendix D).

Indirection: prefix operand with `@`, e.g. `LDA 0 @FOO`.

X (index) field: 0=page-0, 1=PC-relative, 2=AC2-relative, 3=AC3-relative.

## Appendix C — MA Alto-specific opcodes quick reference

Per AltoHW §3.3 and ALTOASM p.3:

| Mnemonic | Octal | Effect |
|---|---|---|
| `JSRII` | varies | Indirect-indirect JSR |
| `JSRIS` | varies | Indirect-static JSR |
| `CYCLE` | varies | Cyclic shift (rotate) |
| `CONVERT` | varies | Character-set conversion |
| `DIR` | 61000B | Disable interrupts |
| `EIR` | 61001B | Enable interrupts |
| `BRI` | varies | Branch and restore interrupts |
| `RCLK` | varies | Read clock |
| `SIO` | varies | Start I/O |
| `BLT` | (microcode at 105B) | Block transfer |
| `BLKS` | (microcode at 106B) | Block store |
| `SIT` | varies | Set interval timer |
| `RDRM` | 61011B | Read microcode RAM |
| `WTRM` | 61012B | Write microcode RAM |
| `JMPRM` | 61010B | Jump into microcode RAM |
| `MUL` | (microcode at 120B) | 16×16 → 32-bit multiply |
| `DIV` | (microcode at 121B) | 32÷16 → 16+remainder divide |
| `BITBLT` | (microcode at 124B) | Bit-boundary block transfer |
| `DIRS` | 61013B | Disable interrupts and skip |
| `VERS` | 61014B | Version query |
| `DREAD` | 61015B | Double-word read (Alto II) |
| `DWRITE` | 61016B | Double-word write (Alto II) |
| `DEXCH` | 61017B | Double-word exchange (Alto II) |
| `DIAGNOSE1` | 61022B | Diagnostic 1 (Alto II) |
| `DIAGNOSE2` | 61023B | Diagnostic 2 (Alto II) |

## Appendix D — MA skip-pseudo-op cheatsheet

Per ALTOASM p.3:

**Two-operand:**

`SZE`, `SZ`, `SNZ`, `SP`, `SGZ`, `SN`, `SEQ`, `SE`, `SNE`, `SLT`, `SLE`, `SGT`, `SGE`, `SGTU`, `SLEU`, `SGEU`, `SLTU`, `SODD`, `SKEVEN`, `SNIL`, `SNNIL`, `MKZERO`, `MKONE`, `MKNIL`, `MKMINUSONE`.

**Zero-operand:**

`NOP`, `SKIP`.

`U` denotes unsigned. `NIL` is -1.

Standard Nova skip codes: `SKP` (always), `SZC` (skip if no carry), `SNC` (skip if carry), `SZR` (skip if zero result), `SNR` (skip if non-zero result), `SEZ` (skip if either), `SBN` (skip if both non-zero).

## Appendix E — Reserved page-1 locations

Per AltoHW Appendix B and `altoconsts23.mu.txt`. Selected entries:

| Address | Symbol | Purpose |
|---|---|---|
| 420B | `DASTART` | Display Control Block list head |
| 421B | `DASTART+1` | Vertical interrupt mask |
| 424B | `MOUSELOC` | Mouse X coordinate |
| 425B | `MOUSELOC+1` | Mouse Y coordinate |
| 426B | `CURLOC` | Cursor X coordinate |
| 427B | `CURLOC+1` | Cursor Y coordinate |
| 430B | `CLOCKLOC` | Real-time clock |
| 431B–450B | `CURMAP` | 16-word cursor bitmap |
| 452B | `WWLOC` (`WW`) | Wakeups Waiting (interrupt pending mask) |
| 453B | `ACTIVE` | Active interrupt channels |
| 460B | `MASKTAB` | CONVERT instruction mask table |
| 500B | `PCLOC` | Saved PC on interrupt |
| 501B–514B | `INTVEC..INTVEC+13B` | Interrupt vector table |
| 521B | `KBLK` | Disk command-block chain head |
| 526B | `TRAPDISP` | Trap dispatch |
| 527B | `TRAPPC` | Trap PC |
| 600B | `EPLOC` | Ethernet post location |
| 601B | `EBLOC` | Ethernet interrupt-bit location |
| 602B | `EELOC` | Ethernet end-count location |
| 603B | `ELLOC` | Ethernet load location |
| 604B | `EICLOC` | Ethernet input-count |
| 605B | `EIPLOC` | Ethernet input-pointer |
| 606B | `EOCLOC` | Ethernet output-count |
| 607B | `EOPLOC` | Ethernet output-pointer |
| 610B | `EHLOC` | Ethernet host address |
| 614B | `DCBR` | Saved-on-parity disk command-block fetch ptr |
| 615B | `KNMAR` | Saved-on-parity disk word fetch ptr |
| 616B | `DWA` | Saved-on-parity display word address |
| 617B | `CBA` | Saved-on-parity display CBA |
| 620B | `PC` | Saved-on-parity PC |
| 621B | `SAD` | Saved-on-parity scratch |
| 177024B | `MEAR` | Memory Error Address Register (Alto II) |
| 177025B | `MESR` | Memory Error Status Register |
| 177026B | `MECR` | Memory Error Control Register |
| 177030B | `UTILIN` | Mouse buttons + keyset bits |
| 177016B | `UTILOUT` | Diablo printer + general output |
| 177034B | `KBDAD` | Keyboard word 0 |
| 177035B | `KBDAD+1` | Keyboard word 1 |
| 177036B | `KBDAD+2` | Keyboard word 2 |
| 177037B | `KBDAD+3` | Keyboard word 3 |
| 177701B | `EIALOC` | EIA serial input |

Full mapping in `altoconsts23.mu.txt:144-360`.

## Appendix F — Sources and citations

Primary sources, all in `crates/rhdl-alto/assets/`:

- `bitsavers/Alto_Hardware_Manual_Aug76.pdf` — Charles Thacker et al., *Alto Hardware Manual*, August 1976. Cited as **AltoHW §X**.
- `bitsavers/Alto_Hardware_Manual_May79.pdf` — May 1979 revision. Cited as **AltoHW79**.
- `bitsavers/AltoHWRef.part1.pdf`, `AltoHWRef.part2.pdf` — Alto II Hardware Reference, ~1979.
- `bitsavers/ALTOASM_Oct74.pdf` — Ed McCreight, *ALTOASM*, 2 October 1974. 6 pages. Cited as **ALTOASM p.N**.
- `bitsavers/AltoSubsystems_Oct79.pdf` — Subsystems reference (disk, display, Ethernet, mouse, keyboard).
- `bitsavers/Alto_Operating_System_Reference_Manual_Jun75.pdf` — *Alto OS Reference Manual*, June 1975.
- `bitsavers/BCPL_Reference_Manual_Sep79.pdf` — James Curry & PARC staff, *BCPL Reference Manual*, September 1979.
- `bitsavers/Compiling_BCPL_Programs_Aug73.pdf` — James Curry, *Compiling BCPL Programs*, 16 August 1973. 5 pages.
- `bitsavers/Alto_Users_Handbook_Sep79.pdf` — *Alto Users' Handbook*, September 1979.
- `bitsavers/CSL-79-11_Alto_A_Personal_Computer.pdf` — Thacker, McCreight, Lampson, Sproull, Boggs, *Alto: A Personal Computer*, CSL Tech Report 79-11.
- `bitsavers/altoIIcode3.mu.txt` — canonical Alto II ROM microcode. 2 230 lines.
- `bitsavers/altocode24.mu.txt` — earlier microcode revision. 2 030 lines.
- `bitsavers/altoconsts23.mu.txt` — canonical 370-line constants and symbols file.
- `altosource/asm.tty` — plain-text mirror of the ALTOASM manual.
- `altosource/LevBcpl.asm.html` — BCPL runtime in MA. Boggs, December 1978. ~440 lines.

Companion documents in `crates/rhdl-alto/`:

- `alto-processor-and-microcode-spec.md` — the canonical RHDL-side architecture specification. The deeper dive on the microengine, registers, BLOCK semantics, IDISP PROM tables, and per-task F1/F2 maps.
- `README.md` — implementation status and pointers to the source modules.

External references for further reading:

- Lampson, Butler. *Alto: A Personal Computer*. In *Computer Structures: Principles and Examples*, 1979.
- Ken Shirriff's blog series on the Alto — http://www.righto.com/ — modern technical analysis with gate-level detail.
- Living Computers Museum's ContrAlto emulator — https://github.com/livingcomputers/ContrAlto — the gold-reference cycle-accurate emulator. The canonical lockstep target for `rhdl-alto` validation.
- Computer History Museum's Xerox Alto file system archive — https://xeroxalto.computerhistory.org/xerox_alto_file_system_archive.html — period source code, including `LevBcpl.asm`, `OSSOURCES.DM`, and many other authentic period MA examples.

---

*End of reference. ~30,000 words. Direct corrections, errors, omissions: file an issue against `crates/rhdl-alto/doc/alto-assembly-reference.md` with the offending claim and the source-document citation that contradicts it.*
