# RCStream Bus Architecture for RHDL — Design Plan

A proposal for a typed, latency-insensitive streaming bus to be the canonical inter-kernel data-flow interface in RHDL: **`RCStream<T, F, D>`** (RHDL-Carloni-Stream) — a stream of `T`-typed items, optionally carrying framing markers of type `F`, in clock domain `D`. The "RC" prefix is load-bearing: it names the two design properties the bus inherits — RHDL's type system and Carloni's latency-insensitive-design theorem — and it disambiguates the type from `futures::Stream` and AXI4-Stream when both might appear in the same code review. The bus is correct-by-construction under arbitrary pipeline insertion (Carloni relay stations are its native pipeline-stage primitive), drops or replaces every awkwardness of AXI4-Stream, and falls out naturally from RHDL's existing type system.

This is the fifth compiler-and-language design plan, alongside `auto-pipelining-plan.md`, `kernel-language-extensions.md`, `vendor-primitive-architecture.md`, and `fsm-architecture.md`. Like those, it is independently shippable in phases. It also has the strongest interlocks with the others: it is the *substrate* the auto-pipeliner operates on, the natural carrier for FSM outputs, and the receiver of vendor-specific primitives' streamed results.

The plan rests on three observations. First, half of the existing `rhdl-fpga::stream::*` widgets and the `axi4lite::channel::{sender, receiver}` primitives are already implementing this pattern under different names — what's missing is the unifying type and the design story. Second, the `lid::carloni` relay station already provides the LID-correct pipeline-insertion primitive that makes the auto-pipeliner's job trivial at inter-kernel boundaries. Third, AXI4-Stream is the de-facto industry interconnect we have to interoperate with, but its wire-level untyped TDATA + magic TKEEP/TSTRB/TUSER fields cause a measurable fraction of FPGA design bugs that RHDL's type system would catch by construction.

---

## 1 — Motivation

Every non-trivial RHDL design sends data between kernels. Today that data flow is expressed in three different idioms:

