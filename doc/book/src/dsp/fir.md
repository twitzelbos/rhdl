# FIR Filters

`dsp::fir` carries two implementations behind one interface, so they
are interchangeable in any slot.

| | taps accepted | multipliers | phase |
|---|---|---|---|
| `Fir` | any, any length | `TAPS` | whatever the taps give |
| `SymmetricFir` | odd length, symmetric | `TAPS/2 + 1` | exactly linear |

Prefer `SymmetricFir` when the taps qualify — a CIC compensator's do by
construction — and `Fir` when they do not: a matched filter, a
fractional delay, a deliberately asymmetric equaliser, or a tap set
arriving from outside the library that you would rather not have to
prove symmetric.

## The fold is an identity, not an approximation

A linear-phase filter has `h[k] == h[L-1-k]`, so the two samples
meeting a shared coefficient can be added *before* the multiply. That
halves the multipliers with **identical arithmetic** — and there is a
test running both implementations on the same symmetric tap set and
asserting they agree bit for bit, so "identical" is checked rather than
claimed.

Symmetry is checked at construction too, because the folded datapath
would otherwise compute a different filter than the taps describe, and
would do it quietly.

## Linear phase is the point, not the multiplier saving

Constant group delay means every frequency in the band is delayed
equally. A filter that delays one part of the band more than another
distorts the envelope, and in a phase-sensitive receiver that is not a
cosmetic defect — it *is* the measurement.

## Saturation, not wrapping

A compensator has gain above one — lifting the band edge back up is
what it is for — so its output can exceed the input's range on signal
that was already near full scale. Wrapping there turns a large positive
sample into a large negative one: a sign flip, not a small error. The
output clamps and reports that it did.

## The adder tree is deliberately not pipelined

The CIC's integrator cascade *is* pipelined, because it runs at the
full converter rate and its depth set fmax. These filters run after
decimation, where the timing budget is `R` times larger — at `R = 32`
there are 32 converter clocks between output samples for a path that is
one multiply and a `log2(TAPS)`-deep add.

That is a reading of where the budget sits, not an oversight, and it
has a limit: at small `R`, long tap sets, or a very high converter rate
it stops being true.
