//! `cic_chain!` — declare a decimation chain from its requirements.
//!
//! The macro runs [`rhdl_dsp_design::cic::chain::design`] **at
//! expansion time** and emits the widgets its answer describes. That is
//! the whole point: a CIC and its compensator take a dozen numbers
//! before you can instantiate one, none of which is a requirement, and
//! this is where the requirements become the numbers.
//!
//! # Why the design runs here and not in a `const fn`
//!
//! Because it cannot. Choosing a decimation split needs a
//! least-squares fit per candidate tap count, which needs floating
//! point, which is not available in a `const fn` on stable Rust. So the
//! computation happens in the compiler's own process, at macro
//! expansion, and its results are substituted as literals — which is
//! precisely what a const-generic widget parameter needs.
//!
//! # Why this crate can reach the designer
//!
//! `architecture.md` §2 forbids `rhdl-macro-core` from depending on
//! `rhdl-core`. The design mathematics therefore lives in
//! `rhdl-dsp-design`, an L0 leaf crate with no RHDL dependency at all,
//! which both this crate and `rhdl-fpga` may depend on. See §5 of that
//! document for the justification; this macro is the consumer that
//! motivated it.
//!
//! # It emits its own working, not just its answer
//!
//! Every derived number becomes a `pub const`, and the design report
//! becomes a doc comment on the generated module. A macro that silently
//! picked `N = 5` and a 51-bit accumulator would be doing something a
//! hardware engineer needs to audit. The convenience is in not having
//! to *compute* the numbers, not in not being allowed to see them.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use rhdl_dsp_design::cic::chain::{self, ChainSpec, Unmet};
use rhdl_dsp_design::cic::compensator::Method;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitFloat, LitInt, Token};

/// The parsed `cic_chain!` invocation.
struct ChainArgs {
    name: Ident,
    spec: ChainSpec,
}

/// One `key = value` pair. Values are numeric; a float literal is
/// accepted anywhere a number is wanted so `decimate = 488` and
/// `fs = 125e6` both read naturally.
fn number(input: ParseStream) -> syn::Result<f64> {
    if input.peek(LitFloat) {
        input.parse::<LitFloat>()?.base10_parse::<f64>()
    } else {
        Ok(input.parse::<LitInt>()?.base10_parse::<u64>()? as f64)
    }
}

impl Parse for ChainArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![,]>()?;

        // Start from the library default and override what is given, so
        // a caller need not restate the parts they do not care about.
        let mut spec = ChainSpec::default();
        let mut seen: Vec<String> = Vec::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let k = key.to_string();
            let span = key.span();

            match k.as_str() {
                "fs" => spec.fs_hz = number(input)?,
                "decimate" => spec.decimate = number(input)? as usize,
                "alias_free_bw" => spec.alias_free_bw_hz = number(input)?,
                "in_w" => spec.input_width = number(input)? as usize,
                "out_w" => spec.output_width = number(input)? as usize,
                "ripple_db" => spec.max_ripple_db = number(input)?,
                "alias_db" => spec.min_alias_rejection_db = number(input)?,
                "snr_db" => spec.min_snr_db = number(input)?,
                "coeff_w" => spec.coeff_width = number(input)? as usize,
                "max_stages" => spec.max_stages = number(input)? as usize,
                "max_taps" => spec.max_taps = number(input)? as usize,
                "stopband_edge" => spec.stopband_edge = number(input)?,
                "stopband_db" => spec.min_stopband_db = number(input)?,
                "max_chain_stages" => spec.max_chain_stages = number(input)? as usize,
                "cascade" => {
                    let v: Ident = input.parse()?;
                    // `cascade = no` pins the chain to one stage; `yes`
                    // restores the default budget. `max_chain_stages`
                    // is the finer control.
                    spec.max_chain_stages = match v.to_string().as_str() {
                        "true" | "yes" => 3,
                        "false" | "no" => 1,
                        other => {
                            return Err(syn::Error::new(
                                v.span(),
                                format!("expected true or false, found `{other}`"),
                            ));
                        }
                    };
                }
                "method" => {
                    let v: Ident = input.parse()?;
                    spec.method = match v.to_string().as_str() {
                        "least_squares" => Method::LeastSquares,
                        "remez" => Method::Remez,
                        other => {
                            return Err(syn::Error::new(
                                v.span(),
                                format!("expected `least_squares` or `remez`, found `{other}`"),
                            ));
                        }
                    };
                }
                other => {
                    return Err(syn::Error::new(
                        span,
                        format!(
                            "unknown parameter `{other}`. Expected one of: fs, decimate, \
                             alias_free_bw, in_w, out_w, ripple_db, alias_db, snr_db, \
                             coeff_w, max_stages, max_taps, stopband_edge, stopband_db, \
                             cascade, max_chain_stages, method"
                        ),
                    ));
                }
            }
            if seen.contains(&k) {
                return Err(syn::Error::new(span, format!("`{k}` given twice")));
            }
            seen.push(k);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(ChainArgs { name, spec })
    }
}

