//! NEC IR remote receiver
//!
//! Decodes the [NEC IR protocol](https://www.sbprojects.net/knowledge/ir/nec.php),
//! the most widespread infrared remote-control format and the one
//! found in the bulk of consumer-electronics remotes (TVs, set-top
//! boxes, fans, simple AV receivers).  The widget consumes the
//! pre-demodulated digital output of a 38 kHz IR receiver module
//! (TSOP4838, VS1838B, etc.), where the line idles **high** and goes
//! **low** while a 38 kHz IR burst is being received.
//!
//! **v1 scope:**
//! - NEC protocol only.  RC5 and RC6 are tracked as v2 follow-ups.
//! - Decodes 32 data bits (manufacturer-typical layout: address +
//!   ~address + command + ~command, MSB-first).  No address-vs-
//!   command split inside the widget — the host masks `code` as it
//!   sees fit.
//! - Repeat-code detection: pulses `repeat` for one cycle when the
//!   sender holds a button down.  The previous `code` value is
//!   *not* re-emitted on a repeat — the host correlates the two
//!   pulses.
//! - **No transmitter** in v1.  An NEC TX widget composes the
//!   shipped (22) `core::pwm` for the 38 kHz carrier with a small
//!   bit-pattern FSM; tracked as a follow-up.
//! - **No automatic frame validation** beyond the leading-burst
//!   length check.  The host can validate the byte/inverse-byte
//!   redundancy by comparing `code & 0xFF` to `~(code >> 8) & 0xFF`
//!   etc.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-------+IrNecRx+-------+
     |                       |
bool |                       | B<32>
+--->| ir_in            code +--->
     |                       | bool
     |                  valid+--->
     |                       | bool
     |                  repeat+-->
     |                       | bool
     |                   busy+--->
     +-----------------------+
")]
//!
//!# Internals
//!
//! All durations are passed in via a [NecTimings] struct in *FPGA
//! cycles*.  The state machine is driven by edges on `ir_in` (the
//! kernel tracks the previous-cycle value to detect transitions)
//! and a per-state tick counter that measures the time between
//! edges.
//!
//! - `Idle`: line high, no transaction in progress.  A falling
//!   edge arms `LeadingBurst`.
//! - `LeadingBurst`: line low.  A rising edge ends it; if the low
//!   duration was longer than `t_lead_burst_min`, we proceed to
//!   `LeadingSpace`, otherwise back to `Idle`.
//! - `LeadingSpace`: line high.  A falling edge ends it.  If the
//!   high duration was greater than `t_lead_data_threshold`, the
//!   transaction is a 32-bit data frame → `DataBurst`.  Otherwise
//!   it is a repeat code → pulse `repeat` and return to `Idle`.
//! - `DataBurst`: line low.  A rising edge ends it (this is each
//!   bit's leading 562 µs burst).
//! - `DataSpace`: line high.  A falling edge ends it; the high
//!   duration determines the bit value (`> t_data_zero_one_threshold`
//!   → 1, else 0).  The bit shifts into the MSB of `code_reg` and
//!   `bit_idx` increments.  After 32 bits, we expect one more
//!   trailing burst (the final stop) → `FinalBurst`.
//! - `FinalBurst`: line low.  A rising edge or an idle timeout
//!   pulses `valid` and returns to `Idle`.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/ir_nec_rx.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/ir_nec_rx.md")]
//!
//! And the auto-generated FSM diagram for the NEC receive walk:
#![doc = include_str!("../../doc/ir_nec_rx_fsm.md")]
use rhdl::prelude::*;

use crate::core::{constant::Constant, dff};

/// State machine.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum NecState {
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Line is low; we're measuring the leading-burst duration.
    #[fsm_state(label = "lead burst")]
    LeadingBurst,
    /// Line is high; we're measuring the leading-space duration.
    #[fsm_state(label = "lead space")]
    LeadingSpace,
    /// Line is low; mid-frame data-bit burst.
    #[fsm_state(label = "bit burst")]
    DataBurst,
    /// Line is high; mid-frame data-bit space (duration → 0/1).
    #[fsm_state(label = "bit space")]
    DataSpace,
    /// Line is low; final stop-bit burst after the 32nd data bit.
    #[fsm_state(label = "stop burst")]
    FinalBurst,
}

