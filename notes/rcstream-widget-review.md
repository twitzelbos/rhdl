# RCStream widget review: inventory, framing, and three gaps

*2026-08-31. Answers "how many RCStream widgets do we have, and what
framings do they support?" — and, because the honest answer to the second
half is "all of them, unconditionally", replaces it with the question that
does discriminate: **what does each widget do to the framing?***

## The headline

**19 widgets** across five groups, plus the bus type itself and a testing
harness.

**Every one is generic over `F: Digital` and none constrains it.** There
is no `Framing` trait, no `where F: Something`, no widget that only works
for `F = bool`. So "which framings are supported" has the same answer
everywhere — any `Digital` type — and it is not a useful axis for
comparing widgets. What differs, and what a user actually has to know, is
whether a widget passes framing through untouched, transforms it, carries
two of them, or checks two against each other.

Three gaps fell out of the survey; they are at the bottom.

## Inventory

### The bus (`rcstream::bus`)

| Type | What it is |
|---|---|
| `RCStream<T, F>` | `Option<Item<T, F>>` data + `bool` ready, one clock domain |
| `AsyncRCStream<T, F, D>` | the same, with the domain in the type |
| `Item<T, F>` | payload `data: T` paired with framing `frame: F` |

### Flat combinators (`rcstream::*`) — 8

| Widget | Framing behaviour |
|---|---|
| `RCStreamRelay<T, F>` | pass-through — the Carloni relay station |
| `RCStreamFilter<T, F>` | pass-through, on the items that survive |
| `RCStreamMap<T, F, S>` | pass-through; payload `T → S` |
| `RCStreamFilterMap<T, F, S>` | pass-through; payload `T → S`, some items dropped |
| `RCStreamFanout<T, F, N>` | pass-through, replicated to `N` consumers |
| `RCStreamCdc<T, F, W, R, N>` | pass-through, across clock domains |
| `RCStreamChunked<T, F, M, N>` | **transforms**: `T` → `[T; N]`, framing `F` → `[F; N]`, positional |
| `RCStreamFlatten<T, F, M, N>` | **transforms**: `[T; N]` → `T`, framing `F` → `(F, bool)` — group marker plus last-of-group |

### Two-stream combinators — 2

| Widget | Framing behaviour |
|---|---|
| `RCStreamZip<A, F, B, G>` | **carries both**: output framing is `(F, G)`. Deliberate: two zipped streams are not thereby framing-synchronised — `a` may end a frame where `b` does not |
| `RCStreamTee<A, F, B, G>` | **splits both**: `Item<A, F>` out one side, `Item<B, G>` the other |

### Credit-based flow control (`rcstream::credit`) — 4

`CreditRCStream<T, F, CREDIT_W>` replaces the ready wire with a credit
count, for links too long for a combinational ready.

| Widget | Framing behaviour |
|---|---|
| `CreditSource<T, F, CREDIT_W>` | pass-through |
| `CreditSink<T, F, CREDIT_W>` | pass-through |
| `CreditRCStreamRelay<T, F, CREDIT_W>` | pass-through |
| `CreditMux<T, F, CREDIT_W, N>` | pass-through, `N` inputs to one output |

### AXI4-Stream interop (`rcstream::axi_stream`) — 2

| Widget | Framing behaviour |
|---|---|
| `AxiToRCStream<T, F>` | `TUSER → F` |
| `RCStreamToAxi<T, F>` | `F → TUSER` |

**There is no TLAST in this interop, by decision.** End-of-frame is
encoded in `F` (e.g. `F = bool`, whose TUSER is one bit that a consumer
wires to its own TLAST). A separate TLAST signal would reintroduce exactly
the untyped sideband the bus exists to remove.

### Utilities (`rcstream::util`) — 3

| Widget | Framing behaviour |
|---|---|
| `IqSplit<W, F>` | **replicates**: one `Iq<W>` stream framed `F` becomes two `SignedBits<W>` streams, each carrying the same `F` |
| `IqCombine<W, F>` | **checks**: two `F`-framed streams become one `Iq<W>` stream; the two markers must agree, and `Out::frame_mismatch` reports it when they do not |
| `RCStreamConstant<T, F>` | emits a fixed `Item { data, frame }` every cycle |

