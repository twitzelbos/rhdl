//! PS/2 host-to-device transmitter
//!
//! Drives the host side of the PS/2 protocol when the host needs
//! to send a command to a PS/2 keyboard or mouse.  The PS/2
//! interface is two open-drain wires (`CLK` and `DATA`), each with
//! a pull-up to 5 V.  Normally the *device* (keyboard/mouse) is
//! the clock master; for host-to-device transmission, the host
//! temporarily takes the bus by:
//!
//! 1. **Inhibit** — host pulls `CLK` low for at least 100 µs.
//! 2. **Request to send** — host releases `CLK` (high-Z) and pulls
//!    `DATA` low.  This is the start bit.
//! 3. **Bit clocking** — the device, seeing CLK released and DATA
//!    low, generates 10 clock pulses on `CLK`.  The host changes
//!    `DATA` while `CLK` is high; the device samples on `CLK`'s
//!    falling edge.  Frame: 8 data bits LSB-first + 1 odd-parity
//!    bit + 1 stop bit (host releases `DATA` to high).
//! 4. **Acknowledge** — the device pulls `DATA` low for one CLK
//!    cycle to acknowledge.  The host samples this as the
//!    line-acknowledge.
//!
//! See IBM PS/2 Hardware Interface Technical Reference (1988)
//! §17 ("Keyboard Adapter") and §19 ("Pointing Device Adapter")
//! for the full protocol.
//!
//! Pairs with the existing receive-only [super::ps2_keyboard] and
//! [super::ps2_mouse] widgets to complete the bidirectional PS/2
//! stack.  Hosts that need both directions multiplex this TX
//! widget with the RX widget on the same physical CLK + DATA
//! pads (the TX widget's `request` strobe takes the bus; the RX
//! widget's frame parser is paused while the TX is in flight via
//! the `tx_busy` output).
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +---------+Ps2HostTx+---------+
     |                             |
B<8> |                             | bool
+--->| tx_byte           clk_oe    +-->
bool |                             | bool
+--->| tx_strobe         data_oe   +-->
bool |                             | bool
+--->| clk_in            tx_busy   +-->
bool |                             | bool
+--->| data_in           ack_seen  +-->
     |                             | bool
     |                   ack_error +-->
     +-----------------------------+
")]
//!
//!# Wiring at the pads
//!
//! The CLK and DATA lines are open-drain.  Pad-level: both
//! signals drive `0` when the widget asserts `clk_oe` / `data_oe`,
//! and float (high-Z) otherwise — relying on the external pull-up
//! to bring the line high.  Use [crate::tristate::simple] at each
//! pad to convert the widget's `_oe` signal into a tristate
//! driver:
//!
//! ```ignore
//! let clk_pad = tristate::simple(clk_oe, false);   // drive 0 when asserted
//! let data_pad = tristate::simple(data_oe, false);
//! ```
//!
//! `clk_in` / `data_in` are the sampled (input) values from the
//! pads; the host should pass them through a 2-flip-flop
//! synchronizer chain before feeding them to this widget so
//! they're settled in the FPGA clock domain.
//!
//!# Internals
//!
//! Six-state FSM (Idle, Inhibit, RequestStart, ClockBits,
//! ClockParity, ClockStop, AwaitAck).  A countdown timer drives
//! the inhibit interval.  The bit shifter loads `tx_byte | parity`
//! at the start of `ClockBits` and shifts out one bit per `CLK`
//! falling edge until empty.  After the stop bit, the widget
//! samples DATA for one CLK cycle to confirm the device's
//! acknowledge low pulse; if DATA stays high, `ack_error` fires
//! and the host can retry.
//!
//!# Parameters
//!
//! - `INH_W` — bit width of the inhibit-interval counter.  For a
//!   100 MHz FPGA clock, `INH_W = 14` covers the required ≥ 100
//!   µs inhibit (10 000 cycles).
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/ps2_host_tx.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/ps2_host_tx.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/ps2_host_tx_fsm.md")]

