# An NMR Receiver Chain

*What the oscillator, the mixer and the CIC decimator each have to be so
that the digital chain does not degrade the measurement — and, in
particular, so that signal averaging keeps working.*

This chapter reads standalone. Every number in it is computed from
`rhdl-dsp-design` and pinned by tests in
`doc/book/src/code/src/dsp/nmr.rs`.

## The one requirement, stated once

An NMR receiver is judged after averaging, not before. That single fact
reorganises the whole budget, because averaging separates errors into two
classes that behave completely differently:

- **Incoherent** error — genuinely random between transients. Averages
  down as `10·log10(N)`. Cheap to fight: average longer.
- **Coherent** error — the same every transient. Averages down **not at
  all**. Its ratio to the signal is fixed forever by the hardware.

So the requirement is:

> **Every quantisation in the chain must be dithered by noise of at least
> about one LSB at that point, and anything that is not dithered sets a
> floor no amount of averaging can lower.**

That is the whole chapter. What follows is what it costs at each stage,
and which knob controls it.

## The budget the converter sets

An ideal `B`-bit quantiser gives `6.02·B + 1.76` dB of full-scale SNR
across the whole Nyquist span. Decimating by `R` narrows the noise
bandwidth by `R` while keeping the signal, so in-band SNR gains the
**processing gain** `10·log10(R)`:

| | full Nyquist | + processing gain (R = 2500) | in a 20 kHz band |
|---|---:|---:|---:|
| 14-bit | 86.0 dB | +34.0 dB | **120.0 dB** |
| 16-bit | 98.1 dB | +34.0 dB | **132.1 dB** |

That in-band figure is the number the rest of the chain must not spoil.
Carrying it needs `0.5·log2(R)` = **5.64 more bits than the input**, so an
output narrower than input + 6 bits discards processing gain you have
already paid for in silicon.

Then averaging adds `10·log10(N)` on top: 4096 transients is a further
**36.1 dB**, putting a 16-bit chain at 168 dB below full scale. Nothing
about the converter changes; the *requirement on everything else* changes
by 36 dB.

## The ADC operating point: noise must exercise the quantiser

This is the point the whole chapter exists for, and it is a *gain*
setting, not a chip choice.

If the analog noise reaching the converter is far below one LSB, a
repeated transient digitises to **identical codes every time**. The
quantisation error is then a deterministic function of the signal, it adds
coherently, and averaging does not reduce it at all. The instrument looks
fine on one shot and refuses to improve.

With `σ` the analog noise in LSB rms:

| `σ` (LSB rms) | quantiser adds | noise-to-full-scale (16-bit) | averaging |
|---:|---:|---:|---|
| 0.05 | 15.36 dB | 116.3 dB | **fails — codes repeat** |
| 0.10 | 9.70 dB | 110.3 dB | **fails** |
| 0.25 | 3.68 dB | 102.4 dB | degraded |
| 0.50 | 1.25 dB | 96.3 dB | `√N` |
| 1.00 | 0.35 dB | 90.3 dB | `√N` |
| 2.00 | 0.09 dB | 84.3 dB | `√N` |
| 4.00 | 0.02 dB | 78.3 dB | `√N` |

Read it from both ends. Below about 0.2 LSB the quantiser stops being
exercised and averaging breaks. Above about 1 LSB the quantiser is
transparent — it adds under 0.4 dB — and every further LSB of noise is
dynamic range spent for nothing.

**Target `σ` ≈ 0.5 to 1 LSB rms.** Set the analog gain so the noise floor
lands there, and neither more nor less. A 16-bit converter run at `σ = 1`
LSB has 90 dB from its own noise up to full scale, which is the dynamic
range available for a single transient.

## The oscillator: the one spec averaging cannot help

The NCO's phase-truncation spurs are **deterministic**. Reset the
accumulator to the same phase each transient — which a phase-coherent
experiment does deliberately — and the spurs are bit-identical every time.
They add exactly as the signal does. Their ratio to the signal is fixed by
the hardware and averaging moves it by precisely zero dB.

