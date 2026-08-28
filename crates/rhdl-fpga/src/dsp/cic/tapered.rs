#![warn(missing_docs)]
//! `cic_interp_tapered!` — a CIC interpolator whose stages are each only
//! as wide as they need to be.
//!
#![doc = badascii_doc::badascii_formal!(r"
      +-+cic_interp_tapered!+----+
      |                          |
+---->+ sample                   |
      |   Option<SignedBits<WI>> |
+---->+ rate              sample |
      |   Bits<CW>   SignedBits  |
+---->+ restart          <W2N>   +----->
      |                          |
+---->+ downstream_ready         |
      |            input_ready   +----->
      |                starved   +----->
      |                overrun   +----->
      +--------------------------+
   same In/Out as cic::interpolator, so it drops into any slot
")]
#![doc = badascii_doc::badascii!(r"
   17        18        19        19        24        30
 +----+    +----+    +----+    +----+    +----+    +----+
 | C1 |    | C2 |    | C3 |    | I1 |    | I2 |    | I3 |
 +----+    +----+    +----+    +----+    +----+    +----+
   combs, at the input rate   |  integrators, every output cycle
                             x R
   each stage one adder deep; combs also carry an output register
   w_in = 16, N = 3, R = 125: 181 register bits, against 270 uniform
")]
//!
//! # This taper is **lossless**, and that is the whole difference from
//! [`cic_pruned!`](crate::cic_pruned)
//!
//! A pruned *decimator* discards low-order bits. Each stage holds the
//! value divided by two to the power of the bits it dropped, so every
//! inter-stage transfer is an arithmetic shift, and the schedule trades
//! noise for area — `cic_pruned!`'s correctness argument is an error
//! budget.
//!
//! A tapered *interpolator* discards nothing. Each stage's output is a
//! finite filter applied to a bounded input, so it has an exact bound of
//! its own ([`super::interp`] derives them); size the stage to that
//! bound and it holds its value exactly, at LSB weight one, like every
//! other stage. There is no shift anywhere in the generated datapath,
//! no noise, and no budget.
//!
//! Which makes the correctness argument far stronger: a tapered
//! interpolator is **bit-identical** to a uniform-width one, and
//! `the_taper_is_bit_identical_to_the_uniform_widget` asserts exactly
//! that rather than measuring an error against a tolerance.
//!
//! # Both cascades are one adder deep
//!
//! Same as [`super::interpolator`], and for the same reason:
//! combinational depth does not care that the comb section is clocked
//! one cycle in `R`. Each stage reads the previous stage's *registered*
//! output, so the depth between registers is one subtractor or one adder
//! however deep the cascade.
//!
//! The comb section pays for that in registers — its delay lines hold
//! each stage's *inputs*, so pipelining the chain needs an output
//! register per stage as well, `N` of them. At the worked configuration
//! that is 54 of the tapered 181 bits.
//! [`super::interp::uniform_state_bits`] counts them.
//!
//! # Every transfer is a widening
//!
//! The exact bounds are not monotonic — the last comb can be wider than
//! the first integrator, because zero-stuffing divides by `R` faster
//! than one integrator re-grows by `R·M`. The generated widths are
//! therefore [`super::interp::implemented_stage_width`], the running
//! maximum, which costs **one bit** at every realistic configuration and
//! removes the narrowing transfer entirely.
//!
//! So the only inter-stage operation is [`crate::dsp::sign_extend`], and
//! it is the same operation at every boundary including the ones where
//! the widths happen to be equal. `cic_pruned!`'s module docs record a
//! bug where a schedule that happened not to prune the first stage hid a
//! scaling error completely; a datapath with one transfer direction and
//! no scaling has nowhere for that to hide.
//!
//! # Shape
//!
//! ```ignore
//! cic_interp_tapered!(TxInterp, w_in = 16, n = 3, r_max = 125, m = 1);
//! ```
//!
//! Generates a `TxInterp` presenting
//! [`super::interpolator::In`]/[`super::interpolator::Out`], so it drops
//! into [`super::interp_stream::StreamInterpolator`],
//! [`crate::dsp::duc::EnvelopeUpsampler`] and both up-converters without
//! any of them knowing. `n` must be a literal 2 to 5.
//!
//! Per-stage state lives in one bundled `Digital` struct rather than in
//! sibling `DFF` fields, which is the pattern CLAUDE.md §3.1 describes:
//! each field carries its own width, which an array cannot express.
//!
//! # The rate is still an input
//!
//! `r_max` sizes the widths and the counter; [`super::interpolator::In::rate`]
//! chooses the rate at run time up to it, exactly as in the uniform
//! widget. Sizing for `R_MAX` covers every smaller rate because the
//! bounds are monotonic in `R`.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_interp_tapered.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_interp_tapered.md")]

