// Writes `doc/cic_report.pdf`: what a CIC does to your signal, and what
// the compensator does about it.
//
// Two pages.
//
// Page 1 is the CIC alone. The full-band plot shows why a CIC belongs
// in front of a decimator -- its nulls sit exactly on the frequencies
// that decimation folds onto DC. The passband plot shows the price: a
// sinc^N droop across the band you meant to keep, several decibels
// deep at the edge.
//
// Page 2 is the compensator: the designed inverse-sinc response, and
// the composite, which is flat. The figures underneath are the ones
// worth arguing about -- ripple before and after, the gain the design
// asks for, and what survives quantisation to real coefficients.
//
// The report builder lives in `rhdl_fpga::doc::report`, not here, so
// you can point it at your own configuration rather than this one.
//
// Deterministic: the PDF carries no timestamp and regenerates
// byte-identically, so it is committed and a diff means something.

use rhdl::prelude::*;
use rhdl_fpga::doc::report::{CicReport, chain_report, cic_report};
use rhdl_fpga::dsp::cic::{accumulator_width, chain, compensator, dc_gain, response};

fn main() -> Result<(), RHDLError> {
    let cfg = CicReport::default();
    let doc = cic_report(cfg).expect("the default configuration must design");

    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("doc")
        .join("cic_report.pdf");
    std::fs::write(&path, doc.to_bytes()).expect("write the report");

    // And the derived-design report: the same machinery pointed at a
    // chain that was *specified* rather than hand-parameterised.
    let derived =
        chain::design(chain::ChainSpec::default()).expect("the default chain spec must design");
    let chain_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("doc")
        .join("cic_chain_report.pdf");
    std::fs::write(&chain_path, chain_report(&derived).to_bytes()).expect("write");
    println!("--- derived chain ---");
    println!("{derived}");
    println!("wrote {}", chain_path.display());
    println!();

    // Say it on stdout too, so running this is useful on its own.
    let (n, r, m) = (cfg.stages, cfg.rate, cfg.delay);
    let spec = compensator::Spec {
        cics: vec![compensator::CicShape {
            decimate: r,
            stages: n,
            delay: m,
        }],
        passband: cfg.passband,
        taps: cfg.taps,
        stopband_edge: 1.0,
        min_stopband_db: 0.0,
        max_ripple_db: 0.1,
        method: compensator::Method::LeastSquares,
    };
    let design = compensator::design(spec).expect("design");
    let quant = compensator::quantise(&design, cfg.coeff_width);

    println!("CIC: N={n} R={r} M={m} W_IN={}", cfg.w_in);
    println!("  DC gain            {}", dc_gain(n, r, m));
    println!(
        "  accumulator width  {} bits",
        accumulator_width(cfg.w_in, n, r, m)
    );
    println!(
        "  passband droop     {:.2} dB at {:.0}% of output Nyquist",
        response::passband_droop_db(cfg.passband, n, r, m),
        cfg.passband * 100.0
    );
    println!(
        "  worst alias        {:.1} dB",
        response::worst_alias_db(cfg.passband, n, r, m)
    );
    println!(
        "Compensator: {} taps, {}-bit coefficients",
        cfg.taps, cfg.coeff_width
    );
    println!("  ideal ripple       {:.4} dB", design.ripple_db);
    println!("  quantised ripple   {:.4} dB", quant.ripple_db);
    println!("  peak gain          {:.2}x", design.peak_gain);
    println!("  fractional bits    {}", quant.shift);
    println!("  DC gain            {:.6}", quant.dc_gain);
    println!("wrote {}", path.display());
    Ok(())
}
