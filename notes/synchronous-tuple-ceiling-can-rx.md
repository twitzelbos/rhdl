# `Synchronous` derive 12-tuple ceiling — CAN 2.0A receiver case (2026-04-29)

## Context

Attempted to implement `serial_bus::can_receiver` (the RX side of
the existing `serial_bus::can_master` TX widget).  CAN 2.0A
receiver state needs (at minimum):

1. Field-walk FSM state (`field`).
2. Bit index within the current field (`field_bit_idx`).
3. Bit-period sampling counter (`bit_phase_counter`).
4. Last sampled bit value (for stuff-bit run length).
5. Stuff-bit run length counter.
6. "Expecting stuff bit next" flag.
7. 11-bit ID accumulator.
8. 4-bit DLC accumulator.
9. 64-bit data accumulator.
10. 15-bit CRC computation register.
11. 15-bit received-CRC accumulator.
12. RTR bit latch.
13. IDE bit latch.
14. CRC-OK result latch.
15. Frame-complete pulse latch.
16. ACK drive flag.
17. `Constant<Bits<DIV_W>>` for the bit period.

Plus `state` for the FSM machinery (the `field` above), so 17
distinct DFFs / `Constant`s in the widget struct.

## What blocked it

Compile error:
```
error[E0277]: can't compare `(..., 17 inferred slots, ...)` with `(...)`
   --> crates/rhdl-fpga/src/serial_bus/can_receiver.rs:141:24
    |
141 | #[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
```

The `Synchronous` derive currently composes child sub-circuits as
a tuple in the auto-generated `Q` and `D` types.  Rust's standard
trait impls for tuples — including `PartialEq` — are only
generated for tuples up to 12 elements (12-tuple ceiling).
17-DFF widgets exceed that.

This is not a CAN-specific issue: it's the same ceiling the
`can_master` CHANGELOG entry noted as load-bearing for that
widget (which sits at 11 fields, deliberately under the limit).

## Why it can't easily be worked around in the widget

The natural workaround — pack multiple DFFs into one struct value
behind a single DFF — works for boolean flags but not for state
that's logically distinct.  E.g.:

- Packing `rtr_q` + `ide_q` + `crc_ok_q` + `frame_pulse` +
  `ack_drive` + `expecting_stuff` + `last_bit_q` into a single
  `dff::DFF<Bits<8>>` saves 6 fields, dropping us to 11.
- But the kernel then has to bit-mask in/out every read/write,
  which (a) is extremely verbose, (b) makes the code hard to
  audit against the CAN spec, and (c) hits the same RHIF-size
  explosion documented in `notes/kernel-macro-oom.md` because
  every flag access becomes an OR-of-shifted-bits compute.

The packing is also unsound for fields that need to be updated
non-atomically inside one cycle (e.g., setting `frame_pulse`
while leaving `crc_ok_q` alone).

## What needs to land before CAN RX is implementable cleanly

The `Synchronous` derive needs to emit a real generated struct
for `Q` / `D` instead of a raw tuple.  Once the macro emits a
named struct with all the fields, the standard `derive(PartialEq,
Clone, Debug, ...)` machinery handles arbitrary field counts —
the 12-tuple ceiling stops mattering.

This is a known follow-up tracked in `widget-roadmap.md` (look
for "12-tuple ceiling for `Synchronous` derive").  It's also
flagged as a load-bearing concern in the can_master CHANGELOG
entry and in `fsm-architecture.md`'s discussion of widget
composition.

## Same constraint also blocks

- **CAN 2.0A receiver** (this widget — 17 fields).
- **Full CAN node combining can_master + can_receiver + error
  management** — would balloon to ~25 fields.
- **SCSI Parallel target/initiator** — needs CDB buffer, data
  buffer, sequence number, status latch, message-out / message-in
  registers, REQ/ACK timing counters, etc.; estimated 20+ fields.
- **Modbus RTU slave** (also blocked by the constraints in
  `notes/kernel-language-constraints-modbus.md`) — the buffer
  model adds 4-5 more fields beyond what the master needs,
  pushing it past 12.

Landing the `Synchronous`-derive-emits-real-struct fix unblocks
all four widgets at the field-count level.  (Modbus also needs
the array / reference / IR-explosion fixes from the Modbus note;
SCSI inherits both.)

## Action items

- [ ] Modify `rhdl-macro-core` `Synchronous` derive to emit a
  named generated struct for `Q` and `D` instead of a tuple.
  All standard derives then work for arbitrary field counts.
- [ ] Once landed, return to:
  - CAN 2.0A receiver (this attempt's draft saved in git history
    of the `refactor/use-fsm-and-or-patterns` branch).
  - Full CAN node with error management (REC/TEC, bus-off).
  - SCSI Parallel widgets.
  - Modbus slave (after also addressing the Modbus-specific
    constraints).
