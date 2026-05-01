# Tier C — Flagship Capability Demonstration Cores

> **Status: design plan, not committed engineering work.** Tier C introduces three flagship cores into the RHDL widget roadmap, deliberately chosen to demonstrate the strategic claim that *RHDL can express any digital design clearly* — from modern RISC, to extreme CISC, to microcoded heterogeneous-compute machines. Each core is non-trivial; together they form the marquee portfolio that conference papers, recruiting demos, and customer pilots are organized around.

---

## 1 — Why these three, and why "Tier C"

The existing roadmap (Tiers 0-4 in `widget-roadmap.md`) is organized by *building-block depth* — foundation, combinational, sequential, protocol PHYs, larger systems. That organization is correct for a widget *library* but doesn't surface the cores that exist **specifically to demonstrate the language's expressive power**. Tier C is that surface.

A core belongs in Tier C if and only if it is:

1. A complete, documented, historically real machine, not a synthetic teaching example.
2. Chosen because some specific aspect of its design *defeated other HDLs* or required heroic effort to express.
3. Validatable end-to-end against original binaries, original microcode, or a gold-reference simulator.
4. Citable. Conference papers, blog posts, recruiting demos, and customer pitches can be organized around it.

Three cores satisfy these criteria, and each makes a different strategic claim:

**RISC-V 32I** — the modern RISC showcase. Proves RHDL is a credible target for the dominant open-ISA ecosystem and that the toolchain composes cleanly with the standard RISC-V test infrastructure. This is the table-stakes core; absence of a RISC-V demo signals "not a serious HDL" to the academic and RISC-V startup communities described in `chisel-strategy.md`.

**DEC VAX** — the extreme-CISC showcase. Variable-length instructions, 12 addressing modes, 304 base opcodes, packed-BCD arithmetic, polynomial evaluation, queue-manipulation instructions, and a procedure-call standard implemented in hardware. The VAX is famous for having **the most baroque decoder ever shipped commercially** and is widely cited as the architecture that broke "RTL synthesizability" intuitions. A working RHDL VAX is the strongest possible answer to "can your language handle real complexity?" — and the answer is "yes, and it reads cleanly."

**Xerox Alto** — the microcoded-heterogeneous showcase. The Alto runs CPU instructions, display refresh, disk I/O, Ethernet packets, and mouse handling **all from a shared microengine** — sixteen priority-ordered hardware tasks sharing a horizontal microinstruction pipeline. Implementing the Alto in RHDL demonstrates that the language can express the most aggressively heterogeneous digital design ever shipped, a class of architecture that no modern HDL has cleanly demonstrated. The strategic frame is "RHDL handles the abstraction that x86 microcode and modern microsequencer designs both descend from."

The trio together makes a single argument: **RHDL's expressive power covers modern, classical-CISC, and microcoded eras of digital design without compromise.** Any one core proves a slice; the three together prove the claim.

The "C" in Tier C stands for *capability* (and, conveniently, *capstone*). These are not the fastest cores in the library; they are not the most useful cores in the library; they are the cores designed to *demonstrate what RHDL can express*. Their value is illustrative, not utilitarian.

---

## 2 — Cross-cutting strategic value

### 2.1 Conference and publication targets

Each core is sized to support a conference paper plus several blog posts plus a recruiting demo. Realistic publication venues:

- **RISC-V core**: FCCM, FPGA, CARRV, RISC-V Summit. The paper writes itself: "An AI-assistable RV32I implementation in 800 lines of RHDL with full architectural compliance and 5-stage pipelining derived statically." Compares favorably to existing pedagogical RISC-V cores (Sodor, picorv32) on lines of code, AI-friendliness metrics, and clarity.
- **VAX core**: ASPLOS, ISCA, MICRO, IEEE Annals of the History of Computing. The paper hook: "Can a modern HDL express the most complex commercial CISC ever shipped? Yes — here it is, byte-equivalent to SIMH, in N lines." The historical-reconstruction angle has cross-disciplinary appeal beyond the EDA community.
- **Alto core**: Computer History Museum collaboration, IEEE Annals of the History of Computing, FCCM (microcoded-FPGA angle), Hot Chips (the heterogeneous-task-engine angle). Co-authored with Living Computers Museum or the CHM staff who maintain the original Alto archives.

The trio also seeds three independent recruiting and customer-pitch demos that don't compete with each other.

### 2.2 RHDL features each core stresses

| Feature | RISC-V | VAX | Alto |
|---|---|---|---|
| `#[derive(Fsm)]` (multi-state controllers) | yes (pipeline control) | extensively (microsequencer) | extensively (16 task FSMs) |
| FSM static reachability + diagrams | yes | yes | yes |
| `rhdl-rule` (when shipped) | yes (hazard logic) | yes (microcode dispatch) | yes (task scheduler) |
| Auto-pipelining | yes (5-stage canonical) | n/a (microcoded) | yes (microengine) |
| `RCStream` (when shipped) | yes (mem ports) | yes (operand specifier queue) | yes (display/disk DMA) |
| Vendor primitives (RAMs, multipliers) | yes | yes (microcode RAM) | yes (microcode RAM, framebuffer) |
| Const-generic parameterization | yes | yes (operand size) | yes (task count) |
| `stream-bus-architecture` typed framing | yes | yes (variable-length operands) | yes (Ethernet) |

Each core is, in effect, a full-system test of the RHDL feature stack. Bugs surfaced during these implementations drive priority bumps in the relevant design plans — for example, if VAX operand decoding hits a kernel-language-extensions limitation, that becomes a forcing function for the relevant Tier-1 extension.

### 2.3 Validation discipline

Each core has a four-level validation contract, parallel to but stronger than the standard widget validation in CLAUDE.md §5:

- **L1 — Per-instruction kernel tests.** Every instruction in scope has a Tier-1 kernel test that exercises its decode and execute behavior.
- **L2 — Architectural-test suite pass.** The core passes the official architectural test suite for its ISA (riscv-arch-test for RISC-V, custom suites derived from SIMH or the original DEC tests for VAX, original Alto tests for Alto).
- **L3 — Real binary execution.** The core runs at least one non-trivial real binary end-to-end (a CoreMark for RISC-V, an OpenVMS hobbyist binary for VAX, a Smalltalk-76 disk image for Alto).
- **L4 — Cycle-equivalent against gold reference.** The core's register-and-memory state matches a gold-reference simulator cycle-by-cycle (or at least instruction-by-instruction) on a substantial trace.