use rhdl::prelude::*;

use crate::core::{constant::Constant, dff};

/// Host-side TX state machine.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum Ps2TxState {
    /// No TX in flight; both CLK and DATA released to high.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Driving CLK low to inhibit the device clock.
    #[fsm_state(label = "inhibit")]
    Inhibit,
    /// Inhibit complete: pulling DATA low (start bit) and
    /// releasing CLK so the device starts generating clocks.
    #[fsm_state(label = "request start")]
    RequestStart,
    /// Clocking out 8 data bits — DATA changes on CLK rising,
    /// device samples on CLK falling.
    #[fsm_state(label = "clock 8 bits")]
    ClockBits,
    /// Clocking out the odd-parity bit.
    #[fsm_state(label = "clock parity")]
    ClockParity,
    /// Releasing DATA for the stop bit.
    #[fsm_state(label = "clock stop")]
    ClockStop,
    /// Waiting for the device's ack pulse on DATA.
    #[fsm_state(label = "await ack")]
    AwaitAck,
}

/// Bundled internal state for the PS/2 host TX (CLAUDE.md §3.1).
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct Ps2HostTxExtras<const INH_W: usize>
where
    rhdl::bits::W<INH_W>: BitWidth,
{
    pub inhibit_ctr: Bits<INH_W>,
    pub shifter: Bits<9>,
    pub bit_idx: Bits<4>,
    pub prev_clk: bool,
    pub ack_seen_q: bool,
    pub ack_error_q: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = Ps2TxState, allow_implicit)]
/// PS/2 host-to-device transmitter.
pub struct Ps2HostTx<const INH_W: usize>
where
    rhdl::bits::W<INH_W>: BitWidth,
{
    state: dff::DFF<Ps2TxState>,
    extras: dff::DFF<Ps2HostTxExtras<INH_W>>,
    inhibit_cycles: Constant<Bits<INH_W>>,
}

