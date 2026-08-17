# Fixed-point and DDS references

Background reading for `crates/rhdl-fpga/src/dsp/`, collected while
building the NCO. Organised by what question each source answers, not
by author, because the useful thing when you are mid-design is "where is
the bit about overflow" rather than "what did Yates write".

Everything committed here is freely redistributable — vendor technical
articles, university course material, and open-access papers. Textbooks
are listed at the bottom with what to look for in each, but not
included.

---

## Fixed-point fundamentals

**`yates-fixed-point-arithmetic-intro.pdf`** — Randy Yates, *Fixed-Point
Arithmetic: An Introduction* (Digital Signal Labs technical reference,
26 pp).

The best starting point. Signed and unsigned fixed-point
representations, the distinction between **precision and range** as
separate properties, what each arithmetic operation does to the format,
and a worked analysis of a fixed-point algorithm. Read this first if the
Q-notation in a datasheet is not immediately obvious.

**`yates-fixed-point-fir-implementation.pdf`** — Randy Yates, *Practical
Considerations in Fixed-Point FIR Filter Implementations*.

The same material applied to a real datapath: choosing the output word
length, quantisation noise, and overflow. Its keyword list is literally
"DSP, FIR, overflow, fixed-point, fractional, signed, unsigned, two's
complement", which is the set of things that bite in practice.

**`gatech-ece4270-L20-fixed-point.pdf`** — Georgia Tech ECE4270
*Fundamentals of DSP*, Lecture 20: fixed-point arithmetic.

Course-slide treatment of finite word length: quantisation as additive
noise, roundoff error propagation, and scaling. Quick to skim.

**`mit-ocw-6341-lec06-quantization.pdf`** — MIT 6.341 *Discrete-Time
Signal Processing*, Lecture 6 (OpenCourseWare).

The Oppenheim-school treatment in lecture form: why a quantiser, being
strongly non-linear, is replaced by an additive-noise model, and what
that buys analytically. This is the compressed version of the textbook
chapter listed below.

### The principle these share

Do **range analysis** — bound every intermediate value — and then choose
a format in which overflow cannot occur, rather than detecting overflow
at runtime and saturating. Saturation is the fallback for when the
bound genuinely cannot be established up front.

That is the rule `dsp::nco::sin_cos_linear_interp` follows: `TABLE_SCALE`
leaves one LSB of headroom so the interpolated sum cannot leave the
18-bit range, and `interpolated_sum_never_leaves_the_range` establishes
the bound by exhausting all 2²² phases rather than by hand. See that
widget's module docs for why wrapping there costs up to 96 dB of SFDR.

---

## Direct digital synthesis

**`adi-almost-pure-dds-sine-tone-generator.pdf`** — Patrick Butler
(Analog Devices), *An Almost Pure DDS Sine Wave Tone Generator*.

Applications-engineer treatment of where DDS spectral purity actually
comes from. Useful for the framing that the dominant limit is phase
quantisation from a finite lookup table, and for the practical view of
what the spur floor looks like on a bench.

**`ddfs-optimized-prakash-2014.pdf`** — *An Optimized Direct Digital
Frequency Synthesizer* (Contemporary Engineering Sciences, open access).

Short paper on ROM compression trade-offs.

---

## Not committed — needs a browser

These are freely readable but served behind bot protection or an
interstitial, so they could not be fetched non-interactively:

- **Palomäki & Nurmi, *Taylor Series Interpolation-Based DDFS with High
  Memory Compression Ratio*** (Sensors 2025) —
  <https://www.mdpi.com/1424-8220/25/8/2403>.
  **The most directly comparable published design**: 16-bit quadrature
  DDFS using *second*-order Taylor interpolation, 107 slices + 3 DSPs on
  an Artix-7, reaching −102.9 dBc. Our widget uses *first*-order
  interpolation at 18 bits and measures −116 dBc, so this is the natural
  comparison point if the numbers are ever challenged.
- **Zhou, Xu & Zhang, *Optimized Design of DDFS Based on Hermite
  Interpolation*** (Sensors 2024, doi:10.3390/s24196285) —
  <https://www.mdpi.com/1424-8220/24/19/6285>. Cubic Hermite
  interpolation, 1792:1 ROM compression, −88.134 dBc at 14-bit output.
- **A Recursive Trigonometric Technique for DDFS** (Electronics 2024) —
  <https://www.mdpi.com/2079-9292/13/23/4762> — and its **Hybrid**
  successor (Electronics 2025) —
  <https://www.mdpi.com/2079-9292/14/15/3027>. The recurrence-based
  alternative to both table lookup and CORDIC.
- **AMD/Xilinx DDS Compiler datasheets** — DS558 (v4.0) and DS794 (v5.0)
  on <https://docs.amd.com>. Source of the −118 dBc swept-frequency
  figures for Taylor-corrected mode that the module docs cite.
- **AMD AM004, Versal DSP engine — Overflow/Underflow/Saturation** —
  <https://docs.amd.com/r/en-US/am004-versal-dsp-engine/Overflow/Underflow/Saturation>.
  The vendor definition of saturating arithmetic.
- **ADI, *Fundamentals of Sampled Data Systems*** (Data Conversion
  Handbook, ch. 2). Source for the two's-complement endpoint convention
  — +FS ≈ FS − 1 LSB — which is *why* the positive rail is the one that
  overflows.

---

## Books — not included, but these are the ones

- **Padgett & Anderson, *Fixed-Point Signal Processing*** (Synthesis
  Lectures on Signal Processing, ~100 pp). Written explicitly to bridge
  ideal-precision DSP teaching and limited-precision implementation on
  DSPs **and FPGAs**. **Chapter 6** is product roundoff error and
  methods of scaling to avoid overflow — the closest published statement
  of the approach this codebase uses.
  <https://link.springer.com/book/10.1007/978-3-031-02533-4>
- **Oppenheim & Schafer, *Discrete-Time Signal Processing***. The
  rigorous treatment: additive-noise model for quantisation, coefficient
  quantisation, roundoff noise, overflow, zero-input limit cycles.
  Chapter numbering moves between editions — in the 3rd edition it is
  the later sections of the structures chapter; the 1975 *Digital Signal
  Processing* put it in ch. 8–10. Search by topic, not number.
- **Meyer-Baese, *Digital Signal Processing with Field Programmable Gate
  Arrays***. The FPGA-side one: computer arithmetic for hardware,
  accuracy against area and latency, plus DDS and CORDIC material
  directly relevant to `dsp::nco`.
- **Vankka & Halonen, *Direct Digital Synthesizers: Theory, Design and
  Applications***. The DDS reference — phase truncation, amplitude
  quantisation, ROM compression, and the interpolation families
  (Taylor, Sunderland dual-ROM, CORDIC) compared.

### What none of them cover

The specific failure this codebase hit — a first-order-interpolated sine
table overshooting the positive rail by exactly one LSB — is not a
worked example anywhere I could find. It falls out of ordinary range
analysis; the DDS literature simply does not discuss it, plausibly
because those papers evaluate their interpolators in floating point
where the rail does not exist. Treat the measured numbers in
`sin_cos_linear_interp`'s module docs as the primary source for that
claim, not any of the above.
