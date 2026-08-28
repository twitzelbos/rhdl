//! [`crate::cic_pruned!`] — declare a CIC whose datapath tapers.
//!
//! Here is the schematic symbol of a generated widget. It is the same
//! symbol [`super::decimator::CicDecimate`] presents — same `In`, same
//! `Out`, same restart semantics — because pruning is an arithmetic
//! decision and not an interface one.
#![doc = badascii_doc::badascii_formal!(r"
      +-+cic_pruned!+------------+
      |                          |
+---->+ sample                   |
      |   Option<SignedBits<WI>> |
      |                   sample |
      |  Option<SignedBits<W2N>> +----->
+---->+ downstream_ready         |
      |                  overrun |
+---->+ restart                  +----->
      |                          |
      +--------------------------+
")]
//!
//! `W2N` is `prune::stage_width(2N, ..)`, which is narrower than the
//! full accumulator — the output of a pruned CIC carries fewer bits,
//! weighing more each. See "Reading the output" below.
//!
//! [`super::decimator::CicDecimate`] runs every stage at the worst-case
//! width. That is correct and wasteful: Hogenauer's §V analysis (see
//! [`super::prune`]) shows later stages may discard low-order bits,
//! because less of the remaining filter is left to amplify their
//! truncation noise. At `W_IN = 18, N = 5, R = 1024` the uniform
//! version spends 68 bits in each of ten registers; the tapered one
//! spends 517 bits in total rather than 680, and the same proportion
//! comes off every adder.
//!
//! Expressing that taper needs a **different width per stage**, which a
//! homogeneous `[SignedBits<W>; N]` cannot hold and const generics
//! cannot compute. So the widget is generated: the macro substitutes
//! literals into [`super::prune::stage_width`], a `const fn`, so each
//! field gets its own width with no `generic_const_exprs`.
//!
//! ```rust
//! use rhdl::prelude::*;
//! use rhdl_fpga::cic_pruned;
//! use rhdl_fpga::core::dff;
//!
//! cic_pruned!(SmallCic, w_in = 8, n = 2, r = 4, m = 1, b_out = 4);
//! let uut = SmallCic::default();
//! ```
//!
//! The two `use` lines are not optional: the expansion names `dff`,
//! and the derives and kernel come from the prelude. This is the same
//! preamble every hand-written widget file carries.
//!
//! # Shape of the generated widget
//!
//! Three registers, whatever `N` is:
//!
//! - `stages` — one `DFF` holding a generated `CicStages` struct with
//!   every integrator and comb delay line, each at its own width. This
//!   is the [CLAUDE.md §3.1] bundling pattern, and here it is not just
//!   tidiness: it is what makes the widget's field count independent of
//!   `N`, so the derived `Q`/`D` tuples never approach their
//!   twelve-element ceiling however deep the cascade.
//! - `phase` — the decimation counter.
//! - `out` — the registered output.
//!
//! [CLAUDE.md §3.1]: https://github.com/twitzelbos/rhdl
//!
//! It is a drop-in for the uniform widget: same
//! [`super::decimator::In`] and [`super::decimator::Out`], same restart
//! semantics, same pipelined integrator cascade.
//!
//! # What it guarantees
//!
//! - Every stage is exactly [`super::prune::stage_width`] wide. Not
//!   asserted — *substituted*, so the declaration cannot drift from the
//!   analysis.
//! - The transfer between stages discards exactly the pruned bits: an
//!   arithmetic right shift by the width difference, via
//!   `dsp::narrow`. A non-monotonic schedule fails at const
//!   evaluation rather than silently zero-extending.
//! - One adder between registers in the integrator cascade regardless
//!   of depth, as in the uniform widget.
//!
//! # What it does not guarantee
//!
//! That the resulting noise is acceptable for *your* signal. `b_out` is
//! a budget you choose, and whether it is the right budget is a
//! question about the measurement, not about the filter. Hogenauer's
//! schedule spends it evenly — it makes every stage contribute roughly
//! equal error — but it does not tell you the total is small enough.
//! Check that behaviourally, against the unpruned widget.
//!
//! # Invoke it at most once per module
//!
//! The generated widget uses `#[rhdl(dq_no_prefix)]`, so its `Q` and
//! `D` types land at module scope, as do the generated `CicStages` and
//! the kernel. Two invocations in one module collide. This is the same
//! constraint CLAUDE.md §7 states as one-widget-per-file, and the fix
//! is the same: give each one its own module.

