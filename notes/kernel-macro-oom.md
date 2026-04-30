# `#[kernel]` macro OOM — MIDI parser case (2026-04-29)

## Symptom

Running `cargo build --package rhdl-fpga` with the original MIDI
parser kernel (commit-staged version, before the `Bits<5>` code
refactor) produced:

```
error: could not compile `rhdl-fpga` (lib); 27 warnings emitted

Caused by:
  process didn't exit successfully: `... rustc ...` (signal: 9, SIGKILL: kill)
```

`SIGKILL` here is the OS killing rustc for memory exhaustion (>~16 GB
on the dev machine).  No diagnostic before the kill.

## What triggered it

The kernel constructed `MidiMessage` values in many code paths
(15+ places: every message kind), where `MidiMessage.kind` was a
22-variant Rust enum (`MidiKind::None`, `NoteOn`, `NoteOff`, ...,
`SystemReset`).  A representative shape:

```rust
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub enum MidiKind {
    #[default] None,
    NoteOff, NoteOn, PolyAftertouch, ControlChange,
    ProgramChange, ChannelAftertouch, PitchBend,
    MtcQuarterFrame, SongPosition, SongSelect, TuneRequest,
    SysExStart, SysExByte, SysExEnd,
    TimingClock, Start, Continue, Stop, ActiveSensing, SystemReset,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MidiMessage {
    pub kind: MidiKind,
    pub channel: Bits<4>,
    pub data1: Bits<8>,
    pub data2: Bits<8>,
}

#[kernel]
pub fn midi_parser(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    // ... ~15 places do this:
    let mut msg = MidiMessage::default();
    msg.kind = MidiKind::NoteOn;        // ← one of 22 variants per site
    msg.channel = (ls & bits::<8>(0xF)).resize::<4>();
    msg.data1 = byte;
    msg.data2 = bits::<8>(0);
    d.out_message = msg;
    // ...
}
```

The `#[kernel]` macro lowers each `MidiMessage::default()` +
field-by-field assignment + final read to RHIF as a chain of
Splice ops on a Struct, with the `kind` field's enum
representation expanded for every variant assignment.  Repeating
this pattern 15 times in one kernel body, with the 22-variant
enum present at every site, multiplies the IR by ~330 nominal
nodes per assignment, and the macro's intermediate AST
representation explodes faster than that.

The crash happened during macro expansion (the `#[kernel]` proc-macro
in `rhdl-macro-core`), not during type checking or codegen.

## Reproduction (regression case)

The pre-refactor kernel is preserved at
`notes/kernel-macro-oom-repro.rs.txt` (literal `.rs.txt` so it
isn't compiled as part of the workspace).  To reproduce:

1. Drop the file's contents into
   `crates/rhdl-fpga/src/serial_bus/midi_parser.rs` (overwriting the
   shipped version).
2. `cargo build --package rhdl-fpga`.
3. Watch memory: `/usr/bin/time -l cargo build --package rhdl-fpga`
   (macOS) or `/usr/bin/time -v cargo build --package rhdl-fpga` (Linux).

Expected outcome: rustc memory grows past available RAM, OS kills
the process with SIGKILL.  On a 16 GB machine this happens within
~30 seconds of starting compilation of `rhdl_fpga`.

## Mitigation (in shipped code)

Replaced the 22-variant `MidiKind` enum with `Bits<5>` code
constants (`MIDI_KIND_NONE`, `MIDI_KIND_NOTE_ON`, ...).  The
`MidiMessage` struct now has `kind: Bits<5>` instead of `kind:
MidiKind`.  Build memory dropped to normal levels.

This keeps the host-actionable information identical (every message
distinction is preserved) while flattening the macro's IR-expansion
path: `Bits<5>` is a primitive that doesn't require per-variant
expansion.

## Pattern to watch for

The OOM is triggered by the *combination* of:

1. A wide enum (~20+ variants).
2. The enum used as a struct field.
3. The struct constructed in many code paths within one kernel.
4. Field-by-field assignment style (`mut` + `msg.field = ...`).

Any of those alone is fine.  All four together → exponential
expansion → OOM.

## What the compiler should do (long-term)

The `#[kernel]` macro should detect this expansion pattern and
either:

- Lift the per-variant assignment to a single look-up table /
  case-on-arg construction (so 22 variants ≠ 22× code size).
- Surface a clear diagnostic well before OOM (e.g., "kernel
  `midi_parser` produces > N RHIF nodes; consider refactoring;
  see `notes/kernel-macro-oom.md`").
- Switch to streaming AST emission so memory stays bounded.

Tracked as a non-NECESSARY follow-up in the FSM extractor PR's
CHANGELOG entry — the workaround is well-documented and easy to
apply, so the immediate priority is the diagnostic + the macro-
emission optimisation, not changing the user-facing kernel
language surface.