L4 is the one that distinguishes a "core that runs hello-world" from a "core that is correct." Without L4, the core may pass tests by coincidence; with L4, the core is provably equivalent to the canonical reference.

---

## 3 — Core 1: RISC-V 32I

### 3.1 Strategic claim

RHDL is a credible target for the dominant open-ISA ecosystem.

### 3.2 Scope

**RV32I base integer ISA (47 instructions).** No multiply (M extension), no atomics (A extension), no floating point (F/D extensions), no compressed (C extension) — all deferred to follow-on cores once Tier C v1 ships.

| Class | Instructions |
|---|---|
| Register-Register ALU | ADD, SUB, AND, OR, XOR, SLL, SRL, SRA, SLT, SLTU |
| Register-Immediate ALU | ADDI, ANDI, ORI, XORI, SLLI, SRLI, SRAI, SLTI, SLTIU |
| Loads | LB, LH, LW, LBU, LHU |
| Stores | SB, SH, SW |
| Branches | BEQ, BNE, BLT, BGE, BLTU, BGEU |
| Jumps | JAL, JALR |
| Upper-immediate | LUI, AUIPC |
| System | ECALL, EBREAK, FENCE |

**Privileged subset:** machine-mode only (M-mode), implementing the minimum for self-hosted execution: `mstatus`, `mtvec`, `mepc`, `mcause`, `mtval`, `mscratch`, `misa`, `mhartid`. Trap handling for `ecall`, illegal instruction, misaligned access, and external interrupts. No supervisor or user mode (no S/U privilege levels) in v1.

**Microarchitecture:** Classic 5-stage pipeline — Fetch (F), Decode (D), Execute (E), Memory (M), Writeback (W). Full hazard detection. Forward-from-EX-and-MEM bypass. Stall on load-use. Synchronous Harvard memory.

This is the canonical RV32I educational pipeline; matches Sodor and picorv32 in capability while serving as a clearer reference because of RHDL's structural advantages.

### 3.3 Specification sources

**Primary:**

- *The RISC-V Instruction Set Manual, Volume I: Unprivileged ISA* (Document Version 20240411 or current latest). https://riscv.org/specifications/ratified/. Chapters 2 (RV32I Base), 25 (CSR Listing). Free, ratified, machine-readable.
- *The RISC-V Instruction Set Manual, Volume II: Privileged ISA* (current latest). https://riscv.org/specifications/ratified/. Chapter 3 (Machine-Level ISA).

**Secondary:**