/// Turn an `Unmet` into a compile error the caller can act on.
///
/// The whole value of a designer that refuses is that it says *which*
/// requirement it could not meet and how close it came. That has to
/// survive into the diagnostic, or the macro is just a mysterious
/// failure.
fn unmet_message(u: &Unmet) -> String {
    match u {
        Unmet::AliasRejection { best_db, needed_db } => format!(
            "cannot meet the alias rejection requirement: asked for {needed_db:.1} dB, \
             the best any depth achieves is {best_db:.1} dB.\n\
             A CIC's nulls and its droop are the same expression, so depth cannot be spent \
             freely on rejection. Narrow `alias_free_bw`, or decimate less."
        ),
        Unmet::Incompatible {
            best_ripple_db,
            needed_ripple_db,
        } => format!(
            "rejection and flatness are jointly infeasible here: every depth that rejects \
             well enough droops more than the compensator can invert. Best ripple \
             {best_ripple_db:.4} dB against the {needed_ripple_db:.4} dB asked for.\n\
             The knob is `alias_free_bw`: a band further from the first null both rejects \
             better and droops less."
        ),
        Unmet::Stopband { best_db, needed_db } => format!(
            "cannot reach the stopband requirement: asked for {needed_db:.1} dB, best \
             {best_db:.1} dB.\nWiden `stopband_edge`, allow more `max_taps`, or try \
             `method = remez` — least squares minimises average error, and a stopband \
             requirement is about the worst case."
        ),
        Unmet::Snr { best_db, needed_db } => format!(
            "cannot reach the SNR requirement: asked for {needed_db:.1} dB, best \
             {best_db:.1} dB even with no pruning at all.\n\
             Unpruned is the quietest this shape can be, so `out_w` is the constraint, \
             not the pruning schedule."
        ),
        Unmet::BandwidthTooWide { passband } => format!(
            "the requested bandwidth is {:.3} of the output Nyquist, which no filter can \
             deliver.\n`2 * alias_free_bw * decimate` must be less than `fs`.",
            passband
        ),
        Unmet::PassbandTouchesNull => "the requested band reaches a CIC null, where the \
             compensator's required gain is unbounded. Narrow `alias_free_bw`."
            .to_string(),
        Unmet::Invalid { reason } => format!("the specification is not self-consistent: {reason}"),
    }
}

/// Expand a `cic_chain!` invocation.
pub fn cic_chain(input: TokenStream) -> syn::Result<TokenStream> {
    let args = syn::parse2::<ChainArgs>(input)?;
    let design = chain::design(args.spec).map_err(|u| {
        syn::Error::new(
            args.name.span(),
            format!(
                "cic_chain! cannot satisfy this specification.\n\n{}",
                unmet_message(&u)
            ),
        )
    })?;
    emit(&args.name, &design)
}

