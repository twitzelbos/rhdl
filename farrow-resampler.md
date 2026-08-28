# The Farrow Resampler

> **Status: a scoping document, not yet built.** It specifies a
> fractional-rate resampler for the transmit and receive chains, the
> architecture it belongs in, and — because this is the part that decides
> whether it is usable at all — **the phase contract it must satisfy and
> the tests that prove it**.
>
> Prerequisites: `crates/rhdl-fpga/src/dsp/cic/` (interpolator, decimator
> and the variable-rate machinery) and `crates/rhdl-dsp-design/src/cic/`.
> Read `interp_chain`'s module docs first; §2 here is the direct
> consequence of its findings.

---

## 0 — Amendment: this widget is often unnecessary, and here is the evidence

**Written after §1–§10, on being asked whether a sinc or Fourier method
would be better and being told the resampling happens upstream of the DUC
and could be done in software.** It could, and if it can then most of
this document is the wrong plan. Recorded here rather than quietly
revised, because the measurements are the useful part.

### Which filter is best depends entirely on where the signal sits

Worst-case fractional-delay error over `μ`, amplitude dB / phase degrees:

| filter | bw = 0.20 | bw = 0.10 | bw = 0.02 |
|---|---|---|---|
| Lagrange `K=4` (cubic) | 0.457 / 3.840 | 0.045 / 0.161 | 0.00008 / 0.00006 |
| Lagrange `K=8` | 0.040 / 0.299 | 0.0002 / 0.0009 | 0.00000 / 0.00000 |
| Lagrange `K=16` | 0.0004 / 0.0029 | 0.00000 / 0.00000 | 0.00000 / 0.00000 |
| Kaiser-sinc `K=16` | 0.0056 / 0.0165 | 0.0050 / 0.0165 | 0.00095 / 0.0134 |
| Kaiser-sinc `K=32` | 0.00028 / 0.00038 | 0.00028 / 0.00038 | 0.00020 / 0.00038 |
| Kaiser-sinc `K=64` | 0.00002 / 0.00007 | 0.00002 / 0.00007 | 0.00002 / 0.00007 |
| Kaiser-sinc `K=256` | 0.00000 / 0.00000 | 0.00000 / 0.00000 | 0.00000 / 0.00000 |

Two things fall out, and they point in opposite directions:

- **Windowed sinc is far better wideband.** At `bw = 0.20`, `K = 16`
  Kaiser-sinc gives 0.0056 dB where cubic Lagrange gives 0.457 — a factor
  of eighty. Its error is also nearly *flat* across bandwidth, because it
  is set by the window's ripple rather than concentrated at high
  frequency.
- **Lagrange is better narrowband, per tap.** At `bw = 0.02`, `K = 8`
  Lagrange is exact to the printed precision while `K = 16` Kaiser-sinc
  still shows 0.00095 dB. Lagrange is maximally flat at DC, which is
  exactly the right optimality criterion for a heavily oversampled
  signal.

So §2's placement argument survives — **after the CIC, Lagrange is the
right filter and a short one suffices.** What changes is that §2's
rejected placement (before the CIC, `bw = 0.2`) is not intrinsically
hopeless; it is hopeless *for Lagrange*. A 32-tap windowed sinc there
gives 0.0003 dB and 0.0004°, which does not falsify phase.

### And in the frequency domain the problem is simply solved

Spectral resampling — FFT the block, zero-pad or truncate, inverse FFT at
the new length — on an exactly-periodic band-limited block:

| ratio | output samples | max error |
|---|---|---|
| 5/4 | 5120 | 1.7e-13 (**−265 dB**) |
| 7/5 | 5734 | 1.4e-13 (−267 dB) |
| 125/124 | 4129 | 2.0e-13 (−264 dB) |

Machine precision, and **100 dB better than any FIR in the table**. Phase
is exact by construction, because a spectral operation has no
approximation to be phase-wrong about.