/// Bus-timing parameters, all in *FPGA cycles*.
///
/// At a 100 MHz clock with standard NEC timings, typical values
/// (in microseconds, multiply by 100 to get FPGA cycles) are:
///
/// | Field                       | Standard NEC |
/// |-----------------------------|--------------|
/// | `t_lead_burst_min`          |   ~8000 µs   |
/// | `t_lead_burst_max`          |  ~10000 µs   |
/// | `t_lead_data_threshold`     |   ~3500 µs   |
/// | `t_lead_space_max`          |   ~5000 µs   |
/// | `t_data_zero_one_threshold` |   ~1100 µs   |
/// | `t_data_space_max`          |   ~2500 µs   |
///
/// `t_lead_data_threshold` is the midpoint between the
/// 4500 µs leading space (data frame) and the 2250 µs leading
/// space (repeat frame). `t_data_zero_one_threshold` is the
/// midpoint between the 562 µs (logic-0) and 1687 µs (logic-1)
/// data-bit space.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct NecTimings<const T_W: usize>
where
    rhdl::bits::W<T_W>: BitWidth,
{
    /// Minimum leading-burst duration to accept a frame start.
    pub t_lead_burst_min: Bits<T_W>,
    /// Maximum leading-burst duration before we abandon the frame.
    pub t_lead_burst_max: Bits<T_W>,
    /// Threshold separating data-frame leading space (~4500 µs)
    /// from repeat-code leading space (~2250 µs).
    pub t_lead_data_threshold: Bits<T_W>,
    /// Maximum leading-space duration before we abandon the frame.
    pub t_lead_space_max: Bits<T_W>,
    /// Threshold separating logic-0 space (~562 µs) from logic-1 space (~1687 µs).
    pub t_data_zero_one_threshold: Bits<T_W>,
    /// Maximum data-space duration before we abandon the frame.
    pub t_data_space_max: Bits<T_W>,
}

/// Bundled internal state for the NEC IR receiver (CLAUDE.md §3.1).
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct IrNecRxExtras<const T_W: usize>
where
    rhdl::bits::W<T_W>: BitWidth,
{
    pub tick: Bits<T_W>,
    pub prev_ir: bool,
    pub code_reg: Bits<32>,
    pub bit_idx: Bits<6>,
    pub valid_pulse: bool,
    pub repeat_pulse: bool,
}

impl<const T_W: usize> Default for IrNecRxExtras<T_W>
where
    rhdl::bits::W<T_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            tick: bits::<T_W>(0),
            prev_ir: true, // line idles high
            code_reg: bits::<32>(0),
            bit_idx: bits::<6>(0),
            valid_pulse: false,
            repeat_pulse: false,
        }
    }
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = NecState, allow_implicit)]
/// NEC IR remote receiver (v1).
pub struct IrNecRx<const T_W: usize>
where
    rhdl::bits::W<T_W>: BitWidth,
{
    state: dff::DFF<NecState>,
    extras: dff::DFF<IrNecRxExtras<T_W>>,
    timings: Constant<NecTimings<T_W>>,
}