Meanwhile the noise floor keeps dropping:

| averages | noise floor | so the NCO must beat |
|---:|---:|---:|
| 1 | −132.1 dB | −132.1 dBc |
| 16 | −144.1 | −144.1 |
| 256 | −156.1 | −156.1 |
| 4096 | −168.2 | −168.2 |
| 65536 | −180.2 | −180.2 |

`dsp::nco::sin_cos_linear_interp` ships four validated configurations, and
this table is how to choose between them — **by the averaging depth the
experiment will actually use**, on a 16-bit front end:

| variant | table | SFDR | adequate up to |
|---|---:|---:|---|
| `SinCosLinearInterpDefault` | 9 Kbit | −104.3 dBc | **already spur-limited at N = 1** |
| `SinCosLinearInterp24` | 48 Kbit | −140.4 dBc | ~7 averages |
| `SinCosLinearInterp28` | 112 Kbit | −164.5 dBc | ~1754 averages |
| `SinCosLinearInterp32` | 256 Kbit | −188.6 dBc | ~450 000 averages |

**The default oscillator is not adequate for a 16-bit NMR receiver even
without averaging.** A serious averaging experiment wants
`SinCosLinearInterp28`, whose 112 Kbit is a handful of block RAMs — cheap
against the alternative, which is an artifact you cannot remove in
post-processing because it is phase-locked to your signal.

A 14-bit front end tolerates more, because its noise floor starts 12 dB
higher: `SinCosLinearInterp24` lasts to ~109 averages there against ~7 on
16 bits. That is the honest sense in which 14 bits is a cheaper
instrument — not in the converter, in what it lets you get away with
downstream.

Two caveats, both worth stating plainly:

- The table compares a *discrete* spur in dBc against an *integrated*
  in-band SNR. In an `M`-point spectrum the per-bin noise floor is a
  further `10·log10(M)` down, so a spur stands out **more** than this
  table implies. The comparison is lenient, not optimistic.
- **Phase cycling does not rescue a marginal oscillator, and the reason
  is exact.** See the next section — this was measured, and the answer is
  worse than "it might not help".

### Phase cycling cannot touch the dominant spur

NMR already cycles the receiver phase with the transmitter, so the obvious
hope is that this decorrelates the oscillator's spurs and lets them
average. **It does not, and the mechanism says exactly why.**

Measured with the bit-accurate `dsp::nco::model`:

For an odd tuning word `W` is invertible mod `2^B`, so adding a phase
offset `Δ` is *exactly a time shift* by `n₀` with `n₀·W ≡ Δ`. A time shift
preserves every magnitude — verified to nine digits — and rotates each
line's phase. A phase cycle aligns the *carrier*, de-rotating by
`φ_k = 2π·Δ_k/2^B`, so a line at the `m`-th harmonic keeps a residual phase
of exactly `(m − 1)·φ_k` — verified to 0.01°. A `K`-step cycle therefore
multiplies that line by

```text
  S(m) = (1/K) · Σ_k exp( j (m−1) φ_k )
```

which is **exactly one or exactly zero**, not `1/√K`. A phase cycle is a
comb in harmonic order, not a statistical average.

Now the uncomfortable part. The dominant spur of a linear-interpolating
DDS sits at harmonic **`m = 2^TBL_W + 1`** — measured, and it is the
architecture's signature. So `m − 1 = 2^TBL_W` and

```text
  (m−1)·φ_k = 2^TBL_W · 2πk/K
```

is a multiple of `2π` for every `k` whenever `K` divides `2^TBL_W`.

| cycle length | suppression of the dominant spur |
|---|---|
| K = 2, 4, 8, 16, 32, 64 | **exactly 0.0 dB** |
| K = 3 | −58 dB |
| K = 5 | −60 dB |

**Every power-of-two cycle gives precisely zero help — and NMR phase
cycles are 2, 4, 8 or 16 steps.** A 3- or 5-step cycle annihilates the
same spur. Whether a non-power-of-two cycle is compatible with the rest of
an experiment's phase bookkeeping is a spectroscopy question, not a DSP
one, but the DSP answer is unambiguous: the standard cycles are exactly
the ones that cannot help.

