//! **The framing test: two independently-marked streams must meet in
//! step at the modulator.**
//!
//! The receive path marks the sample that starts an acquisition. The
//! oscillator marks the first sample affected by a configuration
//! change. Both markers are applied *at their own source*, by the
//! widget that knows the answer exactly, and neither knows anything
//! about the other. If the sequencer's latency arithmetic is right,
//! they arrive at the modulator on the same cycle; if it is wrong, they
//! do not, and [`ComplexMixer`]'s `frame_mismatch` says so.
//!
//! That is the whole claim, and it is worth stating why it is not
//! circular. Each marker's position is fixed by its own widget's
//! internals — the NCO's by a delay line matched to its control
//! latency, the trigger's by its output register. The *lead time* is
//! computed here, in the sequencer, from the two published constants.
//! Nothing arranges for them to agree. The alignment either falls out
//! of the arithmetic or it does not.
//!
//! # The arithmetic
//!
//! ```text
//!   receive:  arm at t_rx   -> marked sample reaches the mixer at
//!                              t_rx + RX_TRIGGER_LATENCY
//!   local osc: change at t_cfg -> marked sample reaches the mixer at
//!                              t_cfg + FREQUENCY_CONTROL
//!
//!   equal when   t_cfg = t_rx + RX_TRIGGER_LATENCY - FREQUENCY_CONTROL
//! ```
//!
//! With today's constants that is `t_rx - 2`: **the oscillator is
//! configured two cycles before the acquisition is armed.** The test
//! never writes `2`; it writes the subtraction, so the day either
//! constant changes the schedule follows.

use rhdl::prelude::*;
use rhdl_fpga::dsp::iq::Iq;
use rhdl_fpga::dsp::mixer::complex::{self, ComplexMixer};
use rhdl_fpga::dsp::nco::composite::{self, NcoDefault};
use rhdl_fpga::dsp::nco::config::PHASE_W;
use rhdl_fpga::dsp::nco::latency;
use rhdl_fpga::dsp::nco::{frequency_composer, phase_composer};
use rhdl_fpga::dsp::rx_trigger::{self, RX_TRIGGER_LATENCY, RxTrigger};
use rhdl_fpga::dsp::sync::SyncMark;
use rhdl_fpga::rcstream::bus::Item;

const W: usize = 18;
/// Complex product width: each partial product is 36 bits and two are
/// summed, so the natural width is 37 and the narrowing drops 19.
const P: usize = 37;
const DR: usize = 19;

/// How far ahead of the acquisition the oscillator must be configured.
///
/// **Derived, never written as a literal.** This subtraction is the
/// thing under test.
const CONFIG_LEAD: usize = latency::FREQUENCY_CONTROL - RX_TRIGGER_LATENCY;

type Mixer = ComplexMixer<SyncMark, W, W, W, P, DR>;

/// The receive path, the oscillator, and the modulator they meet at.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
#[rhdl(dq_no_prefix)]
pub struct Chain {
    /// Marks the first sample of the acquisition.
    trigger: RxTrigger<W>,
    /// Marks the first sample its configuration change affects.
    nco: NcoDefault,
    /// Where the two must agree.
    mixer: Mixer,
}

/// What the sequencer drives.
#[derive(PartialEq, Clone, Copy, Debug, Digital)]
pub struct In {
    /// The received sample, un-framed as it comes off the converter.
    pub rx: Option<Item<Iq<W>, ()>>,
    /// Start the acquisition on the next received sample.
    pub arm: bool,
    /// The oscillator's master frequency term.
    pub freq: Bits<PHASE_W>,
}

impl SynchronousIO for Chain {
    type I = In;
    type O = complex::Out<SyncMark, W>;
    type Kernel = chain_kernel;
}

#[kernel]
#[doc(hidden)]
pub fn chain_kernel(_cr: ClockReset, i: In, q: Q) -> (complex::Out<SyncMark, W>, D) {
    let mut d = D::dont_care();

    d.trigger = rx_trigger::In::<W> {
        stream: i.rx,
        arm: i.arm,
        downstream_ready: true,
    };

    d.nco = composite::In {
        frequency: frequency_composer::In::<PHASE_W> {
            master: i.freq,
            scheduled_offset: bits::<PHASE_W>(0),
            modulation: bits::<PHASE_W>(0),
            calibration: bits::<PHASE_W>(0),
        },
        phase: phase_composer::In::<PHASE_W> {
            pulse: bits::<PHASE_W>(0),
            frame: bits::<PHASE_W>(0),
            calibration: bits::<PHASE_W>(0),
            fine_time: bits::<PHASE_W>(0),
            trim: bits::<PHASE_W>(0),
        },
        downstream_ready: true,
    };

    // Pure wiring, no registers of its own, so the measured alignment is
    // the chain's and not this harness's.
    d.mixer = complex::In::<SyncMark, W, W> {
        a: q.nco.stream.data,
        b: q.trigger.stream.data,
        downstream_ready: true,
    };

    (q.mixer, d)
}

