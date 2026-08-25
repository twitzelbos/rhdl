# The DSP Chain

RHDL's widget library carries a complete narrowband receive chain: a
numerically-controlled oscillator, a complex mixer, cascaded CIC
decimators, and the FIR that undoes what the CICs did to the passband.
This part walks that chain, then shows the thing it exists to
demonstrate — that you can state DSP *requirements* in Rust and have
them lowered into hardware.

## The chain

```text
   rx sample                                            decimated
   (Iq<W>) ──┐                                          (Iq<WA>)
              ▼
        ┌───────────┐   ┌─────────┐   ┌──────────┐   ┌──────────┐
   f ──▶│    NCO    ├──▶│  Mixer  ├──▶│ IqSplit  ├──▶│   CIC    ├──┐
   φ ──▶│  cos/sin  │   │ ×conj   │   │          │   │ decimate │  │
        └───────────┘   └─────────┘   └────┬─────┘   └──────────┘  │
              │                            │         ┌──────────┐  │
              └──▶ master phase            └────────▶│   CIC    ├──┤
                   (reference)                       │ decimate │  │
                                                     └──────────┘  │
                                          ┌──────────┐             │
                                          │ IqCombine│◀────────────┘
                                          └────┬─────┘
                                               ▼
                                        ┌─────────────┐
                                        │ compensator │──▶ flat
                                        │  (FIR)      │
                                        └─────────────┘
```

Three things about that picture are worth stating before any of the
detail.

**The mixing is complex; everything after it is two real paths.**
Multiplying by `e^-jωt` irreducibly needs all four real products, so
the mixer is complex. Decimation is not: it is two independent real
filters, and the chain says so with
[`IqSplit`](../rcstream/bus.md) and `IqCombine` rather than pulling
`.re` and `.im` out by hand. Both paths are the *same type*, which is
what makes an in-phase/quadrature asymmetry — the one error a
phase-sensitive measurement cannot absorb — unrepresentable rather than
merely discouraged.

**The oscillator's phase is a reference, not an accident.** Its
accumulator is never reset, so `master` is absolute elapsed phase and
successive acquisitions are comparable to each other. That is what
makes the chain *phase sensitive*, and it is why the acquisition marker
is threaded all the way through rather than inferred from timing.

**The compensator is not optional.** A CIC's `sinc^N` shape droops
across the band you meant to keep — 9.7 dB at the edge for `N = 4,
R = 32` with a wide passband. A receiver that reports amplitudes and
skips compensation is wrong by that much.

## The chapters

The [widget tour](nco.md) takes the blocks in signal order. Each has a
schematic, a committed waveform trace, and the reasoning behind its
shape — including the places where the obvious implementation is the
wrong one.

[Specifying a chain](design.md) is the part that distinguishes this
from writing Verilog. A CIC and its compensator take a dozen numbers
before you can instantiate one, and none of them is a requirement. The
requirements are the converter rate, the decimation you need, the
bandwidth that must come out alias-free, and how flat, how quiet and
how well rejected it has to be. Those you know; the dozen numbers are
derived.

```admonish note
Every code block in this part is compiled from
`doc/book/src/code/src/dsp/`, and the claims about what a design
produces are asserted in tests there. A chapter that has drifted from
the library fails the build rather than misleading you.
```