The catch is periodicity, and it is bounded rather than fatal: on a
deliberately non-periodic block the error is 0.19 at the edges and 1.3e-4
in the interior. **The error lives entirely at the block boundaries**,
which is what overlap-save with a guard region exists to fix. The other
constraint is that the ratio must be rational with an integral output
length — not a real limitation, since a 48-bit rational approximates any
ratio to below the noise floor of anything downstream.

### Which means the cheapest architecture has no resampler in it at all

If the host can resample in software:

```text
  host: envelope at whatever rate     host: exact resample to fs/R      FPGA
        ------------------------>     [ FFT, -265 dB, phase exact ]  -> [ CIC x R ]
                                      R chosen as an integer            existing widget
```

The host picks an **integer** `R`, resamples its envelope to exactly
`fs / R` — exactly, in software — and the FPGA does integer interpolation
with the CIC that already exists. **No Farrow, no new hardware, and
better accuracy than any hardware option.** The coarse/fine split of §2
survives; the fine part just moves to where it is free.

### And better still: if the envelope is synthesised, do not resample

Resampling is only necessary when the envelope arrives at a rate you did
not choose — a file, a codec, a live feed. If the host *computes* the
envelope — a pulse shape, a chirp, a symbol stream through a shaping
filter — then evaluate the waveform at the output instants `t = n·R/fs`
directly. Zero error, exactly correct phase, no filter of any kind. This
is worth checking before anything else in this document is built.

### So when is the hardware widget still the answer?

Three cases, and they are real:

- **A live source at an unrelated clock** — an ADC feed, a receiver
  chain — where there is nothing upstream to do the resampling and no
  block boundary to work with.
- **A rate that must change continuously**, with no point at which a
  block-based method could re-plan.
- **A closed loop** — sample-rate tracking against a recovered clock —
  where the ratio is a control signal rather than a configuration value.

For those, §§1–10 stand as written and Lagrange-after-CIC is the right
shape. For a host-driven transmit envelope, which is the case that
prompted this document, **the software route is better on every axis and
should be preferred.**

Phasing consequence: **Phase A (design maths) is still worth doing** —
`worst_error` and `required_order` are what let a caller decide between
these options with numbers, and they are days of work with no hardware.
Phases B–E should wait for one of the three cases above to be real.

---

## 1 — The gap

The CIC chain reaches integer rates only, and a *split* chain does not
even reach all of those. `interp_chain` measured it: a `5 × 25` chain
reaches 73 of the 124 rates below 125, and every prime above 25 is out —
setting a stage to `R = 1` does not rescue them, because for a prime
total the cap is `max(per-stage factor)` rather than the product.

Reaching every integer therefore forces a single stage
(`InterpSpec::arbitrary_rate`), and that costs roughly twice the
rate-weighted cost because all `N` integrators run at the converter
clock. It also does not help at all with a rate the host cannot express
as an integer division of the converter rate — a 44.1 kHz-family
envelope on a 125 MHz converter, say, where the ratio is irrational for
practical purposes.

**A fractional resampler removes the integer constraint entirely.** The
standard structure for it is Farrow's: a fractional-delay FIR whose
coefficients are polynomials in the fractional offset, so the offset can
be any value and is computed rather than looked up.

---

## 2 — Where it goes, and why that is not obvious

The tempting placements are both wrong, and the numbers say so clearly.

**Before the CIC, at the envelope rate?** Cheap — it runs at 1 MHz. But
the signal occupies a large fraction of the envelope Nyquist, which is
exactly where a Farrow is worst. Worst-case error over `μ` for a cubic
Lagrange design:

| bandwidth (fraction of `fs`) | amplitude error | phase error |
|---|---|---|
| 0.05 | 0.003 dB | 0.005° |
| 0.10 | 0.045 dB | 0.161° |
| 0.20 | 0.457 dB | 3.84° |
| 0.40 | 6.96 dB | 48.0° |

At the 200 kHz-on-1 Msps configuration the signal sits at 0.2 of the
envelope rate: **0.46 dB and 3.8° of error**, which falsifies phase by
any standard.