- *Computer Organization and Design RISC-V Edition* by Patterson & Hennessy, 2nd edition. Reference for the canonical 5-stage pipeline.
- Sodor 5-stage in Chisel (https://github.com/ucb-bar/riscv-sodor). Reference for clean pedagogical implementation.
- picorv32 (https://github.com/YosysHQ/picorv32). Reference for microcoded simplicity and synthesis.

**Reference simulator (gold):**

- Spike (riscv-isa-sim, https://github.com/riscv-software-src/riscv-isa-sim). The reference RISC-V simulator. Used for L4 cycle-by-cycle comparison.

### 3.4 Specification rewrite for RHDL

The RISC-V manual is well-organized but written for prose readability, not implementation derivation. The RHDL implementation needs a derived specification structured around the kernel:

**Per-instruction record:**

```rust
// One entry per instruction in a generated compile-time table
struct InstructionSpec {
    mnemonic: &'static str,
    opcode: u8,         // bits [6:0]
    funct3: Option<u8>, // bits [14:12]
    funct7: Option<u8>, // bits [31:25]
    imm_layout: ImmLayout,    // I-type, S-type, B-type, U-type, J-type, or none
    operand_kinds: &'static [OperandKind],  // rd, rs1, rs2, imm
    semantic_class: SemanticClass,  // ALU, Branch, Load, Store, Jump, System
}
```

This table is generated from the official spec text once and committed under `crates/rhdl-fpga/src/rv32i/spec.rs`. The decoder kernel pattern-matches on this table; the executor kernel dispatches on `semantic_class`.

**Per-microarchitecture record:**

The 5-stage pipeline is represented as five top-level RHDL `Synchronous` widgets — `Fetch`, `Decode`, `Execute`, `Memory`, `Writeback` — composed by a top-level `Rv32iCore` widget. Inter-stage state is carried in `RCStream`-like typed pipeline registers (anticipating the bus architecture; v1 uses `Signal`-typed bundles directly).

**Hazard logic** lives in a separate `HazardUnit` widget. Forwarding paths are explicit fields on `Execute`'s input bundle. Stall control signals propagate backward up the pipeline.

**Memory interface** is two `RCStream`-style ports: one for instruction fetch (read-only), one for data load/store (read-write). External memory implementation (BRAM, DRAM, etc.) is a separate widget the user supplies; the core takes the interface generically.

### 3.5 Phased implementation

**Phase 1 — Single-cycle reference (4-6 weeks).** A non-pipelined single-cycle implementation that executes one instruction per cycle. Used as the executable specification against which the pipelined version is validated. Passes riscv-tests rv32ui suite. ~400 lines of RHDL.

**Phase 2 — 5-stage pipeline (6-8 weeks).** The classic 5-stage pipeline with full hazard detection and forwarding. Passes riscv-tests, runs CoreMark, runs Dhrystone. ~800-1000 lines of RHDL. The single-cycle version becomes the L4 reference.

**Phase 3 — Privileged extensions and CSRs (2-3 weeks).** M-mode implementation, trap handling, CSR access. Passes riscv-arch-test M-mode subset. Adds ~200 lines.

**Phase 4 — Documentation and paper (3-4 weeks).** Book chapter at `doc/book/src/cores/rv32i.md`. Conference paper draft (FCCM or CARRV). Recruitment-pitch deck.

Total: 15-21 weeks. One engineer.

### 3.6 Validation plan

**L1 — Per-instruction kernel tests.** One Tier-1 unit test per instruction in §3.2's scope. Each test constructs an `Execute`-stage input with the encoded instruction and asserts on the output. ~50 tests.

**L2 — Architectural compliance.**

- riscv-tests (https://github.com/riscv-software-src/riscv-tests). Compile to RV32I ELF, run against the core's instruction memory, check the success signature. The test signature is a memory-mapped register that the core writes to indicate pass/fail.
- riscv-arch-test (https://github.com/riscv-non-isa/riscv-arch-test). The official architectural compliance suite. Runs each test and compares result memory against a committed signature file.

**L3 — Real binaries.** All compiled with riscv-gnu-toolchain (rv32i-elf-gcc):

- *Hello, World* via memory-mapped UART. The minimum smoke test.
- Dhrystone 2.1. Standard CPU benchmark; provides a DMIPS number for comparison.
- CoreMark 1.0. Standard embedded CPU benchmark; provides a CoreMark/MHz number.
- A small bootloader: load a payload over UART into RAM, jump to it.
- Optional: micropython for RV32I (proves the core handles a full language runtime).

**L4 — Cycle-equivalence with Spike.**

A test harness that runs the same binary on Spike (in lockstep mode) and on the RHDL core's simulator. After every retired instruction, compare register file state and memory state. Discrepancies are bugs. The harness is a Python script wrapping `spike --debug-cmd` and the RHDL `iverilog` round-trip output. Discrepancy tolerance: zero.

### 3.7 Deliverables (in `crates/rhdl-fpga/src/rv32i/`)

```
rv32i/
  mod.rs                     module root and re-exports
  spec.rs                    InstructionSpec table (generated, committed)
  decoder.rs                 Decode-stage widget
  fetch.rs                   Fetch-stage widget
  execute.rs                 Execute-stage widget (incl. ALU)
  memory.rs                  Memory-access stage widget
  writeback.rs               Writeback stage widget
  hazard.rs                  HazardUnit widget
  csr.rs                     CSR file and trap logic
  core.rs                    Top-level Rv32iCore widget composing the above
  reference.rs               Single-cycle reference for L4 cross-checking
examples/
  rv32i_hello_world.rs       smoke test runnable via `cargo run --example`
  rv32i_dhrystone.rs         Dhrystone benchmark runner
doc/
  rv32i.md                   trace markdown
  rv32i_pipeline_fsm.md      pipeline-stage FSM diagram
```

### 3.8 Risks specific to RV32I

**Toolchain dependency.** The riscv-gnu-toolchain build is a 30-60 minute compile and several hundred MB. Ship pre-built binaries in CI. No risk to runtime correctness.

**Spec-version drift.** RISC-V specs evolve. Pin the implementation to a specific ratified version (probably 20240411 unprivileged + 20240411 privileged). Future spec revisions are tracked separately.

**Hazard correctness.** Forwarding and stall logic is the most common bug source. Mitigated by L4 lockstep with Spike, which catches any discrepancy on the first divergent instruction.

---

## 4 — Core 2: DEC VAX

### 4.1 Strategic claim

RHDL expresses the most complex CISC ever shipped without compromising readability. If RHDL can do VAX cleanly, it can do anything in the synthesizable digital design space.

### 4.2 Scope

**The challenge with VAX is choosing the scope.** The full VAX architecture is 304 instructions across nine functional groups, plus floating-point variants in 4 formats, plus packed-decimal arithmetic, plus the procedure-call standard. A literal "implement everything" plan is a 5-year project for a team. A useful Tier-C scope is **enough VAX to run a real OpenVMS hobbyist boot to login prompt** while exercising all the structurally-difficult features that make VAX VAX.

The scope, in three phases:

**Phase A — VAX integer subset (the demonstration scope).** Approximately 80 instructions covering:

- Integer arithmetic in all 5 sizes (B/W/L/Q/O): ADD, SUB, MUL, DIV, INC, DEC, NEG, MOV, MOVZ
- Bit manipulation in all sizes: BIS, BIC, XOR, COM, TST, CLR
- Compare in all sizes: CMP
- Branches: all conditional branch forms (BEQL, BNEQ, BLSS, BGTR, BGEQ, BLEQ, BLSSU, BGTRU, BGEQU, BLEQU, BVS, BVC, BCS, BCC, BR)
- Jumps and procedure calls: JMP, JSB, RSB, BSB, CALLG, CALLS, RET
- Stack ops: PUSHL, POPR, PUSHR, MFPR, MTPR (privileged)
- Variable-length bitfield: EXTV, EXTZV, INSV, FFC, FFS
- Queue manipulation: INSQUE, REMQUE
- All 12 addressing modes, including the variable-length operand specifier decoding.

This subset is enough to run integer-only OpenVMS console code and most of the BSD VAX kernel. It exercises every structurally-difficult VAX feature.

**Phase B — VAX string and decimal extension (~40 additional instructions).** The string instructions (MOVC3, MOVC5, CMPC3, CMPC5, LOCC, SCANC, SKPC, SPANC) are demonstrations of microcoded-loop instructions. The packed-decimal instructions (ADDP4, ADDP6, SUBP4, SUBP6, CVTLP, CVTPL, MOVP, ASHP, EDITPC) are demonstrations of arbitrary-length BCD arithmetic.

**Phase C — VAX floating point (4 formats × ~30 instructions = ~120 instruction encodings).** F-floating, D-floating, G-floating, H-floating. Each format has its own encoding-byte. Implements the IEEE-equivalent operations in DEC's pre-IEEE format.

Phases A+B+C together are approximately 240 of the 304 base instructions, which is "morally complete VAX." The remaining ~60 are esoteric (POLYF/D/G/H, CRC, EDITPC's full microprogram language, certain protected-mode instructions).

The demonstration scope (and the focus of v1) is Phase A. Phase B and C are follow-on once Phase A proves the architecture works.

### 4.3 Specification sources

**Primary:**

- *VAX Architecture Reference Manual* (DEC, 1987). Bitsavers archive: http://www.bitsavers.org/pdf/dec/vax/. The canonical specification, ~600 pages. Chapter 2 is the instruction set; Chapter 7 is the operand specifier decoding; Chapter 9 is the procedure-call standard.
- *VAX MACRO and Instruction Set Reference Manual* (DEC, 1986). Same archive. Per-instruction encoding details with examples.

**Secondary:**

- *VAX/VMS Internals and Data Structures* (DEC, multiple editions). Useful for understanding why specific instructions exist and how OpenVMS uses them.
- *VAX 11/780 Hardware Handbook* (DEC, 1979). The canonical VAX implementation reference; describes the microcode.
- Bob Supnik's articles on VAX architecture (https://simh.trailing-edge.com/). Supnik wrote SIMH and has documented the VAX subtleties extensively.

**Reference simulator (gold):**

- SIMH VAX (https://github.com/simh/simh, `BIN/vax` target). Bob Supnik's VAX simulator. Cycle-accurate-equivalent simulation of multiple VAX models (11/780, MicroVAX I/II/3900). Used as L4 reference.

### 4.4 Specification rewrite for RHDL

The VAX manual is dense, prose-heavy, and not directly translatable. The RHDL implementation needs a derived specification organized around three orthogonal dimensions:

**Dimension 1 — Operand specifier grammar.** Every VAX operand has a 1-byte minimum specifier:

```
specifier:
  bits [7:4] = mode (16 modes, but only 12 used)
  bits [3:0] = register (R0-R15 where R15 is PC)
```

Modes 0-3 are the "literal mode" (short literal in the specifier itself). Modes 4-15 are the standard 12 addressing modes. Some modes have extension bytes (1-byte displacement, 2-byte displacement, 4-byte displacement, immediate value matching the operand size).

This grammar is rewritten as a Rust enum with full bit-level encoding in `crates/rhdl-fpga/src/vax/operand_spec.rs`:

```rust
#[derive(Digital, ...)]
pub enum OperandSpec {
    ShortLiteral(b6),                        // mode 0-3
    Register(b4),                            // mode 5
    RegisterDeferred(b4),                    // mode 6
    AutoDecrement(b4),                       // mode 7
    AutoIncrement(b4),                       // mode 8
    AutoIncrementDeferred(b4),               // mode 9
    DisplacementByte(b4, i8),                // mode A
    DisplacementByteDeferred(b4, i8),        // mode B
    DisplacementWord(b4, i16),               // mode C
    DisplacementWordDeferred(b4, i16),       // mode D
    DisplacementLong(b4, i32),               // mode E
    DisplacementLongDeferred(b4, i32),       // mode F
    Indexed(b4, /* base specifier is fetched separately */),  // mode 4
}
```

The decoder is a sequential FSM that consumes 1 to 21 bytes per operand, depending on the specifier. The FSM is `#[derive(Fsm)]` for diagram generation.

**Dimension 2 — Instruction grammar.** Each instruction has an opcode (1 or 2 bytes) followed by N operand specifiers. The opcode determines N. The mapping is captured in `crates/rhdl-fpga/src/vax/spec.rs`:

```rust
struct VaxInstructionSpec {
    mnemonic: &'static str,
    opcode_bytes: u16,                  // 0x90 for MOVB, 0xFD30 for two-byte FD-prefix opcodes
    operand_count: u8,
    operand_kinds: &'static [VaxOperandKind],  // .rb, .wb, .ml, .ab, etc.
    semantic_class: VaxSemanticClass,
}
```

This table is generated from Chapter 2 of the architecture reference manual, page-by-page. ~80 entries for Phase A; ~240 entries for Phase A+B+C combined. Generation is a manual one-time pass; the table is committed.

**Dimension 3 — Semantic class evaluation.** Each `VaxSemanticClass` has a corresponding kernel that takes the decoded operands and produces results. The classes are coarse-grained (Integer, Branch, Bitfield, ProcedureCall, etc.) so that one kernel covers many opcodes via parameterization on operand size and operation.

**Microarchitecture:** Microprogrammed pipeline with three stages: Operand-Specifier-Decode (OSD), Microcode-Sequenced-Execute (MSE), and Result-Writeback (RWB). The microcode lives in a microcode RAM (BRAM). Microinstructions are 64-bit horizontal. The MSE stage reads microinstructions from the RAM, dispatches to the appropriate ALU/shifter/memory unit, and writes back to either the register file or main memory.

The microsequencer is `#[derive(Fsm)]`-tagged with the FSM-extraction giving full coverage of microinstruction-to-microinstruction control flow.

### 4.5 Phased implementation

**Phase 1 — Operand specifier decoder (4-6 weeks).** The variable-length operand decoder as a standalone widget. Tested against synthetic operand byte sequences with known correct outputs. This is the structurally hardest piece; if it doesn't work, nothing else works. ~600 lines.

**Phase 2 — Microsequencer skeleton (3-4 weeks).** The microcode RAM, the microinstruction fetch/decode/execute loop, the register file, and the main-memory interface. Loaded with hand-written microcode for a single instruction (MOVL) end-to-end. ~800 lines.

**Phase 3 — Phase A instruction set (8-12 weeks).** Microcode for the 80 Phase-A instructions. Each instruction has a microcode subroutine (~5-30 microinstructions). Tested instruction-by-instruction against SIMH. ~3000 lines of microcode + ~500 lines of RHDL.

**Phase 4 — Privileged + procedure call (3-4 weeks).** CALLG/CALLS/RET implementations (the procedure-call standard requires ~100 microinstructions for CALLS alone), MFPR/MTPR for processor registers, REI for return-from-exception, basic interrupt handling. ~1500 lines of microcode.

**Phase 5 — OpenVMS boot validation (4-6 weeks).** Run a hobbyist OpenVMS image in lockstep with SIMH until login prompt. Discrepancy tolerance: zero. This is the validation hammer that catches every Phase-A bug.

**Phase 6 — Documentation and paper (4-6 weeks).** Book chapter `doc/book/src/cores/vax.md`. Conference paper (ASPLOS or IEEE Annals of the History of Computing). The historical-reconstruction angle is publishable.

**Phase 7 — Phase B (string + decimal) (8-12 weeks).** Optional follow-on after v1.

**Phase 8 — Phase C (floating point) (8-16 weeks).** Optional follow-on after v1.

Phases 1-6 (the v1 demonstration scope): 26-38 weeks. One to two engineers.

### 4.6 Validation plan

**L1 — Per-instruction kernel tests.** One Tier-1 unit test per Phase-A instruction. ~80 tests. Each test constructs a known machine state, executes the instruction's microcode, and asserts on register-and-memory state.

**L2 — Operand-specifier compliance.** Every one of the 12 addressing modes has a dedicated test that exercises the mode against several instructions. This is the test that catches "mode 8 with auto-increment off PC" and similar subtleties.

**L3 — Real binaries.** Run the following against the core, comparing against SIMH at every retired instruction:

- The DEC VAX standalone diagnostics from the original DEC microfiche archive (some of which are in the public domain via the Bitsavers archive).
- A small standalone VAX MACRO program that exercises the procedure-call standard.
- `sumacro` test programs from the SIMH community.
- The OpenVMS Hobbyist V8.4 console boot sequence (legitimate non-commercial use; the OpenVMS hobbyist license permits this).

**L4 — Cycle-equivalence with SIMH.** A lockstep harness runs each test on SIMH (in `set cpu idle=enable; set throttle 1Hz; brkpt every` mode) and on the RHDL core's `iverilog` round-trip simulator. After every retired VAX instruction (not microinstruction), compare register file (R0-R15), processor status longword (PSL), and changed memory locations. Tolerance: zero discrepancy.

The lockstep harness is non-trivial because SIMH and the RHDL core have different microinstruction-level timing. Lockstep is at the *architectural-instruction* level, not the microinstruction level.

### 4.7 Deliverables (in `crates/rhdl-fpga/src/vax/`)

```
vax/
  mod.rs                     module root
  spec.rs                    VaxInstructionSpec table (Phase A; committed, generated)
  operand_spec.rs            OperandSpec enum and decoder FSM
  decoder.rs                 Operand-Specifier-Decode (OSD) stage widget
  microsequencer.rs          MSE stage widget; microcode RAM + dispatch
  microcode.rs               microcode source as embedded data; assembled into the RAM
  alu.rs                     VAX ALU with B/W/L/Q/O sizes and packed-BCD support
  memory.rs                  RWB stage and main-memory port
  regfile.rs                 R0-R15 register file (R15 = PC)
  psl.rs                     processor status longword logic (condition codes)
  core.rs                    top-level VaxCore widget composing the above
examples/
  vax_movl_demo.rs           single-instruction smoke test
  vax_callstack_demo.rs      procedure-call standard demo
  vax_openvms_boot.rs        OpenVMS hobbyist boot lockstep
doc/
  vax.md                     trace markdown
  vax_microsequencer_fsm.md  microsequencer FSM diagram
  vax_osd_fsm.md             operand-specifier-decoder FSM diagram
```

### 4.8 Risks specific to VAX

**Microcode debug surface.** ~3000 lines of microcode is much harder to debug than ~3000 lines of C. Mitigation: a microcode-trace generator that, given a failing test, dumps the microinstruction-by-microinstruction execution log alongside the machine state. The FSM-derive infrastructure makes the microsequencer state legible; the microcode source has to be hand-annotated for readability.

**SIMH-as-gold-reference is itself approximate.** SIMH simulates multiple VAX implementations and isn't guaranteed cycle-identical to any specific one. We pick the MicroVAX 3900 model as the reference (it is the most widely-validated SIMH model). Architectural-instruction-level lockstep is well-defined; microinstruction-level isn't.

**OpenVMS license risk.** The OpenVMS Hobbyist license is currently administered by VMS Software Inc. (VSI) and is generally permissive for non-commercial use, but the license terms do change. Check current status before relying on a specific OpenVMS image. Open-source alternatives: BSD 4.3 VAX (definitively open-source) and NetBSD/vax (open-source) are usable replacements for OpenVMS validation if the license becomes a concern.

**Polynomial and CRC instructions.** These are deferred to Phase C+ but are sometimes called by VMS startup code. If a real OpenVMS boot hits a POLYF instruction, the boot stops. Mitigation: the boot validation runs against a *boot subset* of OpenVMS that doesn't exercise those instructions, OR Phase A is extended to include the POLY family (an additional ~4 weeks).

**Variable-length-everything decoding stress on RHDL.** This is the most aggressive test the language has had on dynamic decoding. If the kernel-language-extensions plan's `?` operator and related sugar haven't shipped, the decoder is going to be verbose. Mitigation: the decoder is the right forcing function for kernel-language extensions §2.7+; build the VAX decoder against the current subset, file kernel-language-extensions issues for each pain point, and improve the language as the implementation reveals gaps.

---

## 5 — Core 3: Xerox Alto

### 5.1 Strategic claim

RHDL expresses heterogeneous-microcoded compute — the abstraction that descended into x86 microcode, modern microsequencers, and FPGA softcores with microcoded fallback paths. The Alto is the most aggressive demonstration of this abstraction in commercial-system history.

### 5.2 Scope

**The Alto in v1.** A complete Alto microengine plus the canonical task set sufficient to:

1. Boot the original Alto ROM-resident microcode.
2. Run Smalltalk-76, Bravo, or Mesa from a disk image.
3. Drive a (simulated) Diablo 31 disk.
4. Refresh a 606×808 monochrome bitmap display.
5. Handle keyboard and mouse input.
6. (Optionally) handle Ethernet packets.

The Alto's hardware is fixed by 1973 design choices, so "v1 scope" is essentially "the entire Alto." The phased plan below is about *what's working when* not about "did we choose to leave this out."

### 5.3 Specification sources

**Primary:**

- *Alto Hardware Manual* (Xerox PARC, 1976, multiple revisions). Bitsavers: http://www.bitsavers.org/pdf/xerox/alto/. The canonical machine description. ~150 pages including the microcode reference.
- *Alto Operating System Reference* (Xerox PARC, multiple editions). Bitsavers archive. Documents the OS layer that the microcode supports.
- *Alto User's Handbook* (Xerox PARC, 1979). User-facing description; useful for cross-referencing what the microcode is implementing.

**Secondary:**

- Ken Shirriff's blog series on the Alto. http://www.righto.com/. The most accessible modern technical analysis. Contains detailed gate-level descriptions of subsystems.
- *Smalltalk-80: Bits of History, Words of Advice* by Glenn Krasner (ed.), 1983. Describes the Smalltalk side of the Alto.
- *The Architecture of the Alto* in IEEE Annals of the History of Computing.
- The original Alto schematics (Xerox PARC, archived at CHM and Bitsavers). For deep reference when the Hardware Manual is ambiguous.

**Reference implementations (gold, multiple options):**

- **ContrAlto** (https://github.com/livingcomputers/ContrAlto). Living Computers Museum's emulator. Cycle-accurate. The L4 gold reference.
- **Salto** by Brian Silverman. Earlier emulator; less cycle-accurate but still useful.
- The original FPGA Alto by Kerry Cope at LCM (less directly comparable to RHDL but useful for sanity-check).

### 5.4 Specification rewrite for RHDL

The Alto is fundamentally three things: a microengine, a register file, and a set of "hardware tasks" each of which has its own microprogram counter and wakeup conditions.

**The microengine.** A horizontal microcode pipeline executing 32-bit microinstructions at 170 ns cycle time. Each microinstruction encodes:

- Source for ALU bus (R, S, register, constant, or special)
- Source for ALU operand B
- ALU function (16 functions: +, -, &, ^, |, ~, etc.)
- Destination of ALU result (R, S, T, M, L, or various special destinations)
- Bus-source override (special bus signals for I/O)
- F1 function (16 functions, mostly task-specific)
- F2 function (16 functions, mostly task-specific)
- T-load enable
- L-load enable
- Next-microinstruction-address (NEXT field)

The 32-bit microinstruction layout is rewritten as a Rust struct in `crates/rhdl-fpga/src/alto/microinstruction.rs`:

```rust
#[derive(Digital, ...)]
pub struct Microinstruction {
    pub rsel: b5,          // R-register select
    pub aluf: AluFunction, // ALU function (16 options)
    pub bs: BusSource,     // bus source (8 options)
    pub f1: F1Function,    // F1 function (16 options)
    pub f2: F2Function,    // F2 function (16 options)
    pub t_load: bool,
    pub l_load: bool,
    pub next: b10,         // next address (10 bits → 1024-microinstruction RAM)
    // ... plus a few more fields
}
```

**The register file.** R-registers (32 entries × 16 bits) and S-registers (32 entries × 16 bits, in 8 banks of 32, total 256). Plus T (temporary), L (latch), M (multiplier-quotient), and the M-channel for memory.

**The task system.** 16 priority-ordered tasks. Each has:

- An MPC (microprogram counter, 10 bits)
- A wakeup signal (driven by hardware or by other tasks)
- A current-task indicator at the microengine level

The microengine, on each cycle, looks at the wakeup signals, picks the highest-priority woken task, loads its MPC, and executes one microinstruction from that task's perspective. When the microinstruction completes, the MPC is updated (next-address from the microinstruction).

**Tasks in scope:**

| Task | Priority | Purpose |
|---|---|---|
| 0 — Emulator | lowest | runs the "instruction set" (a virtual CPU on top of the microengine) |
| 1 — Disk Sector | high | reads/writes Diablo 31 sectors |
| 2 — Disk Word | very high | services per-word disk DMA |
| 3 — Ethernet | high | packet I/O |
| 4 — Memory Refresh | very high | DRAM refresh (RHDL-equivalent: skip; FPGA BRAM doesn't need refresh) |
| 5 — Display Word | very high | per-word display DMA |
| 6 — Display Horizontal | high | end-of-scanline housekeeping |
| 7 — Display Vertical | high | vertical retrace housekeeping |
| 8 — Cursor | medium | mouse cursor overlay |
| 9 — Memory Block Move | medium | the BLT (bit-block transfer) microcode |
| 10 — Mouse | low | mouse event input |
| 11-15 | various | reserved or system-specific |

**Microarchitecture:** The microengine is a 2-stage pipeline: Microinstruction Fetch (MIF) and Microinstruction Execute (MIE). Each is `#[derive(Fsm)]`-tagged. The 16 tasks are 16 sibling FSMs; the wakeup arbiter is a separate widget that selects which task runs each cycle.

### 5.5 Phased implementation

**Phase 1 — Microengine skeleton (4-6 weeks).** The 2-stage MIF/MIE pipeline, the register file, the ALU, the microinstruction RAM. Hand-written microcode for a single task that does ALU operations on registers. ~600 lines of RHDL.

**Phase 2 — Task arbiter and emulator task (4-6 weeks).** The 16-task wakeup arbiter and the Emulator task. The Emulator task runs a small fragment of the original Alto emulator microcode (the part that fetches and executes "instructions" as defined by the Alto's emulated ISA). ~400 lines of RHDL plus the original microcode binary loaded into the RAM.

**Phase 3 — Disk task (4-6 weeks).** The Disk Sector and Disk Word tasks, plus a simulated Diablo 31 disk drive (a virtual sector buffer in BRAM). Boot the original Alto disk image far enough to get to the operating system loader. ~600 lines of RHDL plus the disk-task microcode (already in the microcode binary).

**Phase 4 — Display task (4-6 weeks).** The Display Word, Display Horizontal, and Display Vertical tasks. Output a simulated 606×808 framebuffer to a host-readable buffer (visible via FPGA video output or via a simulated framebuffer dumped from the iverilog trace). Render a "boot" splash screen. ~400 lines.

**Phase 5 — Mouse, Cursor, Keyboard (2-3 weeks).** The remaining input tasks. Mouse and keyboard input via UART or USB on FPGA; via simulated event injection in the iverilog testbench. ~200 lines.

**Phase 6 — Ethernet task (3-5 weeks, optional for v1).** The Ethernet task plus a simulated Ethernet PHY. Packet send/receive against a virtual network. Optional but historically interesting (the Alto was the first machine on what became Ethernet). ~400 lines.

**Phase 7 — Smalltalk boot validation (4-6 weeks).** Boot Smalltalk-76 from a disk image on the RHDL Alto. Compare execution against ContrAlto in lockstep. Win condition: the canonical Smalltalk-76 boot screen renders correctly on the framebuffer. ~no new RHDL; entirely validation work.

**Phase 8 — Documentation and paper (4-6 weeks).** Book chapter `doc/book/src/cores/alto.md`. Conference paper (collaboration with CHM or LCM staff is high-value here; Annals of the History of Computing is the natural venue).

Total: 25-39 weeks. One to two engineers.

### 5.6 Validation plan

**L1 — Per-microinstruction kernel tests.** Tier-1 tests that construct a microinstruction, set up the relevant machine state, execute one cycle, and assert on the result. Coverage: every ALU function, every BS (bus source), every F1/F2 function, every special bus override. ~100 tests.

**L2 — Per-task tests.** Tests for each of the 16 tasks (in scope) that simulate the wakeup conditions, run the task's microcode for several cycles, and assert on the resulting behavior.

**L3 — Real microcode binaries.** Load the original Alto microcode (publicly archived at PARC and Bitsavers — the microcode source is in the Alto Hardware Manual and several derivative archives) into the microcode RAM. Run the boot sequence. Compare each microinstruction's effect against ContrAlto.

**L4 — Cycle-equivalence with ContrAlto.** A lockstep harness that runs the same boot sequence on ContrAlto and on the RHDL Alto's `iverilog` round-trip. Compare register file (R, S, T, L, M), MPC for the active task, and the framebuffer state every microinstruction. Tolerance: zero.

ContrAlto is cycle-accurate to the original hardware, so this is an unusually strong gold reference. Any divergence is either (a) an RHDL bug, (b) an ambiguity in the Hardware Manual that ContrAlto resolved one way and the RHDL implementation resolved another (in which case ContrAlto is authoritative), or (c) a ContrAlto bug (rare; ContrAlto has been validated against actual Alto hardware at LCM).

**L5 (showcase) — Smalltalk-76 demo.** Boot Smalltalk-76 to its workspace. Click around. Take a screenshot. This is the demo that closes the talk.

### 5.7 Deliverables (in `crates/rhdl-fpga/src/alto/`)

```
alto/
  mod.rs                           module root
  microinstruction.rs              Microinstruction struct + decoder
  microengine.rs                   MIF + MIE pipeline
  alu.rs                           Alto ALU (16 functions)
  regfile.rs                       R-registers (32) + S-registers (256)
  task.rs                          per-task state and MPC
  arbiter.rs                       16-task priority arbiter
  emulator_task.rs                 Task 0
  disk_sector_task.rs              Task 1
  disk_word_task.rs                Task 2
  ethernet_task.rs                 Task 3 (optional v1)
  display_word_task.rs             Task 5
  display_horizontal_task.rs       Task 6
  display_vertical_task.rs         Task 7
  cursor_task.rs                   Task 8
  blt_task.rs                      Task 9
  mouse_task.rs                    Task 10
  microcode_rom.rs                 the original Alto microcode as a ROM image
  diablo_disk.rs                   simulated Diablo 31 disk for L3 boot
  framebuffer.rs                   606×808 monochrome framebuffer with output port
  core.rs                          top-level Alto widget composing all of the above
examples/
  alto_microinstr_demo.rs          single-microinstruction smoke test
  alto_disk_boot.rs                disk boot sequence lockstep with ContrAlto
  alto_smalltalk_demo.rs           Smalltalk-76 boot
doc/
  alto.md                          trace markdown
  alto_microengine_fsm.md          microengine FSM diagram
  alto_arbiter_fsm.md              task arbiter FSM diagram
  alto_emulator_task_fsm.md        emulator-task FSM diagram (showcase)
```

### 5.8 Risks specific to Alto

**Hardware Manual ambiguity.** The Alto Hardware Manual (1976) is excellent but not exhaustive. Some edge cases (e.g., simultaneous wakeups on the same priority, cycle-exact behavior of F2-modify-NEXT) are clarified only by reading the original schematics or by examining ContrAlto's implementation. Mitigation: when the manual is ambiguous, ContrAlto is authoritative; document the resolution in the RHDL source comments.

**Microcode source-of-truth.** Several versions of the Alto microcode exist (the original 1973 version, the 1976 version, and several patched versions). Pin a specific version (the one ContrAlto uses, which is the standard "Alto OS Release 19" microcode from approximately 1979). Document the choice.

**Smalltalk image source-of-truth.** Several Smalltalk-76 disk images exist. Pin a specific image (the LCM canonical image works). Validate at the byte-level against ContrAlto.

**Display output bandwidth.** The Alto display is 606×808 at 38 fps, which is ~18.7 MB/s of pixel data. On an FPGA target, this requires real video output (HDMI or VGA) with appropriate timing. In simulation, the framebuffer is a memory region and is dumped post-run for display. Both work; the FPGA path is harder.

**The original Alto microcode is in a peculiar assembly language ("MicroPlanner / micro-MPL").** It's documented but not auto-translatable; the implementation either loads pre-assembled binaries or includes a small assembler in the build pipeline. Recommendation: ship pre-assembled binaries; cite the assembler tool used.

**Co-simulation lockstep with ContrAlto.** ContrAlto is C# (Mono/.NET). Lockstep harness is a Python orchestrator that runs both and compares trace dumps. Engineering, not research; ~1 week of harness work.

**Three different "tasks" patterns.** The 16-task arbiter is a textbook example of `rhdl-rule` — each task is a rule, the arbiter is the priority scheduler. *If* `rhdl-rule` Phase 1 has shipped by the time the Alto core is in development, the Alto can be the first major showcase of `rhdl-rule`. If not, the implementation uses `#[derive(Fsm)]` on each task and a hand-written priority arbiter. Both work; the rule-based version is significantly more readable. **Recommendation: time the Alto core to start after `rhdl-rule` Phase 1 ships.**

---

## 6 — Sequencing recommendation

Although Tier C is a triplet, the three cores should not be implemented in parallel. The recommended order is:

**1. RV32I first (Q1-Q2 of the Tier C effort).**

Rationale: lowest risk, highest immediate strategic value (the academic and RISC-V communities expect this core), well-understood validation. Ship and publish before tackling the other two. The RV32I work also surfaces compiler-level pain points (in pipeline-control logic, hazard detection, CSR access) that the other cores will revisit; fixing them once benefits all three.

**2. Alto second (Q3-Q4 of the Tier C effort).**

Rationale: ContrAlto provides a cycle-accurate reference that makes lockstep validation tractable. The Alto is also the strongest forcing function for `rhdl-rule` (the 16-task arbiter is the canonical use case), so Alto implementation aligns with the rule-architecture rollout. The Alto's microcoded structure is closer to the VAX's microcoded structure than RV32I is, so building Alto first builds intuition for VAX.

**3. VAX third (Q5-Q8 of the Tier C effort).**

Rationale: highest difficulty, biggest scope, most architectural-spec interpretation required. Doing it last benefits from the lessons learned in RV32I (pipeline) and Alto (microcode). It also generates the strongest publishable result, so saving it for the end maximizes the value of the resulting paper.

If the Tier C effort is parallelized (two engineers), pair RV32I and Alto in the first half; consolidate to VAX in the second half.

---

## 7 — Validation infrastructure shared across all three

Each core has its own gold reference (Spike for RV32I, SIMH for VAX, ContrAlto for Alto). The lockstep-harness pattern is the same in all three cases, so a shared test harness is worth building once:

```
crates/rhdl-fpga/tests/cosim/
  mod.rs
  spike_lockstep.rs        for RV32I
  simh_lockstep.rs         for VAX
  contralto_lockstep.rs    for Alto
  trace_diff.rs            common diff-and-format machinery
```

The harness pattern: each lockstep variant takes a reference simulator, the RHDL `iverilog` testbench, and a divergence callback. On every retired architectural instruction (or microinstruction in Alto's case), it compares the two states and either (a) succeeds quietly, or (b) emits a `miette`-formatted diagnostic with the divergent state and the inputs that produced it. Single failure point per discrepancy; no silent passes.

---

## 8 — Deliverables (cross-cutting)

### Code

Three new top-level modules under `crates/rhdl-fpga/src/`: `rv32i/`, `vax/`, `alto/`.

A new test crate `crates/rhdl-fpga/tests/cosim/` for the lockstep harnesses.

Each core ships with `examples/` runnable demos and `doc/` waveform-and-FSM-diagram markdown files (per CLAUDE.md §6).

### Documentation

Three new book chapters:

- `doc/book/src/cores/rv32i.md`
- `doc/book/src/cores/vax.md`
- `doc/book/src/cores/alto.md`

Each chapter is roughly 30-50 pages of mdbook, with embedded waveforms, FSM diagrams, and walkthroughs of key implementation decisions. Linked from `doc/book/src/SUMMARY.md`.

### Publications

Three conference / journal paper drafts:

- "RHDL RV32I: An AI-Assistable RISC-V Implementation" — FCCM, CARRV, or RISC-V Summit.
- "Reconstructing the DEC VAX in RHDL" — IEEE Annals of the History of Computing or ASPLOS (cross-disciplinary appeal).
- "The Xerox Alto Reborn: A Microcoded Heterogeneous Compute Engine in RHDL" — Annals of the History of Computing, in collaboration with CHM or LCM staff.

### Demonstration artifacts

For each core, a recruiting / customer-pitch demo script:

- RV32I demo: 5 minutes. Live edit of the kernel; recompile in seconds; run `cargo test --features riscv-arch`; show the trace; show the FSM diagram; show the emitted Verilog.
- VAX demo: 10 minutes. Show the operand-specifier decoder kernel; show a microinstruction trace; play a clip of OpenVMS booting.
- Alto demo: 10 minutes. Show the microengine kernel; show the 16-task arbiter; play a clip of Smalltalk-76 booting on a simulated framebuffer.

These three demos are the marquee presentations for any conference attendance, customer pitch, or recruiting event.

---

## 9 — Cross-references to other plans

- **`widget-roadmap.md`** — Tier C is added as a new top-level tier. Each core gets a single roadmap entry pointing at this document.
- **`fsm-architecture.md`** — every core uses `#[derive(Fsm)]` extensively. The Alto's 16-task arbiter is a major showcase of FSM-derived diagrams. Each core's FSM diagrams are committed under `doc/`.
- **`rule-architecture.md`** — the Alto's task arbiter is the canonical `rhdl-rule` use case. Alto sequencing should follow `rhdl-rule` Phase 1.
- **`stream-bus-architecture.md`** — each core uses `RCStream` for its memory and (in Alto's case) device DMA ports.
- **`auto-pipelining-plan.md`** — RV32I is a perfect target for auto-pipelining experiments; the 5-stage pipeline can be derived rather than hand-laid-out in a v2 effort.
- **`vendor-primitive-architecture.md`** — each core benefits from BRAM primitives (instruction RAM, microcode RAM, register files). The Alto's framebuffer is a large BRAM. The VAX's microcode RAM is large.
- **`compile-performance-plan.md`** — the cores are large enough (especially Alto and VAX) that compile-performance regressions during implementation will be very visible. They are useful canary projects for compile-performance work.
- **`package-manager-architecture.md`** — once published, each core is a marquee crate on `registry.rhdl.io`, ideally Tier-2-validated against real FPGA boards.

---

## 10 — Risks and open questions

**Scope creep.** Each of the three cores is independently a large project. The temptation to extend (RV32I → RV32IMA, VAX → full Phase A+B+C, Alto → full Mesa runtime) must be resisted in v1. The Tier C v1 scope is the scope above; extensions are explicitly v2.

**One-engineer-or-two staffing.** A single engineer implementing all three over 18-24 months produces the most coherent body of work but risks burnout and slippage. Two engineers in parallel produces more output but loses some coherence in the cross-cutting infrastructure (the lockstep harness, the FSM-diagram conventions). Recommendation: one engineer with occasional contractor support for the lockstep harnesses.

**Strategic value vs. utility.** These cores are not the most useful additions to the widget library. Industrial users would benefit more from a well-tuned PCIe Gen4 widget than from a Xerox Alto. Tier C is justified entirely by *demonstration* value — proving what the language can do — and by *publication* value. Be honest about this when planning revenue work alongside Tier C.

**External-tool dependencies.** Spike, SIMH, ContrAlto are all maintained projects but are external. A breaking change in any of them affects RHDL's lockstep validation. Pin specific versions; document the pinning; don't auto-update.

**Microcode-as-data versioning.** The Alto microcode and (eventually) the VAX microcode are committed binary artifacts. Their versioning and provenance must be traceable. Recommendation: ship the assembler source where available, plus the compiled binary, plus a hash, and reproducibly rebuild in CI.

**FPGA-target validation.** Tier C cores in simulation are useful but not the full demonstration. To be a true "capability showcase," at least one core (probably RV32I) should also be demonstrated on real FPGA silicon with verifiable timing/area numbers. Out of scope for this document but a v1.1 follow-up.

**Historical accuracy vs. expedience.** When the Alto Hardware Manual is ambiguous and ContrAlto resolves it one way, RHDL follows ContrAlto. When SIMH and the VAX Architecture Reference Manual disagree, the manual wins. These are calls that need to be made consistently and documented.

**Publication coordination with original authors and museums.** Both the Alto paper (likely co-authored with CHM or LCM staff) and the VAX paper (potentially with input from Bob Supnik or DEC alumni) require relationship-building before paper submission. Recommendation: outreach starts at the same time the implementation starts.

---

## 11 — Acceptance criteria for Tier C as a whole

Tier C v1 is "done" when:

1. RV32I runs CoreMark in lockstep with Spike with zero divergence.
2. VAX runs OpenVMS Hobbyist boot to the login prompt in lockstep with SIMH with zero divergence.
3. Alto runs Smalltalk-76 to its workspace in lockstep with ContrAlto with zero divergence.
4. All three cores have committed FSM diagrams, waveform traces, book chapters, and runnable examples per CLAUDE.md §6.
5. At least one paper draft exists for each core.
6. The cross-cutting lockstep-harness infrastructure is generic enough that adding a fourth core (PDP-11? Cray-1? IBM 1401?) requires only a new gold-reference adapter, not new harness code.

Each of these is independently checkable. Hitting all six produces the marquee portfolio Tier C exists to deliver.