//! # Internals
//!
//! The taper, for `W_IN = 18, N = 4, R = 64, M = 1, b_out = 20`:
#![doc = badascii_doc::badascii!(r"
  42        39        34        29        27        26        25        24
+----+    +----+    +----+    +----+    +----+    +----+    +----+    +----+
| I1 |>>3 | I2 |>>5 | I3 |>>5 | I4 |>>2 | C1 |>>1 | C2 |>>1 | C3 |>>1 | C4 |
+----+    +----+    +----+    +----+    +----+    +----+    +----+    +----+
   ^                                       ^
   |  full accumulator width = 42          |  decimate by R here
")]
//!
//! Each `>>k` is the low-order bits the next stage does not keep. In
//! the emitted Verilog they cost nothing: a constant shift feeding a
//! narrowing assignment folds into a bit select, so the saving is
//! register bits and adder width with no shifter logic added.
//!
//! # Reading the output
//!
//! A pruned register does not hold the value — it holds the value
//! divided by `2^(full - width)`. So the output is the full-precision
//! result right-shifted by `full - W2N`, and the DC gain is still
//! [`super::dc_gain`] referred to that shifted scale.
//!
//! This is the one place the substitution can bite a caller: swapping a
//! `CicDecimate` for a generated widget of the same `(N, R, M)` changes
//! the output's LSB weight, not just its width.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../../examples/cic_pruned.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../../doc/cic_pruned.md")]