**Doing the whole rate change in the Farrow?** Also wrong, for a
different reason. Lagrange interpolators are maximally flat at DC, not
stopband designs — their image rejection is poor. A Farrow upsampling by
15 would leave the images the CIC exists to remove.

**The right placement is *after* the CIC, doing only a fine trim.**

- The CIC's **integer** runtime rate does the coarse work. Choose
  `R = ceil(fs / f_env)`, so the intermediate rate lands in
  `[fs, fs + f_env)`.
- The Farrow then corrects a ratio of at most `1 + 1/R` — **under 0.25%**
  at every envelope rate tried.
- And it does that correction where the signal is a *tiny* fraction of
  the sample rate: 200 kHz at 125 Msps is 0.0016.

At which point the error stops being a consideration:

| `K` | order | error at bw = 0.0016 | at 0.02 |
|---|---|---|---|
| 2 (linear) | 1 | 0.00033 dB, 0.000015° | 0.051 dB, 0.028° |
| 3 | 2 | 0.0000005 dB, 0.000004° | 0.00007 dB, 0.0073° |
| **4 (cubic)** | 3 | **0.0000000 dB, 0.0000000°** | 0.00008 dB, 0.000056° |

Cubic Lagrange is exact to the printed precision in this role, with two
decades of bandwidth margin. Even linear would pass, which is the sign
that the *placement* is doing the work rather than the filter order.

```text
  envelope        CIC, integer R          Farrow, ratio ~1        DAC
  f_env  ---->  [ interpolate x R ]  ---->  [ fine trim ]  ---->  fs
  arbitrary       coarse, cheap             fractional, exact
                  images killed here        phase set here
```

**The coarse/fine split is the whole idea.** Neither block is asked to do
the thing it is bad at.

---

## 3 — What a Farrow structure is

A fractional-delay FIR of length `K` whose taps depend on the offset
`μ ∈ [0, 1)`:

```text
  y = Σ_k h_k(μ) · x[n−k],        h_k(μ) = Σ_m c_{k,m} μ^m
```

Reordering the sums moves `μ` outside:

```text
  y = Σ_m μ^m · ( Σ_k c_{k,m} · x[n−k] )
      \_______/   \____________________/
       Horner        M+1 fixed FIRs
```

So it is `M+1` **fixed** FIR branches of length `K`, combined by Horner's
rule in `μ`. Nothing depends on `μ` except the `M` Horner multiplies —
which is what makes an arbitrary, continuously-varying offset affordable.

For Lagrange coefficients `M = K − 1`, and the branch coefficients are
exact rationals with small denominators.

---

## 4 — The phase contract

**This is the section that decides whether the widget is usable.** A
resampler's *purpose* is to shift sample instants, so it changes phase by
construction; the requirement is that the change is **exactly what the
caller asked for, exactly knowable, and identical on both quadratures.**

Six clauses. Each names the hazard, how it is closed, and the test.

### 4.1 — The output's position in input time is exact

**Hazard.** If the phase accumulator drifts — a float accumulator, or an
increment that is not representable — the output's time origin drifts
with it, and a phase measurement is wrong by an amount nobody can
compute.

**Closure.** The accumulator is fixed point, `PHASE_W = 48` bits, with
the same discipline as `dsp::nco`: the increment is an exact integer and
the accumulator represents absolute elapsed *input* time. It is never
reset at a burst boundary, so successive bursts share an origin.

**Exposed, not just correct.** `Out::phase` carries the accumulator, so a
consumer knows precisely where in input time each output sample sits.
This is the resampler's analogue of `Nco`'s `master` output and it is not
optional: the group delay varies per output sample by design, so a
consumer that cannot read the accumulator cannot correct for it.

**Test.** Drive a known ratio for `10^6` output samples and require the
accumulator to equal `n · ratio` exactly, in integers, with no
accumulated error. And require it to be *monotone* modulo the wrap.

### 4.2 — Both quadratures use the same `μ`, structurally

**Hazard.** The one error a phase-sensitive chain cannot absorb. Two
independent resamplers on I and Q, given even slightly different offsets,
rotate the constellation.

