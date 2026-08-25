# Compensation

The CIC droops; the compensator undoes it. A short symmetric FIR at the
**output** rate whose response approximates `1/|H_cic(u)|` across the
passband.

Measured on the real widgets, not the formulas:

| | uncompensated | compensated |
|---|---|---|
| band edge | −7.233 dB | −0.001 dB |
| span across passband | 7.233 dB | **0.035 dB** |

## Why it goes after the decimator

It could equivalently go before, at the input rate, and be worse in
every respect: `R` times as many multiply-accumulates per second for
the same correction. The CIC exists to get the rate down before
anything expensive happens; putting the expensive thing in front of it
defeats the arrangement.

## It must invert the whole cascade

For a two-stage chain, compensating only the *last* CIC leaves the
first one's droop uncorrected. That droop is small — the first stage's
band occupies `R1/R` of its own output Nyquist, 1.6% at `8 × 61`, where
`sinc^N` is nearly flat — but "small" was an argument, and the
measurement disagreed with it: 0.0204 dB against 0.0017 dB, a factor of
twelve.

So `compensator::Spec` takes a list of stages and evaluates each at
*its own* input rate. Getting that scaling wrong is the classic error
in cascade analysis, and a test asserts the stage *order* changes the
response away from DC — which it cannot if the scaling has been
dropped.

## DC gain must be exact

Rounding each tap independently left them summing to 1020 of 1024 — a
DC gain of 0.996, so every amplitude the chain reported was 0.4% low.
That is twenty times the passband ripple the design works to remove,
and unlike ripple it does not average out.

`quantise` trims the centre tap — the largest by an order of
magnitude, so a few LSBs there move the response shape immeasurably —
to make the sum exactly `2^shift`.

## Two methods

`Method::LeastSquares` minimises average squared error. Fine for pure
compensation, where there is no worst-case requirement to miss.

`Method::Remez` minimises the *maximum* weighted error, and is the
right choice whenever a stopband attenuation is specified: "at least
60 dB everywhere" is a statement about the maximum, and least squares
will trade a deep notch here for a shallow one there.

Remez also needs no weight search. Weighting the passband by `|H(u)|`
makes the weighted error exactly the relative deviation
`|A(u)·H(u) − 1|`, so the two dB targets fix the weighting in closed
form. Least squares has to bisect, because there the relationship
between weight and achieved attenuation is empirical.

The stopband is weighted by `|H(u)|` for the same reason, which makes
the weighted error there `|A(u)·H(u)|` — the composite's own stopband
level. Both bands therefore measure the composite, and the equiripple
property lands on the response that actually reaches the output rather
than on the compensator considered alone. The weight is floored once the
cascade is 12 dB past what was asked for: a CIC's stopband contains
exact nulls, the exchange solves with `δ/W`, and an unfloored weight
goes to zero there and takes the whole design with it.

```admonish note title="Remez is self-certifying"
By Chebyshev's alternation theorem, a length-`2M+1` linear-phase filter
whose weighted error attains its maximum at `M+2` points with
alternating sign **is** the optimal filter. So the test for the
implementation is not "it beat least squares" — it is that the
alternation condition holds, which establishes optimality without a
reference implementation to compare against.

Alternation and extremum count are exact. Magnitude equality is
asserted to the design grid's resolution: between grid points the
continuous error overshoots by a few percent, which is inherent to
grid-based Parks-McClellan rather than a defect.
```

## What compensation does not do

It does not improve alias rejection. Compensation shapes the passband;
rejection at the frequencies decimation *folds onto* the passband
remains whatever `response::worst_alias_db` says. If the aliases are too
big the answer is more CIC stages or a narrower band, not a longer
compensator — unless you deliberately ask the compensator to attenuate
as well, which [Specifying a chain](design.md) covers.

Note that this is a different question from `min_stopband_db`, even
though both are about unwanted signal. Alias rejection is about content
folding onto the passband during decimation, and is the cascade's
business alone. The stopband requirement is about content surviving
*above* the stopband edge in the output, and is the composite's. A chain
can be excellent at one and poor at the other.
