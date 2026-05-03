# RCStream — the canonical typed streaming bus

`RCStream<T, F>` is RHDL's canonical typed, latency-insensitive
streaming bus.  The "RC" prefix names the two design properties the
bus inherits: **R**HDL's type system and **C**arloni's
latency-insensitive-design (LID) theorem.

It carries:

- a **payload** of type `T`,
- an optional **framing marker** of type `F`,

paired with a `bool` ready signal in the opposite direction.  It is
correct-by-construction under arbitrary pipeline insertion (the
[`RCStreamRelay`](#pipelining-with-rcstreamrelay) is its native
pipeline-stage primitive), drops or replaces every awkwardness of
AXI4-Stream, and falls out naturally from RHDL's existing type
system.

The full design rationale lives in `stream-bus-architecture.md` at
the repository root.  This chapter is the user-facing reference for
how to **use** the type today.

## The type

```rust
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct Item<T: Digital, F: Digital> {
    pub data: T,
    pub frame: F,
}

#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct RCStream<T: Digital, F: Digital> {
    pub data: Option<Item<T, F>>,
    pub ready: bool,
}
```

Three deliberate properties:

1. **Validity is `Option`-encoded.**  `data: None` means idle (no
   item this cycle, the AXI4-Stream `TVALID = 0` equivalent);
   `data: Some(item)` means a valid item is being transmitted
   (`TVALID = 1`).  There is no separate `valid` signal — the
   `Option`'s discriminant carries it.
2. **The framing parameter `F` is generic.**  It replaces TLAST,
   TUSER, TID, TDEST, TKEEP, and TSTRB combined with a single
   typed slot.
3. **The handshake is the standard AXI Ready/Valid contract.**
   `ready` MAY depend combinationally on `data.is_some()`, but
   `data.is_some()` MUST NOT depend combinationally on `ready`.
   This is the one rule that makes [`RCStreamRelay`](#pipelining-with-rcstreamrelay)
   insertion always sound.

## Common framing patterns

The `F` parameter is the part of the design that most decisively beats
AXI4-Stream.  The widget author picks the framing form that fits the
protocol; the type system enforces the contract.

| `F` | AXI4-Stream equivalent | Use case |
|---|---|---|
| `()` | TLAST = 0, TUSER unused | Pure data stream, no framing |
| `bool` | TLAST | End-of-frame marker |
| `Channel` (`Digital` enum) | TDEST | Multi-channel multiplex |
| `b8`, `b16`, ... | (no AXI equivalent) | Sequence numbering |
| custom struct with last/seq/error/etc. | TUSER | Sideband flags |

Examples:

```rust,ignore
RCStream<b32, ()>          // pure data stream, no framing
RCStream<b32, bool>        // TLAST-equivalent end-of-frame marker
RCStream<b32, Channel>     // multi-channel multiplex
RCStream<b32, Marker>      // rich framing (Marker: struct with last/seq/error)
RCStream<Packet, ()>       // sum-typed payload (Packet: enum with variants)
```

The last form is the one AXI4-Stream cannot natively represent: a
stream of `enum Packet { Header { ... }, Payload { ... }, Footer { ... } }`
becomes a typed bus where the variant tag is part of the payload's
type.  Mismatched variant layouts at the two ends of a connection
become compile errors instead of silent data corruption.

## Byte-keep semantics, handled by the type

Since `T` can be any `Digital` type, byte-keep semantics are just typed
payloads:

```rust,ignore
RCStream<[Option<b8>; 4], bool>  // 4-lane byte stream, per-lane validity, end-of-frame
RCStream<[b8; 4],         bool>  // 4-lane byte stream, all lanes always valid
RCStream<[Lane; 8],       ()>    // 8-lane stream of arbitrary `Lane`-typed items
```

No TKEEP / TSTRB on the wire.  The type carries the validity.  If you
want all-or-nothing per cycle, use `[T; N]`; if you want per-lane
validity, use `[Option<T>; N]`; if your validity has more structure
(e.g., "this lane is data, this lane is sideband, this lane is empty"),
use a custom enum.

This is strictly more expressive than AXI4-Stream because the type
system can carry validity information that AXI4-Stream's two-bit-per-byte
encoding cannot.

## Pipelining with RCStreamRelay

The killer property: **a `RCStream<T, F>` connection can have a Carloni
relay inserted anywhere in the data path without changing functional
behavior**.

```rust,ignore
use rhdl_fpga::stream::RCStreamRelay;

let r: RCStreamRelay<b32, ()> = RCStreamRelay::default();
```

The relay's input is a `RCStream<T, F>`, its output is a `RCStream<T, F>`
(same `T`, same `F`), and the only effect is to add **one cycle of
latency**.  Throughput is unchanged.  This is Carloni's theorem from
the 1999 DAC paper, operationalized — see [`carloni`][carloni-doc] for
the underlying skid-buffer FSM.

Use the relay any time:

- A TVALID/TREADY combinational path is a timing-closure concern.
- An auto-pipeliner needs a sound cut point at an inter-kernel
  boundary (per `auto-pipelining-plan.md`, `RCStream` boundaries are
  the *preferred* cut point because no hazard analysis is required).
- A vendor IP block needs a registered Ready/Valid handshake at its
  boundary.

[carloni-doc]: ../../crates/rhdl-fpga/src/lid/carloni.rs

## Construction helpers

The `bus` module provides kernel-callable helpers for the common
construction idioms:

```rust,ignore
use rhdl_fpga::stream::bus::{idle, send, item, item_unframed};

// An idle cycle with backpressure
let s = idle::<b8, ()>(true);  // data=None, ready=true

// A cycle carrying an item
let it = item_unframed::<b8>(bits::<8>(0x42));
let s  = send::<b8, ()>(it, true);  // data=Some(item), ready=true

// With framing
let it = item::<b32, bool>(bits::<32>(0xFEEDFACE), true /* TLAST */);
let s  = send::<b32, bool>(it, true);
```

## Synchronous-widget convention

`RCStream<T, F>` is used as a Synchronous-widget I/O type via the
following convention:

- When used as widget **`I`** (`SynchronousIO::I`):
    - `data` is the **upstream's data** flowing *in*.
    - `ready` is the **downstream's ready** flowing *in* (= "is
      downstream ready for me to send the next item?").
- When used as widget **`O`** (`SynchronousIO::O`):
    - `data` is the **widget's data** flowing *out* to downstream.
    - `ready` is the **widget's ready** flowing *out* to upstream
      (= "am I ready to accept the next item from upstream?").

The struct shape is identical for both directions; only the semantic
meaning of each field differs by role.  This matches the existing
`StreamIO<T, S>` pattern that the new `RCStream<T, F>` will
eventually subsume.

## Migration from `StreamIO<T, S>`

`StreamIO<T, S>` (the existing stream-widget I/O type) and
`RCStream<T, F>` coexist during the migration window.  See
`stream-bus-architecture.md` §9 for the full migration plan.  Phase
1.1 (this chapter) ships the new type alongside the old; subsequent
phases migrate the existing `stream::*` widgets one by one.

## What about AXI4-Stream interop?

Mandatory, not optional — but a separate subsystem.  Translation
widgets (`AxiStreamToRCStream<T, F>` and `RCStreamToAxiStream<T, F>`)
will live at the FPGA boundary, not pervasively throughout the design.
See `stream-bus-architecture.md` §10 for the design and the future
phase-1.4 milestone.

## See also

- `stream-bus-architecture.md` — full design rationale.
- [Carloni relay station][carloni-doc] — the underlying LID-paper-faithful
  skid-buffer.
- `auto-pipelining-plan.md` — how the auto-pipeliner uses `RCStream`
  boundaries as preferred cut points.
- `vendor-primitive-architecture.md` — how vendor primitives produce
  and consume `RCStream`-typed signals.