**Closure.** The widget takes `Iq<W>` and holds **one** accumulator. Not
two widgets with a shared input — one widget, one `μ`, two arms. The same
reasoning as `EnvelopeUpsampler::new` taking a single interpolator and
cloning it, taken one step further: here the asymmetry is not merely
unrepresentable in the type, it has no place to live.

**Test.** A real-only input must produce a real-only output and a
quadrature-only input a quadrature-only output, at every `μ` — the
arms-do-not-cross test, which catches a swapped or independently-offset
pair.

### 4.3 — DC gain is exactly one at every `μ`

**Hazard.** The subtle and expensive one. If `Σ_k h_k(μ) ≠ 1`, the gain
depends on `μ`, and `μ` sweeps at the resampling beat frequency — so a
constant envelope acquires **amplitude modulation at the beat rate**, a
discrete spur rather than noise. This is the resampler's version of the
rounding-rule spur measurement in `dsp::mixer`.

**Closure.** Lagrange coefficients satisfy `Σ_k h_k(μ) = 1` identically,
which in branch terms is

```text
  Σ_k c_{k,0} = 1        and        Σ_k c_{k,m} = 0   for m ≥ 1
```

Verified above: the DC gain error is zero or one machine epsilon at every
`K` and every `μ` tried.

**But quantisation breaks it**, and this is the requirement that has to
be written down. Rounding each branch's coefficients independently
destroys both sums. The fix is exact and integral: trim one coefficient
per branch so that branch zero sums to exactly `1 << SHIFT` and every
higher branch sums to exactly zero. `compensator::quantise` already does
the analogous thing for a compensator's centre tap, and the same argument
applies with more force here, because the error is `μ`-correlated rather
than static.

**Test.** For every branch, assert the integer coefficient sum is exactly
`1 << SHIFT` or exactly `0`. Then, behaviourally: a constant input must
produce an **exactly constant** output while `μ` sweeps the full range —
the same shape of exact test as the CIC interpolator's
`a_constant_input_becomes_an_exact_constant`, and for the same reason.

### 4.4 — The phase error against ideal is bounded and reported

**Hazard.** A resampler that is "close enough" without saying how close
cannot be used in a phase-sensitive measurement, because the caller
cannot put a number on their uncertainty.

**Closure.** `rhdl-dsp-design` gains a `farrow` module reporting, for a
given `(K, M, bandwidth)`, the worst-case amplitude and phase error over
`μ ∈ [0,1)` — the tables in §2, computed rather than tabulated by hand.
The PDF report gains a page.

**Test.** The design-maths figures are checked against a direct
evaluation of the quantised branch coefficients, and the *hardware* is
checked against the design maths: drive a tone, measure the output phase
by DFT, compare against the accumulator's prediction, and require the
residual to be inside the reported bound.

### 4.5 — The framing mark maps to a defined output sample

**Hazard.** `SyncMark` anchors an acquisition. If a mark's output
position is "wherever it happens to fall", the anchor is meaningless and
every downstream phase measurement is relative to nothing.

**Closure.** Define it once, explicitly: **a mark on an input sample
emerges on the first output sample whose time is at or after that input's
time.** Computable exactly from the accumulator, and independent of `K`
and of the pipeline depth — which is what makes it a contract rather than
an implementation detail. The group delay is reported separately and is
the caller's to subtract, exactly as `interp_stream` already specifies
for the interpolator.

**Test.** Sweep the mark across every position within an input period at
several ratios, and require the output index to match the closed-form
prediction every time. Off-by-one here is the single most likely bug in
the widget.

### 4.6 — The group delay is stated, in both of its parts

**Hazard.** Understating it. The up-converter's docs already got this
wrong once by a factor of the FIR's own group delay.

**Closure.** Two parts, documented separately because only one is
constant:

- `(K − 1) / 2` input samples — the interpolator's own centre, fixed.
- `μ` input samples — **varies per output sample**, by design, readable
  from `Out::phase`.

Plus the pipeline registers, which are constant and counted.

