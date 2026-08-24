// A phase-sensitive CIC-based digital down-converter, on tune.
//
// The input is a complex tone at 5 MHz against a 125 MHz sample clock,
// the oscillator is tuned to the same 5 MHz, and the CIC decimates by
// sixteen.
//
// What to look for:
//
//   - `sample` on the output appears on one cycle in sixteen. Between
//     them the widget is still working: the mixer runs every cycle and
//     the CIC integrators accumulate every cycle. Only the comb section
//     and the output run at the reduced rate.
//   - The output settles to a roughly constant complex value. That is
//     the point of a down-converter: a tone at the tuned frequency
//     becomes DC. Its *phase* is meaningful, not just its magnitude --
//     that is what "phase sensitive" means, and it is why the
//     oscillator is conjugated before mixing rather than used directly.
//   - `master` advances every cycle and is never reset. It is the
//     oscillator's absolute phase, the reference successive
//     acquisitions are measured against.
//   - The acquisition marker set on input sample 5 comes out on the
//     first output that follows it. Sample 5 is not an output cycle, so
//     the marker has to be sticky; losing it would leave the
//     acquisition unanchored.
//   - `overrun` and `frame_mismatch` stay low. Both are fault reports
//     rather than flow control.
//
// Deterministic (no RNG), so the committed trace regenerates
// byte-identically.

use rhdl::prelude::*;
use rhdl_fpga::doc::write_svg_as_markdown;
use rhdl_fpga::dsp::ddc::{In, UniformDdc};
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::nco::config::PHASE_W;
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::bus::Item;
use std::f64::consts::TAU;

const W: usize = 18;
const WA: usize = 26;
const S: usize = 2;
const R: usize = 16;
const M: usize = 1;
const CW: usize = 4;
const PROD_W: usize = W + 18 + 1;
const FS: f64 = 125_000_000.0;

fn main() -> Result<(), RHDLError> {
    let uut = UniformDdc::<W, WA, S, R, M, CW, PROD_W>::default();

    let f = 5_000_000.0;
    let full = (1u128 << PHASE_W) as f64;
    let tune = ((f / FS * full).rem_euclid(full)) as u128;
    let amp = 100_000.0;

    let seq: Vec<In<W>> = (0..64)
        .map(|k| {
            let t = TAU * f * (k as f64) / FS;
            In::<W> {
                sample: Some(Item::<Iq<W>, SyncMark> {
                    data: Iq::<W> {
                        re: signed::<W>((amp * t.cos()) as i128),
                        im: signed::<W>((amp * t.sin()) as i128),
                    },
                    // One marked sample, deliberately not on an output
                    // boundary.
                    frame: SyncMark { sync: k == 5 },
                }),
                frequency: bits::<PHASE_W>(tune),
                phase: bits::<PHASE_W>(0),
                downstream_ready: true,
            }
        })
        .collect();

    let stream = seq.into_iter().with_reset(1).clock_pos_edge(100);
    let vcd = uut.run(stream).collect::<SvgFile>();
    write_svg_as_markdown(vcd, "ddc.md", SvgOptions::default().with_io_filter())?;
    Ok(())
}