/// Generate a width-tapered CIC interpolator.
///
/// See the module docs for the shape, the guarantees, and why the taper
/// is lossless where [`cic_pruned!`](crate::cic_pruned)'s is not.
#[macro_export]
macro_rules! cic_interp_tapered {
    ($name:ident, w_in = $wi:tt, n = 2, r_max = $r:tt, m = $m:tt) => {
        $crate::cic_interp_tapered_impl!(
            $name,
            $wi,
            2,
            $r,
            $m,
            c0,
            o0,
            [(c1, o1, o0, 2, 1)],
            o1,
            i0,
            [(i1, 4, i0, 3)],
            i1,
            4
        );
    };
    ($name:ident, w_in = $wi:tt, n = 3, r_max = $r:tt, m = $m:tt) => {
        $crate::cic_interp_tapered_impl!(
            $name,
            $wi,
            3,
            $r,
            $m,
            c0,
            o0,
            [(c1, o1, o0, 2, 1), (c2, o2, o1, 3, 2)],
            o2,
            i0,
            [(i1, 5, i0, 4), (i2, 6, i1, 5)],
            i2,
            6
        );
    };
    ($name:ident, w_in = $wi:tt, n = 4, r_max = $r:tt, m = $m:tt) => {
        $crate::cic_interp_tapered_impl!(
            $name,
            $wi,
            4,
            $r,
            $m,
            c0,
            o0,
            [(c1, o1, o0, 2, 1), (c2, o2, o1, 3, 2), (c3, o3, o2, 4, 3)],
            o3,
            i0,
            [(i1, 6, i0, 5), (i2, 7, i1, 6), (i3, 8, i2, 7)],
            i3,
            8
        );
    };
    ($name:ident, w_in = $wi:tt, n = 5, r_max = $r:tt, m = $m:tt) => {
        $crate::cic_interp_tapered_impl!(
            $name,
            $wi,
            5,
            $r,
            $m,
            c0,
            o0,
            [
                (c1, o1, o0, 2, 1),
                (c2, o2, o1, 3, 2),
                (c3, o3, o2, 4, 3),
                (c4, o4, o3, 5, 4)
            ],
            o4,
            i0,
            [
                (i1, 7, i0, 6),
                (i2, 8, i1, 7),
                (i3, 9, i2, 8),
                (i4, 10, i3, 9)
            ],
            i4,
            10
        );
    };
}