So the oscillator table above stands, and **the SFDR has to be bought in
hardware**.

### Does the decimator remove them instead?

Mostly yes, and the rate is measurable.

**The spur spectrum is sparse and its structure is word-independent.** At
the shipped `Default` widths the tall lines sit at harmonics
`m = 255, 257` (−96.4 / −96.3 dBc) and `511, 513` (−108.6 / −108.2),
*identically for every tuning word* — only their positions move, because
harmonic `m` sits at offset `(m − 1)·word mod 2^B`. Only 4 lines lie within
3 dB of the worst, and the tallest holds 23% of the total error power.
Exact-arithmetic and as-built quantised models agree, so this is the
interpolation residual, not amplitude quantisation.

Because the harmonic list is fixed, the in-band question is pure
arithmetic per carrier — no FFT. Swept over every odd tuning word from 1 to
60 MHz:

| | fraction |
|---|---:|
| carriers with a tall spur inside ±10 kHz | **0.09%** |
| a ±300 Hz carrier window containing one | **0.5%** |

So for a carrier you cannot place precisely — the usual case, and one that
drifts — the risk is roughly **one in two hundred**, not a certainty.

```admonish warning title="Do not sample adversarial words and call it a rate"
An earlier version of this section reported that the decimator "never
helps", from eight *hand-picked* tuning words of which six put a tall spur
in band. Those were `0x100001`, `0x155555`, `0x3FFFFF` and similar —
exactly the structured words `model::adversarial_words` exists to generate.
`0x100001` has its low fourteen bits equal to one, which places harmonic
257 at 256 bins from the carrier *by construction*.

The representative sweep differs from that sample by three orders of
magnitude. The difference is the sampling, not the physics, and the wrong
figure was the one that felt like a finding.
```

### A spur is a ghost of the signal, not a tone

This is the correction that decides whether any of it matters, and it
follows from the mixer being a *multiplier*:

```text
  out = in × nco = in × ideal  +  in × err
```

The spur term is `in × err` — **proportional to the input**. So an
oscillator spur does not add a fixed-level tone to the spectrum; it adds a
*ghost of the signal*, displaced by the spur's offset frequency, at
`−SFDR` **relative to the signal that cast it**. That ratio is independent
of how large the signal is.

Two consequences, pulling in opposite directions:

- **Averaging does not help**, as stated above: signal and ghost are both
  coherent, so their ratio is fixed by the hardware forever.
- **But the criterion is not the noise floor referred to full scale.** The
  ghost becomes visible when the *achieved SNR of the strongest peak*
  exceeds the SFDR. The oscillator table earlier in this chapter compares
  SFDR against `6.02·B + 1.76 + 10·log10(R)`, which is the SNR of a
  **full-scale** signal — so it is conservative by exactly the headroom
  between the strongest peak and full scale.

Worked: 16-bit converter, analog noise at `σ = 1` LSB, so full scale sits
90.3 dB above the noise. Add 34.0 dB of processing gain for `R = 2500`:

| strongest peak | its SNR after `R` | ghost, relative to noise | after 256 averages |
|---|---:|---:|---:|
| full scale | 124.3 dB | +28.0 dB | +52.1 dB |
| −20 dB FS | 104.3 dB | +8.0 dB | +32.1 dB |
| −40 dB FS | 84.3 dB | −12.0 dB | +12.1 dB |
| −60 dB FS | 64.3 dB | −32.0 dB | −7.9 dB |

(ghost at −96.3 dBc relative to the peak.)

**So the question that actually decides the oscillator is: how far below
full scale does the strongest peak sit, and how deep is the averaging?**

The boundary on the 9 Kbit table is exact, and worth remembering:

> A 16-bit chain at `σ = 1` LSB and `R = 2500` puts the ghost **precisely
> at the noise floor** when the strongest peak is 40 dB below full scale
> and 16 transients are averaged.