// ---------------------------------------------------------------------

/// One cycle of stimulus.
fn drive(k: usize, arm_at: usize, cfg_at: usize, freq: u128) -> In {
    let theta = 2.0 * std::f64::consts::PI * (k as f64) / 16.0;
    let amp = 100_000.0;
    In {
        rx: Some(Item::<Iq<W>, ()> {
            data: Iq::<W> {
                re: signed::<W>((theta.cos() * amp) as i128),
                im: signed::<W>((theta.sin() * amp) as i128),
            },
            frame: (),
        }),
        arm: k == arm_at,
        // The frequency term steps once, and stays.  A step is what the
        // oscillator detects as a change.
        freq: bits::<PHASE_W>(if k >= cfg_at { freq } else { 0 }),
    }
}

fn run(arm_at: usize, cfg_at: usize) -> Vec<complex::Out<SyncMark, W>> {
    let uut = Chain::default();
    let freq = 1u128 << (PHASE_W - 3);
    let seq: Vec<In> = (0..32).map(|k| drive(k, arm_at, cfg_at, freq)).collect();
    uut.run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .synchronous_sample()
        .map(|s| s.output)
        .collect()
}

fn marked(out: &[complex::Out<SyncMark, W>]) -> Vec<usize> {
    out.iter()
        .enumerate()
        .filter(|(_, o)| match o.stream.data {
            Some(item) => item.frame.sync,
            None => false,
        })
        .map(|(k, _)| k)
        .collect()
}

fn mismatches(out: &[complex::Out<SyncMark, W>]) -> Vec<usize> {
    out.iter()
        .enumerate()
        .filter(|(_, o)| o.frame_mismatch)
        .map(|(k, _)| k)
        .collect()
}

const ARM_AT: usize = 12;

/// **The claim.** Scheduled by the published constants, the two markers
/// arrive together and the modulator is satisfied.
#[test]
fn correctly_scheduled_markers_arrive_together() {
    let out = run(ARM_AT, ARM_AT - CONFIG_LEAD);
    assert_eq!(
        mismatches(&out),
        Vec::<usize>::new(),
        "the modulator reported a framing disagreement on a correctly \
         scheduled chain; the two markers did not coincide"
    );
    assert_eq!(
        marked(&out).len(),
        1,
        "expected exactly one anchored product, got {:?}",
        marked(&out)
    );
}

/// **The falsifiability check.**
///
/// A test that cannot fail proves nothing. Issuing the configuration
/// one cycle early and one cycle late must both be caught — otherwise
/// `correctly_scheduled_markers_arrive_together` passing would say
/// nothing about the arithmetic, only that markers exist.
#[test]
fn a_mis_scheduled_configuration_is_caught() {
    for (label, cfg_at) in [
        ("one cycle early", ARM_AT - CONFIG_LEAD - 1),
        ("one cycle late", ARM_AT - CONFIG_LEAD + 1),
    ] {
        let out = run(ARM_AT, cfg_at);
        assert!(
            !mismatches(&out).is_empty(),
            "the modulator did not notice a configuration issued {label}; \
             the alignment check is vacuous"
        );
    }
}

/// The oscillator's marker and the receive marker are genuinely
/// independent — neither widget knows about the other.
///
/// Demonstrated by arming with the oscillator never reconfigured: the
/// receive marker still arrives, alone, and is reported as a
/// disagreement rather than silently accepted.
#[test]
fn a_lone_receive_marker_is_not_silently_accepted() {
    // cfg_at beyond the run, so the oscillator never steps and never
    // marks.
    let out = run(ARM_AT, 1000);
    assert!(
        !mismatches(&out).is_empty(),
        "an acquisition anchored against an unconfigured oscillator must \
         not look well-framed"
    );
}

/// The whole chain survives `iverilog`, RTL and NTL.
///
/// The alignment is a claim about hardware, so it has to hold in the
/// Verilog rather than only in the Rust simulator — which is exactly
/// where the zero-width comparison bug hid.
#[test]
fn the_chain_round_trips_through_iverilog() -> miette::Result<()> {
    let uut = Chain::default();
    let freq = 1u128 << (PHASE_W - 3);
    let cfg_at = ARM_AT - CONFIG_LEAD;
    let seq: Vec<In> = (0..24).map(|k| drive(k, ARM_AT, cfg_at, freq)).collect();
    let tb = uut
        .run(seq.into_iter().with_reset(1).clock_pos_edge(100))
        .collect::<SynchronousTestBench<_, _>>();
    tb.rtl(&uut, &Default::default())?.run_iverilog()?;
    tb.ntl(&uut, &Default::default())?.run_iverilog()?;
    Ok(())
}
