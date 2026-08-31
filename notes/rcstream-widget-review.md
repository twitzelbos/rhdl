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
| `RCStreamCdc<T, F, W, R, N>` | pass-through, across clock domains — **atomically**; see the CDC addendum |
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

**So yes — the trait brings its own kernel.** `SynchronousIO::Kernel` is
part of the bound, so each implementor supplies one and the parent gets
whichever the instantiated policy carries. Two policies over the same
parent widget produce different observable behaviour, which is what
`the_policy_supplies_the_behaviour` asserts.

**But the parent's kernel does not *call* it.** The framework *wires* it:
the parent writes `d.policy` and reads `q.policy`, and the policy's kernel
runs as its own circuit. That distinction is the whole content of the two
failures above — dispatch is structural, at instantiation, not a call.

**Verified end to end**: builds, `descriptor()` succeeds, the policy
appears as a real `top_policy` submodule rather than being inlined away,
and it passes the `iverilog` round-trip on *both* the RTL and the NTL
path, plus an iterator simulation. The associated type flows through
`SynchronousDQ`'s generated `Q`/`D` without special handling.

## What it costs — measured, not assumed

An earlier draft of this section said the policy "holds its own
registers". That is wrong as a general claim. **A policy holds whatever
registers it declares, and a stateless one declares none:** the same
parent composed over a `DFF`-holding policy emits one
`always @(posedge …)` block, and composed over a purely combinational
policy emits **zero**. Both round-trip through `iverilog` on RTL and NTL.
`a_stateless_policy_costs_no_flops` pins it.

So the cost is a *module boundary* in the emitted Verilog, not silicon
state. Caller-supplied combinational framing logic is therefore
expressible after all — as a stateless policy widget, at zero register
cost — just not as logic folded into the parent's own kernel body.

**One ergonomics wrinkle.** Generic *helper* code over the policy
parameter needs more bounds than the widget does: the simulator wants
bounds on `P::S` that `FramingPolicy` does not imply, so a helper written
as `fn boundaries<P: FramingPolicy>(uut: &Framed<P>)` does not compile
where the widget itself does. The test suite worked around it with a
macro. Widget code is fine; test scaffolding generic over the parameter is
where this bites.

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

---

# Addendum: when the stream crosses a clock domain, so must the framing

*2026-08-31. It does, and the mechanism is sound. What the survey turned
up is a missing test and a missing paragraph — and a measurement that
refuted the warning it was written to support.*

## The mechanism is right

`RCStreamCdc` instantiates a single `AsyncFIFO<Item<T, F>, W, R, N>`. The
marker is a *field of the same `Digital` value* as the payload, so it
crosses atomically and cannot be separated. There is no second
synchroniser for framing — which is the shape that would be wrong, since
two independent crossings for one item's two halves would land marks on
the wrong items.

## But nothing tested it

**Every test in `cdc.rs` used `F = ()`** — one `frame: ()` literal in the
whole file — and the example is `RCStreamCdc::<b8, (), Red, Blue, 3>`. So
the guarantee was purely structural, and structure was the only thing
holding it: a refactor that narrowed the FIFO to the payload and carried
framing alongside would have passed the entire suite while corrupting
every marked stream.

Now pinned by `the_framing_marker_crosses_with_its_item`, which asserts
each received item's marker against *its own payload index* — after a
crossing the cycle means nothing and item identity is all that is left.
Verified to fail when the expected marker is shifted by one item.

## The measurement that refuted the warning

The paragraph about to be written said: a crossing perturbs the cycle a
mark lands on, so `dsp::sync`'s **same-cycle** alignment contract cannot
survive one. **Measured, and false.** Crossing one stimulus through
crossings of *different depth* produces marks on *identical* read-domain
cycles. A saturating source keeps the FIFO full, so the read side's
backpressure alone sets the cadence and the depth is invisible.

What does move the cycle is the **drainage**. So:

- two crossings behind one consumer stay in lockstep, because their state
  is a deterministic function of identical inputs;
- two behind different consumers, or different backpressure, do not.

`drainage_not_depth_sets_the_cycle_a_mark_emerges_on` asserts both halves:
depth-invisible as an equality, drainage-decisive as an inequality.

## The rule that follows

**Cross once, as one `Item`** — not because a crossing is lossy, but
because one crossing cannot be drained asymmetrically with itself.

```text
  right:  ... -> RCStreamCdc<Iq<W>, SyncMark, W, R, N> -> IqSplit -> ...
  wrong:  ... -> IqSplit -> two RCStreamCdc -> IqCombine -> ...
```

The wrong shape would have `IqCombine::frame_mismatch` firing — or, worse,
coincidentally not firing.

`dsp::ddc` cannot get this wrong: it is single-domain throughout. Nothing
in the type system prevents a caller from getting it wrong, which is why
it is now documented in both `rcstream::cdc` (with the measurement) and
`dsp::sync` (whose alignment contract previously did not mention clock
domains at all — zero hits for "domain", "clock" or "cdc" in the file).

## What this says about the phantom domain

`Item<T, F>`'s framing parameter carries no domain, and should not. The
domain lives on the *stream* — `AsyncRCStream<T, F, D>` — and framing is a
per-item value, not a per-cycle one. A domain-parameterised `F` would be
claiming the marker means something about a clock, when what it means is
"this item is the anchor". The cycle is where the domain enters, and the
cycle is a property of the connection, not of the marker.
