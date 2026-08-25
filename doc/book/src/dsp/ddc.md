# The Down-Converter

`dsp::ddc` assembles the chain: oscillator, conjugate mixer, split
into two real paths, decimate each, recombine.

## Generic over its decimator

`Ddc<W, WA, PROD_W, C>` takes any `C` presenting the decimator
interface — a plain `CicDecimate`, a `cic_pruned!`-generated one, or a
`CompensatedCic`. `Ddc::new` takes **one** decimator and clones it into
both arms: one argument, not two, because an in-phase/quadrature
asymmetry rotates the constellation and that is the one error a
phase-sensitive measurement cannot absorb. Cloning makes the arms
identical by construction rather than by the caller's care.

On compensated arms, the down-converter's own passband goes from
4.822 dB of span to 0.186 dB.

## The marker survives decimation

A decimator throws away `R − 1` of every `R` frames, and a sync mark is
almost never on the sample that survives. So `StreamDecimator` latches
it: seen anywhere in the window, it rides out on the next output, and
it restarts the window so the marked output is built only from
post-trigger samples.

```admonish warning title="A bug worth knowing about"
A mark arriving on cycle `T` restarts the window at `T` — but the
output emerging at `T` was registered from `T−1` and belongs entirely
to the *old* window. Attaching the arriving mark to it labels
pre-trigger data as the start of an acquisition: precisely the error the
restart exists to prevent. Only a *carried* mark may ride out.

That bites one input in `R`, which is why a test marking a single fixed
offset passed for a long time. The lesson generalises: anything that
latches across a window wants its stimulus swept *over* the window, not
placed at one phase of it.
```

## The marks on the two paths must agree

Both decimators are fed from one split and restart on the same mark, so
their output marks should be identical. The rule is therefore **and**,
with a disagreement flagged:

- agreeing marks pass through, and `and` is that same value;
- disagreeing marks yield *unmarked* — the conservative answer, because
  forgetting an acquisition boundary is better than claiming one on a
  sample where only half the complex value is known to be aligned — and
  `frame_mismatch` reports it.

`IqCombine` previously took the real side's frame and discarded the
imaginary side's, with a comment claiming the type system required them
equal. It requires the same *type*, not the same *value*, so two paths
that had drifted produced a confident wrong answer.

## Gain is not normalised

The output carries the CIC's full `(R·M)^N` DC gain. Undoing it costs
either a multiply or a shift that discards bits the filter was built to
keep, and which is right depends on what comes next — so the widget
reports the factor rather than guessing.
