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

---

# Addendum: can framing behaviour be a trait?

*2026-08-31. The question was whether Rust composition — traits
implementing a specific framing behaviour — could express what the tables
above document by hand. Answer: **partly, and the boundary is sharp**.
Everything below was compiled and run, not reasoned about.*

## Two things that cannot work

**A trait method cannot be called inside a `#[kernel]`.**
`rhdl-macro-core/src/kernel.rs`'s `method_call` matches against a
21-name allowlist — `any`, `all`, `xor`, `as_signed`, `as_unsigned`,
`val`, `resize`, `raw`, `xadd`, `xsub`, `xmul`, `xneg`, `xext`, `xshl`,
`xshr`, `xsgn`, `dyn_bits`, `as_bits`, `as_signed_bits` — and anything
else is a hard error:

```text
Unsupported method call ... in an rhdl kernel function
```

So `frame.advance()` on a user trait is out, and this is a deliberate
allowlist rather than an oversight: the kernel compiler has to *lower*
the call, and it can only lower operations it knows.

**An associated type cannot be called either — and that one is plain
Rust.** A path call in a kernel lowers to
`<path as DigitalFn>::kernel_fn()`, so pointing at a named `#[kernel] fn`
works. Pointing at an associated type does not:

```rust
pub trait Framing: Digital {
    type Advance: DigitalFn;
}

#[kernel]
pub fn use_it<F: Framing>(f: F) -> F {
    <F as Framing>::Advance(f)      // error[E0575]
}
```

```text
error[E0575]: expected method or associated constant,
              found associated type `Framing::Advance`
```

`rustc` rejects it before RHDL sees it — an associated type cannot appear
in call position, and neither can a bare generic parameter. **So there is
no way to inject caller-supplied logic into an existing kernel body.**

## What does work: the behaviour is its own widget

The sub-widget becomes a generic parameter whose trait bound **pins its
`I` and `O`**. `dsp::ddc` already does this —
`Ddc<W, WA, PROD_W, C>` where

```rust
C: SynchronousIO<I = decimator::In<W>, O = decimator::Out<WA>>
    + Synchronous + Clone + std::fmt::Debug
```

— which is how one down-converter accepts a plain `CicDecimate`, a
`cic_pruned!`-generated one, or a `CompensatedCic`. The kernel carries
`C` and repeats the same bound; its *body* never mentions `C`, because
`q.dec_i` and `d.dec_i` have the pinned projection types.

That generalises to framing, and the associated type is welcome as long
as it is only ever used *as a type*:

```rust
pub trait FramingPolicy:
    Synchronous
    + SynchronousIO<I = PolicyIn<Self::Frame>, O = PolicyOut<Self::Frame>>
    + Clone + std::fmt::Debug
{
    /// The framing type this policy speaks.
    type Frame: Digital;
}

pub struct Framed<P: FramingPolicy + Default> { policy: P }

impl<P: FramingPolicy + Default> SynchronousIO for Framed<P> {
    type I = PolicyIn<P::Frame>;
    type O = PolicyOut<P::Frame>;
    type Kernel = framed<P>;
}

#[kernel]
pub fn framed<P>(cr: ClockReset, i: PolicyIn<P::Frame>, q: Q<P>)
    -> (PolicyOut<P::Frame>, D<P>)
where P: FramingPolicy + Default { ... }
```

**Verified end to end**: this builds, `descriptor()` succeeds, the policy
appears as a real `top_policy` submodule rather than being inlined away,
and it passes the `iverilog` round-trip on *both* the RTL and the NTL
path, plus an iterator simulation. The associated type flows through
`SynchronousDQ`'s generated `Q`/`D` without special handling.

**The cost is that it is structural, not zero-cost.** The policy is a
submodule and holds its own registers. If what you wanted was
caller-supplied *combinational* logic folded into the parent's kernel,
there is no route to it — see the two failures above.

## What this means for the three gaps

- **Gap 3 (no composition table) is the one a trait genuinely fixes.**
  The framing transforms are **type-level functions** — `chunked` maps
  `F → [F; N]`, `flatten` maps `F → (F, bool)`, `zip` maps `(F, G)` — and
  a type-level function is exactly an associated type used as a type,
  which is legal. Something like

  ```rust
  pub trait ChunkFraming<const N: usize> { type Chunked: Digital; }
  pub trait FlattenFraming { type Flattened: Digital; }
  ```

  would state each rule once, machine-check it, and make
  `flatten ∘ chunked = ([F; N], bool)` a thing the compiler knows rather
  than a paragraph a reader has to derive. That is a real improvement over
  a documentation table, and it is available today.
- **`IqCombine`'s validation needs no trait at all.** Comparing two
  markers needs only `PartialEq`, which `Digital` already provides.
- **A `Framing` trait with *methods* is not worth designing**, because no
  widget can call one. The library's twelve pass-through widgets need
  nothing from the marker beyond moving it, and moving it needs no trait.

## The third route, for completeness

Where the kernel itself must differ per parameterisation, this repository
already uses **macro monomorphisation** rather than traits:
`cic_pruned!` and `cic_interp_tapered!` generate a widget *and* its kernel
per shape, with one macro arm per depth. That is the escape hatch when the
logic — not just the type — has to vary, and it is why `tapered.rs` has
arms for `n = 2..5` instead of a trait.
