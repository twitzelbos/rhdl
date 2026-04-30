# SCSI Parallel Interface — deferred to follow-up (2026-04-29)

## Context

Asked to implement SCSI Parallel Interface widgets per the
agreed scope:

- **(A)** SCSI-2 async, 8-bit, target + initiator with the
  mandatory command set (TEST UNIT READY, REQUEST SENSE,
  INQUIRY, READ(6), WRITE(6), READ CAPACITY, MODE SENSE/SELECT).
- **(B)** Same as A + synchronous transfer + 16-bit wide data.

Estimated total scope: ~5000 LOC across both widgets, with
target and initiator each carrying:

- BUSY / SEL / ATN / MSG / I/O / C/D / REQ / ACK signal logic
  (the parallel SCSI handshake is non-trivial — 8 control
  signals plus 9 data signals plus parity).
- Bus-phase state machine: BUS FREE → ARBITRATION → SELECTION
  (or RESELECTION) → INFORMATION TRANSFER (Command, Data, Status,
  Message phases) → BUS FREE.
- Command Descriptor Block (CDB) buffer: 6, 10, 12, or 16 bytes
  depending on opcode group.
- Data buffer: up to 256 bytes typical.
- Status / Message / Identify register state.
- Per-target sequence number tracking (initiator side) for
  reconnect support.
- Synchronous transfer offset and period agreement (Sync mode,
  scope B) — REQ/ACK pulse pacing without per-byte handshake.

A widget at this scope needs roughly 20+ DFF fields per side
(target and initiator).

## What blocks it

This widget hits **all three** of the previously-documented
constraints simultaneously:

1. **Synchronous-derive 12-tuple ceiling**
   (`notes/synchronous-tuple-ceiling-can-rx.md`).  20+ fields per
   side > 12-element tuple ceiling.  Same blocker as CAN RX.
2. **Kernel-language array constraints**
   (`notes/kernel-language-constraints-modbus.md`):
   - The CDB buffer (up to 16 bytes) is fine.
   - The data buffer (up to 256 bytes) exceeds the 32-element
     `Default` ceiling.
   - CRC-style helpers walking the buffer can't take `&[T; N]`
     references.
3. **Kernel-macro IR-size explosion** (`notes/kernel-macro-oom.md`).
   The bus-phase FSM with per-phase state logic + CDB-decode arm
   per opcode (8 in the mandatory set) + status / message
   handling is structurally similar to (and probably larger
   than) the MIDI parser pre-refactor that triggered the OOM.

Any one of these would block clean implementation; all three
together makes attempting SCSI now actively counter-productive
(the time would be spent fighting the language rather than
implementing the protocol).

## What needs to land before SCSI is implementable

Same infrastructure fixes as the other deferred widgets, in
priority order:

1. **Synchronous derive emits a real struct** (not a tuple) for
   `Q` / `D`.  Removes the field-count ceiling.  Required by
   CAN RX, full CAN node, and SCSI.
2. **`Default` impl for `[T; N]` of arbitrary `N`** in
   `rhdl-bits` / `rhdl-core`.  Required by Modbus and SCSI.
3. **Allow immutable `&[T; N]` and `&T` in helper kernel
   arguments**.  Required by Modbus, SCSI, and any widget with
   array-walking helpers.
4. **Diagnose / mitigate kernel-macro IR-size explosion** for
   the many-arms-each-construct-buffers pattern.  Required by
   any large protocol parser.

Once those land, SCSI becomes ~5000 LOC of careful protocol
implementation — large but structurally tractable.

## Why deferring is the right move

Per CLAUDE.md TL;DR: shipping a "v1" SCSI widget that's blocked
by infrastructure constraints would either (a) work around the
constraints with brittle hacks (constraint 1 → packed flag bits;
constraint 2 → buffer chains in many DFFs; constraint 3 →
multi-cycle walker states), or (b) ship a sliver (e.g., "just
target side, just one command, just async").

(a) creates technical debt that's harder to remove than the
underlying constraint fix.  (b) is exactly the silent-v1 the
TL;DR rule forbids.

The honest move is to land the four infrastructure fixes in
focused PRs against `rhdl-macro-core` and `rhdl-bits`, then
return to SCSI as a single coherent piece.

## What's shippable in this PR's scope

This PR (`refactor/use-fsm-and-or-patterns`) ships:

- 8 new widgets in `serial_bus/` (SMPTE LTC decoder, MIDI
  parser, full PS/2 stack including device-side encoders).
- 67 new tests, all passing.
- 3 documented constraint notes (this one + Modbus + the OOM
  case + the tuple-ceiling case) with concrete action items
  for the infrastructure fixes.
- APB bus added to `widget-roadmap.md` as Tier 4 #64.

This is the slice of the original ask (CAN, MIDI, SMPTE, SCSI,
+ later additions: PS/2 stack expansion, Modbus full, PS/2
encoders) that's deliverable without infrastructure changes.

## Action items

- [ ] Land the four infrastructure fixes (consolidated list
  from this note + the Modbus note + the tuple-ceiling note).
- [ ] Then implement SCSI Parallel A, B, target, initiator,
  command set as a single coherent piece (~5000 LOC, single
  PR following CLAUDE.md §11.1).