`IqCombine` is the only widget in the library that *validates* framing
rather than moving it. Two streams split from one source should carry
identical markers, so disagreement means something upstream desynchronised
them — reported rather than resolved, which is the same choice
`dsp::mixer::real_part` makes with its `frame_mismatch`.

## Concrete framing types in the tree

`F` is unconstrained, so this is a survey of *use*, not of capability.

| Type | Where | Uses |
|---|---|---|
| `dsp::sync::SyncMark` | the DSP chain, end to end | 41 |
| `serial_bus::smpte_ltc_decoder::LtcFrame` | LTC decode | 2 |
| `()` | tests and un-framed streams | many |
| `bool` | the documented TLAST idiom | doc only |

`SyncMark` is a newtype over `bool` rather than `bool` itself,
deliberately: it names *what* the marker means (this sample is the anchor
of a timing relationship) and it makes a `SyncMark` stream and a
`bool`-framed stream different types, so they cannot be connected by
accident.

**Only two concrete framing types exist.** The `bool` / enum / `b8` /
sideband-struct idioms in `bus.rs`'s table are documented and supported
but unexercised outside doc text and tests. That is not a defect — the
point of the parameter is that a user brings their own — but it does mean
the table is a design intent rather than a report of use.

## Gap 1 — three widgets do not meet the five-clause contract

`util::{split, combine, constant}` have a schematic symbol, Tier 1 unit
tests, Tier 2 stream tests and a Tier 4 `iverilog` round-trip. They have
**no Tier 3 HDL snapshot, no Tier 5 VCD digest, no runnable example and no
committed waveform trace.**

| widget | T1 | T2 | T3 | T4 | T5 | example | trace |
|---|---:|---:|---:|---:|---:|---|---|
| every other RCStream widget | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `util::split` | ✓ | ✓ | — | ✓ | — | — | — |
| `util::combine` | ✓ | ✓ | — | ✓ | — | — | — |
| `util::constant` | ✓ | ✓ | — | ✓ | — | — | — |

Per CLAUDE.md §12 rule 2 and §6 Layer C these are incomplete widgets, not
a stylistic difference. `IqCombine` is the worst of the three to leave
un-snapshotted, because it is the one with non-trivial behaviour to
regress: the framing comparison and the `frame_mismatch` output.

## Gap 2 — `RCStreamFanout` is not re-exported with its siblings

`rcstream::mod` re-exports nine names. `RCStreamFanout` is a flat module
exactly like `filter` and `map`, is documented in the module docs, has an
example and full test tiers — and is reachable only as
`rcstream::fanout::RCStreamFanout`. The `credit`, `util` and `axi_stream`
widgets are also absent from the top level, but those are grouped
sub-modules that re-export at their own level, which is a defensible
choice. `fanout` is just missed.

## Gap 3 — the framing-transform rules are documented per widget and nowhere together

`chunked` produces `[F; N]`, `flatten` produces `(F, bool)`, `zip`
produces `(F, G)`. Each is explained well in its own module. Nothing states
the composition rule, so a user chaining `chunked → flatten` has to derive
that the framing type round-trips to `([F; N], bool)` rather than back to
`F` — and that `flatten ∘ chunked` is therefore *not* the identity on
framing even where it is the identity on payload. Worth a paragraph in
`rcstream::mod`, or a book section, with the composition table.

## What is not a gap

- **No widget constrains `F`, and that is right.** A `Framing` trait would
  buy nothing here: no widget needs an operation on the marker beyond
  moving it, comparing it (`IqCombine`, which needs only `PartialEq`, which
  `Digital` gives) and pairing it.
- **No TLAST in the AXI interop** — see above; a decision with a reason.
- **`SyncMark` as a newtype** rather than `bool` — the same reasoning.
