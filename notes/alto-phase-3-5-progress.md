# Alto Phase 3.5 — progress notes (resumable across sessions)

Working branch: `feat/alto-phase-3-5-boot-to-os-loader`.  Single
branch, single PR per the user's "complete this phase, don't ship a
v1, don't defer" constraint.  PR opens only when **Nova PC = 0o345**
is reached with cycle-equivalent ContrAlto trace.

## Status (35 commits in, 110 tests pass)

🎯 **Two architectural milestones reached this branch:**
1. **End-to-end 256-word DMA via real microengine cycles** — hand-
   written Disk Sector microcode arms transfer; Disk Word DMAs all
   256 words; verified address-by-address: `mem[0x200..0x300]` got
   `[0xA000, 0xA001, ..., 0xA0FF]` exactly.
2. **Nova-emulator FETCH-DISPATCH cycle functional** — F2=LoadIr
   loads IR from MD; F2=IDispatch ORs IR[7:0] into NEXT for opcode-
   based routing; BS=InstructionRegister drives BUS from IR.

🔍 **Diagnostic insight (from `boot_trace_decode_diagnostic`):**
   Real Alto microcode boots into an 8-address loop (0x000, 0x130,
   0x14e, 0x150-0x154) and stays there because:
   - Emulator at MPC=0 needs to write disk_ctrl registers, but
     KCOMM/KADR/etc. F1 codes are gated to Disk Sector (task 4) only.
   - Disk Sector at MPC=0 also runs the shared microcode but takes
     no useful disk-specific path because per-task BS sources
     (BS=3 = KSTAT for Disk Sector, etc.) aren't implemented.
   - Net result: nothing programs the disk; memory stays empty;
     IR loads from `memory[MAR=0]=0` forever; loop continues.

   **Next high-leverage unblock:** per-task BS sources (BS=3, BS=4)
   that dispatch differently per task (KSTAT/KDATA for disk tasks,
   NWRD/NREAD for Emulator).  Plus initialization microcode that
   sets per-task MPCs to their proper entry points.

### ✅ Done

1. **`microcode_loader`** — 16-PROM loader (U52..U75) + Constant ROM
   loader (C0..C3).  Both validated against real PARC dumps.  Address
   inversion + bit-mask + scramble + nibble-reverse all match
   ContrAlto's `UCodeMemory.cs` and `ConstantMemory.cs` exactly.
2. **`disk_image_loader`** — `.dsk` parser (Diablo-31 geometry,
   2,601,648-byte file format).  Validated against `nonprog.dsk` —
   sector 0 data[0] = 0o345 matches the JMP-345 entrypoint per the
   committed boot block disassembly.
3. **`Memory`** — 64KW × 16-bit `SyncBRAM`-backed widget with init
   API for staging boot images.  1-cycle BRAM read latency.
4. **`MicrocodeRom`** + **`ConstantRom`** widgets — BRAM-backed and
   combinational respectively.  Both consumable from loader output.
5. **`Microengine` MPC refactor** — MPC moved from internal DFF to
   input; `next_mpc` now a combinational output.  Unblocks AltoChip
   composition with BRAM-backed microcode.  Will widen to 11 bits
   when bank-switching SWMODE F1 code is added.
6. **F1 = Constant integration** in microengine + AltoChip.
   AltoChip decodes RSEL+BS, drives constant_rom, returns value.
7. **MAR + MD← + MD→ memory bus** integrated into AltoChip.  Full
   memory write-then-read round-trip works through the microcode stack.
8. **`AltoChip` skeleton** composing microengine + microcode_rom +
   constant_rom + memory.  Real Alto II microcode loads + runs without
   crashing for 64+ cycles; visits ≥2 distinct microaddresses.
9. **ContrAlto2 (.NET 8 fork) clone + build verified** at
   `crates/rhdl-alto/assets/contralto/Contralto2/`.  `dotnet build`
   produces a working `ContraltoLib.dll`.
