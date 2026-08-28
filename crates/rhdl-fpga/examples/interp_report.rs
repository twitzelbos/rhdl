// Writes `doc/interp_report.pdf` and `doc/interp_chain_report.pdf`: what
// an interpolation chain does to a transmit signal, and what the
// pre-compensator does about it.
//
// The transmit counterpart of `cic_report`. The questions are different,
// so the plots are different.
//
// Page 1 answers "how far down are the images". Upsampling by R repeats
// the envelope's spectrum at every multiple of the envelope rate, and
// the CIC's sinc^N nulls sit exactly on those multiples -- so the image
// bands are marked and the worst of them is a line you can read off.
// Then a zoom on the first image, where the requirement is met or
// missed, and the stage table with both the uniform and the tapered
// widths.
//
// Page 2 is the compensator: the droop it inverts, its own response, and
// the flat composite. Plus the plot that makes the central transmit-side
// point visible -- the compensator's response over two periods of the
// envelope rate, with the signal band and the first image band marked on
// it. The gain is the same in both. That is why more taps cannot improve
// image rejection, and the page says so in as many words, because a
// reader carrying receive-side intuition will try exactly that.
//
// Two reports are written. The first is a configuration specified by
// hand -- "I chose these parameters, what do they do". The second is
// derived from requirements -- "here is the bandwidth, the flatness and
// the image floor I need, what should I build". Same renderer, so they
// cannot drift.
//
// The builders live in `rhdl_fpga::doc::interp_report`, not here, so you
// can point them at your own configuration.
//
// Deterministic: the PDF carries no timestamp and regenerates
// byte-identically, so it is committed and a diff means something.

use rhdl::prelude::*;
use rhdl_fpga::doc::interp_report::{InterpReport, as_design, interp_chain_report, interp_report};
use rhdl_fpga::dsp::cic::interp_chain;

fn main() -> Result<(), RHDLError> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("doc");

    // ---- the hand-specified configuration ----
    let cfg = InterpReport::default();
    let doc = interp_report(cfg).expect("the default configuration must design");
    let path = root.join("interp_report.pdf");
    std::fs::write(&path, doc.to_bytes()).expect("write the report");

    let hand = as_design(cfg).expect("designable");
    println!("--- specified parameters ---");
    println!(
        "  {:.4} kHz x {} = {:.1} MHz, {:.1} kHz signal",
        hand.input_rate_hz / 1e3,
        cfg.rate,
        cfg.fs_hz / 1e6,
        hand.spec.image_free_bw_hz / 1e3
    );
    println!(
        "  N={} M={}: images {:.1} dB down, worst at {:.4} MHz",
        cfg.stages,
        cfg.delay,
        hand.achieved_image_db,
        hand.worst_image_hz / 1e6
    );
    println!(
        "  ripple {:.4} dB after compensation, {} register bits ({} as built)",
        hand.achieved_ripple_db, hand.register_bits, hand.built_register_bits
    );
    println!("  wrote {}", path.display());
    println!();

    // ---- and the derived design ----
    //
    // The same total interpolation, but the depth and the split chosen
    // to meet a stated image floor rather than picked in advance.
    let spec = interp_chain::InterpSpec::default();
    let derived = interp_chain::design(spec.clone()).expect("the default spec must design");
    let chain_path = root.join("interp_chain_report.pdf");
    std::fs::write(&chain_path, interp_chain_report(&derived).to_bytes()).expect("write");

    println!("--- derived design ---");
    println!(
        "  asked: {:.1} kHz signal, images >= {:.0} dB, ripple <= {:.2} dB",
        spec.image_free_bw_hz / 1e3,
        spec.min_image_rejection_db,
        spec.max_ripple_db
    );
    println!(
        "  chose: split {:?}, N={:?}, M={:?}",
        derived.split(),
        derived.depths(),
        derived.delays()
    );
    println!(
        "  headroom: {} bits for any input, {} in-band only",
        derived.mid_width_any_input, derived.mid_width_in_band
    );
    println!(
        "  got:   images {:.1} dB down, ripple {:.4} dB, {} taps",
        derived.achieved_image_db,
        derived.achieved_ripple_db,
        derived.compensator.taps.len()
    );
    println!(
        "  cost:  {} register bits uniform, {} as built (lossless)",
        derived.register_bits, derived.built_register_bits
    );
    let missing = derived.unreachable_rates();
    if missing.is_empty() {
        println!("  rates:  every rate from 2 to 125 is settable at run time");
    } else {
        println!(
            "  rates:  {} of {} rates in 2..=125 are NOT reachable by this split \
             (e.g. {:?}) -- set arbitrary_rate to force a single stage",
            missing.len(),
            derived.spec.interpolate - derived.spec.rate_min + 1,
            &missing[..missing.len().min(5)]
        );
    }
    if let Some(a) = &derived.alternative {
        println!(
            "  runner-up: split {:?} N={:?} M={:?} -- {}",
            a.split, a.stages, a.delays, a.why
        );
    }
    println!("  wrote {}", chain_path.display());
    Ok(())
}