- The `stream::*` widgets, which use a `StreamIO<T>` type that pairs a `DataValid<T>` with a `Ready` in the opposite direction. This is the de-facto kernel-to-kernel interconnect for designs that compose stream operators (map, filter, zip, ...).
- The `axi4lite::channel::{sender, receiver}` widgets, which formalize the same Ready/Valid handshake as a generic-over-`T` channel. Used inside the AXI4-Lite endpoint and switch.
- Hand-rolled per-widget I/O, where individual widgets define `In<T>` and `Out<T>` structs with their own conventions (e.g., FIFO's `data: Option<T>` plus separate `next` and `full` signals).

These three idioms are the *same* concept under three different names. They are also the same concept as **AXI4-Stream**, the AMBA-family streaming protocol that is the lingua franca for inter-IP data flow in the FPGA industry. AXI4-Stream is AXI's Ready/Valid handshake plus a small bag of marker fields (TLAST, TKEEP, TSTRB, TID, TDEST, TUSER) layered on top.

The cost of having three names for the same thing is real. Widget authors choose ad hoc which idiom to use; cross-widget composition requires translation widgets; and the framework loses the ability to reason structurally about inter-kernel data flow. The cost of *not* having a typed bus that interoperates with AXI4-Stream is also real: every commercial IP block exposes AXI4-Stream, and an RHDL design that wants to use commercial IP needs translation widgets at every boundary.

A single, named, typed, LID-correct bus type — `RCStream<T, F, D>` — solves both problems. It is the canonical RHDL inter-kernel interconnect *and* the substrate from which AXI4-Stream interop wrappers are derived as a special case. It makes the auto-pipeliner's job at inter-kernel boundaries trivial because the LID semantics give register insertion as a free operation. And it lets the type system carry information that AXI4-Stream punts to out-of-band convention — payload schema, framing semantics, clock domain, byte-keep granularity, channel multiplexing.

---

## 2 — What's wrong with AXI4-Stream

A specific, concrete enumeration of AXI4-Stream pain points, drawn from the AMBA AXI4-Stream Protocol Specification (ARM IHI 0051A) and from years of community experience with the protocol. Each point is a place where a typed RHDL bus would do strictly better.

**1. Untyped at the wire level.** TDATA is `[N-1:0]`. The bus carries zero structural information; both ends must agree on the layout out-of-band. Schema mismatches surface as silently corrupt data with no compiler-detectable error. This is the single largest source of AXI4-Stream bugs in practice.

**2. TKEEP and TSTRB are byte-granular and almost-redundant.** TKEEP marks "this byte is part of the packet" (vs. trailing pad bytes); TSTRB marks "this byte is data" (vs. position bytes). Together they form a four-state encoding (data / position / null / reserved) that almost no one uses fully — most designs use TKEEP only and ignore TSTRB. Both signals assume byte-aligned data, which is awkward for non-byte-multiple element types.

**3. TLAST is the only first-class framing primitive.** Anything more structured (start-of-frame, sub-frame markers, frame-type tags) has to be encoded into TUSER, which is wholly user-defined.

**4. TUSER is a free-for-all.** Width and semantics are implementation-defined. Cross-IP interop is brittle; this is the second-largest source of "the simulation worked but the silicon doesn't" bugs in AXI4-Stream designs.

**5. TID and TDEST are flat integer namespaces.** No type structure. Multiplexing a stream of `Result<Header, ParseError>` over the same physical channel requires manual encoding into TID with no compiler-enforced invariant that the layouts agree at both ends.

**6. TVALID/TREADY combinational paths are the most common AXI4-Stream timing-closure problem.** The AMBA spec says TREADY *may* depend combinationally on TVALID but TVALID *must not* depend on TREADY. Most third-party IP gets this wrong at least once. The fix is always a Carloni-style relay station, which AXI4-Stream does not provide as a first-class primitive — every IP vendor reinvents it.

**7. No clock-domain typing.** AXI4-Stream signals are just wires. CDC is "use a CDC FIFO" by convention; nothing prevents a designer from cabling a stream from one domain into a sink in another.

**8. Width parameterization is per-stream, not per-element.** A stream of `(b8, b8, b16)` triples must pick TDATA width once and pack manually. Width converters between two semantically-identical streams are a mandatory piece of plumbing.

**9. The data-width-converter problem.** When two stream segments have different TDATA widths, an explicit "data width converter" widget is required, even though the *logical* data type is identical. Converters are the most common pieces of AXI4-Stream IP and the second most common source of bugs (after TUSER mismatches).

**10. Lowest-common-denominator interop.** The structural advantage of AXI4-Stream is that everyone's IP speaks it. We give that up if we don't provide a translation widget.

Where AXI4-Stream is genuinely better: third-party IP interop, Vivado / Quartus IP Integrator routing, existing engineer familiarity. The design plan must address (1) and (2) explicitly via translation widgets and explicit migration documentation.

---

## 3 — What RHDL already has

The substrate is more complete than first appears. Three components that already exist:

- **`axi4lite::channel::{sender, receiver}`.** A generic Ready/Valid channel over `T: Digital`. The wire-level pair is `DataValid<T> { data: T, valid: bool }` source→sink and `Ready { ready: bool }` sink→source. This is the typed Ready/Valid that AXI4-Stream wishes it were.
- **`stream::*`.** A widget library — `map`, `filter`, `filter_map`, `flatten`, `zip`, `tee`, `chunked`, `stream_buffer`, `fifo_to_stream`, `stream_to_fifo`, `pipe_wrapper`, `xfer` — that compose like Rust iterators. The handshake is implicit in the `StreamIO<T>` type.
- **`lid::carloni`.** Carloni's relay station from the 2015 *Proceedings of the IEEE* paper. The 3-signal interface (data / void / stop) is the LID-paper-faithful version of Ready/Valid. The relay-station FSM (Run / Stall states, main + auxiliary registers) is the canonical pipeline-insertion primitive that adds latency without changing throughput or functional behavior.

What's missing is the *unifying type*. The three components above use three different signal-naming conventions for the same protocol. A user assembling a non-trivial design walks through a translation matrix in their head every time they cross a category boundary.

What's also missing is the *story*. The connection between Carloni's LID theorem, the existing stream cores, the AXI4-Lite handshakes, and the auto-pipelining track is implicit. Making it explicit unlocks formal-equivalence reasoning at inter-kernel boundaries, which is exactly what auto-pipelining needs.

---

## 4 — Design principles

Five principles, each non-negotiable.

**Typed payload, typed framing, typed domain.** The bus type is `RCStream<T: Digital, F: Digital, D: Domain>`. Payload schema is the type `T`. Framing semantics are the type `F`. Clock domain is the phantom-typed parameter `D`. Mismatches at any of the three are compile errors.

**Latency-insensitive by construction.** The protocol has the Carloni LID property: a relay station can be inserted on any `RCStream` connection without changing functional behavior. This is what makes the auto-pipeliner sound at inter-kernel boundaries.

**No magic fields.** Every wire signal has a typed meaning derived from `T`, `F`, or `D`. There is no equivalent of TKEEP / TSTRB / TUSER — semantic information that AXI4-Stream punts to convention is a typed field in `T` or `F`.

**Compose, don't translate.** Every existing `stream::*` widget should adopt the `RCStream<T, F, D>` type with no semantic change. AXI4-Stream interop happens in dedicated translation widgets at the FPGA boundary, not pervasively throughout the design.

**Idiomatic at the kernel level.** Kernels operate on `Option<Item<T, F>>`-typed signals. The protocol becomes invisible inside `#[kernel]` bodies — the kernel sees a typed payload and decides whether to consume it.

---

## 5 — The proposed type

```rust
/// A typed, latency-insensitive stream of T-typed items, optionally carrying
/// framing markers of type F, in clock domain D.
///
/// At the wire level: `Option<Item<T, F>>` source -> sink, paired with a
/// `bool` ready signal sink -> source. `None` on the data line means idle
/// (TVALID = 0); `Some(item)` means data this cycle (TVALID = 1). The Carloni
/// relay station is the canonical pipeline-insertion primitive: a relay
/// inserted on any `RCStream` connection adds one cycle of latency without
/// changing functional behavior or throughput.
///
/// `T` is the payload type. `F` is the framing-marker type — `()` for streams
/// without per-item framing, `bool` for TLAST-equivalent end-of-frame markers,
/// or any `Digital` enum/struct for richer framing semantics.
pub struct RCStream<T: Digital, F: Digital, D: Domain> {
    /// Source -> sink. `None` = idle, `Some(item)` = data this cycle.
    pub data: Signal<Option<Item<T, F>>, D>,
    /// Sink -> source. `true` = ready to accept the next item.
    pub ready: Signal<bool, D>,
}

#[derive(Digital, Copy, Clone, PartialEq, Debug)]
pub struct Item<T: Digital, F: Digital> {
    /// Payload data.
    pub data: T,
    /// Framing marker. `()` for streams without per-item framing.
    pub frame: F,
}
```

Three deliberate properties:

1. **The validity bit is `Option<Item<T, F>>::is_some()`.** No separate TVALID. The type carries it. The protocol is, literally, an `Option`-encoded signal in one direction and a `bool` in the other.
2. **The framing parameter `F` is generic.** It replaces TLAST, TUSER, TID, TDEST, TKEEP, and TSTRB combined.
3. **The clock domain `D` is part of the type.** Crossing domains without explicit retiming is a compile error, exactly like every other signal in RHDL.

Default for `F` is `()` (no framing). Common idioms:

```rust
RCStream<b32, (),       Red>  // pure data stream, no framing
RCStream<b32, bool,     Red>  // TLAST-equivalent end-of-frame marker
RCStream<b32, Channel,  Red>  // multi-channel multiplex (Channel: Digital enum)
RCStream<b32, Marker,   Red>  // rich framing (Marker: struct with last/seq/error)
RCStream<Packet, (),    Red>  // sum-typed payload (Packet: enum with variants)
```

The last form is the one AXI4-Stream cannot natively represent: a stream of `enum Packet { Header { ... }, Payload { ... }, Footer { ... } }` becomes a typed bus where the variant tag is part of the payload's type.

---

## 6 — Framing patterns in detail

The framing parameter `F` is the part of the design that most decisively beats AXI4-Stream. Catalogued patterns:

**No framing — `F = ()`.** Streams that don't have per-item structure. A pixel pipeline, a continuous audio sample stream, a register-file-style observation tap. AXI4-Stream equivalent: TLAST always 0, TKEEP always all-ones, TUSER unused.

**End-of-frame — `F = bool`.** A boolean per item indicating "this is the last item of the current frame." AXI4-Stream equivalent: TLAST.

**Channel multiplex — `F = Channel`** for a `Digital` enum `Channel`. Multiple logical streams sharing one physical bus. AXI4-Stream equivalent: TDEST (but typed instead of an integer namespace).

**Sequence numbering — `F = b8`** or wider. Each item carries an explicit position. Useful for protocols where the receiver needs to detect drops or reorderings.

**Sideband flags — `F = Marker`** for a `Digital` struct with last/error/seq/etc. fields. AXI4-Stream equivalent: TUSER, but typed.

**Variant-shaped streams — `F = ()` with `T = Packet`** (an enum-with-payload). The payload itself is sum-typed; framing is implicit in which variant is being transmitted. Useful for protocols like Ethernet where the stream carries a sequence of variant-typed records (Header → multiple Payload → Footer). AXI4-Stream cannot represent this without TID + TUSER hacks.

**Combined — `F = Marker` and `T = Packet`.** Both payload and framing are ADTs. The fully expressive form.

The widget author picks the framing form that fits the protocol; the type system enforces the contract. There is no equivalent of "TUSER mismatch between two IP blocks" because mismatched `F` is a compile error.

---

## 7 — Byte-keep semantics, handled by the type

Since `T` can be any `Digital` type, byte-keep semantics are just typed payloads:

```rust
RCStream<[Option<b8>; 4], bool, Red>  // 4-lane byte stream, per-lane validity, end-of-frame marker
RCStream<[b8; 4],         bool, Red>  // 4-lane byte stream, all lanes always valid, end-of-frame
RCStream<[Lane; 8],       (),   Red>  // 8-lane stream of arbitrary `Lane`-typed items
```

No TKEEP / TSTRB on the wire. The type carries the validity. If you want all-or-nothing per cycle, use `[T; N]`; if you want per-lane validity, use `[Option<T>; N]`; if your validity has more structure (e.g., "this lane is data, this lane is sideband, this lane is empty"), use a custom enum.

This is strictly more expressive than AXI4-Stream because the type system can carry validity information that AXI4-Stream's two-bit-per-byte encoding cannot.

---

## 8 — Composition with LID and the Carloni relay

The killer property: **a `RCStream<T, F, D>` connection can have a Carloni relay inserted anywhere in the data path without changing functional behavior**.

Concretely, the relay station's input is a `RCStream<T, F, D>`, its output is a `RCStream<T, F, D>` (same `T`, same `F`, same `D`), and the only effect is to add one cycle of latency. This is Carloni's theorem from the 1999 DAC paper, operationalized.

```rust
pub struct RCStreamRelay<T: Digital, F: Digital, D: Domain> { /* ... */ }

impl<T: Digital, F: Digital, D: Domain> SynchronousIO for RCStreamRelay<T, F, D> {
    type I = RCStream<T, F, D>;
    type O = RCStream<T, F, D>;
    type Kernel = stream_relay_kernel<T, F, D>;
}
```

The relay is a thin wrapper around the existing `lid::carloni::Carloni` widget, parameterized to operate on `RCStream<T, F, D>` rather than the LID-paper-faithful `data/void/stop` 3-signal triple.

For the auto-pipeliner (per `auto-pipelining-plan.md`):

- Every `RCStream` boundary in the NTL graph is a *zero-cost cut point*. The auto-pipeliner can insert a `RCStreamRelay` there with no hazard analysis, no functional verification, no semantic reasoning. The LID theorem guarantees correctness.
- Phase 2 of auto-pipelining (stateful kernels with hazard analysis) becomes dramatically easier: the hazard analysis only has to consider intra-kernel feedback paths because inter-kernel paths are LID-correct by construction.
- The auto-pipeliner can preferentially place relays at `RCStream` boundaries (cheap, sound) before considering arbitrary NTL cuts (expensive, requires hazard analysis).

This is the single biggest architectural payoff of formalizing the bus.

---

## 9 — Composition with the existing stream library

> **Decision (post-Phase-1.1 ship):** the existing `rhdl-fpga::stream::*` widgets are NOT being migrated to `RCStream<T, F>`.  `rcstream` lives in parallel to `stream` as an opt-in bus type for **new** widgets that want the typed-framing-marker / typed-clock-domain / LID-correct properties.  The earlier "migrate every widget" plan in this section is preserved below for historical context but is no longer the project plan.
>
> The `rcstream` and `stream` modules will coexist indefinitely — `stream::*` widgets continue to use `StreamIO<T, S>`; new widgets that prefer the typed bus opt into `rcstream::{Item, RCStream, RCStreamRelay}`.  No `StreamIO<T, S>` deprecation, no breakage.

**Original Phase 1 plan (for historical context — NOT being executed):**

The original plan was to migrate widgets one by one, with each existing widget getting a generic-over-`F` upgrade — `stream::map` becoming `Map<T, S, F, D, K>` where `F` flows through unchanged, existing call sites instantiated with `F = ()`, etc.  After review the project decided this migration cost outweighed the benefit: existing `stream::*` widgets work, are tested, and have downstream consumers.  The typed-bus value comes from new widgets that explicitly want it, not from retrofitting old ones.

**What actually shipped in Phase 1.1 (PR #51):**

- `RCStream<T, F>` and `Item<T, F>` types in a new `rcstream` module (parallel to `stream`, NOT inside `stream`).
- `RCStreamRelay<T, F>` widget wrapping the existing `lid::carloni::Carloni` with the typed encoding.
- Book chapter `doc/book/src/rcstream/bus.md`.
- Convenience re-exports `rhdl_fpga::rcstream::{Item, RCStream, RCStreamRelay}`.

Existing `stream::*` widgets and `axi4lite::*` (including `axi4lite::stream::axi_to_rhdl`) are unchanged.

---

## 10 — AXI4-Stream interop

> **Decision (post-Phase-1.1 ship):** AXI4-Stream interop *is* planned, but built **inside `rcstream/`** as a new sub-module (e.g., `rcstream::axi_stream`), independent of and parallel to the existing `axi4lite::stream::{axi_to_rhdl, rhdl_to_axi}` widgets.  The existing widgets stay unchanged (they translate AXI ↔ `StreamIO<T, S>`); the new widgets translate AXI ↔ `RCStream<T, F>`.  Two parallel translation paths, no migration of existing code.

The translation widgets:

```rust
/// Wraps an AXI4-Stream master input as an RHDL `RCStream<T, F>` source.
/// Lives at `rhdl_fpga::rcstream::axi_stream::AxiStreamToRCStream`.
pub struct AxiStreamToRCStream<T: Digital, F: Digital> {
    /* internal: bit-pack T into TDATA, F into TUSER+TLAST */
}

/// Wraps an RHDL `RCStream<T, F>` source as an AXI4-Stream master output.
/// Lives at `rhdl_fpga::rcstream::axi_stream::RCStreamToAxiStream`.
pub struct RCStreamToAxiStream<T: Digital, F: Digital> {
    /* internal: unpack TDATA into T, TUSER+TLAST into F */
}
```

Each translation widget is ~80 LOC.  They handle TKEEP packing for byte-aligned `T`, encode `F` into TUSER+TLAST per a documented schema, and produce/consume the AXI4-Stream signal set.  The schema is itself documented as part of the widget rustdoc and exported as a `Digital`-derived type so external IP can consume it via the same encoding.

The round-trip property is the validation criterion: `axi → RCStream<T, F> → axi` must produce a byte-identical waveform on the AXI side.  If the translation loses information, that is a bug in the schema or the widget.

For the typed-stream-of-`enum`-Packet case where AXI4-Stream cannot natively represent the variant tag, the translation widget bit-packs the variant discriminant into TUSER and the payload into TDATA per a documented schema.

**Why parallel to `axi4lite::stream::*` rather than refactoring it:** the existing widgets target `StreamIO<T, S>`-using designs, which are not migrating (per §9).  The new widgets target `RCStream<T, F>`-using designs.  Both interop paths coexist; the user picks based on which bus type their design uses.

**Phasing:** lands as a follow-up PR after Phase 1.1 (this PR).  Priority is set by when an actual `RCStream<T, F>`-using design needs to talk to AXI4-Stream IP.

---

## 11 — Credit-based variant (Phase 3)

For very long inter-block paths or multi-source aggregation where TVALID/TREADY combinational behavior is a timing concern, a credit-based variant decouples the source's send decision from the sink's instantaneous ready signal.

```rust
/// A typed, latency-insensitive stream with credit-based flow control.
/// The sink publishes credit grants; the source decrements local credit per
/// item sent. No reverse signal in the data-cycle critical path.
pub struct CreditRCStream<T: Digital, F: Digital, D: Domain, const CREDIT_W: usize> {
    pub data: Signal<Option<Item<T, F>>, D>,
    pub credit_grant: Signal<Bits<CREDIT_W>, D>, // sink -> source: tokens granted this cycle
}
```

Use cases: (a) inter-block paths where the sink-to-source ready signal cannot meet timing as a combinational input to the source's TVALID generator; (b) one sink receiving from multiple sources where reverse-direction arbitration would be expensive; (c) virtual channels (one physical SerDes link carrying multiple logical streams).

`CreditRCStream` and `RCStream` translate to each other via small wrapper widgets (~50 LOC each direction). The conversion is lossless for all framing forms.

This variant is Phase 3 because most kernel-to-kernel connections within a single design are short enough that simple Ready/Valid is fine. Phase 3 lands when an actual design hits the long-path / multi-source case.

---

## 11.3 — Pipelining a credit link: `CreditRCStreamRelay` (shipped)

The credit variant exists for **long inter-block paths**, and until now
it was the one form of the bus you could *not* break with a register:
`RCStreamRelay` only speaks simple Ready/Valid. `rcstream::credit::relay`
closes that.

**Decision recorded — a register pair, not a skid buffer.** The credit
protocol has no forward backpressure at all: a source sends only when it
holds a credit, and the sink has already reserved space for every credit
it issued. There is no stall for a skid buffer to absorb, so the relay
forwards unconditionally and a Carloni buffer here would be dead
silicon. It cannot be overrun either — credit accounting bounds
in-flight items to the sink's reserved capacity, and the relay holds at
most one.

**Decision recorded — the reverse path must stay an ungated register.**
`credit_grant` is a *count*, not a level. The invariant is that the
running total reaching the source equals the total the sink issued:
grants may be delayed, never dropped, merged, or duplicated. Lose one
and the source is permanently a token short and the link eventually
deadlocks; duplicate one and it can overrun the sink's buffer. A plain
register shifts each cycle's value by one and conserves the total; that
is the entire correctness argument.

**Decision recorded — this relay does NOT preserve throughput, and that
asymmetry with `RCStreamRelay` is load-bearing.** There, Carloni's
theorem makes insertion free at any depth. Credit flow control sustains
full rate only while `credits >= round-trip latency`, and each stage
adds two cycles to that loop. Measured over a 20k-cycle window:

| Credit pool | 1 relay | 6 relays |
|---|---|---|
| 4 credits (`FIFO_N=2`) | 131 items | 48 items |
| 16 credits (`FIFO_N=4`) | 195 items | 185 items |

So insertion is always correct and only conditionally free. Both halves
are checked in `tests/rcstream_credit_relay_insertion.rs`; if the
throughput property ever stops holding, the sizing guidance in the
module docs is wrong and must be rewritten rather than the test relaxed.

**Constraint surfaced:** `CreditSink` requires `FIFO_N >= 2`. Its buffer
is a `SyncFIFO`, and that widget panics at address width 1 —
`Bits<1>` arithmetic overflow inside its read/write logic, reproducible
without any `rcstream` code by simulating a bare `SyncFIFO<b8, 1>`. That
is a defect in `SyncFIFO`, not in the credit variant; documented on
`CreditSink` because the panic otherwise surfaces from deep inside the
FIFO with no hint of its cause.

---

## 11.3.1 — Cross-domain credit: analysed and parked (NOT shipped)

Recorded so it is not re-derived. The follow-up list has long carried
"cross-domain credit variants — the credit counter at the source uses
the source's clock; the credit grant from the sink crosses through a
`Sync1Bit` synchronizer." On investigation that sketch does not close.

**The sketch omits the data path, and that is load-bearing.** It
specifies how the *grant* crosses. Multi-bit *data* cannot cross clock
domains by registering it across — it needs a dual-clock FIFO or a full
handshake. Add the dual-clock FIFO and you have rebuilt
[§11.5's `RCStreamCdc`](#115--cross-clock-domain-variant-phase-2-shipped);
at that point the credit accounting duplicates the async FIFO's own
gray-coded pointer synchronisation, which *is* space accounting under
another name.

**The motivation does not survive the crossing either.** §11 justifies
the credit variant by the need to break a combinational sink→source
`ready` path. `RCStreamCdc`'s `ready` is already registered — it is
`!full`, and `full` is a registered output of the FIFO's write logic
with the incoming data not in its cone (documented and tested in §11.5).
On-chip, the timing problem credit solves is already solved by the
crossing itself.

**Where it would genuinely earn its place is off-chip** — SerDes, PCIe —
where a physical layer handles the data crossing via serialisation and
clock recovery, and credit rides the link because the two ends share no
memory. That needs a PHY abstraction RHDL does not have, and there is no
such consumer in the tree.

**Decision:** parked, not rejected. Revisit when an off-chip link
consumer exists, and at that point design it around the PHY boundary
rather than around `Sync1Bit`. The effort was redirected to `CreditMux`
(§11.3.2), which addresses credit's *other* §11 motivation —
multi-source aggregation — and is not redundant with anything.

---

## 11.3.2 — `CreditMux`: multi-source aggregation (shipped)

§11 gives credit two motivations.  §11.3.1 shows the first (breaking a
long `ready` path) does not survive a clock crossing.  This is the
second, which the plan calls *the classical use case*: one sink
receiving from many sources, where reverse-direction arbitration would
be expensive.  Unlike cross-domain credit, nothing in the tree already
provides it — `rcstream` had no arbiter at all.

`rcstream::credit::mux::CreditMux<T, F, CREDIT_W, FIFO_N, M, N>`.

**Decision recorded — per-source credit pools, not a shared one.**  Each
source gets its own `CreditSink`, hence its own buffer and pool.  A
shared pool would let a fast or misbehaving source consume the whole
thing and starve the others; independent pools mean a source can only
exhaust its own credit, which is the *virtual channel* property §11
lists.  The cost is `N` buffers, which is the honest price of
non-interference.

**Decision recorded — round-robin, not priority.**  With strict priority
a source that always has data starves every lower-ranked source
indefinitely.  An aggregator's purpose is to merge streams, so
permanently dropping one is a failure of purpose rather than a tunable
policy.  The arbiter is work-conserving: idle sources are skipped, not
waited on.

**Bug found by this work:** `CreditSink` initialised its credit pool to
`2^FIFO_N`, but `SyncFIFO<_, FIFO_N>` holds `2^FIFO_N - 1` items.  The
sink issued one more token than its buffer could accept and the extra
item was **silently dropped**.  Invisible to an always-ready downstream
— which is what every pre-existing credit test used — and exposed here
because three sinks sharing one output port each drain a third of the
time.  Fixed, with a regression test in
`tests/rcstream_credit_no_loss.rs`.

**Testing convention this establishes:** a flow-control widget tested
without backpressure is untested.  The whole point of credit accounting
is what happens when the sink cannot keep up; a permissive sink
exercises every path except the one that matters.

---

## 11.4 — Combinators (shipped)

Phases 1.1 through 3 all built *transport*: the bus type, the relay, AXI
interop, the credit variant, the clock-domain crossing.  None of them
let a design **transform** a stream.  Because §9 decided the existing
`stream::*` widgets are not migrating, an `RCStream` pipeline had no
`map` or `filter` to reach for and had to hand-roll its own.  This was a
gap in the original phasing rather than a deliberate omission — the plan
never listed combinators at all.

Shipped in `rcstream::{map, filter, filter_map, zip, tee}`:

| Widget | Function signature | Shape |
|---|---|---|
| `RCStreamMap<T, F, S>` | `fn(cr, T) -> S` | `RCStream<T,F>` → `RCStream<S,F>` |
| `RCStreamFilter<T, F>` | `fn(cr, Item<T,F>) -> bool` | `RCStream<T,F>` → `RCStream<T,F>` |
| `RCStreamFilterMap<T, F, S>` | `fn(cr, Item<T,F>) -> Option<Item<S,F>>` | `RCStream<T,F>` → `RCStream<S,F>` |
| `RCStreamZip<A, F, B, G>` | — | two streams → `RCStream<(A,B), (F,G)>` |
| `RCStreamTee<A, F, B, G>` | — | `RCStream<(A,B), (F,G)>` → two streams |

**Decision recorded — the payload/item asymmetry is a hazard boundary,
not a style choice.**  `map` takes the payload alone and preserves `F`
automatically; `filter` and `filter_map` take the whole `Item`.  The
line is drawn where the operation can *destroy framing*: a `map` cannot
drop anything, so `F` is orthogonal to it, whereas dropping the item
that carries an end-of-frame marker means the frame never ends — a
data-dependent, run-time failure no type check catches.  Exposing `F` to
the predicate makes it visible at the point of decision, and the
framing-safe idiom is `it.frame || keep(it.data)`.  Eliminating exactly
this species of out-of-band convention is the bus's reason for existing.

**Decision recorded — rejected items are consumed without waiting for
the sink.**  `d.input.ready = i.ready || dropping`.  The contract lets a
sink gate its `ready` on `data.is_some()`; a dropped item shows such a
sink nothing, so waiting for downstream would deadlock.  Note that the
older `stream::filter` sets `d.input_buffer.ready = i.ready`
unconditionally and therefore appears to have this exposure — flagged
for investigation rather than asserted as a bug, since its own test
suite drives unconditionally-ready sinks.

**Decision recorded — `zip` carries `(F, G)` rather than picking one.**
The two markers are independent run-time values and zipping does not
synchronise framing, so choosing one would discard information by fiat.
`((), ())` costs no wire bits in the unframed case.

**Decision recorded — `tee` splits, matching `stream::tee`.**  A genuine
fan-out needs per-branch delivery state (two sinks can go ready on
different cycles, and a held item would otherwise be delivered twice);
it is a separate widget and was not smuggled in.

Every combinator is built from `RCStreamRelay` skid buffers and carries a
`drc::no_combinatorial_paths` test, so all of them remain valid relay
insertion points.

**Not shipped:** `flatten`, `chunked`, and the FIFO adapters that
`stream::*` has.  Also no `rcstream::testing` fixture module — the
Tier-2 tests use closed-loop `run_fn` directly.  If a third round of
combinators arrives, consolidating that harness the way
`stream::testing` does becomes worthwhile.

---

## 11.5 — Cross-clock-domain variant (Phase 2, shipped)

`RCStream<T, F>` as shipped in Phase 1.1 carries no clock-domain
parameter: in the single-domain (`Synchronous`) family the framework
fans one `ClockReset` out to every sub-circuit, so the domain is
implicit and uniform, and a `Signal`-wrapped bus type would be pure
overhead.  Phase 2 adds the multi-domain story.

**`rcstream::cdc::RCStreamCdc<T, F, W, R, N>`** — the crossing widget.
A `Circuit`-family widget wrapping a single
`fifo::asynchronous::AsyncFIFO<Item<T, F>, W, R, N>`, presenting an
ordinary `RCStream` ready/valid face in each domain.  Holds `2^N - 1`
items.

**`rcstream::bus::AsyncRCStream<T, F, D>`** — the domain-typed bus
type from §5, with `Signal<_, D>` fields, plus `bus::lift` / `bus::lower`
to move a port losslessly between the domain-less and domain-typed
forms.

**Decision recorded — the bundled type cannot express a crossing.**
§5's `AsyncRCStream<T, F, D>` bundles `data` and `ready` in a *single*
domain `D`, so it describes one **end** of a connection.  A crossing's
data-in (domain `W`) and ready-in (domain `R`) are in different domains
by construction, so `RCStreamCdc` cannot use the bundled type for its
ports and names the two domains separately in its `In` / `Out` structs
instead.  The bundled type is therefore *not* the crossing's interface;
it is the port type for a single-domain widget participating in a
multi-domain composition.  Both ship, with the distinction documented on
the type itself.

**Decision recorded — gating, not a skid buffer.**  A conforming
`RCStream` source may assert `data = Some(item)` while `ready` is false
(the contract forbids `data.is_some()` from depending combinationally on
`ready`).  A raw FIFO would treat that as a write and overflow when
full.  The crossing therefore gates the write (`accept = if !full { data
} else { None }`) and the read (`next = ready && data.is_some()`).  The
alternative — reusing `stream::stream_to_fifo`'s two-element skid buffer
— was rejected because it would make `rcstream` depend on the
`stream::Ready<T>` type and blur the module boundary §9 deliberately
draws; the gate is also strictly cheaper.

**Not shipped:** a *credit-based* cross-domain variant (the credit
counter in `W`, the grant crossing back through a `Sync1Bit`).  That
remains a Phase 4 follow-up per §11, and is the natural shape for SerDes
link layers and PCIe-style protocols.

---

## 12 — Phasing

| Phase | Deliverable | Status |
|---|---|---|
| 1.1 | `RCStream<T, F>` + `Item<T, F>` types + `RCStreamRelay<T, F>` in new `rcstream` module + book chapter | **shipped (PR #51)** |
| ~~1.2~~ | ~~Migrate existing `stream::*` widgets~~ | **dropped** — see §9 |
| ~~1.4~~ | ~~Consolidate `axi4lite::channel`~~ | **dropped** — see §9 |
| 1.5 | AXI4-Stream interop widgets in `rcstream::axi_stream` (`AxiStreamToRCStream<T, F>`, `RCStreamToAxiStream<T, F>`) — parallel to the existing `axi4lite::stream::{axi_to_rhdl, rhdl_to_axi}` | **planned** — see §10; lands when a `RCStream`-using design needs to talk to AXI4-Stream IP |
| 2 | `AsyncRCStream<T, F, D>` cross-clock-domain variant — `rcstream::cdc::RCStreamCdc<T, F, W, R, N>` + the domain-typed `bus::AsyncRCStream<T, F, D>` + `bus::lift`/`bus::lower` | **shipped** — see §11.5 |
| 3 | `CreditRCStream<T, F, CREDIT_W>` for long-path / multi-source aggregation | when an actual design hits the timing wall |
| 4 | Auto-pipelining integration: NTL-pass recognition of `RCStream` boundaries as preferred cut points | coordinates with `auto-pipelining-plan.md` Phase 1 |

The original Phase-1 plan (this section, pre-update) bundled four sub-phases that aimed to make `RCStream` the unifying replacement for `StreamIO<T, S>`.  After review, the project decided `rcstream` should be an **opt-in bus type for new widgets** rather than a forced migration of existing code.  The `stream` and `rcstream` modules coexist indefinitely.

---

## 13 — Validation

Per `CLAUDE.md` §11.1, every phase is a compiler-or-library change with the full PR contract: tests at every applicable level, Justification section, documentation in code + book + this design plan, CHANGELOG entry naming the guarantee preserved.

Specific test requirements:

- **Phase 1.** Each existing `stream::*` widget gets re-tested with the new `RCStream<T, F, D>` signature; emitted Verilog must be byte-identical to the pre-migration form when `F = ()`. Round-trip test for AXI4-Stream interop (`axi → Stream → axi` is byte-identical on the AXI side). New chapter `doc/book/src/stream/bus.md`.
- **Phase 2 — relay-insertion invariance. Shipped:** `crates/rhdl-fpga/tests/rcstream_relay_insertion.rs`. A chain of `N` relays delivers exactly the source item sequence for `N = 1..=8`, and a `map → N relays → filter` pipeline produces output independent of `N` for `N = 1..=5`, anchored by a test that the pipeline computes the expected function (so invariance cannot be satisfied vacuously by a uniformly-broken pipeline). A separate test asserts relay depth costs no throughput beyond its one-cycle-per-stage fill. The suite was mutation-checked: breaking the chain's backpressure wiring fails three of the four tests.

  Deliberately *not* the "100 random widgets × 0–10 relays" form originally specified. Fixed depths over a real pipeline give the same signal with a deterministic, debuggable failure — a randomised harness that reports "depth 7 of widget #43 diverged" is far harder to act on, and RHDL tests are required to be deterministic (CLAUDE.md §12 rule 10). Broadening to more widget shapes is worthwhile as `rcstream` grows; randomising the depth is not.
- **Phase 3.** Round-trip test for `Stream <-> CreditRCStream` translation. Long-path timing test demonstrates that `CreditRCStream` removes the TVALID/TREADY combinational dependency.
- **Phase 4.** Auto-pipelining meta-test: random `RCStream`-using designs are pipelined to a target frequency and the output is verified functionally equivalent to the un-pipelined version (offset by the inserted latency).

The validation matrix is the same shape as `auto-pipelining-plan.md` §8 and `fsm-architecture.md` §9.

---

## 14 — Risks and open questions

**Naming conflict with `futures::Stream`.** Rust's async ecosystem uses `RCStream` for asynchronous iterator-like types. The two contexts don't overlap (async Rust isn't valid inside `#[kernel]`), but the name clash is real. Alternatives considered: `Bus`, `Channel`, `Conduit`, `Flow`. Recommendation: keep `RCStream` because (a) the existing `rhdl-fpga::stream::*` module already uses it; (b) the type lives in a clearly hardware-flavored namespace (`rhdl-fpga::stream::bus::Stream`); (c) any other name is a worse fit semantically. The name conflict is a documentation problem, not a technical one.

**Default for `F`.** Most streams don't need framing, and writing `RCStream<T, ()>` everywhere is ergonomic friction. Rust supports type-parameter defaults: `pub struct RCStream<T, F = (), D = SystemClock>`. Recommendation: ship with `F = ()` and `D = SystemClock` defaults so the common case is `RCStream<T>`.

**Backwards compatibility with the existing `StreamIO<T>`.** The migration plan in §9 keeps both types alive during Phase 1.2. Once all `stream::*` widgets are migrated, `StreamIO<T>` becomes a deprecated type alias to `RCStream<T, (), Red>` (or whatever the default domain is) for one release cycle, then removed.

**Variable-rate streams.** AXI4-Stream uses TKEEP for variable items per cycle. `RCStream<[Option<T>; N], F, D>` handles this with the type system, but kernels then have to handle the array. Worth thinking about whether to provide a dedicated `MultiRCStream<T, F, D, N>` that exposes per-lane `RCStream`-like semantics.

**Stream of streams.** `RCStream<RCStream<...>>` doesn't compose naively because the inner stream has its own clock-domain and ready-signal contract. The right encoding is `RCStream<Frame, Marker>` where `Frame` is a payload type marking "start of inner stream / inner data / end of inner stream." Worth documenting as an explicit pattern.

**Multi-sink fan-out.** A `RCStream` is point-to-point. Fan-out (one source, multiple sinks) requires a `Tee` widget that broadcasts. The existing `stream::tee` is the natural starting point; it should be migrated to use `RCStream<T, F, D>` and become the canonical fan-out primitive. Multi-sink is then explicit at the topology level.

**Multi-source aggregation.** Conversely, fan-in (multiple sources, one sink) requires a `MergeStream` widget that arbitrates. This naturally composes with the (12) round-robin arbiter and (13) strict-priority arbiter from `widget-roadmap.md`. The fan-in case is also where `CreditRCStream` shines — credit-based avoids the reverse-direction arbitration mess.

**AXI4-Stream schema documentation.** The translation widget's bit-packing schema for TUSER must be documented precisely enough that external IP can consume it. The schema lives next to the widget as a `Digital`-derived struct, with a documented C/C++ header that matches the encoding.

**Migration path when FSM widgets get formalized.** Per `fsm-architecture.md`, FSM-shaped widgets get `#[derive(Fsm)]` metadata. Many of those widgets emit `RCStream`-typed outputs (UART, SPI, CAN, MIDI, SDI, etc.). The FSM track and this track compose naturally — an `Fsm` widget with `type O = RCStream<...>` is exactly the canonical protocol-PHY shape.

**Composition with vendor primitives.** Per `vendor-primitive-architecture.md`, some widgets compose vendor SERDES. The PHY layer below the SERDES is vendor-primitive territory; the protocol layer above the SERDES is `RCStream<T, F, D>` territory. The two design plans interlock at the SERDES boundary — vendor primitives produce/consume `RCStream`-typed signals.

---

## 15 — Comparison summary (the load-bearing table)

| Property | AXI4-Stream | `RCStream<T, F, D>` |
|---|---|---|
| Wire-level typing | none | Rust type system |
| Framing semantics | TLAST + TUSER (untyped) | typed `F` parameter |
| Multi-channel multiplex | TID/TDEST (flat int) | sum-typed `T` or `F` |
| Byte-keep | TKEEP/TSTRB (byte-granular) | typed payload |
| Clock domain | none | phantom-typed `D` |
| Sideband signaling | TUSER (free-for-all) | typed field of `T` or `F` |
| LID composability | implicit, ad hoc | explicit, by construction |
| Auto-pipelining soundness | manual hazard analysis | automatic via Carloni relay |
| Width parameterization | per-stream | per-element via `T` |
| Width converter required | yes | no |
| Cross-IP TUSER mismatch | common bug | compile error |
| Schema mismatch | silent data corruption | compile error |
| Third-party IP interop | universal | translation widget required |
| IP-Integrator auto-routing | yes | not applicable |

The plan accepts the last two as the price of the first twelve.

---

## 16 — References

[1] Carloni, L.P., McMillan, K.L., and Sangiovanni-Vincentelli, A.L. *A Methodology for Correct-by-Construction Latency-Insensitive Design.* DAC 1999. — The foundational paper. The LID transformation, the relay-station construct, and the marker-propagation proof technique originate here.

[2] Carloni, L.P. *The Theory of Latency-Insensitive Design.* IEEE Transactions on Computer-Aided Design, 2001. — The follow-up paper formalizing the theorem statements and proofs.

[3] Carloni, L.P. *From Latency-Insensitive Design to Communication-Based System-Level Design.* Proceedings of the IEEE, 2015. — The retrospective. Figure 4 of this paper is what `lid::carloni.rs` implements directly. The canonical RHDL reference for the relay station.

[4] ARM. *AMBA AXI4-Stream Protocol Specification* (ARM IHI 0051A). — The protocol this document defines an alternative to. Cited for the wire-level signal definitions and the documented "TVALID must not depend combinationally on TREADY" rule.

[5] Bluespec, Inc. *Bluespec System Verilog Reference Guide.* — The atomic-action / scheduler model that takes a different but related approach to LID. Worth comparing for the design rationale of why we don't go full Bluespec.

[6] Singh, M., and Theobald, M. *Generalized Latency-Insensitive Systems for Single-Clock and Multi-Clock Architectures.* DATE 2004. — An extension of LID to multi-clock systems. Relevant for the `D` parameter and cross-domain `RCStream` handling.

[7] Vijayaraghavan, M., and Arvind. *Bounded Dataflow Networks and Latency-Insensitive Circuits.* MEMOCODE 2009. — The dataflow-network framing of LID. Useful for thinking about Stream-of-Stream composition (§14).

[8] Lavagno, L., and Sentovich, E. *ECL: A Specification Environment for System-Level Design.* DAC 1999 (companion paper to [1]). — Historical context for the LID transformation in the broader system-level-design movement.

[9] OpenCores. *Wishbone B4 Specification.* — Alternative open bus, useful as a comparison point for Ready/Valid-style handshakes.

[10] Skarman, F., and Gustafsson, O. *Spade: An Expression-Based HDL With Pipelines.* OSDA 2023. — Spade's `pipeline` keyword approach to similar problems; cited for the design-rationale comparison.

[11] Basu, Samit. *RHDL: Rust as a Hardware Description Language.* LATTE '25, March 2025. (`doc/latte25/latte.tex`.) — The RHDL paper. The kernel-as-pure-fn invariant that this design plan exploits is established here.

[12] Pellauer, M., et al. *A-Ports: An Efficient Abstraction for Cycle-Accurate Performance Models on FPGAs.* FPGA 2008. — A performance-modeling abstraction adjacent to LID; useful as a comparison point.

---

## 17 — Decisions captured

For the record (also reflected in `architecture.md` and `CLAUDE.md` once shipped):

- **The bus type is `RCStream<T, F, D>`.** Three type parameters: payload, framing marker, clock domain. Default `F = ()`, default `D = SystemClock`.
- **The wire encoding is `Option<Item<T, F>>` source→sink, `bool` ready sink→source.** Validity is encoded in the `Option`, not as a separate signal.
- **Carloni relay stations are the canonical pipeline-insertion primitive.** A `RCStreamRelay` on any `RCStream` connection adds one cycle of latency without changing functional behavior. This is what makes auto-pipelining sound at inter-kernel boundaries.
- **No magic fields.** TKEEP/TSTRB/TLAST/TUSER/TID/TDEST are replaced by typed fields of `T` and `F`. There is no equivalent of TUSER at the bus level.
- **AXI4-Stream interop is via dedicated translation widgets, not pervasively.** `AxiStreamToRCStream<T, F, D>` and `RCStreamToAxiStream<T, F, D>` live at the FPGA boundary. The byte-pack schema is documented and exported as a `Digital`-derived type for external IP consumption.
- **The existing `stream::*` widgets migrate to the new type without behavior change.** `F = ()` for streams that don't need framing; the migration is additive for one release cycle, with the old `StreamIO<T>` becoming a deprecated alias before removal.
- **The credit-based variant `CreditRCStream<T, F, D, CREDIT_W>` is a Phase 3 follow-on**, not part of the canonical bus. Used only when long-path or multi-source aggregation makes Ready/Valid timing-impractical.
- **The bus type integrates with `lid::carloni`, the existing `stream::*` library, the auto-pipelining track, the FSM-derive track, and the vendor-primitive track** by design. Each interlock is documented in its respective design plan.