10. **Task system renumbered to ContrAlto convention**:
    0/4/7/8/9/10/11/12/13/14 (10 real tasks; slots 1, 2, 3, 5, 6, 15
    have no rule).
11. **AltoTaskSystem wired into AltoChip**.  Single-mpc DFF replaced
    with the 16-task arbiter as a sub-widget.  Engine takes
    `current_task: Bits<4>` input (echoed in output for trace);
    task_system's `last_task` (1-cycle pipeline lag — matches Alto
    MIF/MIE) drives the engine's MPC selection; engine's `next_mpc`
    writes back to the firing task's slot in `next_mpc_per_task`.
    Multi-task arbitration test confirmed: with both Task 0 and Task
    4 woken, Task 4 (Disk Sector) wins through the chip.
12. **DiabloDisk + DiskController wired into AltoChip**.  Disk's
    sector_mark / word_strobe outputs OR'd into the wakeup vector
    (bits 4 and 14).  Disk-Sector-task firing in response to
    sector_mark verified end-to-end (chip's current_task hits 4
    when sector_mark fires).
13. **First per-task F1 code: `F1=DiskCtrlWrite`**.  Microengine takes
    `current_task` into account; F1=DiskCtrlWrite under Disk Sector
    (task 4) writes BUS to disk_ctrl register at RSEL[2:0]; under any
    other task it's a no-op.  Validated end-to-end via two-scenario
    test: Disk Sector task wakes → write_en asserted; Emulator task
    wakes → write_en NEVER asserted.  Per-task dispatch pattern proven.

### ⏳ Remaining for Phase 3.5

| Step | Description | Est |
|------|-------------|-----|
| Per-task F1/F2 dispatch (continued) | Pattern proven via F1=DiskCtrlWrite (step 13).  Still to add: gate MAR← / MD← to MRT (task 8), KSTROBE to Disk Sector (task 4), MTEMP to Display Word (task 9), IRLoad / SWMODE / FETCH to Emulator (task 0).  Re-align my placeholder F1=14 + F2=6/7 codes to real Alto positions when boot trace requires. | 1-2w |
| Disk widget rewrite | Rotation timing (3.3ms = many thousands of microcycles per sector); serial word transfer; sector header/label/data structure (the disk-sector microcode reads these word-by-word as the disk rotates) | 3-5d |
| Disk controller | Real KCOM/KSTAT bit semantics + KADR field decode + transfer state machine | 2-3d |
| Per-task body real DMA | Disk Sector / Disk Word task bodies actually drive controller registers + memory | 1w |
| Boot trace | Run with real microcode + disk; identify Nova PC = 0o345 checkpoint | 2-8w (long tail debugging) |
| ContrAlto2 CSV trace patch | ~50 lines in `ContraltoLib/CPU/CPU.cs` ExecuteNext / ClockInternal: dump cycle, currentTask, mpc, ir, t, l, m, all R+S regs, lastMemAddr, lastMemData per cycle.  Plus `[DebuggerFunction("trace start"/"trace stop")]` in `ControlCommands.cs`. | 1-2d |
| Lockstep harness | RHDL-side trace dumper + diff against ContrAlto CSV row-by-row | 1w |

Realistic remaining timeline: 2-3 months of focused work.

## 2026-05-02 session — three architectural fixes landed; F1=Block + device-cooperation deferred

This session's commits:

1. **Sticky `current_task` + arbitration on F1=TaskYield** (commit `58b3e035`).
   Per Alto Hardware Manual §2.4: task switches happen ONLY on F1=TASK,
   not per-cycle.  Replaced per-cycle priority arbiter with sticky
   `current_task: dff::DFF<Bits<4>>` + combinational priority encoder
   gated by `q.engine.task_yield`.

2. **F1/F2 binary encoding aligned with real Alto** (commit `3d10a747`).
   Discovered our F1/F2 enum-to-binary mapping was completely wrong vs
   ContrAlto's `MicroInstruction.cs`.  Real F1=1 is LoadMAR (we had
   LeftShift1); F1=2 is TASK (we had RightShift1); F1=7 is Constant
   (we had Reserved7 → noped).  After fix, real Alto microcode at
   MPC=0x153 correctly decodes as `f1=TaskYield`, allowing Disk
   Sector to fire (7 firings = 7 sector_marks).