/// Declare a CIC decimator with a Hogenauer-pruned datapath.
///
/// ```rust
/// use rhdl::prelude::*;
/// use rhdl_fpga::cic_pruned;
/// use rhdl_fpga::core::dff;
///
/// cic_pruned!(SmallCic, w_in = 8, n = 2, r = 4, m = 1, b_out = 4);
/// ```
///
/// See the [module docs](self) for the shape of what this generates and
/// the one-per-module rule.
///
/// `n` must be a literal in `2..=8`: each arm has to name its own
/// fields, because `macro_rules!` cannot synthesise identifiers, so
/// there is one arm per stage count. Eight is where the list stops
/// being worth writing, not a structural limit — the bundled-state
/// shape means an arm for a deeper cascade would work fine.
#[macro_export]
macro_rules! cic_pruned {
    ($name:ident, w_in = $wi:tt, n = 2, r = $r:tt, m = $m:tt, b_out = $bo:tt) => {
        $crate::cic_pruned_impl!(
            $name,
            $wi,
            2,
            $r,
            $m,
            $bo,
            i0,
            [(i1, 2, i0, 1)],
            i1,
            c0,
            o0,
            [(c1, o1, o0, 4, 3)],
            o1
        );
    };
    ($name:ident, w_in = $wi:tt, n = 3, r = $r:tt, m = $m:tt, b_out = $bo:tt) => {
        $crate::cic_pruned_impl!(
            $name,
            $wi,
            3,
            $r,
            $m,
            $bo,
            i0,
            [(i1, 2, i0, 1), (i2, 3, i1, 2)],
            i2,
            c0,
            o0,
            [(c1, o1, o0, 5, 4), (c2, o2, o1, 6, 5)],
            o2
        );
    };
    ($name:ident, w_in = $wi:tt, n = 4, r = $r:tt, m = $m:tt, b_out = $bo:tt) => {
        $crate::cic_pruned_impl!(
            $name,
            $wi,
            4,
            $r,
            $m,
            $bo,
            i0,
            [(i1, 2, i0, 1), (i2, 3, i1, 2), (i3, 4, i2, 3)],
            i3,
            c0,
            o0,
            [(c1, o1, o0, 6, 5), (c2, o2, o1, 7, 6), (c3, o3, o2, 8, 7)],
            o3
        );
    };
    ($name:ident, w_in = $wi:tt, n = 5, r = $r:tt, m = $m:tt, b_out = $bo:tt) => {
        $crate::cic_pruned_impl!(
            $name,
            $wi,
            5,
            $r,
            $m,
            $bo,
            i0,
            [
                (i1, 2, i0, 1),
                (i2, 3, i1, 2),
                (i3, 4, i2, 3),
                (i4, 5, i3, 4)
            ],
            i4,
            c0,
            o0,
            [
                (c1, o1, o0, 7, 6),
                (c2, o2, o1, 8, 7),
                (c3, o3, o2, 9, 8),
                (c4, o4, o3, 10, 9)
            ],
            o4
        );
    };
    ($name:ident, w_in = $wi:tt, n = 6, r = $r:tt, m = $m:tt, b_out = $bo:tt) => {
        $crate::cic_pruned_impl!(
            $name,
            $wi,
            6,
            $r,
            $m,
            $bo,
            i0,
            [
                (i1, 2, i0, 1),
                (i2, 3, i1, 2),
                (i3, 4, i2, 3),
                (i4, 5, i3, 4),
                (i5, 6, i4, 5)
            ],
            i5,
            c0,
            o0,
            [
                (c1, o1, o0, 8, 7),
                (c2, o2, o1, 9, 8),
                (c3, o3, o2, 10, 9),
                (c4, o4, o3, 11, 10),
                (c5, o5, o4, 12, 11)
            ],
            o5
        );
    };
    ($name:ident, w_in = $wi:tt, n = 7, r = $r:tt, m = $m:tt, b_out = $bo:tt) => {
        $crate::cic_pruned_impl!(
            $name,
            $wi,
            7,
            $r,
            $m,
            $bo,
            i0,
            [
                (i1, 2, i0, 1),
                (i2, 3, i1, 2),
                (i3, 4, i2, 3),
                (i4, 5, i3, 4),
                (i5, 6, i4, 5),
                (i6, 7, i5, 6)
            ],
            i6,
            c0,
            o0,
            [
                (c1, o1, o0, 9, 8),
                (c2, o2, o1, 10, 9),
                (c3, o3, o2, 11, 10),
                (c4, o4, o3, 12, 11),
                (c5, o5, o4, 13, 12),
                (c6, o6, o5, 14, 13)
            ],
            o6
        );
    };
    ($name:ident, w_in = $wi:tt, n = 8, r = $r:tt, m = $m:tt, b_out = $bo:tt) => {
        $crate::cic_pruned_impl!(
            $name,
            $wi,
            8,
            $r,
            $m,
            $bo,
            i0,
            [
                (i1, 2, i0, 1),
                (i2, 3, i1, 2),
                (i3, 4, i2, 3),
                (i4, 5, i3, 4),
                (i5, 6, i4, 5),
                (i6, 7, i5, 6),
                (i7, 8, i6, 7)
            ],
            i7,
            c0,
            o0,
            [
                (c1, o1, o0, 10, 9),
                (c2, o2, o1, 11, 10),
                (c3, o3, o2, 12, 11),
                (c4, o4, o3, 13, 12),
                (c5, o5, o4, 14, 13),
                (c6, o6, o5, 15, 14),
                (c7, o7, o6, 16, 15)
            ],
            o7
        );
    };
}

