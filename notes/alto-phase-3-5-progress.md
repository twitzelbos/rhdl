# Alto Phase 3.5 — progress notes (resumable across sessions)

Working branch: `feat/alto-phase-3-5-boot-to-os-loader`.  Single
branch, single PR per the user's "complete this phase, don't ship a
v1, don't defer" constraint.  PR opens only when **Nova PC = 0o345**
is reached with cycle-equivalent ContrAlto trace.

## Status (10 commits in)

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

### ⏳ Remaining for Phase 3.5

| Step | Description | Est |
|------|-------------|-----|
| Per-task F1/F2 dispatch | Add `current_task: Bits<4>` to microengine.In; per-task code dispatch (most importantly: gate MAR← / MD← to MRT (task 8 in real Alto), KSTROBE / KCOM / KADR← to Disk Sector (task 4), MTEMP to Display Word (task 9), IRLoad / SWMODE / FETCH to Emulator (task 0)) | 1-2w |
| Disk widget rewrite | Rotation timing (3.3ms = many thousands of microcycles per sector); serial word transfer; sector header/label/data structure (the disk-sector microcode reads these word-by-word as the disk rotates) | 3-5d |
| Disk controller | Real KCOM/KSTAT bit semantics + KADR field decode + transfer state machine | 2-3d |
| Task system task numbering | Renumber to ContrAlto convention: 0/4/7/8/9/10/11/12/13/14 (current scheme is sequential 0-15 which doesn't match) | 1d |
| `AltoChip` task system integration | Wire `AltoTaskSystem` as sub-widget, replace single-MPC DFF with per-task MPC ownership in task_system | 2-3d |
| Per-task body real DMA | Disk Sector / Disk Word task bodies actually drive controller registers + memory | 1w |
| Boot trace | Run with real microcode + disk; identify Nova PC = 0o345 checkpoint | 2-8w (long tail debugging) |
| ContrAlto2 CSV trace patch | ~50 lines in `ContraltoLib/CPU/CPU.cs` ExecuteNext / ClockInternal: dump cycle, currentTask, mpc, ir, t, l, m, all R+S regs, lastMemAddr, lastMemData per cycle.  Plus `[DebuggerFunction("trace start"/"trace stop")]` in `ControlCommands.cs`. | 1-2d |
| Lockstep harness | RHDL-side trace dumper + diff against ContrAlto CSV row-by-row | 1w |

Realistic remaining timeline: 2-3 months of focused work.

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

- **Task numbering**: my `task_system.rs` uses 0-15 sequentially; real
  Alto + ContrAlto use 0/4/7/8/9/10/11/12/13/14.  Renumber before
  wiring task_system into AltoChip.
- **Per-task F1/F2 dispatch shape**: pure-table lookup vs match-on-task
  vs new sub-widget per task.  Probably match-on-task is simplest; the
  per-task code semantics are documented well enough in
  `Contralto/CPU/MicroInstruction.cs` and the per-task .cs files.
- **MRT timing**: real Alto memory access is 5-cycle.  Phase 3.5 has
  collapsed to 1-cycle via SyncBRAM.  May or may not matter for boot
  correctness — the microcode doesn't care about absolute cycle counts,
  but ContrAlto-lockstep does.  Resolve when lockstep starts diverging.
