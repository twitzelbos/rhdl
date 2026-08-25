# Complex Mixer

`dsp::mixer::complex` multiplies the received sample by the
oscillator's **conjugate**, which is what makes it a down-converter
rather than an up-converter.

## The conjugation is the whole thing

`rx × conj(LO)` shifts the tuned frequency down to DC. `rx × LO` shifts
it *up*. The two differ by one sign, and the failure mode is
instructive: with the wrong sign the on-tune output is a flat,
entirely plausible magnitude, so a test that checks "is there signal at
DC" passes.

What catches it is sweeping the oscillator and asking *where* the
response peaks. Before the fix it peaked at −f instead of +f, 27 times
stronger there, and out-of-band rejection measured 3× instead of
334,000×. A number that bad should have been the first clue.

## Framing must agree

Both inputs carry a framing type, and the mixer checks they agree
rather than preferring one. Two streams whose marks have drifted
produce a product whose mark means nothing, and `frame_mismatch`
reports it. See `dsp::sync` for the alignment contract.

## Width

The product of a `W`-bit and an `A`-bit signed value needs `W + A + 1`
bits. `PROD_W` is checked against that at construction rather than
documented, because a too-narrow product does not degrade the output,
it corrupts it.
