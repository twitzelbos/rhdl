//! PS/2 device-side transmitter (FPGA emulates a PS/2 device)
//!
//! Drives the *device* side of the PS/2 protocol: the FPGA acts
//! as the keyboard or mouse, generating CLK pulses and shifting
//! data bits to the host.  The reverse of
//! [super::ps2_keyboard::Ps2Keyboard] (host-side RX) and the
//! complement of [super::ps2_host_tx::Ps2HostTx] (host-side TX
//! for sending commands TO the device).
//!
//! ## Protocol (device-to-host direction)
//!
//! 1. Device idles with both CLK and DATA released (high via
//!    pull-ups).
//! 2. Device asserts DATA low (start bit).
//! 3. Device generates 10 CLK pulses (10–16.7 kHz typical).
//!    For each pulse: drive DATA to the bit value while CLK is
//!    high; the host samples on CLK falling edge.  Frame:
//!    8 data bits LSB-first + odd-parity bit + stop bit (DATA
//!    released to high).
//! 4. Device returns to idle.
//!
//! The host can interrupt the device by pulling CLK low (the
//! "inhibit" signal).  When the device sees CLK held low for
//! ≥ 100 µs, it must abort its current transmission and check
//! whether the host wants to send a command (DATA low while CLK
//! is released = host's start bit, see [super::ps2_host_tx]).
//! For simplicity, this widget aborts on inhibit and signals
//! `host_inhibit` to the parent so the parent can sequence a
//! receive cycle (typically using ps2_keyboard or ps2_mouse for
//! the actual byte capture).
//!
//! ## Pacing
//!
//! The CLK rate is parameterized via `clk_div` — the parent
//! provides cycles per CLK half-period.  For a 100 MHz FPGA
//! clock and a 12.5 kHz PS/2 CLK (mid-range, well within spec),
//! `clk_div = 4000` (40 µs half-period).
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-------+Ps2DeviceTx+-------+
     |                           |
B<8> |                           | bool
+--->| tx_byte         clk_oe    +-->
bool |                           | bool
+--->| tx_strobe       data_oe   +-->
bool |                           | bool
+--->| clk_in          tx_busy   +-->
     |                           | bool
     |                tx_done    +-->
     |                           | bool
     |                host_inhibit+-->
     +---------------------------+
")]
//!
//!# Internals
//!
//! Six-state FSM (Idle, Start, ClockData, ClockParity, ClockStop,
//! Aborted).  An interval timer paces each CLK half-period.  A
//! parity computation runs once at start (XOR of 8 bits, inverted
//! for odd parity).  The shifter is loaded with `{stop=1, parity,
//! data[7..0], start=0}` (11 bits) and shifted out one bit per
//! CLK falling edge.
//!
//! Inhibit detection: at every CLK cycle, sample `clk_in`.  If
//! the host has pulled CLK low (we observe `clk_in == low` while
//! the device is NOT itself driving CLK low), abort.
//!
//!# Parameters
//!
//! - `DIV_W` — bit width of the half-period divisor.  For
//!   100 MHz clock + 12.5 kHz PS/2 CLK, `DIV_W = 13` covers the
//!   ~4000-cycle half-period.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/ps2_device_tx.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/ps2_device_tx.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/ps2_device_tx_fsm.md")]

use rhdl::prelude::*;

use crate::core::{constant::Constant, dff};

/// Device-side TX state machine.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum Ps2DevTxState {
    /// No TX in flight; CLK + DATA released.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Driving DATA low (start bit) before generating first CLK rise.
    #[fsm_state(label = "start bit")]
    StartBit,
    /// Clocking out the 8 data bits + parity + stop (10 bits total).
    #[fsm_state(label = "clock 10 bits")]
    ClockBits,
    /// Aborted by host inhibit; parent should sequence a receive cycle.
    #[fsm_state(label = "aborted")]
    Aborted,
}

/// Bundled internal state for the PS/2 device TX (CLAUDE.md §3.1).
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Ps2DeviceTxExtras<const DIV_W: usize>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    shifter: Bits<11>,
    bit_idx: Bits<4>,
    div_ctr: Bits<DIV_W>,
    clk_out: bool,
    done_pulse: bool,
    aborted_q: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = Ps2DevTxState, allow_implicit)]
/// PS/2 device-side transmitter.
pub struct Ps2DeviceTx<const DIV_W: usize>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    state: dff::DFF<Ps2DevTxState>,
    extras: dff::DFF<Ps2DeviceTxExtras<DIV_W>>,
    half_period: Constant<Bits<DIV_W>>,
}