/// The implementation [`cic_interp_tapered!`] expands into.
///
/// The first comb and the first integrator are passed separately because
/// their inputs come from outside the cascade — the sample and the comb
/// section's output. The rest arrive as `(field, j, previous_j)` for the
/// combs, which chain through a local, and `(field, j, previous_field,
/// previous_j)` for the integrators, which read the previous stage's
/// *register*. Stage indices are one-based and ordered the way the
/// signal travels, matching [`super::interp`].
#[macro_export]
#[doc(hidden)]
macro_rules! cic_interp_tapered_impl {
    (
        $name:ident, $wi:tt, $n:tt, $r:tt, $m:tt,
        $c0:ident, $o0:ident,
        [$(($ck:ident, $ok:ident, $okprev:ident, $ckj:tt, $ckpj:tt)),* $(,)?],
        $olast:ident,
        $i0:ident, [$(($ik:ident, $ikj:tt, $ikprev:ident, $ikpj:tt)),* $(,)?],
        $ilast:ident, $ilastj:tt
    ) => {
        /// Per-stage state, bundled into one register so the widget's
        /// field count does not grow with the stage count (CLAUDE.md
        /// §3.1).
        ///
        /// Each field is at its own tapered width, which is the whole
        /// point — an array could not express this.
        #[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
        #[allow(missing_docs)]
        pub struct InterpStages {
            pub $c0: [SignedBits<{
                $crate::dsp::cic::interp::implemented_stage_width(1, $wi, $n, $r, $m)
            }>; $m],
            $(pub $ck: [SignedBits<{
                $crate::dsp::cic::interp::implemented_stage_width($ckj, $wi, $n, $r, $m)
            }>; $m],)*
            // Each comb stage's registered output: what makes the comb
            // cascade one subtractor deep instead of `N`.
            pub $o0: SignedBits<{
                $crate::dsp::cic::interp::implemented_stage_width(1, $wi, $n, $r, $m)
            }>,
            $(pub $ok: SignedBits<{
                $crate::dsp::cic::interp::implemented_stage_width($ckj, $wi, $n, $r, $m)
            }>,)*
            pub $i0: SignedBits<{
                $crate::dsp::cic::interp::implemented_stage_width($n + 1, $wi, $n, $r, $m)
            }>,
            $(pub $ik: SignedBits<{
                $crate::dsp::cic::interp::implemented_stage_width($ikj, $wi, $n, $r, $m)
            }>,)*
        }

        #[doc = concat!("A ", stringify!($n), "-stage CIC interpolator with a width-tapered datapath.")]
        ///
        /// Generated by
        /// [`cic_interp_tapered!`](crate::cic_interp_tapered); see that
        /// macro's module docs for the shape and the guarantees.
        #[derive(Clone, Debug, Synchronous, SynchronousDQ)]
        #[rhdl(dq_no_prefix)]
        pub struct $name {
            /// Comb delay lines and integrators, each at its own width.
            stages: dff::DFF<InterpStages>,
            /// Counts output cycles since the last input was taken.
            phase: dff::DFF<Bits<{ $crate::dsp::cic::interp::rate_width($r) }>>,
            /// The interpolated result, registered.
            out: dff::DFF<SignedBits<{
                $crate::dsp::cic::interp::implemented_stage_width($ilastj, $wi, $n, $r, $m)
            }>>,
            /// An input cycle found nothing on the input.
            starved: dff::DFF<bool>,
        }

        impl Default for $name {
            fn default() -> Self {
                assert!(
                    $r >= 2,
                    "an interpolation factor below two is not an interpolator"
                );
                assert!($m >= 1, "the differential delay must be at least one");
                Self {
                    stages: dff::DFF::new(InterpStages::default()),
                    phase: dff::DFF::new(bits(0)),
                    out: dff::DFF::new(SignedBits::default()),
                    starved: dff::DFF::new(false),
                }
            }
        }

        impl SynchronousIO for $name {
            type I = $crate::dsp::cic::interpolator::In<
                $wi,
                { $crate::dsp::cic::interp::rate_width($r) },
            >;
            type O = $crate::dsp::cic::interpolator::Out<{
                $crate::dsp::cic::interp::implemented_stage_width($ilastj, $wi, $n, $r, $m)
            }>;
            type Kernel = cic_interp_tapered_kernel;
        }

        #[kernel]
        #[doc(hidden)]
        #[allow(clippy::type_complexity)]
        pub fn cic_interp_tapered_kernel(
            cr: ClockReset,
            i: $crate::dsp::cic::interpolator::In<
                $wi,
                { $crate::dsp::cic::interp::rate_width($r) },
            >,
            q: Q,
        ) -> (
            $crate::dsp::cic::interpolator::Out<{
                $crate::dsp::cic::interp::implemented_stage_width($ilastj, $wi, $n, $r, $m)
            }>,
            D,
        ) {
            let mut d = D::dont_care();
            d.phase = q.phase;

            // ---- the phase counter, where the run-time rate lives ----
            let at_zero =
                q.phase == bits::<{ $crate::dsp::cic::interp::rate_width($r) }>(0);
            let take = at_zero || i.restart;

            let phase_now = if i.restart {
                bits::<{ $crate::dsp::cic::interp::rate_width($r) }>(0)
            } else {
                q.phase
            };
            // `+ 1 >= rate` rather than `== rate - 1`: `rate = 0` would
            // underflow, and lowering the rate mid-count must wrap now
            // rather than after a lap of the old rate.
            let wrap = (phase_now
                + bits::<{ $crate::dsp::cic::interp::rate_width($r) }>(1))
                >= i.rate;
            d.phase = if wrap {
                bits::<{ $crate::dsp::cic::interp::rate_width($r) }>(0)
            } else {
                phase_now + bits::<{ $crate::dsp::cic::interp::rate_width($r) }>(1)
            };

            // One prior state serves both cascades, and a restart zeroes
            // it -- which clears the comb lines and the integrators
            // together, as `cic::interpolator::In::restart` requires.
            let pc = if i.restart {
                InterpStages::default()
            } else {
                q.stages
            };
            let mut st = pc;

            // ---- the input, or zero ----
            //
            // `sign_extend` to stage one's width and *no shift*. This is
            // where a pruned decimator has to scale -- its stage one
            // holds the value divided by two to the power of its
            // discarded bits -- and where a tapered interpolator does
            // not: every stage here is at LSB weight one.
            let mut starved_now = false;
            let mut x = signed::<{
                $crate::dsp::cic::interp::implemented_stage_width(1, $wi, $n, $r, $m)
            }>(0);
            if take {
                match i.sample {
                    Some(s) => {
                        x = $crate::dsp::sign_extend::<$wi, {
                            $crate::dsp::cic::interp::implemented_stage_width(
                                1, $wi, $n, $r, $m
                            )
                        }>(s);
                    }
                    None => {
                        // Zero, not the previous sample: see
                        // `cic::interpolator`'s docs on the stuck DC
                        // offset that holding leaves behind.
                        starved_now = true;
                    }
                }
            }
            d.starved = starved_now;

            // ---- comb cascade, once per input ----
            //
            // *** Pipelined: each stage reads the previous stage's
            // REGISTERED output. *** One subtractor between registers
            // however deep the cascade. Combinational depth does not
            // care that this section is clocked one cycle in `R`; see
            // `cic::interpolator`.
            let mut feed = signed::<{
                $crate::dsp::cic::interp::implemented_stage_width($n, $wi, $n, $r, $m)
            }>(0);
            if take {
                // `y = x - x[-M]`, then shift this stage's delay line.
                let delayed = pc.$c0[$m - 1];
                let mut line = pc.$c0;
                for j in 0..$m {
                    // Shift toward the tail, newest at index 0.
                    let idx = $m - 1 - j;
                    line[idx] = if idx == 0 { x } else { pc.$c0[idx - 1] };
                }
                st.$c0 = line;
                st.$o0 = x - delayed;
                $(
                    let vin = $crate::dsp::sign_extend::<
                        {
                            $crate::dsp::cic::interp::implemented_stage_width(
                                $ckpj, $wi, $n, $r, $m
                            )
                        },
                        {
                            $crate::dsp::cic::interp::implemented_stage_width(
                                $ckj, $wi, $n, $r, $m
                            )
                        },
                    >(pc.$okprev);
                    let delayed = pc.$ck[$m - 1];
                    let mut line = pc.$ck;
                    for j in 0..$m {
                        let idx = $m - 1 - j;
                        line[idx] = if idx == 0 { vin } else { pc.$ck[idx - 1] };
                    }
                    st.$ck = line;
                    st.$ok = vin - delayed;
                )*
                // The *registered* last stage, so the integrator's adder
                // is one deep from a register rather than a subtractor
                // plus an adder.
                feed = pc.$olast;
            }

            // ---- integrator cascade, every output cycle ----
            //
            // Pipelined: each stage reads the previous stage's REGISTERED
            // output. One adder between registers however deep the
            // cascade, which matters because this section runs at the
            // converter rate and sets fmax.
            //
            // `feed` is zero on every cycle that did not take an input,
            // which *is* the zero-stuffing -- there is no separate
            // upsampler.
            st.$i0 = pc.$i0
                + $crate::dsp::sign_extend::<
                    {
                        $crate::dsp::cic::interp::implemented_stage_width(
                            $n, $wi, $n, $r, $m
                        )
                    },
                    {
                        $crate::dsp::cic::interp::implemented_stage_width(
                            $n + 1, $wi, $n, $r, $m
                        )
                    },
                >(feed);
            $(
                st.$ik = pc.$ik
                    + $crate::dsp::sign_extend::<
                        {
                            $crate::dsp::cic::interp::implemented_stage_width(
                                $ikpj, $wi, $n, $r, $m
                            )
                        },
                        {
                            $crate::dsp::cic::interp::implemented_stage_width(
                                $ikj, $wi, $n, $r, $m
                            )
                        },
                    >(pc.$ikprev);
            )*
            d.stages = st;
            d.out = st.$ilast;

            let mut o = $crate::dsp::cic::interpolator::Out::<{
                $crate::dsp::cic::interp::implemented_stage_width($ilastj, $wi, $n, $r, $m)
            }> {
                sample: q.out,
                input_ready: at_zero,
                starved: q.starved,
                overrun: !i.downstream_ready,
                // Exact at these widths: every stage is at or above its
                // own growth bound, so nothing can exceed its register.
                saturated: false,
            };

            if cr.reset.any() {
                d.stages = InterpStages::default();
                d.phase = bits::<{ $crate::dsp::cic::interp::rate_width($r) }>(0);
                d.out = signed::<{
                    $crate::dsp::cic::interp::implemented_stage_width(
                        $ilastj, $wi, $n, $r, $m
                    )
                }>(0);
                d.starved = false;
                o.sample = signed::<{
                    $crate::dsp::cic::interp::implemented_stage_width(
                        $ilastj, $wi, $n, $r, $m
                    )
                }>(0);
                o.input_ready = false;
                o.starved = false;
                o.overrun = false;
                o.saturated = false;
            }

            (o, d)
        }
    };
}