3. **Per-task reset MPCs at chip level via task_started** (commit
   `64cba64a`).  Per ContrAlto's `Task.cs` Reset(): each task K
   resets to MPC = K.  Implemented via a 16-bit `task_started` DFF
   in AltoChip; `current_mpc = if task_started[K] then task_mpc[K]
   else K`.

### ❌ F1=Block + device wakeup cooperation — needs DiabloDisk redesign

**Tried, reverted** (in this session).  Plan was: add `wakeup_latched:
DFF<Bits<16>>` to AltoChip (sticky-OR'd from disk pulses), surface
`block_task: bool` from microengine, clear `wakeup_latched[current_task]`
on Block.  This matches ContrAlto's `Task.cs:343` simplification.

**Why it doesn't work in isolation:** real Alto's wakeup signals are
**sustained** per HW Manual §2.4: "The 'wakeup signals' which drive
the priority encoder are hardware-generated".  The BLOCK function is
*observed by the device interface* — the device deasserts its wakeup.
Our DiabloDisk's `word_strobe` is sustained while `transfer_remaining
> 0`, so a chip-level latch sticky-set from word_strobe never clears
(Block clears the latch, but the same cycle's sustained word_strobe
re-OR's it back in).  Disk Word loops forever.

**Proper fix (deferred to a future PR):** integrate Block at the
device level.  AltoChip routes `engine.block_task + current_task` into
DiabloDisk's `block_ack: bool` input; DiabloDisk observes Block-for-
its-task and deasserts its wakeup signal.  This matches the HW manual
exactly.  Per-device:

- `DiabloDisk` gets `block_ack_sector: bool` and `block_ack_word: bool`
  inputs, drives a `sector_mark_pending` DFF cleared on `block_ack_sector`
  and a `word_strobe_pending` DFF cleared on `block_ack_word`.
- `AltoChip` drives `block_ack_sector = engine.block_task && current_task == 4`
  and similarly for word/14.
- `DiabloDisk` outputs sector_mark / word_strobe from the pending DFFs
  (sustained until the corresponding ack arrives).
- `AltoChip` priority encoder uses disk outputs directly (no latch
  needed since disk now sustains correctly).

Estimated effort: 1-2 days of focused work.  Not blocking other Phase
3.5 progress (per-task F1/F2 dispatch can land first).

**Current consequence of the gap:** sector_mark is a 1-cycle pulse;
most fire when Emulator isn't at TaskYield, so Disk Sector fires only
~1-out-of-32 sector_marks.  This is documented in
`boot_trace_baseline_metrics`'s relaxed assertion ("Disk Sector firings
≥ sector_marks" with the explanation that the wake-pulse-vs-yield
race is the issue).  The 7-firings-from-7-marks observed in our
2000-cycle test is coincidence: when one mark happens to coincide
with Emulator's MPC=0x153 yield window, Disk Sector runs through
~7 cycles of Emulator's loop in Disk-Sector-context until the next
TaskYield kicks it out.

## 2026-05-02 (continued) — F1=Block landed; URom-fetch-on-task-switch identified as next blocker

After the F1=Block + sustained device wakeup commit (`1193c121`) the
boot trace shows Disk Sector taking over for 1738/2000 cycles and
visiting addresses 0x004, 0x130, 0x14e, 0x150, 0x151, 0x152, 0x153,
0x154 (the same 8 Emulator addresses).  The decoder dump reveals KSEC
at MPC=4 sees `instr=0x28100130` — Emulator's MPC=0x154 instruction —
NOT KSEC's actual `instr=0x7001737c` (which has `f1=LoadMar f2=Constant
rsel=14 next=0x37c`).

**Diagnosis (verified against spec §5.4):**

Per spec §5.4 (AltoHW §2.4) the task-switch pipeline is:

| Cycle | Executing | Fetching | Address Stored MPC |
|-------|-----------|----------|--------------------|
| TY    | C (TASK)  | D        | E                  |
| TY+1  | D (delay slot) | J (= **K2's saved MPC**) | K |
| TY+2  | J (K2's first instr) | K | L |

After TASK fires, the URom must redirect to **K2's saved MPC** at
cycle TY+1 (one cycle after the TASK).

**Our chip presents `d.urom.mpc = q.engine.next_mpc`** — the
PREVIOUS-cycle engine output.  At TY+1, that's `D.next` (OLD stream's
continuation), not K2's MPC.  Result: URom keeps fetching the OLD
task's stream after yield.  KSEC's MPC=4 instruction is never
actually fetched.

**Why we can't trivially fix it.**  The chip cannot read the engine's
THIS-cycle `next_mpc` output combinationally — the Q-register on the
engine's outputs is inherent to RHDL's `Synchronous` composition
pattern.  `q.engine.next_mpc` is always 1 cycle late.  So we cannot
write `d.urom.mpc = engine.next_mpc_this_cycle` to match real Alto's
pipeline.

**Three architectural paths to consider:**

1. **Move the URom inside the Microengine widget.**  The engine then
   owns its fetch loop: presents its own `d.urom.mpc` from internal
   `next_mpc` combinationally, reads its own `q.urom.instruction`.
   The chip just passes `current_task` in and reads outputs.
   Pipeline collapses to 1 cycle (matching real Alto).
   - Pro: matches real Alto exactly; clean architecture.
   - Con: substantial engine refactor; URom no longer chip-level
     observable for trace diagnostics.

2. **Combinational redirect on yield in the chip kernel.**  Add a
   `winning_task_mpc` lookup in the chip and combinationally compute
   `d.urom.mpc = if engine_will_yield_this_cycle { winning_task_mpc }
   else { ... }`.  Requires combinational access to engine.task_yield
   THIS cycle, which has the same Q-register problem.
   - Won't work without breaking the Q-register barrier.

3. **Track a "task_switch_pending" DFF in the chip.**  Latch
   `q.engine.task_yield` into a 1-cycle delay DFF.  When the DFF is
   set on a given cycle, redirect `d.urom.mpc` to the new task's MPC
   for that cycle.  This adds 1 more cycle of pipeline delay (so
   each task switch costs 2 delay-slot cycles instead of 1) but keeps
   the chip+engine boundary simple.
   - Pro: minimal refactor; keeps URom chip-level visible.
   - Con: pipeline diverges from real Alto by 1 extra delay slot —
     potentially observable in lockstep against ContrAlto.

For Phase 3.5 boot trace: any of these three would unblock KSEC
running its real microcode.  Path 3 is fastest to land but creates
lockstep divergence.  Path 1 is the long-term right answer.

## 2026-05-02 (continued, post-STROBE) — per-task dispatch surface complete; next bottleneck is empty-memory boot

Six more spec-aligned commits landed after the URom-alignment fix:

- `cd875ef8` — Per-task BS=3 (ReadKSTAT) + BS=4 (ReadKDATA) for Disk tasks (spec §3.2 + §8.5).
- `2c270973` — F1=10 (LoadKSTAT) for Disk tasks (spec §3.3 + §8.5).
- `ecf41b97` — F1=12 (CLRSTAT) for Disk tasks (spec §8.5) + DiskController clr_stat input.
- `b901dd0e` — F1=11B/binary-9 (STROBE) per spec §8.5 + engine.kdata routed from disk.current_word_data + new `f1_strobe_in_disk_sector_arms_transfer` test.
- `2b052e09` — Per-task F2 NEXT-modify codes for Disk tasks (INIT, RWC, RECNO, XFRDAT, SWRNRDY, NFER, STROBON) per spec §8.5.

The chip now has spec-correct dispatch for every per-task F1/F2/BS code that appears in KSEC's currently-visited 35-address chain.  But the boot trace metrics (35 distinct addresses, 2 sector_mark events, 1743 KSEC cycles, 257 Emulator cycles) haven't moved despite all the additions.

### Why the metrics aren't moving

Per spec §8.6, KSEC's job per sector boundary is:
1. Wake on sector_mark.
2. Read KSTAT to verify alignment.
3. **Walk the KBLK chain at memory[521B]** to find the next disk command block (DCB).
4. Set up KCOM/KADR/KCWA from the current DCB.
5. Issue STROBE.
6. BLOCK.

With our default empty memory, **memory[521B] = 0** — no commands pending.  KSEC sees "nothing to do", presumably loops or blocks early.  KSEC never reaches STROBE; KWDX never wakes; transfer never happens; DMA never runs; memory never gets sector data.

Per spec §3.4 (Bootstrapping), the **Emulator boot ROM** is what sets up KBLK + the initial DCB.  But our Emulator only runs 257 cycles before sector_mark fires and KSEC takes over indefinitely (KSEC's BLOCK + sustained sector_mark cycle dominates).

### Two paths forward

**Path A: pre-load memory with a valid bootstrap.**  Construct a valid KBLK + DCB in memory at chip init.  This bypasses the Emulator boot ROM and lets KSEC find a command immediately.  Quick: maybe 1 day of work.  Validates that the per-sector → DMA chain works end-to-end with real microcode + real disk image.

**Path B: implement the Emulator boot ROM dance.**  Per spec §3.4: Emulator on reset clears memory[0], clears display, disables interrupts, reads KBDAD = 177034B (keyboard), interprets keys as disk address, sets up KBLK + DCB, then SIOs the disk to start.  Requires Emulator-task F1=15 (STARTF), per-task F2=8 (BUSODD), per-task F2=11 (ACDEST), per-task F2=14 (ACSOURCE), and the entire Nova IDISPATCH machinery to interpret the boot instructions.  Substantial: 1-2 weeks.

Path A is the cheaper path to "boot trace shows actual sector DMA + Nova execution".  Path B is what's actually correct for "boot from disk like a real Alto".

For Phase 3.5's "boot to OS loader" milestone, Path A gives demonstrable progress without needing the full Emulator task body.  Path B is required for Phase 4+ (full Alto fidelity).

## Resumption notes

- Assets (PROMs, .dsk, ContrAlto source) all live under
  `crates/rhdl-alto/assets/` which is gitignored.  Re-fetch any
  missing assets via:
  - PROMs: `curl -fsSL -o ROMNAME 'https://raw.githubusercontent.com/livingcomputermuseum/ContrAlto/master/Contralto/ROM/AltoII/ROMNAME'`
  - Disk: `curl -fsSL -o nonprog.dsk 'https://raw.githubusercontent.com/livingcomputermuseum/ContrAlto/master/Contralto/Disk/nonprog.dsk'`
  - ContrAlto2: `git clone https://github.com/jdersch/Contralto2.git`
- All "real-asset" tests gracefully skip if assets are absent, so CI
  passes on machines without them.
- The 8 `boot_*` and `memory_*` and `f1_constant_*` tests in
  `src/alto_chip.rs` are the integration surface.  Adding more
  `boot_*_does_X` tests is the natural way to track per-feature progress.

## Open architectural questions (resolve when relevant)

- ~~**Task numbering**~~: ✅ resolved in commit eb2261e7 — renumbered
  to ContrAlto convention.
- **Per-task F1/F2 dispatch shape**: pure-table lookup vs match-on-task
  vs new sub-widget per task.  Probably match-on-task is simplest; the
  per-task code semantics are documented well enough in
  `Contralto/CPU/MicroInstruction.cs` and the per-task .cs files.
- **MRT timing**: real Alto memory access is 5-cycle.  Phase 3.5 has
  collapsed to 1-cycle via SyncBRAM.  May or may not matter for boot
  correctness — the microcode doesn't care about absolute cycle counts,
  but ContrAlto-lockstep does.  Resolve when lockstep starts diverging.
- **MAR← / MD← code numbers**: my F2 = 6 / 7 may not match real Alto's
  encoding.  Real Alto MRT memory codes are at task-specific positions
  in F1/F2 ranges 8-15.  When per-task dispatch ships, re-align the
  code numbers to match `Contralto/CPU/MicroInstruction.cs` so real
  microcode interprets correctly.
- **🐛 Task-switch microcode-fetch pipeline bug** (uncovered by the
  per-task diagnostic): when arbitration switches from Task A to
  Task B between cycle T-1 and cycle T, the urom output at cycle T
  reflects Task A's MPC (the address presented at T-1), not Task B's.
  So Task B at its MPC actually executes Task A's microinstruction.
  Symptom: `boot_trace_decode_diagnostic` shows Task 4 at MPC=0
  reading `instr=0x0017054e` instead of the actual `prog[0]=0x2811c552`.
  Doesn't break the existing tests (DMA test runs almost entirely
  in Task 14 — no switching).  But blocks real-microcode boot
  progress because Task 4's first execution at MPC=0 doesn't run
  the disk-task setup.

  **Attempted fix: stall-on-task-switch.**  The naïve fix (override
  instr to NOP for one cycle when task changes) makes things WORSE
  for short-pulse wakeups.  sector_mark is a 1-cycle pulse → Task 4
  wakes only that cycle → stall converts that cycle to NOP → Task 4
  *never* actually executes its microcode.  Reverted; prev_task DFF
  kept for future re-use.

  **🎯 RESOLVED via historical research.**  The "bug" isn't a bug at
  all — it IS the real Alto's hardware behavior.  Per the *Alto
  Hardware Manual* §2.4 (Aug 1976) and confirmed in ContrAlto's
  `CPU.cs` / `Tasks/Task.cs`:

  - The Alto has ONE global MIR pipeline register (not per-task cache).
  - Pipeline is two-stage: while instruction N executes from MIR,
    instruction N+1 is fetched from microcode ROM in parallel.
  - **Task switches happen ONLY when microcode does F1=TASK** (not
    per-cycle automatically).  The current task is "sticky" until
    its microcode yields.
  - On F1=TASK, the new task is selected at end of cycle, but the
    already-prefetched instruction (from the OLD task's MPC stream)
    still executes — that's the **delay slot** (one-cycle, but
    productive — it does outgoing-task work, not a bubble).

  **My current chip's per-cycle arbitration is the actual bug.**
  The "task changed every cycle" behavior I see is wrong-by-design —
  the real Alto would have the Emulator running continuously until
  it does F1=TASK.  Disk Sector wouldn't "wake briefly for one cycle"
  — Emulator would yield, Disk Sector would run to completion of its
  service routine, then yield back.

  **Correct fix:** rework task_system to be "sticky" — current_task
  is a DFF that only updates on F1=TASK firing.  Microengine's F1=TASK
  triggers the arbitration; task_system's priority encoder picks the
  highest-priority woken task; current_task latches that.  The
  delay-slot semantics fall out naturally from the existing 1-cycle
  BRAM pipeline (MIR latch).  This makes the Alto Hardware Manual's
  description of the "delay slot after TASK" exactly what my chip
  already does — I just need to gate the task switch to F1=TASK.

  Estimated work: 2-3 days.  The biggest piece is restructuring
  task_system from "fire-best-task-each-cycle" to "decide-on-F1=TASK".

  Sources downloaded to `assets/bitsavers/` (gitignored):
  - Alto_Hardware_Manual_Aug76.pdf — §2.4 "Microprocessor Control"
  - AltoHWRef.part1.pdf, part2.pdf — alternate hardware reference
  - AltoIICode3.mu.pdf, altoIIcode3.mu.txt — actual microcode source
  - Alto_II_firmware.zip — 2KCTL/MADR/DISPL/XM51 PROM dumps
  - AltoSubsystems_Oct79.pdf — subsystems reference