/// Body of [`cic_pruned!`]. Not a stable surface; the arm shape is an
/// implementation detail and may change.
///
/// Each repeated stage carries `(field, j, previous_field, previous_j)`
/// because `macro_rules!` repetition cannot look at the preceding
/// element, and the inter-stage transfer needs both widths.
#[doc(hidden)]
#[macro_export]
macro_rules! cic_pruned_impl {
    (
        $name:ident, $wi:tt, $n:tt, $r:tt, $m:tt, $bo:tt,
        $i0:ident, [$(($ik:ident, $ikj:tt, $ikprev:ident, $ikpj:tt)),* $(,)?], $ilast:ident,
        $c0:ident, $o0:ident,
        [$(($ck:ident, $ok:ident, $okprev:ident, $ckj:tt, $ckpj:tt)),* $(,)?],
        $olast:ident
    ) => {
        /// Per-stage state of the generated CIC, bundled into one
        /// register so the widget's field count does not grow with the
        /// stage count (CLAUDE.md §3.1).
        ///
        /// Each field is at its own Hogenauer-pruned width, which is
        /// the whole point — an array could not express this.
        #[derive(Clone, Copy, PartialEq, Debug, Digital, Default)]
        #[allow(missing_docs)]
        pub struct CicStages {
            pub $i0: SignedBits<{ $crate::dsp::cic::prune::stage_width(1, $wi, $n, $r, $m, $bo) }>,
            $(pub $ik: SignedBits<{ $crate::dsp::cic::prune::stage_width($ikj, $wi, $n, $r, $m, $bo) }>,)*
            pub $c0: [SignedBits<{ $crate::dsp::cic::prune::stage_width($n + 1, $wi, $n, $r, $m, $bo) }>; $m],
            $(pub $ck: [SignedBits<{ $crate::dsp::cic::prune::stage_width($ckj, $wi, $n, $r, $m, $bo) }>; $m],)*
            // Each comb stage's registered output: what makes the comb
            // cascade one subtractor deep instead of `N`.
            pub $o0: SignedBits<{ $crate::dsp::cic::prune::stage_width($n + 1, $wi, $n, $r, $m, $bo) }>,
            $(pub $ok: SignedBits<{ $crate::dsp::cic::prune::stage_width($ckj, $wi, $n, $r, $m, $bo) }>,)*
        }

        #[doc = concat!("A ", stringify!($n), "-stage CIC decimator with a Hogenauer-pruned datapath.")]
        ///
        /// Generated by [`cic_pruned!`](crate::cic_pruned); see that
        /// macro's module docs for the shape and the guarantees.
        #[derive(Clone, Debug, Synchronous, SynchronousDQ)]
        #[rhdl(dq_no_prefix)]
        pub struct $name {
            /// Integrators and comb delay lines, each at its own width.
            stages: dff::DFF<CicStages>,
            /// Counts input samples toward the next output.
            phase: dff::DFF<Bits<{ $crate::dsp::cic::counter_width($r) }>>,
            /// The decimated result, registered.
            out: dff::DFF<Option<SignedBits<{ $crate::dsp::cic::prune::stage_width(2 * $n, $wi, $n, $r, $m, $bo) }>>>,
        }

        impl Default for $name {
            fn default() -> Self {
                assert!($r >= 2, "a decimation factor below two is not a decimator");
                assert!($m >= 1, "the differential delay must be at least one");
                Self {
                    stages: dff::DFF::new(CicStages::default()),
                    phase: dff::DFF::new(bits(0)),
                    out: dff::DFF::new(None),
                }
            }
        }

        impl SynchronousIO for $name {
            type I = $crate::dsp::cic::decimator::In<$wi>;
            type O = $crate::dsp::cic::decimator::Out<
                { $crate::dsp::cic::prune::stage_width(2 * $n, $wi, $n, $r, $m, $bo) },
            >;
            type Kernel = cic_pruned_kernel;
        }

        #[kernel]
        #[doc(hidden)]
        #[allow(clippy::type_complexity)]
        pub fn cic_pruned_kernel(
            cr: ClockReset,
            i: $crate::dsp::cic::decimator::In<$wi>,
            q: Q,
        ) -> (
            $crate::dsp::cic::decimator::Out<
                { $crate::dsp::cic::prune::stage_width(2 * $n, $wi, $n, $r, $m, $bo) },
            >,
            D,
        ) {
            let mut d = D::dont_care();

            // Hold by default: an idle cycle must not advance the sums.
            d.stages = q.stages;
            d.phase = q.phase;
            d.out = None;

            let mut have = false;
            let mut x =
                signed::<{ $crate::dsp::cic::prune::stage_width(1, $wi, $n, $r, $m, $bo) }>(0);
            if let Some(s) = i.sample {
                have = true;
                // Two steps, and both are load bearing.
                //
                // `sign_extend`, not `resize`, to reach full width: `s`
                // is unwrapped from an `Option` and `resize` there
                // zero-extends in the emitted Verilog while the Rust
                // simulator sign-extends. See `crate::dsp::sign_extend`.
                //
                // Then `narrow` to stage one's width, which is *not* a
                // formality. A pruned register does not hold the value,
                // it holds the value divided by two to the power of its
                // discarded bits; stage one's LSB weight is
                // `2^(full - W_1)` and the input arrives at weight one.
                // Sign-extending straight to `W_1` would inject the
                // sample at the wrong scale by a factor of `2^(full -
                // W_1)`. When stage one is unpruned the two widths are
                // equal and this shift is nothing, which is why a
                // schedule that happens not to prune the first stage
                // hides the error completely.
                let full = $crate::dsp::sign_extend::<
                    $wi,
                    { $crate::dsp::cic::accumulator_width($wi, $n, $r, $m) },
                >(s);
                x = $crate::dsp::narrow::<
                    { $crate::dsp::cic::accumulator_width($wi, $n, $r, $m) },
                    { $crate::dsp::cic::prune::stage_width(1, $wi, $n, $r, $m, $bo) },
                >(full);
            }

            if have {
                // A restart clears every stage at once. The comb delay
                // lines belong to the old window as much as the
                // integrators do -- see `decimator::In::restart`.
                let pc = if i.restart {
                    CicStages::default()
                } else {
                    q.stages
                };
                let mut st = pc;

                // ---- integrator cascade, at the input rate ----
                //
                // Pipelined: each stage reads the previous stage's
                // REGISTERED value. One adder between registers however
                // deep the cascade, which matters because this section
                // runs at the full converter rate and sets fmax.
                //
                // `narrow` is where the pruning happens: stage k+1 is
                // narrower than stage k, and the bits it drops are the
                // low ones.
                st.$i0 = pc.$i0 + x;
                $(
                    st.$ik = pc.$ik
                        + $crate::dsp::narrow::<
                            { $crate::dsp::cic::prune::stage_width($ikpj, $wi, $n, $r, $m, $bo) },
                            { $crate::dsp::cic::prune::stage_width($ikj, $wi, $n, $r, $m, $bo) },
                        >(pc.$ikprev);
                )*
                d.stages = st;
                let carry = st.$ilast;

                // ---- decimation gate ----
                let phase_now = if i.restart {
                    bits::<{ $crate::dsp::cic::counter_width($r) }>(0)
                } else {
                    q.phase
                };
                let last = phase_now
                    == bits::<{ $crate::dsp::cic::counter_width($r) }>(($r - 1) as u128);
                d.phase = if last {
                    bits::<{ $crate::dsp::cic::counter_width($r) }>(0)
                } else {
                    phase_now + bits::<{ $crate::dsp::cic::counter_width($r) }>(1)
                };

                if last {
                    // ---- comb cascade, once per R input samples ----
                    //
                    // *** Pipelined: each stage reads the previous
                    // stage's REGISTERED output. *** One subtractor
                    // between registers however deep the cascade;
                    // combinational depth does not care that this
                    // section is clocked one cycle in `R`. See
                    // `cic::decimator`.
                    let mut cs = st;
                    let v = $crate::dsp::narrow::<
                        { $crate::dsp::cic::prune::stage_width($n, $wi, $n, $r, $m, $bo) },
                        { $crate::dsp::cic::prune::stage_width($n + 1, $wi, $n, $r, $m, $bo) },
                    >(carry);
                    let mut line = pc.$c0;
                    for j in 0..$m {
                        // Shift toward the tail, newest at index 0.
                        let idx = $m - 1 - j;
                        line[idx] = if idx == 0 { v } else { pc.$c0[idx - 1] };
                    }
                    cs.$c0 = line;
                    cs.$o0 = v - pc.$c0[$m - 1];
                    $(
                        let vin = $crate::dsp::narrow::<
                            { $crate::dsp::cic::prune::stage_width($ckpj, $wi, $n, $r, $m, $bo) },
                            { $crate::dsp::cic::prune::stage_width($ckj, $wi, $n, $r, $m, $bo) },
                        >(pc.$okprev);
                        let mut line = pc.$ck;
                        for j in 0..$m {
                            let idx = $m - 1 - j;
                            line[idx] = if idx == 0 { vin } else { pc.$ck[idx - 1] };
                        }
                        cs.$ck = line;
                        cs.$ok = vin - pc.$ck[$m - 1];
                    )*
                    d.stages = cs;
                    d.out = Some(cs.$olast);
                }
            }

            let mut o = $crate::dsp::cic::decimator::Out::<
                { $crate::dsp::cic::prune::stage_width(2 * $n, $wi, $n, $r, $m, $bo) },
            > {
                sample: q.out,
                // A pruned datapath truncates, it does not clip: every
                // stage is wide enough for its own arithmetic, and the
                // bits it drops are the low ones.
                saturated: false,
                overrun: !i.downstream_ready,
            };

            if cr.reset.any() {
                d.stages = CicStages::default();
                d.phase = bits::<{ $crate::dsp::cic::counter_width($r) }>(0);
                d.out = None;
                o.overrun = false;
            }
            (o, d)
        }
    };
}