impl<const DIV_W: usize> Ps2DeviceTx<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    /// Create a device TX with the given CLK half-period.
    /// For 100 MHz FPGA + 12.5 kHz PS/2 CLK, pass
    /// `bits::<13>(4000)`.
    pub fn new(half_period: Bits<DIV_W>) -> Self {
        Self {
            state: dff::DFF::default(),
            extras: dff::DFF::default(),
            half_period: Constant::new(half_period),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [Ps2DeviceTx].
pub struct In {
    /// Byte to transmit.
    pub tx_byte: Bits<8>,
    /// One-cycle strobe to start a transmission.  Ignored unless idle.
    pub tx_strobe: bool,
    /// Sampled CLK line (pad-side, after synchronizer).  Used for
    /// inhibit detection — when the device is NOT driving CLK low
    /// but `clk_in` is low, the host is inhibiting.
    pub clk_in: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [Ps2DeviceTx].
pub struct Out {
    /// Drive CLK low when true; release otherwise.
    pub clk_oe: bool,
    /// Drive DATA low when true; release otherwise.
    pub data_oe: bool,
    /// True while a TX is in flight.
    pub tx_busy: bool,
    /// Pulses for one cycle when TX completes successfully.
    pub tx_done: bool,
    /// True when the host inhibited mid-transmission (latched).
    pub host_inhibit: bool,
}

impl<const DIV_W: usize> SynchronousIO for Ps2DeviceTx<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = ps2_device_tx<DIV_W>;
}

#[kernel]
/// Kernel for [Ps2DeviceTx].
// `data_oe = false` is the safe state for a tristate enable, and it
// stays as the declared default even though every path below assigns
// it -- so a path added later that forgets to is not driving the bus
// by accident.
#[allow(unused_assignments)]
pub fn ps2_device_tx<const DIV_W: usize>(cr: ClockReset, i: In, q: Q<DIV_W>) -> (Out, D<DIV_W>)
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    let mut d = D::<DIV_W>::dont_care();
    d.state = q.state;
    let mut next = q.extras;
    next.done_pulse = false;

    let mut clk_oe = !q.extras.clk_out;
    let mut data_oe = false;

    let host_inhibiting = q.extras.clk_out && !i.clk_in && q.state != Ps2DevTxState::Idle;

    if host_inhibiting {
        d.state = Ps2DevTxState::Aborted;
        next.aborted_q = true;
        clk_oe = false;
        data_oe = false;
    } else {
        match q.state {
            Ps2DevTxState::Idle => {
                clk_oe = false;
                data_oe = false;
                next.aborted_q = false;
                if i.tx_strobe {
                    let b = i.tx_byte;
                    let p0 = (b >> 0) & bits::<8>(1);
                    let p1 = (b >> 1) & bits::<8>(1);
                    let p2 = (b >> 2) & bits::<8>(1);
                    let p3 = (b >> 3) & bits::<8>(1);
                    let p4 = (b >> 4) & bits::<8>(1);
                    let p5 = (b >> 5) & bits::<8>(1);
                    let p6 = (b >> 6) & bits::<8>(1);
                    let p7 = (b >> 7) & bits::<8>(1);
                    let xor_all = p0 ^ p1 ^ p2 ^ p3 ^ p4 ^ p5 ^ p6 ^ p7;
                    let parity_bit: Bits<8> = xor_all ^ bits::<8>(1);
                    let packed: Bits<11> = (b.resize::<11>() << 1)
                        | (parity_bit.resize::<11>() << 9)
                        | (bits::<11>(1) << 10);
                    next.shifter = packed;
                    next.bit_idx = bits::<4>(0);
                    next.div_ctr = q.half_period;
                    next.clk_out = true;
                    d.state = Ps2DevTxState::StartBit;
                    data_oe = true;
                }
            }
            Ps2DevTxState::StartBit => {
                let bit = (q.extras.shifter & bits::<11>(1)) != bits::<11>(0);
                data_oe = !bit;
                if q.extras.div_ctr == bits::<DIV_W>(0) {
                    next.clk_out = !q.extras.clk_out;
                    next.div_ctr = q.half_period;
                    if q.extras.clk_out {
                        d.state = Ps2DevTxState::ClockBits;
                    }
                } else {
                    next.div_ctr = q.extras.div_ctr - bits::<DIV_W>(1);
                }
            }
            Ps2DevTxState::ClockBits => {
                let bit = (q.extras.shifter & bits::<11>(1)) != bits::<11>(0);
                data_oe = !bit;
                if q.extras.div_ctr == bits::<DIV_W>(0) {
                    next.clk_out = !q.extras.clk_out;
                    next.div_ctr = q.half_period;
                    if q.extras.clk_out {
                        next.shifter = q.extras.shifter >> 1;
                        next.bit_idx = q.extras.bit_idx + bits::<4>(1);
                        if q.extras.bit_idx == bits::<4>(10) {
                            next.done_pulse = true;
                            d.state = Ps2DevTxState::Idle;
                            next.clk_out = false;
                        }
                    }
                } else {
                    next.div_ctr = q.extras.div_ctr - bits::<DIV_W>(1);
                }
            }
            Ps2DevTxState::Aborted => {
                clk_oe = false;
                data_oe = false;
                if i.clk_in {
                    d.state = Ps2DevTxState::Idle;
                }
            }
        }
    }

    if cr.reset.any() {
        d.state = Ps2DevTxState::Idle;
        next = Ps2DeviceTxExtras::<DIV_W>::default();
    }

    d.extras = next;

    let mut o = Out::dont_care();
    o.clk_oe = clk_oe;
    o.data_oe = data_oe;
    o.tx_busy = q.state != Ps2DevTxState::Idle && q.state != Ps2DevTxState::Aborted;
    o.tx_done = q.extras.done_pulse;
    o.host_inhibit = q.extras.aborted_q;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In {
            tx_byte: bits(0),
            tx_strobe: false,
            clk_in: true, // host releases CLK in idle
        }
    }

    #[test]
    fn test_idle_outputs_high_z() -> miette::Result<()> {
        let stream_in: Vec<In> = std::iter::repeat_n(idle_in(), 16).collect();
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2DeviceTx::<8>::new(bits(4));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        assert!(
            outputs
                .iter()
                .all(|s| !s.output.clk_oe && !s.output.data_oe)
        );
        Ok(())
    }

    #[test]
    fn test_strobe_starts_tx() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 4];
        let mut start = idle_in();
        start.tx_byte = bits(0xAA);
        start.tx_strobe = true;
        stream_in.push(start);
        for _ in 0..200 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2DeviceTx::<8>::new(bits(4));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        assert!(outputs.iter().any(|s| s.output.tx_busy));
        assert!(outputs.iter().any(|s| s.output.data_oe));
        Ok(())
    }

    #[test]
    fn test_tx_done_pulses() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 4];
        let mut start = idle_in();
        start.tx_byte = bits(0x55);
        start.tx_strobe = true;
        stream_in.push(start);
        for _ in 0..400 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2DeviceTx::<8>::new(bits(4));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let done_count = outputs.iter().filter(|s| s.output.tx_done).count();
        assert_eq!(done_count, 1, "tx_done should pulse exactly once per tx");
        Ok(())
    }

    #[test]
    fn test_host_inhibit_aborts() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 4];
        let mut start = idle_in();
        start.tx_byte = bits(0xAA);
        start.tx_strobe = true;
        stream_in.push(start);
        // Brief silence then host pulls CLK low.
        for _ in 0..30 {
            stream_in.push(idle_in());
        }
        for _ in 0..30 {
            let mut s = idle_in();
            s.clk_in = false;
            stream_in.push(s);
        }
        for _ in 0..30 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2DeviceTx::<8>::new(bits(4));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        assert!(outputs.iter().any(|s| s.output.host_inhibit));
        Ok(())
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Ps2DeviceTx::<13>::new(bits(4000));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 1000, "HDL length {}", hdl.len());
        Ok(())
    }

    #[test]
    fn test_ps2_device_tx_hdl_works() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 2];
        let mut start = idle_in();
        start.tx_byte = bits(0xAA);
        start.tx_strobe = true;
        stream_in.push(start);
        for _ in 0..40 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2DeviceTx::<8>::new(bits(4));
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_ps2_device_tx_trace() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 2];
        let mut start = idle_in();
        start.tx_byte = bits(0x55);
        start.tx_strobe = true;
        stream_in.push(start);
        for _ in 0..200 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2DeviceTx::<8>::new(bits(4));
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("ps2_device_tx");
        std::fs::create_dir_all(&root).unwrap();
        let _ = vcd.dump_to_file(root.join("ps2_device_tx.vcd")).unwrap();
        let _ = expect![[r#""#]];
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = Ps2DeviceTx::<13>::fsm_descriptor();
        assert_eq!(desc.widget_name, "Ps2DeviceTx");
        assert_eq!(desc.variants().len(), 4);
    }
}
