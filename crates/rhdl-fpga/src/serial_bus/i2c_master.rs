//! I2C master (write-only, single-byte v1)
//!
//! Standard 7-bit I2C master that performs single-byte WRITE
//! transactions: `START → addr+W → ACK → data → ACK → STOP`.
//! This v1 hardcodes the write direction (R/W bit = 0), one
//! data byte per transaction, and no clock stretching.  Read
//! transactions and multi-byte bursts are tracked as follow-ups.
//!
//! The master drives **open-drain** outputs: `scl_drive_low` and
//! `sda_drive_low` are high when the master is *actively pulling
//! the line low*, low when the master *releases* the line (at
//! which point an external pull-up resistor takes over and the
//! line goes high).  Wrap each output with [super::super::tristate::simple]
//! to expose true `BitZ` tristate buses to the I/O pads.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-------+I2cMaster+-------+
     |                         |
B<7> |                         | bool
+--->| addr        scl_drv_low +--->
B<8> |                         | bool
+--->| data        sda_drv_low +--->
bool |                         | bool
+--->| start            ack_ok +--->
bool |                         | bool
+--->| sda_in             busy +--->
     |                    done +--->
     +-------------------------+
")]
//!
//!# Internals
//!
//! The bus rate is set by the `divisor` parameter at construction:
//! each of the four phases per bit (`SCL low setup`, `SCL low hold`,
//! `SCL high sample`, `SCL high hold`) takes `divisor` FPGA cycles.
//! So one I2C bit = `4 * divisor` FPGA cycles, and one byte =
//! `9 * 4 * divisor` cycles (8 data bits + 1 ACK bit).
//!
//! State machine:
//!
//! - `Idle`: lines released (high), waiting for `start`.
//! - `Start`: drive SDA low while SCL is high (the START condition).
//! - `Addr`: shift out the 7-bit address + R/W=0, MSB first.
//! - `AckAddr`: release SDA, sample for slave ACK.
//! - `Data`: shift out the 8 data bits, MSB first.
//! - `AckData`: release SDA, sample for slave ACK.
//! - `Stop`: drive SDA low while raising SCL, then release SDA
//!   (the STOP condition).
//!
//!# Behavior
//!
//! - `scl_drive_low == 1` ⇒ master is pulling SCL low.
//! - `sda_drive_low == 1` ⇒ master is pulling SDA low.
//! - `sda_in` is the sampled value of SDA (after the external
//!   pull-up resolution).  The master uses it during ACK phases.
//! - `ack_ok` is high after a successful transaction (both ACKs
//!   were observed low).  Held until the next transaction begins.
//! - `done` pulses for one cycle at the end of the STOP condition.
//!
//!# Parameters
//!
//! - `DIV_W` — bit width of the per-phase divisor counter
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/i2c_master.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/i2c_master.md")]
//!
//! And the auto-generated FSM diagram for the I2C transaction:
#![doc = include_str!("../../doc/i2c_master_fsm.md")]
use rhdl::prelude::*;

use crate::core::{constant::Constant, dff};

/// State of the I2C transaction.
///
/// Encoded as a 3-bit `Digital` enum so it fits in a tight DFF.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum I2cState {
    /// Bus idle.  Both lines released to the pull-up.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Driving SDA low while SCL is high — the START condition.
    #[fsm_state(label = "START")]
    Start,
    /// Shifting out the 7-bit address + R/W bit, MSB-first.
    Addr,
    /// Released SDA, sampling for slave's address-ACK.
    #[fsm_state(label = "ACK addr")]
    AckAddr,
    /// Shifting out the 8-bit data byte, MSB-first.
    Data,
    /// Released SDA, sampling for slave's data-ACK.
    #[fsm_state(label = "ACK data")]
    AckData,
    /// Driving SDA low while raising SCL, then releasing — the STOP condition.
    #[fsm_state(label = "STOP")]
    Stop,
}