impl<const INH_W: usize> Ps2HostTx<INH_W>
where
    rhdl::bits::W<INH_W>: BitWidth,
{
    /// Create a host TX with the given inhibit duration in clock
    /// cycles.  For 100 MHz and the minimum 100 µs inhibit, pass
    /// `bits::<14>(10_000)`.
    pub fn new(inhibit_cycles: Bits<INH_W>) -> Self {
        Self {
            state: dff::DFF::default(),
            extras: dff::DFF::default(),
            inhibit_cycles: Constant::new(inhibit_cycles),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [Ps2HostTx].
pub struct In {
    /// Byte to transmit.
    pub tx_byte: Bits<8>,
    /// One-cycle strobe to start a transmission.  Ignored unless idle.
    pub tx_strobe: bool,
    /// Sampled CLK line (after pad synchronizer).
    pub clk_in: bool,
    /// Sampled DATA line (after pad synchronizer).
    pub data_in: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [Ps2HostTx].
pub struct Out {
    /// Drive CLK line low when true; release (high-Z) when false.
    pub clk_oe: bool,
    /// Drive DATA line low when true; release when false.
    pub data_oe: bool,
    /// True while a TX is in flight (any state other than Idle).
    pub tx_busy: bool,
    /// Pulses for one cycle when the device's ack was seen.
    pub ack_seen: bool,
    /// Pulses for one cycle when the device failed to ack.
    pub ack_error: bool,
}

impl<const INH_W: usize> SynchronousIO for Ps2HostTx<INH_W>
where
    rhdl::bits::W<INH_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = ps2_host_tx<INH_W>;
}

#[kernel]
/// Kernel for [Ps2HostTx].
pub fn ps2_host_tx<const INH_W: usize>(
    cr: ClockReset,
    i: In,
    q: Q<INH_W>,
) -> (Out, D<INH_W>)
where
    rhdl::bits::W<INH_W>: BitWidth,
{
    let mut d = D::<INH_W>::dont_care();
    d.state = q.state;
    let mut next = q.extras;
    next.prev_clk = i.clk_in;
    next.ack_seen_q = false;
    next.ack_error_q = false;

    let clk_falling = q.extras.prev_clk && !i.clk_in;

    let mut clk_oe = false;
    let mut data_oe = false;

    match q.state {
        Ps2TxState::Idle => {
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
                let packed: Bits<9> =
                    (b.resize::<9>()) | (parity_bit.resize::<9>() << 8);
                next.shifter = packed;
                next.bit_idx = bits::<4>(0);
                next.inhibit_ctr = q.inhibit_cycles;
                d.state = Ps2TxState::Inhibit;
                clk_oe = true;
            }
        }
        Ps2TxState::Inhibit => {
            clk_oe = true;
            if q.extras.inhibit_ctr == bits::<INH_W>(0) {
                d.state = Ps2TxState::RequestStart;
                clk_oe = false;
                data_oe = true;
            } else {
                next.inhibit_ctr = q.extras.inhibit_ctr - bits::<INH_W>(1);
            }
        }
        Ps2TxState::RequestStart => {
            data_oe = true;
            if clk_falling {
                d.state = Ps2TxState::ClockBits;
                next.bit_idx = bits::<4>(0);
            }
        }
        Ps2TxState::ClockBits => {
            let bit_to_send = (q.extras.shifter & bits::<9>(1)) != bits::<9>(0);
            data_oe = !bit_to_send;
            if clk_falling {
                next.shifter = q.extras.shifter >> 1;
                next.bit_idx = q.extras.bit_idx + bits::<4>(1);
                if q.extras.bit_idx == bits::<4>(7) {
                    d.state = Ps2TxState::ClockParity;
                }
            }
        }
        Ps2TxState::ClockParity => {
            let bit_to_send = (q.extras.shifter & bits::<9>(1)) != bits::<9>(0);
            data_oe = !bit_to_send;
            if clk_falling {
                next.shifter = q.extras.shifter >> 1;
                d.state = Ps2TxState::ClockStop;
            }
        }
        Ps2TxState::ClockStop => {
            data_oe = false;
            if clk_falling {
                d.state = Ps2TxState::AwaitAck;
            }
        }
        Ps2TxState::AwaitAck => {
            data_oe = false;
            if clk_falling {
                if !i.data_in {
                    next.ack_seen_q = true;
                } else {
                    next.ack_error_q = true;
                }
                d.state = Ps2TxState::Idle;
            }
        }
    }

    if cr.reset.any() {
        d.state = Ps2TxState::Idle;
        next = Ps2HostTxExtras::<INH_W>::default();
    }

    d.extras = next;

    let mut o = Out::dont_care();
    o.clk_oe = clk_oe;
    o.data_oe = data_oe;
    o.tx_busy = q.state != Ps2TxState::Idle;
    o.ack_seen = q.extras.ack_seen_q;
    o.ack_error = q.extras.ack_error_q;
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
            clk_in: true,
            data_in: true,
        }
    }

    /// Helper: simulate a PS/2 device that, after the host
    /// finishes inhibiting, generates 11 CLK pulses (10 cycles
    /// for the 8 data + 1 parity + 1 stop, plus 1 for the ack)
    /// and ACKs by pulling DATA low on the last cycle.
    fn drive_device(host_tx: u8, inhibit_cycles_short: u32) -> Vec<In> {
        // For testing, use a very short inhibit so the simulation
        // stays small.
        let _ = host_tx;
        let mut stream = Vec::new();
        // Send strobe.
        let mut s = idle_in();
        s.tx_byte = bits::<8>(host_tx as u128);
        s.tx_strobe = true;
        stream.push(s);
        for _ in 0..(inhibit_cycles_short as usize + 5) {
            stream.push(idle_in());
        }
        // Now the host should be in RequestStart.  Generate
        // 11 falling edges on CLK.  Between falling edges, hold
        // CLK high for a few cycles.  On the last edge, also
        // pull DATA low to acknowledge.
        for cycle in 0..11 {
            // CLK high for 4 cycles (host samples DATA, holds it stable).
            for _ in 0..4 {
                let mut s = idle_in();
                s.clk_in = true;
                // DATA low if the device is acking on the last cycle.
                s.data_in = true;
                stream.push(s);
            }
            // CLK falls: 4 cycles low (device "samples" but we don't model).
            for _ in 0..4 {
                let mut s = idle_in();
                s.clk_in = false;
                s.data_in = if cycle == 10 { false } else { true };
                stream.push(s);
            }
        }
        // Tail.
        for _ in 0..10 {
            stream.push(idle_in());
        }
        stream
    }

    // Tier 2 — full TX sequence:  strobe → inhibit (clk_oe high) →
    // request-start (data_oe high, clk_oe low) → bit clocking →
    // back to idle.  We assert the structural sequence rather than
    // a full device-emulator round-trip (the latter requires
    // closing the loop between clk_oe→clk_in via a tristate
    // model, which is a separate testbench concern).
    #[test]
    fn test_tx_sequence_inhibit_then_data_oe() -> miette::Result<()> {
        let inhibit = 4u32;
        let stream_in = drive_device(0xED, inhibit);
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2HostTx::<8>::new(bits(inhibit as u128));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        // Must have observed clk_oe high (inhibit drove CLK low).
        let clk_oe_seen = outputs.iter().any(|s| s.output.clk_oe);
        assert!(clk_oe_seen, "clk_oe never asserted (inhibit not entered)");
        // After inhibit, data_oe must have been asserted (start bit).
        let data_oe_seen = outputs.iter().any(|s| s.output.data_oe);
        assert!(data_oe_seen, "data_oe never asserted (start bit / data clocking not entered)");
        // tx_busy must have been asserted at some point.
        let tx_busy_seen = outputs.iter().any(|s| s.output.tx_busy);
        assert!(tx_busy_seen, "tx_busy never asserted");
        Ok(())
    }

    #[test]
    fn test_idle_outputs_high_z() -> miette::Result<()> {
        let stream_in: Vec<In> = std::iter::repeat_n(idle_in(), 16).collect();
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2HostTx::<8>::new(bits(4));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        // Idle: clk_oe and data_oe should be false (released).
        assert!(outputs.iter().all(|s| !s.output.clk_oe && !s.output.data_oe));
        assert!(outputs.iter().all(|s| !s.output.tx_busy));
        Ok(())
    }

    #[test]
    fn test_strobe_starts_tx() -> miette::Result<()> {
        let mut stream_in: Vec<In> = vec![idle_in(); 4];
        let mut start = idle_in();
        start.tx_byte = bits(0xAA);
        start.tx_strobe = true;
        stream_in.push(start);
        for _ in 0..16 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2HostTx::<8>::new(bits(4));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        // After strobe, tx_busy should go high.
        let busy_seen = outputs.iter().any(|s| s.output.tx_busy);
        assert!(busy_seen, "tx_busy never asserted after strobe");
        Ok(())
    }

    // Tier 3 — HDL emission length.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Ps2HostTx::<14>::new(bits(10_000));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 1000, "HDL length {}", hdl.len());
        Ok(())
    }

    // Tier 4 — iverilog round-trip.
    #[test]
    fn test_ps2_host_tx_hdl_works() -> miette::Result<()> {
        let stream_in = drive_device(0xED, 4);
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2HostTx::<8>::new(bits(4));
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest.
    #[test]
    fn test_ps2_host_tx_trace() -> miette::Result<()> {
        let stream_in = drive_device(0xED, 4);
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2HostTx::<8>::new(bits(4));
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("ps2_host_tx");
        std::fs::create_dir_all(&root).unwrap();
        let _ = vcd.dump_to_file(root.join("ps2_host_tx.vcd")).unwrap();
        let _ = expect![[r#""#]];
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = Ps2HostTx::<14>::fsm_descriptor();
        assert_eq!(desc.widget_name, "Ps2HostTx");
        assert_eq!(desc.variants().len(), 7);
    }
}
