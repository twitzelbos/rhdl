//! Numerically-controlled oscillator building blocks.
//!
//! Split deliberately: the pieces that carry an *invariant* are
//! separate widgets, so each invariant is testable on its own.
//!
//! | module | role | carries an invariant? |
//! |---|---|---|
//! | [`phase_accumulator`] | the free-running master phase | **yes** — a phase offset never perturbs the master trajectory |
//! | [`phase_composer`] | §8.2 phase terms → `phase_offset` | no, an adder tree |
//! | [`frequency_composer`] | §8.3 frequency terms → `frequency_word` | no, an adder tree |
//! | [`sin_cos_linear_interp`] | phase → quadrature amplitude | **yes** — the interpolated sum cannot leave the output range |
//! | [`ramp`] | §8.5 scheduled segments, ramps and chirps | **yes** — a sub-LSB step still moves |
//! | [`modulation`] | §8.6 sample-synchronous frequency deviation | **yes** — an absent sample contributes zero, never hold-last |
//! | [`composite`] | all of the above wired into one `Nco` | **yes** — the truncation takes the high bits |
//! | [`config`] | the coupled numeric constants, as `const fn` | build-time assertions |
//! | [`latency`] | §8.4 control latencies, measured not asserted | build-time assertions |
//! | [`model`] | bit-accurate DDS + spur analysis (not a widget) | — |
//!
//! Note that [`ramp`] and [`modulation`] are **not** inside
//! [`composite`]'s `Nco`. A scheduler wires their outputs into the
//! frequency composer's `master` and `modulation` terms.
//!
//! **This split is deliberate**, decided rather than inherited: §8.4
//! describes a local timing agent that composes these pieces and issues
//! each control change at its own lead time, so `Nco` is a subassembly
//! and the scheduler owns the wiring. The cost is that a composed
//! latency crossing the boundary cannot be measured inside any one
//! widget — so [`latency`]'s `harness` module builds exactly that wiring
//! and measures [`latency::MODULATION_CONTROL`] through it. Any future
//! term added outside `Nco` owes the same treatment.
//!
//! # Structure
//!
//! The accumulator is deliberately minimal. The control surface layers
//! *around* it, and each layer is a plain adder tree or a small FSM
//! carrying no invariant of its own:
//!
//! ```text
//!   scheduled offset ─┐
//!   modulation stream ─┼─► frequency composer ─► frequency_word ─┐
//!   calibration ──────┘                                          │
//!                                                    phase_accumulator ─► phase
//!   pulse phase ──────┐                                          │
//!   frame phase ──────┼─► phase composer ─────► phase_offset ────┘
//!   calibration/trim ─┘
//! ```
//!
//! Keeping them separate means the offset-independence property is
//! provable on a widget with one register, rather than buried inside a
//! block with a dozen control inputs.
//!
//! [`latency`] carries the §8.4 control latencies as compile-time
//! constants, each measured against simulation rather than asserted.
//!
//! **The two control paths do not share a lead time.** Phase reaches
//! the output in [`latency::PHASE_CONTROL`] cycles and frequency in
//! [`latency::FREQUENCY_CONTROL`], because the accumulator adds the
//! offset to its *output* and the frequency word to its *register*. A
//! phase change and a frequency change that must land on the same
//! sample are issued [`latency::FREQUENCY_LEADS_PHASE_BY`] cycles
//! apart.
//!
//! # Sizing the phase-to-amplitude stage
//!
//! > **This section is the historical record of how the architecture was
//! > chosen, and its conclusion was superseded.** It sizes a *plain
//! > lookup table*, and recommends `P = 13`. What was actually built is
//! > [`sin_cos_linear_interp`] — a 10-bit coarse table plus first-order
//! > interpolation, reaching −116 dBc from ~9 Kbit rather than 78 dB
//! > from 32 Kbit. The pivot is recorded under "Linear interpolation vs
//! > CORDIC" below, and that is the live decision. Read this section for
//! > *why the sweep was necessary*, not for the sizing answer.
//!
//! When this was written, phase-to-amplitude conversion was not yet
//! built, on purpose — the choice between a lookup table, a CORDIC, and
//! a hybrid is a spur-performance question, and spur positions move with
//! the tuning word. What *could* be settled up front was the sizing,
//! because the target pins it down.
//!
//! ## Target: 60–70 dB SFDR, quadrature output
//!
//! Phase truncation into the table gives the classic estimate
//!
//! ```text
//! SFDR ≈ 6.02 · P − 3.92   dB        (P = phase bits addressing the LUT)
//! ```
//!
//! | Target | Required `P` |
//! |---|---|
//! | 60 dB | 11 bits |
//! | 70 dB | 13 bits |
//!
//! Amplitude quantisation should sit below that floor: `AMP_W ≥ 12` for
//! 70 dB, and 16 leaves margin while still suiting a 14-bit DAC.
//!
//! ## Quadrature is nearly free
//!
//! Complex modulation needs both sine and cosine. Naively that is two
//! tables. It is not, because
//!
//! ```text
//! cos(θ) = sin(θ + π/2)          π/2 = 2^(P−2) in phase units
//! ```
//!
//! so **one** quarter-wave table serves both, read at two addresses.
//! On a device with true dual-port block RAM that is a single primitive
//! delivering both components in the same cycle.
//!
//! With quarter-wave symmetry the table holds `2^(P−2)` entries:
//!
//! | `P` | entries | at `AMP_W = 16` | BRAM36 |
//! |---|---|---|---|
//! | 11 (60 dB) | 512 | 8 Kbit | 1 |
//! | 13 (70 dB) | 2048 | 32 Kbit | 1 |
//!
//! One block RAM for 70 dB in quadrature. That is the finding that
//! makes the LUT route attractive: **CORDIC exists to avoid
//! exponentially large tables, and at this target the table is not
//! large.** CORDIC would trade one BRAM for a pipeline of adders and
//! shifters, plus its own quantisation behaviour to characterise.
//! Unless the sweep says otherwise, the table wins.
//!
//! ## Budget as a const generic
//!
//! The resource limit belongs in the type, so that the sizing decision
//! is made once and checked by the compiler:
//!
//! ```rust,ignore
//! pub struct SinCosLut<
//!     const PHASE_W: usize,      // accumulator width arriving
//!     const AMP_W: usize,        // bits per component out
//!     const MAX_LUT_BITS: usize, // resource budget
//! >;
//!
//! // Largest addressing width the budget affords, given quarter-wave
//! // symmetry.  A const fn, evaluated by rustc.
//! const fn addr_bits_for(budget_bits: usize, amp_w: usize) -> usize { /* ... */ }
//! ```
//!
//! and then the budget and the requirement meet in a build-time check:
//!
//! ```rust,ignore
//! const _: () = assert!(
//!     sfdr_estimate_db(addr_bits_for(MAX_LUT_BITS, AMP_W)) >= TARGET_SFDR_DB,
//!     "LUT budget is too small for the SFDR target"
//! );
//! ```
//!
//! Shrink the budget below what the target needs and the build fails
//! with the reason, rather than the instrument quietly acquiring spurs.
//!
//! ## Measured: the sweep result
//!
//! Run at 125 MHz, 1 MHz analysis band, 48-bit accumulator, 16-bit
//! amplitude, ~540 adversarially-chosen tuning words per configuration
//! (see [`model::adversarial_words`]):
//!
//! | `P` | formula | **measured worst** | median | entries | table |
//! |---|---|---|---|---|---|
//! | 10 | 56.3 | **60.0** | 92.0 | 256 | 4 Kbit |
//! | 11 | 62.3 | 66.0 | 97.9 | 512 | 8 Kbit |
//! | 12 | 68.3 | 71.9 | 104.7 | 1024 | 16 Kbit |
//! | 13 | 74.3 | **78.1** | 110.1 | 2048 | 32 Kbit |
//!
//! **Recommendation (superseded): `P = 13`** — 78 dB worst case, 8 dB of
//! margin over a 70 dB target, one BRAM36, sin and cos from the single
//! table.
//!
//! This was the right answer *for a plain table*, and it is not what
//! shipped. Interpolation attacks the truncation error rather than
//! merely reducing it, so [`sin_cos_linear_interp`] beats this by 38 dB
//! (−116.1 against −78.1) on 9 Kbit of table against 32 Kbit. See
//! "Linear interpolation vs CORDIC" below for the measurement that
//! overturned it.
//!
//! ### The band restriction buys nothing at worst case
//!
//! An earlier, naive sweep suggested restricting analysis to 1 MHz
//! bought ~25 dB over the full-Nyquist formula, which would have
//! justified a 256-entry table. **That was sampling error.** It tested
//! 24 tuning words, all in the benign regime. Against adversarial words
//! the worst case sits within 3.6–3.8 dB of `6.02·P − 3.92` at every
//! width — a consistent offset, not scatter, which is itself good
//! evidence the model and analysis are correct.
//!
//! ### Why adversarial selection is mandatory
//!
//! Truncation error has period `2^B / gcd(low, 2^B)`, where `low` is
//! the truncated remainder. Short periods concentrate the error into
//! few strong spurs. Every worst-case word found has `low` at a **pure
//! power of two** or its complement — and those are not exotic
//! frequencies, they are what you get when a human types a round
//! number. Uniform random sampling lands in the benign regime almost
//! every time and reports numbers 20–30 dB too optimistic.
//!
//! ### The distribution, not just the minimum
//!
//! At `P = 13` the median word gives 110 dB and the worst gives 78. The
//! design must be sized for the worst, because an experiment can select
//! one — but the spread is worth knowing, because it explains why
//! bench measurement at a few frequencies can look far better than the
//! specification and still be consistent with it.
//!
//! ## Linear interpolation vs CORDIC: measured, and not close
//!
//! "Hybrid" covers several circuits. The two candidates here share a
//! coarse table and differ only in the fine rotator. Measured on the
//! exact analysis, same word, same band:
//!
//! | coarse/fine | rotator | worst dBc | cost | latency |
//! |---|---|---|---|---|
//! | 10/12 | **linear (Taylor)** | **−116.1** | **2 mult** | **1 cyc** |
//! | 10/12 | CORDIC 4 stages | −77.4 | 8 add | 4 cyc |
//! | 10/12 | CORDIC 8 stages | −103.6 | 16 add | 8 cyc |
//! | 8/10 | linear | −91.3 | 2 mult | 1 cyc |
//! | 8/10 | CORDIC 8 stages | −91.5 | 16 add | 8 cyc |
//!
//! Linear interpolation is exact to **second order** in the remainder;
//! CORDIC converges at roughly one bit per stage. Matching a
//! second-order method by linear convergence takes many stages, and
//! each costs a cycle.
//!
//! **Decision: linear interpolation, no parameterised rotator.** A
//! pluggable fine stage was considered and rejected — CORDIC is worse
//! on spurs, worse on latency, and cheaper only where DSP slices are
//! scarce, which on this device they are not. Revisit only if the
//! multipliers are needed elsewhere.
//!
//! ## Why the sweep was required

//!
//! `6.02·P − 3.92` is a worst-case, **full-Nyquist** figure. The
//! requirement here is SFDR over a 1 MHz band, which is a different
//! question:
//!
//! - Spurs outside the band are removed by the decimation filter that
//!   follows, so in-band SFDR can be *better* than the formula.
//! - But truncation spurs move with the tuning word, so for particular
//!   words a spur can land inside the band.
//!
//! Only a bit-accurate sweep across the tuning-word space answers that.
//! The const assertion above is a **screen** — it catches an obviously
//! undersized table — not a validation. Treating it as proof would be
//! the more dangerous mistake, because it looks like rigour.
pub mod composite;
pub mod config;
pub mod frequency_composer;
pub mod latency;
pub mod model;
pub mod modulation;
pub mod phase_accumulator;
pub mod phase_composer;
pub mod ramp;
pub mod sin_cos_linear_interp;
