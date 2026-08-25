//! Does `cic_chain!` actually produce working hardware?
//!
//! The macro runs the filter design at expansion time. That means a
//! *compile* of this file exercises the designer, and the tests below
//! exercise what it emitted — so a design that produces an
//! uninstantiable widget fails the build, and one that produces a
//! wrong filter fails a test.

use rhdl::prelude::*;
use rhdl_fpga::dsp::cic::stream::In;
use rhdl_fpga::dsp::iq::Real;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::Item;

// The worked example: 125 Msps down to about 256 ksps, carrying a
// 128 kHz-wide complex channel.
cic_chain!(
    NarrowbandChain,
    fs = 125e6,
    decimate = 488,
    alias_free_bw = 64e3,
    in_w = 16,
    out_w = 24,
    ripple_db = 0.1,
    alias_db = 60,
    snr_db = 80,
);

// A shallow one, to cover the single-stage path.
cic_chain!(
    ShallowChain,
    fs = 100e6,
    decimate = 16,
    alias_free_bw = 400e3,
    in_w = 12,
    out_w = 20,
    alias_db = 40,
    cascade = no,
);

#[test]
fn the_derived_numbers_are_visible() {
    // The macro's whole convenience is not having to compute these.
    // Being unable to *see* them would be a different thing.
    assert_eq!(narrowband_chain::DECIMATE, 488);
    assert_eq!(narrowband_chain::SPLIT.iter().product::<usize>(), 488);
    assert_eq!(narrowband_chain::SPLIT.len(), 2, "expected a cascade");
    assert!(narrowband_chain::RIPPLE_DB <= 0.1);
    assert!(narrowband_chain::ALIAS_REJECTION_DB >= 60.0);
    assert!(narrowband_chain::SNR_DB >= 80.0);
    assert_eq!(narrowband_chain::TAPS.len() % 2, 1, "odd, so linear phase");
    println!(
        "split {:?}, {} taps, {} register bits, ripple {:.4} dB",
        narrowband_chain::SPLIT,
        narrowband_chain::TAPS.len(),
        narrowband_chain::REGISTER_BITS,
        narrowband_chain::RIPPLE_DB
    );
}

#[test]
fn the_shallow_chain_is_a_single_stage() {
    assert_eq!(shallow_chain::SPLIT, [16]);
    assert_eq!(shallow_chain::DECIMATE, 16);
}

/// The taps must be symmetric, or the folded filter it feeds computes a
/// different filter than the design describes.
#[test]
fn the_emitted_taps_are_symmetric() {
    for c in [&narrowband_chain::TAPS[..], &shallow_chain::TAPS[..]] {
        let n = c.len();
        for k in 0..n {
            assert_eq!(c[k], c[n - 1 - k], "taps not symmetric");
        }
    }
}

/// **The chain elaborates to hardware.**
#[test]
fn both_chains_elaborate() -> miette::Result<()> {
    let a = narrowband_chain::new();
    let _ = a.descriptor("top".into())?;
    let b = shallow_chain::new();
    let _ = b.descriptor("top".into())?;
    Ok(())
}

/// **The compensator is optional, and both forms are real hardware.**
///
/// `Chain` is decimation alone; a compensator does not have to sit
/// immediately behind the decimator and does not have to be in the
/// FPGA at all. `Compensated` is the opt-in for when it does.
#[test]
fn the_compensator_is_separable() -> miette::Result<()> {
    // Decimation only.
    let plain = shallow_chain::new();
    let _ = plain.descriptor("top".into())?;
    // The same chain with the compensator behind it.
    let with = shallow_chain::compensated();
    let _ = with.descriptor("top".into())?;
    // And the filter on its own, for placing elsewhere.
    let fir = shallow_chain::compensator();
    let _ = fir.descriptor("top".into())?;
    Ok(())
}

/// Both figures are reported: what the chain does unaided, and what is
/// left if the taps are applied.
#[test]
fn the_droop_and_the_compensated_ripple_are_both_reported() {
    // Skipping the compensator is a real choice, so the cost of
    // skipping it has to be visible.
    assert!(
        narrowband_chain::DROOP_DB < -1.0,
        "the chain alone must droop measurably: {}",
        narrowband_chain::DROOP_DB
    );
    assert!(narrowband_chain::RIPPLE_DB <= 0.1);
    assert!(
        narrowband_chain::RIPPLE_DB < narrowband_chain::DROOP_DB.abs(),
        "compensation must improve on doing nothing"
    );
    println!(
        "uncompensated droop {:.3} dB, compensated ripple {:.4} dB",
        narrowband_chain::DROOP_DB,
        narrowband_chain::RIPPLE_DB
    );
}

fn item<const W: usize>(v: i128, sync: bool) -> In<W>
where
    rhdl::bits::W<W>: BitWidth,
{
    In::<W> {
        stream: Some(Item::<Real<W>, SyncMark> {
            data: Real::<W> { v: signed::<W>(v) },
            frame: SyncMark { sync },
        }),
        downstream_ready: true,
    }
}

fn idle<const W: usize>() -> In<W>
where
    rhdl::bits::W<W>: BitWidth,
{
    In::<W> {
        stream: None,
        downstream_ready: true,
    }
}

/// **It decimates by what was asked for**, and the mark survives.
#[test]
fn the_shallow_chain_decimates_and_carries_the_mark() {
    let r = shallow_chain::DECIMATE;
    let mut seq: Vec<In<12>> = (0..(r * 30)).map(|_| item::<12>(60, false)).collect();
    seq[r * 8] = item::<12>(60, true);
    seq.extend(std::iter::repeat_n(idle::<12>(), 8));
    let out: Vec<(i128, bool)> = shallow_chain::new()
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter_map(|s| {
            s.output
                .stream
                .data
                .map(|it| (it.data.v.raw(), it.frame.sync))
        })
        .collect();
    let want = (r * 30) / r;
    assert!(
        out.len() + 2 >= want && out.len() <= want,
        "expected about {want} outputs, got {}",
        out.len()
    );
    assert_eq!(
        out.iter().filter(|(_, s)| *s).count(),
        1,
        "the mark must come out exactly once"
    );
}

/// The cascade decimates by the product of its split.
#[test]
fn the_narrowband_chain_decimates_by_488() {
    let r = narrowband_chain::DECIMATE;
    let n = r * 4;
    let mut seq: Vec<In<16>> = (0..n).map(|_| item::<16>(100, false)).collect();
    seq.extend(std::iter::repeat_n(idle::<16>(), 16));
    let out: Vec<i128> = narrowband_chain::new()
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .filter_map(|s| s.output.stream.data.map(|it| it.data.v.raw()))
        .collect();
    assert!(
        (1..=4).contains(&out.len()),
        "expected up to 4 outputs from {n} samples at /{r}, got {}",
        out.len()
    );
}

/// `iverilog` agrees with the simulator on the emitted chain.
#[test]
fn the_shallow_chain_round_trips_through_iverilog() -> miette::Result<()> {
    let uut = shallow_chain::new();
    let r = shallow_chain::DECIMATE;
    let mut seq: Vec<In<12>> = (0..(r * 5))
        .map(|k: usize| item::<12>((k as i128 * 37) % 401 - 200, k == 3))
        .collect();
    seq.extend(std::iter::repeat_n(idle::<12>(), 4));
    let tb = uut
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}