/// Generate the module for a design.
fn emit(name: &Ident, d: &chain::ChainDesign) -> syn::Result<TokenStream> {
    let module = format_ident!("{}", to_snake(&name.to_string()));
    let report = format!("{d}");
    let doc_header =
        " Decimation chain derived from requirements by [`cic_chain!`](crate::cic_chain).\n\n \
         Everything here was computed at compile time; the numbers are `pub const` so they \
         can be audited rather than trusted.\n\n # Design report\n\n ```text"
            .to_string();
    let doc_lines: Vec<String> = report.lines().map(|l| format!(" {l}")).collect();

    // --- the derived numbers, as inspectable constants ---
    // Unsuffixed throughout. The generated module is meant to be read
    // -- that is the point of emitting the working as constants -- and
    // `[usize ; 2usize]` is not reading material.
    let u = Literal::usize_unsuffixed;
    let decimate = u(d.spec.decimate);
    let split: Vec<Literal> = d.split().into_iter().map(u).collect();
    let n_split = u(d.split().len());
    let out_rate = d.output_rate_hz;
    let passband = d.passband;
    let ripple = d.achieved_ripple_db;
    // What the decimators do to the band unaided. The chain is usable
    // without a compensator -- the caller may apply the taps elsewhere,
    // or accept the droop -- so the figure they would be living with
    // has to be reported alongside the compensated one.
    let droop = {
        let shapes: Vec<rhdl_dsp_design::cic::compensator::CicShape> = d
            .cics
            .iter()
            .map(|c| rhdl_dsp_design::cic::compensator::CicShape {
                decimate: c.decimate,
                stages: c.stages,
                delay: c.delay,
            })
            .collect();
        let edge = rhdl_dsp_design::cic::response::passband_edge_out(d.passband);
        20.0 * rhdl_dsp_design::cic::compensator::cascade_magnitude(&shapes, edge).log10()
    };
    let alias = d.achieved_alias_db;
    let snr = d.achieved_snr_db;
    let reg_bits = u(d.register_bits);
    let taps: Vec<i64> = d.compensator.taps.clone();
    let n_taps = u(taps.len());
    let half = u(taps.len() / 2);
    let shift = u(d.compensator.shift as usize);
    let coeff_w = u(d.compensator.coeff_width);
    let in_w = u(d.spec.input_width);

    let consts = quote! {
        /// Total decimation, as asked for.
        pub const DECIMATE: usize = #decimate;
        /// How the decimation was split across stages.
        pub const SPLIT: [usize; #n_split] = [#(#split),*];
        /// Resulting output sample rate, in Hz.
        pub const OUTPUT_RATE_HZ: f64 = #out_rate;
        /// Fraction of the output Nyquist the alias-free band occupies.
        pub const PASSBAND: f64 = #passband;
        /// Passband droop of the decimation chain **on its own**, in dB.
        ///
        /// This is what `Chain` does to the band with no compensation
        /// applied. If you are not applying the taps, this is your
        /// amplitude error.
        pub const DROOP_DB: f64 = #droop;
        /// Passband ripple **if the compensator is applied**, in dB.
        ///
        /// Not a property of `Chain` alone — the taps have to be applied
        /// somewhere for this to be the figure you get. Where is up to
        /// you: `Compensated` puts them in hardware right behind the
        /// decimator, and `TAPS` is exported so they can be applied
        /// anywhere else instead, including outside the FPGA.
        pub const RIPPLE_DB: f64 = #ripple;
        /// Achieved alias rejection, in dB.
        pub const ALIAS_REJECTION_DB: f64 = #alias;
        /// Achieved output SNR, in dB.
        pub const SNR_DB: f64 = #snr;
        /// Register bits across the decimators, one real path.
        pub const REGISTER_BITS: usize = #reg_bits;
        /// Compensator taps, at `SHIFT` fractional bits.
        pub const TAPS: [i64; #n_taps] = [#(#taps),*];
        /// Fractional bits in the tap values.
        pub const TAP_SHIFT: usize = #shift;
        /// Coefficient width the taps fit.
        pub const COEFF_WIDTH: usize = #coeff_w;
        /// Input sample width.
        pub const INPUT_WIDTH: usize = #in_w;
    };

    // --- one pruned CIC per stage, each in its own module ---
    //
    // `cic_pruned!` puts `Q`, `D`, `CicStages` and its kernel at module
    // scope, so two invocations cannot share one module. A proc macro
    // can synthesise the module names that `macro_rules!` cannot.
    let mut stage_mods = Vec::new();
    for (k, c) in d.cics.iter().enumerate() {
        let m = format_ident!("stage{}", k + 1);
        // **Unsuffixed literals.** `quote!` renders a `usize` as
        // `2usize`, and `cic_pruned!` discriminates its arms on a bare
        // literal `n = 2` -- a suffixed token does not match any arm and
        // the error names the token rather than the cause.
        let w_in = Literal::usize_unsuffixed(c.input_width);
        let n = Literal::usize_unsuffixed(c.stages);
        let r = Literal::usize_unsuffixed(c.decimate);
        let delay = Literal::usize_unsuffixed(c.delay);
        let b_out = Literal::usize_unsuffixed(c.prune_budget);
        let doc = format!(
            " Stage {} of the cascade: /{} with {} integrator/comb pairs at {:.4} MHz.",
            k + 1,
            r,
            n,
            c.input_rate_hz / 1e6
        );
        stage_mods.push(quote! {
            #[doc = #doc]
            pub mod #m {
                use rhdl::prelude::*;
                use rhdl_fpga::core::dff;
                rhdl_fpga::cic_pruned!(
                    Cic,
                    w_in = #w_in,
                    n = #n,
                    r = #r,
                    m = #delay,
                    b_out = #b_out
                );
            }
        });
    }

    // --- widths along the chain ---
    let stage_out: Vec<usize> = d.cics.iter().map(|c| c.output_width()).collect();
    let last_cic_out = *stage_out.last().expect("a design has at least one stage");
    let fir_acc = u(rhdl_dsp_design::fir::accumulator_width(
        last_cic_out,
        d.compensator.coeff_width,
        d.compensator.taps.len(),
    ));
    let last_out_lit = u(last_cic_out);

    let fir_ty = quote! {
        rhdl_fpga::dsp::fir::SymmetricFir<
            #last_out_lit, #coeff_w, #fir_acc, #n_taps, #half, #shift, #last_out_lit
        >
    };

    // The compensator sits inside the *last* stage, at the output rate,
    // so the pair presents the plain decimator interface and
    // `StreamDecimator` can frame it.
    let last = format_ident!("stage{}", d.cics.len());
    let types = build_types(d, &stage_out, &fir_ty, &last);

    let doc_attrs = doc_lines.iter().map(|l| quote! { #[doc = #l] });
    Ok(quote! {
        #[doc = #doc_header]
        #(#doc_attrs)*
        #[doc = " ```"]
        pub mod #module {
            #consts
            #(#stage_mods)*
            #types
        }
    })
}

/// The chain's type aliases and constructors.
///
/// **`Chain` is the decimation alone.** The compensator is emitted
/// beside it, not inside it: a compensator does not have to sit
/// immediately behind the decimator, and does not have to exist in
/// hardware at all. The taps are a design output whatever you do with
/// them — apply them further down the fabric, or off the FPGA
/// entirely, and `TAPS` is still what you need.
///
/// `Compensated` is there for the common case where it *does* go right
/// behind the decimator in hardware. Choosing it is a decision the
/// caller makes, not one the macro makes for them.
fn build_types(
    d: &chain::ChainDesign,
    stage_out: &[usize],
    fir_ty: &TokenStream,
    last: &Ident,
) -> TokenStream {
    let u = Literal::usize_unsuffixed;
    let w_in = u(d.spec.input_width);
    let n_taps = u(d.compensator.taps.len());
    let coeff_w = u(d.compensator.coeff_width);
    let last_out = u(*stage_out.last().expect("at least one stage"));

    let compensator = quote! {
        /// The compensating FIR, at the output rate.
        ///
        /// Emitted but **not wired in** — see `Compensated` if you want
        /// it directly behind the decimator.
        pub type Fir = #fir_ty;

        /// The compensator's taps, as the filter wants them.
        ///
        /// `TAPS` carries the same values as plain integers, for a
        /// compensator that lives somewhere this type cannot reach.
        pub fn taps() -> [rhdl::prelude::SignedBits<#coeff_w>; #n_taps] {
            let mut t = [rhdl::prelude::SignedBits::<#coeff_w>::default(); #n_taps];
            let mut k = 0;
            while k < #n_taps {
                t[k] = rhdl::prelude::signed::<#coeff_w>(TAPS[k] as i128);
                k += 1;
            }
            t
        }

        /// The compensating filter, ready to place wherever you want it.
        pub fn compensator() -> Fir {
            <Fir>::new(taps())
        }
    };

    if d.cics.len() == 1 {
        quote! {
            #compensator

            /// The decimation chain: one framed decimator.
            ///
            /// No compensator. `DROOP_DB` is what this does to the band.
            pub type Chain = rhdl_fpga::dsp::cic::stream::StreamDecimator<
                #w_in, #last_out, #last::Cic
            >;

            /// The decimator with its compensator immediately behind it,
            /// in hardware.
            ///
            /// One of several places the compensator could go; the
            /// others are not this macro's business.
            pub type Compensated = rhdl_fpga::dsp::cic::stream::StreamDecimator<
                #w_in,
                #last_out,
                rhdl_fpga::dsp::cic::compensated::CompensatedCic<
                    #last_out, #last_out, #last_out, #last::Cic, Fir
                >,
            >;

            /// Build the decimation chain.
            pub fn new() -> Chain {
                <Chain>::new(Default::default())
            }

            /// Build the chain with the compensator behind it.
            pub fn compensated() -> Compensated {
                <Compensated>::new(
                    rhdl_fpga::dsp::cic::compensated::CompensatedCic::new(
                        Default::default(),
                        compensator(),
                    ),
                )
            }
        }
    } else {
        let first_out = u(stage_out[0]);
        quote! {
            #compensator

            /// The first, fast decimation stage, framed.
            pub type Stage1Framed = rhdl_fpga::dsp::cic::stream::StreamDecimator<
                #w_in, #first_out, stage1::Cic
            >;

            /// The final decimation stage, framed.
            pub type Stage2Framed = rhdl_fpga::dsp::cic::stream::StreamDecimator<
                #first_out, #last_out, #last::Cic
            >;

            /// The decimation chain: two framed stages, composed through
            /// their framing alone.
            ///
            /// No compensator. `DROOP_DB` is what this does to the band.
            pub type Chain = rhdl_fpga::dsp::cic::cascaded::CascadedDecimator<
                #w_in, #first_out, #last_out, Stage1Framed, Stage2Framed
            >;

            /// The final stage with the compensator inside it.
            pub type Stage2Compensated = rhdl_fpga::dsp::cic::stream::StreamDecimator<
                #first_out,
                #last_out,
                rhdl_fpga::dsp::cic::compensated::CompensatedCic<
                    #last_out, #last_out, #last_out, #last::Cic, Fir
                >,
            >;

            /// The chain with the compensator immediately behind the
            /// last decimation stage, in hardware.
            pub type Compensated = rhdl_fpga::dsp::cic::cascaded::CascadedDecimator<
                #w_in, #first_out, #last_out, Stage1Framed, Stage2Compensated
            >;

            /// Build the decimation chain.
            pub fn new() -> Chain {
                <Chain>::new(Default::default(), Default::default())
            }

            /// Build the chain with the compensator behind the last
            /// stage.
            pub fn compensated() -> Compensated {
                <Compensated>::new(
                    Default::default(),
                    <Stage2Compensated>::new(
                        rhdl_fpga::dsp::cic::compensated::CompensatedCic::new(
                            Default::default(),
                            compensator(),
                        ),
                    ),
                )
            }
        }
    }
}

/// `NarrowbandChain` -> `narrowband_chain`.
fn to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    /// The expansion for the worked example, as a snapshot.
    ///
    /// Deliberately checked as text rather than by compiling: this test
    /// is about *what the macro decided*, and the numbers in the
    /// expansion are the decision. `crates/rhdl-fpga/tests/
    /// cic_chain_macro.rs` covers whether it compiles and works.
    #[test]
    fn the_worked_example_expands() {
        let out = cic_chain(quote! {
            NarrowbandChain,
            fs = 125e6,
            decimate = 488,
            alias_free_bw = 64e3,
            in_w = 16,
            out_w = 24,
            ripple_db = 0.1,
            alias_db = 60,
            snr_db = 80,
        })
        .expect("must design")
        .to_string();

        // The module is named after the widget, snake-cased.
        assert!(out.contains("pub mod narrowband_chain"), "{out}");
        // The split it chose, emitted as an auditable constant.
        assert!(out.contains("SPLIT : [usize ; 2] = [8 , 61]"), "{out}");
        // Two pruned CICs, each in its own module because
        // `cic_pruned!` puts `Q`/`D` at module scope.
        assert!(out.contains("pub mod stage1"), "{out}");
        assert!(out.contains("pub mod stage2"), "{out}");
        // Unsuffixed literals, or `cic_pruned!`'s arms do not match.
        assert!(out.contains("n = 2"), "stage 1 depth: {out}");
        assert!(!out.contains("n = 2usize"), "suffixed literal leaked");
        // Composed through framing, not through wired internals.
        assert!(out.contains("CascadedDecimator"), "{out}");
        assert!(out.contains("StreamDecimator"), "{out}");
        // **The compensator is emitted beside the chain, not inside
        // it.** A compensator need not sit immediately behind the
        // decimator, and need not be in hardware at all -- so `Chain`
        // is decimation only, and `Compensated` is the opt-in.
        assert!(out.contains("pub type Chain"), "{out}");
        assert!(out.contains("pub type Compensated"), "{out}");
        assert!(out.contains("pub fn compensator"), "{out}");
        // Both figures, so a caller who skips the compensator knows
        // what they are carrying.
        assert!(out.contains("DROOP_DB"), "{out}");
        assert!(out.contains("RIPPLE_DB"), "{out}");
        // And the report is carried as documentation.
        assert!(out.contains("Design report"), "{out}");
    }

    /// A single-stage spec emits no cascade.
    #[test]
    fn a_shallow_spec_emits_one_stage() {
        let out = cic_chain(quote! {
            Shallow,
            fs = 100e6,
            decimate = 16,
            alias_free_bw = 400e3,
            in_w = 12,
            out_w = 20,
            alias_db = 40,
            cascade = no,
        })
        .expect("must design")
        .to_string();
        assert!(out.contains("SPLIT : [usize ; 1] = [16]"), "{out}");
        assert!(out.contains("pub mod stage1"), "{out}");
        assert!(!out.contains("pub mod stage2"), "{out}");
        assert!(!out.contains("CascadedDecimator"), "{out}");
    }

    /// **An infeasible spec must fail with a diagnostic that names the
    /// requirement and the knob.**
    ///
    /// A macro that says only "could not design" turns a solvable
    /// problem into a mystery. The designer already knows which
    /// constraint it missed and by how much; the job here is to not
    /// lose that on the way to the compiler.
    #[test]
    fn an_infeasible_spec_explains_itself() {
        let err = cic_chain(quote! {
            TooMuch,
            fs = 125e6,
            decimate = 488,
            alias_free_bw = 120e3,
            alias_db = 90,
        })
        .expect_err("this is not available at any depth");
        let msg = err.to_string();
        // Names the requirement, the shortfall, and what to change.
        assert!(msg.contains("cic_chain!"), "{msg}");
        assert!(
            msg.contains("alias_free_bw") || msg.contains("decimate"),
            "the message must name a knob: {msg}"
        );
        assert!(
            msg.contains("dB"),
            "the message must quantify the shortfall: {msg}"
        );
    }

    /// A bandwidth that cannot fit says so specifically.
    #[test]
    fn an_impossible_bandwidth_is_named_as_such() {
        let err = cic_chain(quote! {
            TooWide,
            fs = 125e6,
            decimate = 488,
            alias_free_bw = 200e3,
        })
        .expect_err("wider than the output Nyquist");
        let msg = err.to_string();
        assert!(msg.contains("output Nyquist"), "{msg}");
        assert!(msg.contains("2 * alias_free_bw * decimate"), "{msg}");
    }

    #[test]
    fn an_unknown_parameter_lists_the_valid_ones() {
        let err = cic_chain(quote! { Bad, fs = 100e6, wibble = 3 })
            .expect_err("wibble is not a parameter");
        let msg = err.to_string();
        assert!(msg.contains("unknown parameter `wibble`"), "{msg}");
        assert!(
            msg.contains("alias_free_bw"),
            "must list the valid ones: {msg}"
        );
    }

    #[test]
    fn a_repeated_parameter_is_rejected() {
        let err = cic_chain(quote! { Dup, decimate = 8, decimate = 16 }).expect_err("given twice");
        assert!(err.to_string().contains("given twice"), "{}", err);
    }

    #[test]
    fn a_bad_method_names_the_alternatives() {
        let err = cic_chain(quote! { M, method = magic }).expect_err("no such method");
        let msg = err.to_string();
        assert!(msg.contains("least_squares"), "{msg}");
        assert!(msg.contains("remez"), "{msg}");
    }

    #[test]
    fn snake_case_handles_the_shapes_that_appear() {
        assert_eq!(to_snake("NarrowbandChain"), "narrowband_chain");
        assert_eq!(to_snake("Chain"), "chain");
        assert_eq!(to_snake("chain"), "chain");
        assert_eq!(to_snake("DdcChain"), "ddc_chain");
    }
}