Every 6 dB of extra headroom buries it by 6 dB; every quadrupling of the
averaging lifts it by 6 dB. Fill the converter and average 256 times and
the ghost is 52 dB above the noise — that chain needs
`SinCosLinearInterp28`, whose −164.5 dBc puts it 16 dB under the noise
again.

### One correction to the published SFDR figures

The error spectrum has a component at the carrier frequency itself,
harmonic `m = 1`. Added to the ideal output that is a **gain and
static-phase error**, not an artifact: it scales the signal and shifts its
phase by a constant, which a spectrum corrects in post-processing and which
averaging preserves harmlessly.

`model::worst_exact_spur_for` filters lines by their distance from the
carrier, so it *includes* that line — and at the shipped `Default` widths
it is the tallest line in the spectrum, 4.4 dB above the worst genuine
spur. **So the published SFDR figures are a few dB pessimistic**, not
wrong. Recorded rather than corrected: they are used as a budget, and a
conservative budget errs in the safe direction.

## The mixer

Three properties, none negotiable.

**Convergent rounding (round-half-to-even).** Chosen by measurement, not
convention. Narrowing the oscillator to 14 bits, worst discrete spur
against the broadband floor:

| rule | worst spur | floor | DC |
|---|---:|---:|---:|
| truncate | −81.1 dBc | −138.3 | **−79.1** |
| round-half-up | −98.0 | −138.3 | −96.0 |
| **convergent** | **−103.0** | −137.3 | −102.2 |
| dither | −104.1 | −125.3 | −102.2 |

The usual argument for skipping convergent — that exact ties are rare —
holds when many bits are discarded. Here the drop is small, ties are about
**one sample in 16**, and rounding all of them the same way is an error
*correlated with the signal*: a spur, not noise. So it is coherent, so it
never averages away, so truncation costs you 22 dB of permanent artifact
floor for nothing.

Note the DC column. After downconversion, a rounding-induced DC offset
lands at the **centre of the spectrum** — the artifact NMR spectroscopists
know as the centre spike. Truncation puts it at −79 dB and convergent at
−102 dB, and it is coherent, so it is there after a million transients.

**The dither row deserves a re-read for an averaging instrument.**
`dsp::mixer`'s recorded decision rejects dither because it "buys 1.1 dB of
spur for 13 dB of floor, which is the wrong trade for a
sensitivity-limited instrument". That reasoning is about a *single*
transient and is right for one. After 4096 averages the floor has dropped
36 dB and the spur has not moved: every rule is spur-limited, the floor
penalty has become free, and dither's spur is the better of the two by
1.1 dB. The conclusion does not change much — 1.1 dB is not worth a
redesign — but the *reason* for it does, and anyone re-opening that
decision for a heavily-averaged instrument should know the trade inverts.

**Product width `PROD_W = A + B + 1`.** Each output component is a
difference of two products, and the widget asserts this rather than
trusting it. Too narrow does not lose precision, it wraps on the largest
sample: a wrong answer, not a noisy one.

**No saturation.** The full product is carried at its natural width, so
the maximum-negative-squared case cannot overflow. Overflow at a narrowing
stage is a consequence of the output width you chose, not of the
multiplier.

## The CIC decimator

### Accumulator width is correctness, not precision

`W_ACC` must satisfy `cic::accumulator_width_is_sufficient`, and `Default`
asserts it. Too narrow is **not** a precision trade: the integrators wrap
continuously and only cancel in the combs when the datapath is wide enough
to carry `(R·M)^N` times the input. One bit short is a wrong answer.

### The pruning budget is a dial between silicon and dynamic range

Hogenauer's §V schedule deliberately injects truncation noise up to the
output's own quantisation step, and it is spent against **the SNR you
asked for**. Same chain, same widths, only `min_snr_db` changing:

| asked | achieved | register bits |
|---:|---:|---:|
| 0 dB | 4.5 dB | 264 |
| 40 | 44.2 | 281 |
| 80 | 84.3 | 309 |
| 100 | 107.0 | 330 |
| 120 | 124.5 | 360 |