**Test.** An impulse in, and the first-nonzero-output index must match
`(K−1)/2 + μ + pipeline` for the `μ` the accumulator held — fitted across
ratios, as `interp_variable_rate.rs` does for the comb pipeline, so the
constant part and the varying part are separately confirmed.

---

## 5 — Architecture

One new module, `crates/rhdl-fpga/src/dsp/farrow/`, and one design-maths
module, `crates/rhdl-dsp-design/src/farrow.rs`.

```rust
/// A fractional-rate resampler for a complex envelope.
pub struct FarrowResampler<
    const W: usize,      // sample width per component
    const WC: usize,     // branch coefficient width
    const WACC: usize,   // branch accumulator width
    const K: usize,      // interpolator length
    const M: usize,      // polynomial order, K-1 for Lagrange
    const SHIFT: usize,  // coefficient fractional bits
> { .. }

pub struct In<const W: usize> {
    /// The input stream. Consumed when the accumulator says so, which is
    /// zero, one or two samples per output cycle for a ratio near one.
    pub stream: Option<Item<Iq<W>, SyncMark>>,
    /// Input samples per output sample, in `PHASE_W` fixed point.
    /// `1 << (PHASE_W - 1)` is unity.
    pub ratio: Bits<PHASE_W>,
    pub downstream_ready: bool,
}

pub struct Out<const W: usize> {
    pub stream: RCStream<Iq<W>, SyncMark>,
    /// Absolute elapsed input time. §4.1 — this is load bearing.
    pub phase: Bits<PHASE_W>,
    /// The input was not there when the accumulator asked.
    pub starved: bool,
    pub overrun: bool,
    pub saturated: bool,
}
```

**It presents an `RCStream`, so it composes** with
`EnvelopeUpsampler`, both up-converters and the down-converter without
any of them knowing. It is not an interpolator and does not present
`interpolator::In`/`Out` — it changes rate by a *ratio*, not a factor,
and pretending otherwise would misuse the slot.

**Consuming zero, one or two inputs per output cycle** is the one
structural wrinkle. For a ratio near one it is almost always one; the
delay line therefore needs a two-position shift, and `Out::stream.ready`
is asserted on cycles where an input is taken. A ratio above two is
rejected at construction: that is a job for the CIC.

---

## 6 — Design maths

`rhdl-dsp-design/src/farrow.rs`:

- `lagrange_branches(K) -> Vec<Vec<f64>>` — the exact branch
  coefficients, `M+1` branches of `K`.
- `quantise(branches, shift, width) -> Quantised` — integer coefficients
  **with the sum constraints enforced exactly** (§4.3), plus the residual
  error the trimming introduced.
- `worst_error(branches, bandwidth) -> (amp_db, phase_deg)` — worst over
  `μ`, which is what §4.4 reports.
- `required_order(bandwidth, amp_db, phase_deg) -> usize` — the inverse:
  the shortest `K` meeting a stated pair of tolerances. This is the
  function a caller actually wants, and it is what makes the widget
  spec-driven rather than parameter-driven.
- `coarse_rate(fs_hz, f_env_hz) -> usize` — the CIC's integer `R` for the
  coarse/fine split, `ceil(fs / f_env)`, plus the resulting trim ratio so
  a caller can see it is under a percent.

Least-squares and minimax alternatives to Lagrange are a follow-up, not
Phase 1: Lagrange is exact at DC, which §4.3 depends on, and at this
bandwidth its error is already below measurement.

---

## 7 — Validation contract

The five tiers as usual, plus the phase tests of §4, plus:

- **Against an independently written software resampler**, as every CIC
  widget is. Written from the polynomial definition rather than from the
  branch decomposition, so a transcription error in the Horner ordering
  has to be reproduced in a different shape to survive.
- **Round trip.** Resample by `r` then by `1/r` and require the result to
  match the input to within the reported error bound. This is the test
  that catches a sign error in `μ`, which otherwise looks like a small
  delay.