/// Bundled internal state for the I2C master.  Per CLAUDE.md
/// §3.1, all non-FSM internal registers live in one
/// Digital-derived struct behind a single DFF.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct I2cMasterExtras<const DIV_W: usize>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    phase_sub: Bits<DIV_W>,
    phase: Bits<2>,
    bit_idx: Bits<4>,
    addr_reg: Bits<8>,
    data_reg: Bits<8>,
    ack_addr_ok: bool,
    ack_data_ok: bool,
    done_pulse: bool,
}

impl<const DIV_W: usize> Default for I2cMasterExtras<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            phase_sub: bits::<DIV_W>(0),
            phase: bits::<2>(0),
            bit_idx: bits::<4>(0),
            addr_reg: bits::<8>(0),
            data_reg: bits::<8>(0),
            ack_addr_ok: false,
            ack_data_ok: false,
            done_pulse: false,
        }
    }
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = I2cState, allow_implicit)]
/// I2C master (write-only, single-byte v1).
pub struct I2cMaster<const DIV_W: usize>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    state: dff::DFF<I2cState>,
    extras: dff::DFF<I2cMasterExtras<DIV_W>>,
    divisor: Constant<Bits<DIV_W>>,
}

impl<const DIV_W: usize> I2cMaster<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    /// Create an I2C master with the given per-phase divisor.
    /// Total bit time = `4 * divisor` FPGA cycles.
    pub fn new(divisor: Bits<DIV_W>) -> Self {
        Self {
            state: dff::DFF::default(),
            extras: dff::DFF::new(I2cMasterExtras::default()),
            divisor: Constant::new(divisor),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [I2cMaster].
pub struct In {
    /// 7-bit slave address (latched at `start`).
    pub addr: Bits<7>,
    /// 8-bit data byte to write (latched at `start`).
    pub data: Bits<8>,
    /// Strobe to begin a transaction.  Ignored while `busy`.
    pub start: bool,
    /// Sampled SDA value (after external pull-up resolution).
    pub sda_in: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [I2cMaster].
pub struct Out {
    /// `1` ⇒ master pulls SCL low; `0` ⇒ master releases SCL.
    pub scl_drive_low: bool,
    /// `1` ⇒ master pulls SDA low; `0` ⇒ master releases SDA.
    pub sda_drive_low: bool,
    /// High while a transaction is in progress.
    pub busy: bool,
    /// Pulses for one cycle at end of transaction.
    pub done: bool,
    /// `true` if the last transaction was successfully ACKed by the slave
    /// (both address and data ACKs observed low).
    pub ack_ok: bool,
}

impl<const DIV_W: usize> SynchronousIO for I2cMaster<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = i2c_master<DIV_W>;
}

#[kernel]
/// Kernel for [I2cMaster].
// Collapsing the `if` into the outer `match` needs a match guard,
// which the kernel subset does not accept.
#[allow(clippy::collapsible_match)]
pub fn i2c_master<const DIV_W: usize>(cr: ClockReset, i: In, q: Q<DIV_W>) -> (Out, D<DIV_W>)
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    let one_div: Bits<DIV_W> = bits::<DIV_W>(1);
    let zero_div: Bits<DIV_W> = bits::<DIV_W>(0);
    let one_b2: Bits<2> = bits::<2>(1);
    let zero_b2: Bits<2> = bits::<2>(0);
    let three_b2: Bits<2> = bits::<2>(3);
    let one_b4: Bits<4> = bits::<4>(1);
    let zero_b4: Bits<4> = bits::<4>(0);
    let eight_b4: Bits<4> = bits::<4>(8);
    let zero_b8: Bits<8> = bits::<8>(0);

    let mut d = D::<DIV_W>::dont_care();
    d.state = q.state;
    let mut next = q.extras;
    next.done_pulse = false;

    let phase_done = q.extras.phase_sub == (q.divisor - one_div);

    let in_byte_phase = match q.state {
        I2cState::Idle => false,
        I2cState::Start
        | I2cState::Addr
        | I2cState::AckAddr
        | I2cState::Data
        | I2cState::AckData
        | I2cState::Stop => true,
    };

    if !in_byte_phase {
        if i.start {
            d.state = I2cState::Start;
            next.phase = zero_b2;
            next.phase_sub = zero_div;
            next.bit_idx = zero_b4;
            next.addr_reg = (i.addr.resize::<8>()) << 1;
            next.data_reg = i.data;
            next.ack_addr_ok = false;
            next.ack_data_ok = false;
        }
    } else {
        let sample_ack = q.extras.phase == one_b2 && q.extras.phase_sub == zero_div;
        if sample_ack {
            match q.state {
                I2cState::AckAddr => {
                    if !i.sda_in {
                        next.ack_addr_ok = true;
                    }
                }
                I2cState::AckData => {
                    if !i.sda_in {
                        next.ack_data_ok = true;
                    }
                }
                _ => {}
            }
        }
        if phase_done {
            next.phase_sub = zero_div;
            if q.extras.phase == three_b2 {
                next.phase = zero_b2;
                match q.state {
                    I2cState::Start => {
                        d.state = I2cState::Addr;
                        next.bit_idx = zero_b4;
                    }
                    I2cState::Addr => {
                        let next_bit = q.extras.bit_idx + one_b4;
                        next.addr_reg = q.extras.addr_reg << 1;
                        if next_bit == eight_b4 {
                            d.state = I2cState::AckAddr;
                            next.bit_idx = zero_b4;
                        } else {
                            next.bit_idx = next_bit;
                        }
                    }
                    I2cState::AckAddr => {
                        d.state = I2cState::Data;
                        next.bit_idx = zero_b4;
                    }
                    I2cState::Data => {
                        let next_bit = q.extras.bit_idx + one_b4;
                        next.data_reg = q.extras.data_reg << 1;
                        if next_bit == eight_b4 {
                            d.state = I2cState::AckData;
                            next.bit_idx = zero_b4;
                        } else {
                            next.bit_idx = next_bit;
                        }
                    }
                    I2cState::AckData => {
                        d.state = I2cState::Stop;
                        next.bit_idx = zero_b4;
                    }
                    I2cState::Stop => {
                        d.state = I2cState::Idle;
                        next.done_pulse = true;
                    }
                    I2cState::Idle => {
                        d.state = I2cState::Idle;
                    }
                }
            } else {
                next.phase = q.extras.phase + one_b2;
            }
        } else {
            next.phase_sub = q.extras.phase_sub + one_div;
        }
    }

    if cr.reset.any() {
        d.state = I2cState::Idle;
        next = I2cMasterExtras::<DIV_W>::default();
    }

    d.extras = next;

    let _ = zero_b8;
    let mut scl_drive_low = false;
    let mut sda_drive_low = false;
    match q.state {
        I2cState::Idle => {}
        I2cState::Start => {
            if q.extras.phase != zero_b2 {
                sda_drive_low = true;
            }
        }
        I2cState::Addr => {
            if q.extras.phase == zero_b2 || q.extras.phase == three_b2 {
                scl_drive_low = true;
            }
            let bit_val = (q.extras.addr_reg >> bits::<8>(7)) & bits::<8>(1);
            if bit_val == bits::<8>(0) {
                sda_drive_low = true;
            }
        }
        I2cState::Data => {
            if q.extras.phase == zero_b2 || q.extras.phase == three_b2 {
                scl_drive_low = true;
            }
            let bit_val = (q.extras.data_reg >> bits::<8>(7)) & bits::<8>(1);
            if bit_val == bits::<8>(0) {
                sda_drive_low = true;
            }
        }
        I2cState::AckAddr | I2cState::AckData => {
            if q.extras.phase == zero_b2 || q.extras.phase == three_b2 {
                scl_drive_low = true;
            }
        }
        I2cState::Stop => {
            if q.extras.phase == zero_b2 {
                scl_drive_low = true;
            }
            if q.extras.phase == zero_b2 || q.extras.phase == one_b2 {
                sda_drive_low = true;
            }
        }
    }

    let busy = q.state != I2cState::Idle;
    let ack_ok = q.extras.ack_addr_ok && q.extras.ack_data_ok;

    let mut o = Out::dont_care();
    o.scl_drive_low = scl_drive_low;
    o.sda_drive_low = sda_drive_low;
    o.busy = busy;
    o.done = q.extras.done_pulse;
    o.ack_ok = ack_ok;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In {
            addr: bits(0),
            data: bits(0),
            start: false,
            sda_in: true, // pull-up
        }
    }

    // Tier 2 — drive a write transaction with a "perfect slave" (always ACKs).
    #[test]
    fn test_write_transaction_completes() -> miette::Result<()> {
        let divisor = 2;
        let uut = I2cMaster::<4>::new(bits(divisor));
        // Build the input sequence.  The "slave" model: SDA pulled low
        // by master if sda_drive_low.  Otherwise pulled low by slave
        // during ACK phases (we just always say sda_in=false during Ack
        // phase 1 / phase_sub 0 sample point).
        // Easier: just compute SDA based on the master's outputs.
        // Drive start=true for the first cycle.
        // Total cycle count: idle 2 + 1+8+1+8+1 = 19 bit times × 4 phases × divisor = 152 cycles + slack.
        let n_cycles = 200;
        let mut stream_in: Vec<In> = Vec::with_capacity(n_cycles);
        for k in 0..n_cycles {
            let mut inp = idle_in();
            if k == 0 {
                inp.addr = bits(0x42);
                inp.data = bits(0x55);
                inp.start = true;
            }
            // For sda_in: simulated slave ACKs by pulling low.
            // We'll just set it low always except during the data-out phases
            // where the master is driving.  The master only samples SDA during
            // ACK phases; setting it low there means "slave ACKed".
            inp.sda_in = false;
            stream_in.push(inp);
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect::<Vec<_>>();
        let done_idx = outputs.iter().position(|s| s.output.done);
        assert!(done_idx.is_some(), "no done pulse seen");
        let final_ack = outputs[done_idx.unwrap()].output.ack_ok;
        assert!(final_ack, "expected ack_ok at done");
        Ok(())
    }

    #[test]
    fn test_idle_releases_lines() -> miette::Result<()> {
        let uut = I2cMaster::<4>::new(bits(2));
        let stream = std::iter::repeat_n(idle_in(), 32)
            .with_reset(1)
            .clock_pos_edge(100);
        let any_drive = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .any(|s| s.output.scl_drive_low || s.output.sda_drive_low);
        assert!(!any_drive);
        Ok(())
    }

    // Tier 3 — HDL emission length sanity check
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = I2cMaster::<4>::new(bits(2));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["18346"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog round-trip
    #[test]
    fn test_i2c_master_hdl_works() -> miette::Result<()> {
        let uut = I2cMaster::<4>::new(bits(2));
        let mut stream_in: Vec<In> = vec![In {
            addr: bits(0x42),
            data: bits(0x55),
            start: true,
            sda_in: false,
        }];
        for _ in 0..200 {
            let mut inp = idle_in();
            inp.sda_in = false;
            stream_in.push(inp);
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        let tm = test_bench.ntl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest
    #[test]
    fn test_i2c_master_trace() -> miette::Result<()> {
        let uut = I2cMaster::<4>::new(bits(2));
        let mut stream_in: Vec<In> = vec![In {
            addr: bits(0x42),
            data: bits(0x55),
            start: true,
            sda_in: false,
        }];
        for _ in 0..160 {
            let mut inp = idle_in();
            inp.sda_in = false;
            stream_in.push(inp);
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("i2c_master");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["f65d9f052421473af5abdd70b57471ca3db35ff74a3dd75fd0fc28935d593acc"];
        let digest = vcd.dump_to_file(root.join("i2c_master.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