**Leaving `min_snr_db` slack destroys the instrument, silently.** A chain
pruned to 4.5 dB looks perfectly healthy on a single transient's spectrum
and cannot average. And the fix is cheap: 120 dB of dynamic range costs
36% more register bits than 0 dB, and *nothing else* — the filter, the
split and the alias rejection are identical, because this is a noise
decision, not a response decision.

Is the pruning noise itself dithered? Yes, at `σ ≈ 1.4–1.7` LSB of the
final stage — above the quantisation step, so it is random and it averages
down — **provided the signal reaching it carries noise of at least an
LSB**. Which is the ADC requirement again, propagated. Every quantisation
in the chain inherits it.

### A wider output word is better *and* cheaper

At a fixed 100 dB requirement, 16-bit input:

| `output_width` | achieved | register bits |
|---:|---:|---:|
| 16 | 100.7 dB | 480 |
| 20 | 103.8 | 360 |
| 24 | 107.0 | 330 |
| 28 | 108.3 | **309** |

Widening the output from 16 to 28 bits gains 7.6 dB **and saves 171
register bits.** The reason: the output quantisation floor is `1/12` LSB²
of an *output* LSB, so a narrow output has a high absolute noise floor and
leaves the schedule no room to prune — the stage widths have to stay large
to meet the requirement.

**So size the output word from the processing gain, not from the ADC.**
Input + 6 bits is the floor; more is free or better.

## Phase correctness

Four mechanisms, three of them structural.

- **The CIC is linear phase by construction.** Its composite response is a
  boxcar of length `R·M` cascaded `N` times, which is symmetric. There is
  no phase distortion to correct and no phase error from declining to
  compensate.
- **Both arms are forced to be the same type.** `Ddc::new` takes *one*
  decimator and clones it into the in-phase and quadrature paths. An I/Q
  asymmetry rotates the constellation, which is the one error a
  phase-sensitive measurement cannot absorb — so it is made
  unrepresentable rather than merely discouraged.
- **The framing alignment is checked.** `IqCombine` requires the two arms'
  markers to agree and reports `frame_mismatch` when they do not. An
  earlier version took the real side's frame and discarded the imaginary
  side's, which turned two drifted paths into a confident wrong answer.
- **The oscillator's phase accumulator is never reset** by the
  down-converter: its phase is absolute elapsed time, which is what makes
  successive acquisitions comparable.

Group delay for a `50 × 50` split with an 11-tap compensator is **17 904
input samples = 0.14 ms**, dominated by the compensator. Constant and
exact — but it is dead time before the first valid sample, and it belongs
in the sequence timing.

## Compensation *is* needed here

Note the contrast with a narrowband transmit pulse, where the droop is
microdecibels and a compensator is pure cost. On receive the output rate
is set by the *spectral width*, so the band is genuinely full: at
`R = 2500` and ±10 kHz the passband is **0.4 of the output Nyquist**, and
the uncompensated droop across it is 1.2 dB at `N = 2` rising to 3.5 dB at
`N = 6`.

A 3 dB tilt across the spectrum is a systematic amplitude error on every
peak, varying with offset — an integration error, not a noise problem, and
one that averaging preserves perfectly. So compensate on receive.

And unlike the transmit case, a receive-side compensator sits *after* the
fold, so its stopband **is** part of the alias budget: taps buy rejection
here.

## The worked chain

```rust,ignore
{{#rustdoc_include ../code/src/dsp/nmr.rs:spec}}
```

At 16-bit in, 24-bit out, 100 dB asked: split `[100, 25]`, `N = [2, 6]`,
achieving 107.0 dB of dynamic range, 83.7 dB of alias rejection and
0.031 dB of passband ripple for 330 register bits.

## Checklist