- **Spur floor.** A constant envelope with `μ` sweeping must produce no
  discrete spur above the quantisation floor — the behavioural form of
  §4.3, measured the way `dsp::mixer`'s rounding rules were.
- **End to end through a DUC**, with the resampler between the CIC and
  the mixer, confirming the single-sideband property survives: images
  20× down and the sideband following the envelope's rotation, exactly
  as `duc::real`'s tests require today.

---

## 8 — Cost

`(M+1)` branches × `K` taps, plus `M` Horner multiplies, per component.
Cubic Lagrange on a complex sample: `2 × (4 × 4 + 3) = 38` multiplies at
the converter clock.

That is not cheap, and it should be compared honestly against the
alternative it replaces — the single-stage CIC, which at `R = 125` and
60 dB needs `N = 5` integrators at the converter clock and 351 register
bits per arm, and still only reaches integer rates.

Two reductions worth pricing in Phase 1:

- **Lagrange branch coefficients are small dyadic rationals.** The cubic
  branches are built from `1/6`, `1/2`, `1` and `2`; several taps are
  exactly `±1` or `±1/2` and need a shift rather than a multiplier. The
  38 is an upper bound and the achievable number should be measured from
  the emitted Verilog, the way `RealPartMixer`'s two multiplies are.
- **A lower order.** The table says linear passes at this bandwidth with
  0.0003 dB and 0.000015°. Cubic is chosen for margin, not necessity, and
  `required_order` exists so a caller can make that trade with a number
  rather than a guess.

---

## 9 — Phasing

**A. Design maths.** §6, with the error tables and `required_order`. No
hardware. Tests are the exact-DC-gain property, the quantisation sum
constraints, and agreement with a direct evaluation. Days, not weeks, and
it de-risks everything downstream because the coefficient contract is
where the phase guarantees live.

**B. The real-valued resampler.** One component, so the arms question
does not arise yet. All six phase clauses of §4 except 4.2. Tiers 1–5.

**C. The complex resampler.** `Iq` in, one accumulator, §4.2's structural
guarantee and its test. This is the shippable widget.

**D. The coarse/fine chain.** A `ResampledUpsampler` composing the
variable-rate CIC with the resampler, plus `coarse_rate` wired in, so a
caller states an envelope rate in hertz and gets a working chain. The
end-to-end DUC test lives here.

**E. Report and book chapter.** A page in the interpolation report
showing the error against bandwidth and the trim ratio actually used, and
a chapter on the coarse/fine architecture — which is the part a reader
will get wrong if they meet the resampler on its own.

---

## 10 — Risks and rejected alternatives

- **Polyphase FIR with `L` fixed branches.** Reaches only rational rates
  `L/M`, and the coefficient ROM grows with `L`. Fine for a fixed
  conversion, wrong for a continuously-variable one. Rejected for this
  role; it remains the right answer for a *fixed* rational rate and is
  cheaper there.
- **Linear or cubic interpolation "by hand", without the Farrow
  decomposition.** Numerically identical for a fixed `μ`, but the taps
  must be recomputed per sample, which is exactly what the Farrow form
  avoids. Rejected as a false economy.
- **Putting the resampler before the CIC.** §2. Rejected on measured
  error: 0.46 dB and 3.8° at the configuration in hand.
- **The `μ`-dependent gain is the failure mode to fear**, not the
  passband error. Passband error at this bandwidth is below measurement;
  a broken coefficient sum produces a discrete spur that a broadband
  noise measurement will not find. §4.3's integer sum assertions are
  cheap and are the only thing standing between the widget and that spur.
- **A ratio near an exact integer boundary** makes the integer part of
  the accumulator increment alternate between one and two, so the
  consume-two path is exercised rarely and in a data-dependent way. That
  is the shape of a bug that survives testing; the tests should include a
  ratio chosen deliberately to hit it often and one chosen to hit it
  almost never.
- **Nothing here is measured on hardware.** The error figures are the
  design maths and the cost figures are multiply counts. As with the
  comb-cascade pipelining, a timing and resource report from a machine
  with vendor tools is what would turn them into facts.