impl<const T_W: usize> IrNecRx<T_W>
where
    rhdl::bits::W<T_W>: BitWidth,
{
    /// Create an NEC receiver with the given timings (all in FPGA cycles).
    pub fn new(timings: NecTimings<T_W>) -> Self {
        Self {
            state: dff::DFF::default(),
            extras: dff::DFF::new(IrNecRxExtras::default()),
            timings: Constant::new(timings),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [IrNecRx].
pub struct In {
    /// Pre-demodulated IR signal: idles high, low while carrier is present.
    pub ir_in: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [IrNecRx].
pub struct Out {
    /// Last-received 32-bit code (held until the next valid frame).
    pub code: Bits<32>,
    /// Pulses for one cycle when a valid 32-bit frame has been received.
    pub valid: bool,
    /// Pulses for one cycle when a repeat code (button-held) has been detected.
    pub repeat: bool,
    /// `true` while a frame is in progress.
    pub busy: bool,
}

impl<const T_W: usize> SynchronousIO for IrNecRx<T_W>
where
    rhdl::bits::W<T_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = ir_nec_rx<T_W>;
}

#[kernel]
/// Kernel for [IrNecRx].
// An arriving edge and an expiring space are different causes with
// the same effect, and saying so as two arms keeps the two exit
// paths visible. Collapsing them to `||` also re-shapes the emitted
// mux tree, which churns the Tier-3 snapshot for no behavioural gain.
#[allow(clippy::if_same_then_else)]
pub fn ir_nec_rx<const T_W: usize>(cr: ClockReset, i: In, q: Q<T_W>) -> (Out, D<T_W>)
where
    rhdl::bits::W<T_W>: BitWidth,
{
    let one_t: Bits<T_W> = bits::<T_W>(1);
    let zero_t: Bits<T_W> = bits::<T_W>(0);
    let one_b6: Bits<6> = bits::<6>(1);
    let zero_b6: Bits<6> = bits::<6>(0);

    let t = q.timings;

    let mut d = D::<T_W>::dont_care();
    d.state = q.state;
    let mut next = q.extras;
    next.tick = q.extras.tick + one_t;
    next.prev_ir = i.ir_in;
    next.valid_pulse = false;
    next.repeat_pulse = false;

    let falling_edge = q.extras.prev_ir && !i.ir_in;
    let rising_edge = !q.extras.prev_ir && i.ir_in;

    match q.state {
        NecState::Idle => {
            next.tick = zero_t;
            if falling_edge {
                d.state = NecState::LeadingBurst;
                next.tick = zero_t;
            }
        }
        NecState::LeadingBurst => {
            if rising_edge {
                if q.extras.tick >= t.t_lead_burst_min {
                    d.state = NecState::LeadingSpace;
                    next.tick = zero_t;
                    next.bit_idx = zero_b6;
                    next.code_reg = bits::<32>(0);
                } else {
                    d.state = NecState::Idle;
                    next.tick = zero_t;
                }
            } else if q.extras.tick >= t.t_lead_burst_max {
                d.state = NecState::Idle;
                next.tick = zero_t;
            }
        }
        NecState::LeadingSpace => {
            if falling_edge {
                if q.extras.tick >= t.t_lead_data_threshold {
                    d.state = NecState::DataBurst;
                    next.tick = zero_t;
                } else {
                    next.repeat_pulse = true;
                    d.state = NecState::Idle;
                    next.tick = zero_t;
                }
            } else if q.extras.tick >= t.t_lead_space_max {
                d.state = NecState::Idle;
                next.tick = zero_t;
            }
        }
        NecState::DataBurst => {
            if rising_edge {
                d.state = NecState::DataSpace;
                next.tick = zero_t;
            }
        }
        NecState::DataSpace => {
            if falling_edge {
                let bit_val = q.extras.tick >= t.t_data_zero_one_threshold;
                let bit_bits = if bit_val {
                    bits::<32>(1)
                } else {
                    bits::<32>(0)
                };
                next.code_reg = (q.extras.code_reg << 1) | bit_bits;
                let next_idx = q.extras.bit_idx + one_b6;
                if next_idx == bits::<6>(32) {
                    d.state = NecState::FinalBurst;
                    next.tick = zero_t;
                } else {
                    next.bit_idx = next_idx;
                    d.state = NecState::DataBurst;
                    next.tick = zero_t;
                }
            } else if q.extras.tick >= t.t_data_space_max {
                d.state = NecState::Idle;
                next.tick = zero_t;
            }
        }
        NecState::FinalBurst => {
            if rising_edge {
                next.valid_pulse = true;
                d.state = NecState::Idle;
                next.tick = zero_t;
            } else if q.extras.tick >= t.t_data_space_max {
                next.valid_pulse = true;
                d.state = NecState::Idle;
                next.tick = zero_t;
            }
        }
    }

    if cr.reset.any() {
        d.state = NecState::Idle;
        next = IrNecRxExtras::<T_W>::default();
    }

    d.extras = next;

    let mut o = Out::dont_care();
    o.code = q.extras.code_reg;
    o.valid = q.extras.valid_pulse;
    o.repeat = q.extras.repeat_pulse;
    o.busy = q.state != NecState::Idle;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    /// Compact test timings — every duration small enough that a full frame
    /// fits in a few hundred FPGA cycles.  Standard NEC ratios are preserved.
    fn test_timings() -> NecTimings<14> {
        NecTimings {
            t_lead_burst_min: bits(80),          // ~8 ms in scaled units
            t_lead_burst_max: bits(120),         // ~12 ms
            t_lead_data_threshold: bits(35),     // midpoint of 22 (repeat) and 45 (data)
            t_lead_space_max: bits(60),          // ~6 ms
            t_data_zero_one_threshold: bits(11), // midpoint of 6 and 17
            t_data_space_max: bits(30),          // give the FinalBurst timeout some slack
        }
    }

    fn idle_in() -> In {
        In { ir_in: true }
    }

    /// Drive the FSM with a hand-crafted NEC waveform encoding the given
    /// 32-bit code, MSB-first.  Returns the input vector (excluding any
    /// reset cycles).
    fn nec_waveform(code: u32, t: &NecTimings<14>) -> Vec<In> {
        let mut v = Vec::new();
        let push_low = |v: &mut Vec<In>, n: u128| {
            for _ in 0..n {
                v.push(In { ir_in: false });
            }
        };
        let push_high = |v: &mut Vec<In>, n: u128| {
            for _ in 0..n {
                v.push(In { ir_in: true });
            }
        };
        // 16 cycles of idle-high to settle prev_ir
        push_high(&mut v, 16);
        // Leading burst: ~9 ms scaled = 90
        push_low(&mut v, 90);
        // Leading space (data frame): ~4.5 ms scaled = 45
        push_high(&mut v, 45);
        // 32 bits, MSB-first
        for bit in (0..32).rev() {
            // Each bit's leading burst: ~562 µs scaled = 6
            push_low(&mut v, 6);
            // Bit's space: ~562 µs (=6) for 0, ~1687 µs (=17) for 1
            let one = (code >> bit) & 1 == 1;
            push_high(&mut v, if one { 17 } else { 6 });
            let _ = t; // suppress unused warning
        }
        // Final stop burst
        push_low(&mut v, 6);
        // Trailing idle-high to flush FinalBurst's exit
        push_high(&mut v, 50);
        v
    }

    fn nec_repeat_waveform() -> Vec<In> {
        let mut v = Vec::new();
        // Leading-high settle
        for _ in 0..16 {
            v.push(In { ir_in: true });
        }
        // Leading burst (~9 ms)
        for _ in 0..90 {
            v.push(In { ir_in: false });
        }
        // Repeat space (~2.25 ms scaled = 22)
        for _ in 0..22 {
            v.push(In { ir_in: true });
        }
        // Trailing stop burst
        for _ in 0..6 {
            v.push(In { ir_in: false });
        }
        // Final idle
        for _ in 0..50 {
            v.push(In { ir_in: true });
        }
        v
    }

    #[test]
    fn test_idle_emits_no_pulses() -> miette::Result<()> {
        let uut = IrNecRx::<14>::new(test_timings());
        let stream = std::iter::repeat_n(idle_in(), 64)
            .with_reset(1)
            .clock_pos_edge(100);
        let any = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .any(|s| s.output.valid || s.output.repeat || s.output.busy);
        assert!(!any, "idle should not emit valid/repeat/busy");
        Ok(())
    }

    #[test]
    fn test_decodes_data_frame() -> miette::Result<()> {
        let uut = IrNecRx::<14>::new(test_timings());
        let t = test_timings();
        let code: u32 = 0x12345678;
        let mut stream_in = nec_waveform(code, &t);
        // Padding to ensure we observe the valid pulse.
        for _ in 0..32 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let valid_idx = outputs
            .iter()
            .position(|s| s.output.valid)
            .expect("no valid pulse");
        let received = outputs[valid_idx].output.code.raw() as u32;
        assert_eq!(
            received, code,
            "decoded code mismatch: got 0x{received:08x}, expected 0x{code:08x}"
        );
        Ok(())
    }

    #[test]
    fn test_decodes_repeat_code() -> miette::Result<()> {
        let uut = IrNecRx::<14>::new(test_timings());
        let mut stream_in = nec_repeat_waveform();
        for _ in 0..32 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let any_repeat = outputs.iter().any(|s| s.output.repeat);
        let any_valid = outputs.iter().any(|s| s.output.valid);
        assert!(any_repeat, "expected a repeat pulse");
        assert!(
            !any_valid,
            "repeat-code waveform should not emit a data valid pulse"
        );
        Ok(())
    }

    #[test]
    fn test_short_burst_rejected() -> miette::Result<()> {
        // A leading burst shorter than t_lead_burst_min should be rejected
        // and never emit valid/repeat.
        let uut = IrNecRx::<14>::new(test_timings());
        let mut stream_in: Vec<In> = Vec::new();
        for _ in 0..16 {
            stream_in.push(In { ir_in: true });
        }
        // Burst of only 30 cycles (< 80 minimum)
        for _ in 0..30 {
            stream_in.push(In { ir_in: false });
        }
        for _ in 0..200 {
            stream_in.push(In { ir_in: true });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let any_pulse = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .any(|s| s.output.valid || s.output.repeat);
        assert!(!any_pulse, "short burst should be ignored");
        Ok(())
    }

    // Tier 3 — HDL emission length sanity check.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = IrNecRx::<14>::new(test_timings());
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["13689"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip.
    #[test]
    fn test_ir_nec_rx_hdl_works() -> miette::Result<()> {
        let uut = IrNecRx::<14>::new(test_timings());
        let t = test_timings();
        let mut stream_in = nec_waveform(0x12345678, &t);
        for _ in 0..32 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest.
    #[test]
    fn test_ir_nec_rx_trace() -> miette::Result<()> {
        let uut = IrNecRx::<14>::new(test_timings());
        let t = test_timings();
        let mut stream_in = nec_waveform(0x12345678, &t);
        for _ in 0..32 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("ir_nec_rx");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["5beb044e36d7d576742efd7b39b67f64514a6a466d375268bee6ecc4ee9b1568"];
        let digest = vcd.dump_to_file(root.join("ir_nec_rx.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