| stage | requirement | why |
|---|---|---|
| Analog gain | noise `σ ≈ 0.5–1` LSB rms | below it the quantiser freezes and averaging fails |
| ADC | 14 or 16 bit | sets the pre-averaging floor: 120 or 132 dB in a 20 kHz band |
| NCO | SFDR below the *final averaged* floor | spurs are coherent and never average down |
| NCO | phase accumulator not reset | absolute phase makes acquisitions comparable |
| Mixer | convergent rounding | truncation's error is signal-correlated: 22 dB of permanent artifact |
| Mixer | `PROD_W = A + B + 1` | narrower wraps on the largest sample |
| CIC | `accumulator_width_is_sufficient` | one bit short is a wrong answer, not a noisy one |
| CIC | `min_snr_db` stated, not slack | the pruning schedule spends exactly what you allow |
| CIC | output width ≥ input + `0.5·log2(R)` | narrower discards processing gain, and costs more registers |
| CIC | compensator fitted | on receive the band is full; 1–3.5 dB of tilt otherwise |
| Both arms | same decimator type | I/Q asymmetry rotates the constellation |
| Sample clock | `σ_t` < 199 fs rms at 10 MHz for 16 bits | jitter SNR is `−20·log10(2π f_in σ_t)`; it averages down but caps the single transient |

## Sampling clock jitter

The one budget item above that is *not* in the digital design at all, and
frequently the binding one. Aperture jitter `σ_t` on the sample clock
converts input slew into amplitude error, giving

```text
  SNR_jitter = −20·log10( 2π · f_in · σ_t )   dB
```

which depends on the **input frequency**, not on the sample rate — so it
is a direct-sampling constraint and it is unforgiving:

| `σ_t` | `f_in` = 1 MHz | 10 MHz | 50 MHz | 100 MHz |
|---:|---:|---:|---:|---:|
| 10 ps | 84 dB | 64 dB | 50 dB | 44 dB |
| 1 ps | 104 dB | 84 dB | 70 dB | 64 dB |
| 100 fs | 124 dB | 104 dB | 90 dB | 84 dB |
| 10 fs | 144 dB | 124 dB | 110 dB | 104 dB |

Read against the converter — the jitter that puts the jitter floor exactly
at the converter's own SNR:

| | at `f_in` = 10 MHz | at 100 MHz |
|---|---:|---:|
| 14-bit (86.0 dB) | 794 fs | 79.4 fs |
| 16-bit (98.1 dB) | **199 fs** | **19.9 fs** |

A 16-bit converter sampling a 10 MHz input wants **200 femtoseconds rms**,
and 20 fs at 100 MHz. Those are demanding numbers — the second is beyond
most clock sources — and they scale inversely with input frequency, which
is exactly what makes direct sampling of a high-field NMR signal hard.

(An earlier draft of this section said "better than 1 ps at 10 MHz". That
was wrong by 5×, and 5× on a jitter spec is the difference between an
ordinary oscillator and an expensive one. The number is computed and
pinned now.)

Three consequences for this chapter:

- **Jitter noise is broadband and random, so it *does* average down.**
  Unlike an NCO spur it is incoherent between transients — which is the
  one piece of good news, and the reason it belongs at the end rather than
  the beginning.
- **Processing gain applies to it too.** The `10·log10(R)` above is a
  noise-bandwidth argument and does not care where the noise came from,
  so a jitter-limited chain still benefits from decimation.
- **But it is a floor the digital design cannot lift.** If jitter puts the
  single-transient SNR at 70 dB, then the 132.1 dB in the budget table is
  fiction and every width in this chapter is oversized. **Measure the
  clock before sizing the datapath.**

Nothing in this repository models jitter, and nothing can: it is a property
of the clock source and the converter, not of the RTL. It is named here so
the omission is visible rather than implied.

## What this does not cover

- **The analog front end** — preamp noise figure, image rejection ahead of
  the converter, anti-alias filtering.
- **Timing closure.** Nothing about fmax is measured anywhere in this
  repository; the widths above are correct but their achievable clock rate
  has never been checked against a vendor timing report.
- **Non-power-of-two phase cycles.** The measurement says K = 3 or 5
  annihilates the dominant spur where K = 2/4/8/16 cannot. Whether such a
  cycle fits an experiment's phase bookkeeping is a spectroscopy question
  this chapter cannot answer.
