//! Modbus RTU master — full FC 0x01–0x06, 0x0F, 0x10 coverage.
//!
//! Companion to [super::modbus_rtu_slave].  Modbus is Modicon's
//! 1979 industrial-control protocol — the single most-installed
//! fieldbus on Earth.  Every PLC, HVAC supervisor, solar inverter,
//! water-treatment SCADA, and factory-automation cell speaks it.
//! Modbus RTU is the binary over-serial framing — typically 8N1
//! over RS-485 — and is the ~80 % of installed-base case (the
//! others being Modbus ASCII and Modbus TCP).
//!
//! The master:
//!
//! 1. Latches a request `(slave_addr, fc, addr, count_or_value,
//!    write_regs, write_coils)` on `start`.
//! 2. Assembles the request frame in an internal 64-byte buffer:
//!    `addr | fc | payload`, then walks the buffer accumulating
//!    a CRC-16/Modbus checksum and appends `crc_lo | crc_hi`.
//! 3. Streams the frame out one byte per `tx_ready` strobe via
//!    `tx_byte` / `tx_valid`.
//! 4. Waits for response bytes on `rx_byte` / `rx_valid`.
//! 5. Validates the response CRC and address.
//! 6. Decodes the response into typed output arrays (read_regs for
//!    FC 0x03 / 0x04, read_coils for FC 0x01 / 0x02, error +
//!    error_code for exception responses).
//! 7. Pulses `done` when finished.
//!
//! **Function-code coverage:**
//!
//! | FC   | Name                          | Direction       |
//! |------|-------------------------------|-----------------|
//! | 0x01 | Read Coils                    | master → slave  |
//! | 0x02 | Read Discrete Inputs          | master → slave  |
//! | 0x03 | Read Holding Registers        | master → slave  |
//! | 0x04 | Read Input Registers          | master → slave  |
//! | 0x05 | Write Single Coil             | master → slave  |
//! | 0x06 | Write Single Register         | master → slave  |
//! | 0x0F | Write Multiple Coils          | master → slave  |
//! | 0x10 | Write Multiple Registers      | master → slave  |
//!
//! Function codes 0x07, 0x11, 0x14, 0x15, 0x16, 0x17, 0x2B and the
//! diagnostic / encapsulated-interface codes are not implemented —
//! requesting one is a programming error (the kernel will treat it
//! as a write_multiple variant by default, which the slave will
//! reject as ILLEGAL_FUNCTION).  Cover the rare FCs in a future PR.
//!
//! Composes [super::super::core::dff::DFF] for the FSM state, the
//! transmit/receive buffers, and a bundled-extras struct holding
//! the latched request, the running CRC, and the per-FC buffer-
//! walk index — per CLAUDE.md §3.1.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +---------+ModbusRtuMaster+---------+
     |                                   |
B<8> |                                   | B<8>
+--->| slave_addr                tx_byte +--->
B<8> |                                   | bool
+--->| fc                       tx_valid +--->
B<16>|                                   | bool
+--->| addr                         busy +--->
B<16>|                                   | bool
+--->| count_or_value               done +--->
[..] |                                   | bool
+--->| write_regs                  error +--->
[..] |                                   | B<8>
+--->| write_coils            error_code +--->
bool |                                   | [..]
+--->| start                   read_regs +--->
B<8> |                                   | [..]
+--->| rx_byte                read_coils +--->
bool |                                   | B<16>
+--->| rx_valid               read_count +--->
bool |                                   |
+--->| tx_ready                          |
     +-----------------------------------+
")]
//!
//!# FSM
//!
//! 1. **Idle** — wait for `start`.  On strobe, latch all request
//!    parameters and transition to BuildReq.
//! 2. **BuildReq** — multi-cycle.  Walks `build_idx` and populates
//!    `req_buf` per FC: 0x01–0x04 / 0x05 / 0x06 are 6-byte requests
//!    (addr + fc + addr_hi + addr_lo + count_hi / value_hi +
//!    count_lo / value_lo); 0x0F adds `byte_count + ceil(count/8)
//!    bytes of packed coils; 0x10 adds `byte_count + 2*count` bytes
//!    of register data.  Walks until `req_len` is reached.
//! 3. **BuildReqCrc** — multi-cycle.  Folds each `req_buf[i]` into
//!    `running_crc` then appends `crc_lo | crc_hi` to `req_buf`.
//! 4. **Sending** — emit `req_buf[i]` on `tx_byte` / `tx_valid`,
//!    advance on `tx_ready`.  At the end, transition to RxWait.
//! 5. **RxWait** — wait for the first `rx_valid` byte from the
//!    slave.  (No timeout in this version — production users
//!    should add an external timeout that drives `cr.reset` to
//!    abort.)
//! 6. **Receiving** — accumulate bytes into `resp_buf`, fold each
//!    into `running_crc`.  Inter-frame silence (t3.5) detection
//!    transitions to ValidateResp.
//! 7. **ValidateResp** — check `running_crc == 0` and addr matches
//!    our slave_addr.  If FC has the 0x80 bit set, this is an
//!    exception response: latch `error = true` and `error_code =
//!    resp_buf[2]`.  Otherwise decode the body into read_regs /
//!    read_coils.
//! 8. **DecodeRead** — multi-cycle.  Walks the response bytes (per
//!    FC) and unpacks them into the output arrays.
//! 9. **Done** — pulse `done` and return to Idle.
//!
//!# Constants
//!
//! - This master uses no compile-time slave address; the
//!   destination address is supplied per request via [`In::slave_addr`].
//!
//!# Parameters
//!
//! - `NREG` — capacity of `write_regs` (FC 0x10 input) and
//!   `read_regs` (FC 0x03 / 0x04 output) arrays.  Both are sized
//!   `[Bits<16>; NREG]`.
//! - `NCOIL` — capacity of `write_coils` (FC 0x0F input) and
//!   `read_coils` (FC 0x01 / 0x02 output) arrays.  Both are sized
//!   `[bool; NCOIL]`.
//!
//!# Buffer size
//!
//! The transmit and receive buffers are fixed at 64 bytes each —
//! same as the slave.  This bounds the largest FC 0x10 request to
//! ~30 registers and FC 0x0F to ~480 coils.  Future: const-generic
//! the buffer size to match the slave.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/modbus_rtu_master.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/modbus_rtu_master.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/modbus_rtu_master_fsm.md")]

use rhdl::prelude::*;

use super::modbus_rtu_slave::crc16_step;
use crate::core::{constant::Constant, dff};

/// Modbus RTU request / response buffer size, bytes.  Same as the slave.
const BUF_LEN: usize = 64;

/// Special error codes returned in [`Out::error_code`] when the
/// failure isn't a Modbus exception (which uses values 0x01–0x0B per
/// the spec).  Use values >= 0x80 to keep them distinct.
pub mod error_code {
    /// CRC over the response frame did not match.
    pub const CRC_MISMATCH: u128 = 0x80;
    /// Response addr did not match the requested slave_addr.
    pub const ADDR_MISMATCH: u128 = 0x81;
}

/// Internal state machine.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum MasterState {
    /// Idle — waiting for `start`.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Multi-cycle: assemble the request payload in `req_buf`.
    #[fsm_state(label = "build req")]
    BuildReq,
    /// Multi-cycle: fold `req_buf[0..req_len]` into the running CRC,
    /// then append `crc_lo | crc_hi`.
    #[fsm_state(label = "req CRC")]
    BuildReqCrc,
    /// Multi-cycle: emit `req_buf` byte-by-byte to the UART.
    #[fsm_state(label = "send")]
    Sending,
    /// Single-cycle: wait for the first response byte from the slave.
    #[fsm_state(label = "rx wait")]
    RxWait,
    /// Multi-cycle: accumulate response bytes; t3.5 silence triggers
    /// validation.
    #[fsm_state(label = "rx")]
    Receiving,
    /// Single-cycle: validate response CRC + addr; dispatch to
    /// DecodeRead or Done (for write responses / exceptions).
    #[fsm_state(label = "validate")]
    ValidateResp,
    /// Multi-cycle: unpack the response body into read_regs / read_coils.
    #[fsm_state(label = "decode")]
    DecodeRead,
    /// Single-cycle: pulse `done` and return to Idle.
    #[fsm_state(label = "done")]
    Done,
}

/// Bundled scratch state per CLAUDE.md §3.1.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct MasterExtras {
    /// Latched slave address (target of the request).
    pub slave_addr: Bits<8>,
    /// Latched function code.
    pub fc: Bits<8>,
    /// Latched address (start_addr for reads / multi-writes; coil
    /// or register address for single-writes).
    pub addr: Bits<16>,
    /// Latched count (for reads / multi-writes) or value (for FC
    /// 0x05 / 0x06).
    pub count_or_value: Bits<16>,
    /// Length of the request being built (bytes in req_buf, not
    /// including the trailing CRC).  Set during BuildReq.
    pub req_len: Bits<8>,
    /// Length of the response received (bytes in resp_buf, not
    /// including the trailing CRC at first; updated to the full
    /// frame length once Receiving completes).
    pub resp_len: Bits<8>,
    /// Per-byte walk index for BuildReq / BuildReqCrc / Sending /
    /// Receiving / DecodeRead.
    pub build_idx: Bits<8>,
    /// Running CRC.  Used both for outgoing-frame CRC computation
    /// and incoming-frame validation.
    pub running_crc: Bits<16>,
    /// Inter-frame silence counter, in clock cycles.  Used during
    /// Receiving to detect end-of-frame.
    pub t35_counter: Bits<16>,
    /// Latched at ValidateResp: was this an exception response?
    pub error: bool,
    /// Exception code, or one of [`error_code::CRC_MISMATCH`] /
    /// [`error_code::ADDR_MISMATCH`] / 0 for no error.
    pub error_code: Bits<8>,
    /// Pulses for one cycle when a request completes.
    pub done_pulse: bool,
}

/// Modbus RTU master widget.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = MasterState, allow_implicit)]
pub struct ModbusRtuMaster<const NREG: usize, const NCOIL: usize>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    state: dff::DFF<MasterState>,
    extras: dff::DFF<MasterExtras>,
    req_buf: [dff::DFF<Bits<8>>; BUF_LEN],
    resp_buf: [dff::DFF<Bits<8>>; BUF_LEN],
    /// Latched copy of write_regs (input data for FC 0x10).  We
    /// snapshot it at `start` so the user can release the input.
    write_regs: [dff::DFF<Bits<16>>; NREG],
    /// Latched copy of write_coils (input data for FC 0x0F).
    write_coils: [dff::DFF<bool>; NCOIL],
    /// Decoded read data (for FC 0x03 / 0x04).
    read_regs: [dff::DFF<Bits<16>>; NREG],
    /// Decoded read data (for FC 0x01 / 0x02).
    read_coils: [dff::DFF<bool>; NCOIL],
    t35_threshold: Constant<Bits<16>>,
}

impl<const NREG: usize, const NCOIL: usize> ModbusRtuMaster<NREG, NCOIL>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    /// Create a master with the supplied inter-frame silence
    /// threshold (in clock cycles).
    pub fn new(t35_threshold: Bits<16>) -> Self {
        Self {
            state: dff::DFF::default(),
            extras: dff::DFF::default(),
            req_buf: array_init::array_init(|_| dff::DFF::new(bits::<8>(0))),
            resp_buf: array_init::array_init(|_| dff::DFF::new(bits::<8>(0))),
            write_regs: array_init::array_init(|_| dff::DFF::new(bits::<16>(0))),
            write_coils: array_init::array_init(|_| dff::DFF::new(false)),
            read_regs: array_init::array_init(|_| dff::DFF::new(bits::<16>(0))),
            read_coils: array_init::array_init(|_| dff::DFF::new(false)),
            t35_threshold: Constant::new(t35_threshold),
        }
    }
}

impl<const NREG: usize, const NCOIL: usize> Default for ModbusRtuMaster<NREG, NCOIL>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    fn default() -> Self {
        Self::new(bits::<16>(100))
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [ModbusRtuMaster].
pub struct In<const NREG: usize, const NCOIL: usize> {
    /// Modbus slave address (1..247 per spec; 0 = broadcast).
    pub slave_addr: Bits<8>,
    /// Function code (0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0F, 0x10).
    pub fc: Bits<8>,
    /// Starting address (for reads / multi-writes), or coil/register
    /// address (for single-writes).
    pub addr: Bits<16>,
    /// Count (for reads / multi-writes), or value (for FC 0x05 / 0x06).
    /// FC 0x05 special values: 0x0000 = OFF, 0xFF00 = ON.
    pub count_or_value: Bits<16>,
    /// Register data for FC 0x10 (Write Multiple Registers).  Only
    /// the first `count_or_value` entries are used.
    pub write_regs: [Bits<16>; NREG],
    /// Coil data for FC 0x0F (Write Multiple Coils).  Only the
    /// first `count_or_value` entries are used.
    pub write_coils: [bool; NCOIL],
    /// Strobe to begin assembling and transmitting a request.
    /// Ignored while busy.
    pub start: bool,
    /// Latest received byte from the UART.  Only meaningful while
    /// `rx_valid` is high.
    pub rx_byte: Bits<8>,
    /// One-cycle pulse: a new `rx_byte` is available this cycle.
    pub rx_valid: bool,
    /// One-cycle pulse from the downstream UART: "I consumed the
    /// previous `tx_byte`; advance to the next."
    pub tx_ready: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [ModbusRtuMaster].
pub struct Out<const NREG: usize, const NCOIL: usize> {
    /// Current byte to transmit (only meaningful while `tx_valid` is high).
    pub tx_byte: Bits<8>,
    /// `true` while a fresh `tx_byte` is available for the UART to consume.
    pub tx_valid: bool,
    /// True from `start` until `done` pulses.
    pub busy: bool,
    /// Pulses for one cycle when the request-response cycle completes.
    pub done: bool,
    /// Set in the `done` cycle if the response was an exception or
    /// failed CRC / address validation.
    pub error: bool,
    /// In the `done` cycle: 0 if no error, otherwise the Modbus
    /// exception code (0x01–0x0B) or one of the [`error_code`] values.
    pub error_code: Bits<8>,
    /// Decoded register read data (for FC 0x03 / 0x04).  Only the
    /// first `read_count` entries are valid.
    pub read_regs: [Bits<16>; NREG],
    /// Decoded coil read data (for FC 0x01 / 0x02).  Only the
    /// first `read_count` entries are valid.
    pub read_coils: [bool; NCOIL],
    /// Number of valid items in `read_regs` / `read_coils`.  Equal
    /// to the request's `count_or_value` for read FCs; 0 for write
    /// or exception responses.
    pub read_count: Bits<16>,
}

impl<const NREG: usize, const NCOIL: usize> SynchronousIO for ModbusRtuMaster<NREG, NCOIL>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    type I = In<NREG, NCOIL>;
    type O = Out<NREG, NCOIL>;
    type Kernel = modbus_rtu_master<NREG, NCOIL>;
}

#[kernel]
/// Kernel for [ModbusRtuMaster].
pub fn modbus_rtu_master<const NREG: usize, const NCOIL: usize>(
    cr: ClockReset,
    i: In<NREG, NCOIL>,
    q: Q<NREG, NCOIL>,
) -> (Out<NREG, NCOIL>, D<NREG, NCOIL>) {
    let mut d = D::<NREG, NCOIL>::dont_care();
    d.state = q.state;
    d.extras = q.extras;
    for k in 0..BUF_LEN {
        d.req_buf[k] = q.req_buf[k];
        d.resp_buf[k] = q.resp_buf[k];
    }
    for k in 0..NREG {
        d.write_regs[k] = q.write_regs[k];
        d.read_regs[k] = q.read_regs[k];
    }
    for k in 0..NCOIL {
        d.write_coils[k] = q.write_coils[k];
        d.read_coils[k] = q.read_coils[k];
    }

    let mut e = q.extras;
    e.done_pulse = false;

    let mut tx_byte: Bits<8> = bits::<8>(0);
    let mut tx_valid: bool = false;

    match q.state {
        MasterState::Idle => {
            if i.start {
                e.slave_addr = i.slave_addr;
                e.fc = i.fc;
                e.addr = i.addr;
                e.count_or_value = i.count_or_value;
                e.error = false;
                e.error_code = bits::<8>(0);
                e.build_idx = bits::<8>(0);
                e.running_crc = bits::<16>(0xFFFF);
                e.t35_counter = bits::<16>(0);
                // Snapshot write_regs / write_coils.
                for k in 0..NREG {
                    d.write_regs[k] = i.write_regs[k];
                    // Clear any prior read result.
                    d.read_regs[k] = bits::<16>(0);
                }
                for k in 0..NCOIL {
                    d.write_coils[k] = i.write_coils[k];
                    d.read_coils[k] = false;
                }
                // Set up req_buf header (addr + fc).
                d.req_buf[bits::<8>(0)] = i.slave_addr;
                d.req_buf[bits::<8>(1)] = i.fc;
                // Set req_len based on FC.
                let count_v: Bits<16> = i.count_or_value;
                if i.fc == bits::<8>(0x01)
                    || i.fc == bits::<8>(0x02)
                    || i.fc == bits::<8>(0x03)
                    || i.fc == bits::<8>(0x04)
                    || i.fc == bits::<8>(0x05)
                    || i.fc == bits::<8>(0x06)
                {
                    // 6-byte request: addr + fc + addr_hi + addr_lo + count_hi/value_hi + count_lo/value_lo
                    e.req_len = bits::<8>(6);
                } else if i.fc == bits::<8>(0x0F) {
                    // 7 + ceil(count/8) bytes.
                    let bytes16: Bits<16> = (count_v + bits::<16>(7)) >> bits::<16>(3);
                    let bytes8: Bits<8> = bytes16.resize();
                    e.req_len = bits::<8>(7) + bytes8;
                } else {
                    // 0x10: 7 + 2*count bytes.
                    let bytes16: Bits<16> = count_v << bits::<16>(1);
                    let bytes8: Bits<8> = bytes16.resize();
                    e.req_len = bits::<8>(7) + bytes8;
                }
                d.state = MasterState::BuildReq;
            }
        }
        MasterState::BuildReq => {
            // Walk build_idx to populate req_buf.  Header bytes 0,1
            // were set in Idle.  Build from index 2 onward.
            let bi: Bits<8> = q.extras.build_idx;
            let fc: Bits<8> = q.extras.fc;
            let addr: Bits<16> = q.extras.addr;
            let count_v: Bits<16> = q.extras.count_or_value;
            let req_len: Bits<8> = q.extras.req_len;
            let buf_idx: Bits<8> = bi + bits::<8>(2);

            if buf_idx >= req_len {
                // Done populating.  Move to BuildReqCrc.
                d.state = MasterState::BuildReqCrc;
                e.build_idx = bits::<8>(0);
                e.running_crc = bits::<16>(0xFFFF);
            } else {
                // Per-FC byte assignment.
                let addr_hi: Bits<8> = (addr >> bits::<16>(8)).resize();
                let addr_lo: Bits<8> = (addr & bits::<16>(0xFF)).resize();
                let count_hi: Bits<8> = (count_v >> bits::<16>(8)).resize();
                let count_lo: Bits<8> = (count_v & bits::<16>(0xFF)).resize();

                if bi == bits::<8>(0) {
                    d.req_buf[buf_idx] = addr_hi;
                } else if bi == bits::<8>(1) {
                    d.req_buf[buf_idx] = addr_lo;
                } else if bi == bits::<8>(2) {
                    d.req_buf[buf_idx] = count_hi;
                } else if bi == bits::<8>(3) {
                    d.req_buf[buf_idx] = count_lo;
                } else if bi == bits::<8>(4) {
                    // Byte_count for multi-write FCs (0x0F, 0x10).
                    if fc == bits::<8>(0x0F) {
                        let bytes16: Bits<16> = (count_v + bits::<16>(7)) >> bits::<16>(3);
                        d.req_buf[buf_idx] = bytes16.resize();
                    } else if fc == bits::<8>(0x10) {
                        let bytes16: Bits<16> = count_v << bits::<16>(1);
                        d.req_buf[buf_idx] = bytes16.resize();
                    }
                    // Unreachable for 0x01..0x06 (req_len = 6, so we never
                    // get here with bi=4 because buf_idx=6 >= 6).
                } else {
                    // bi >= 5: data bytes for multi-write FCs.
                    let data_idx: Bits<8> = bi - bits::<8>(5);
                    if fc == bits::<8>(0x0F) {
                        // Pack write_coils[data_idx*8..(data_idx+1)*8] LSB-first.
                        let coil_base: Bits<16> = (data_idx.resize()) << bits::<16>(3);
                        let mut packed: Bits<8> = bits::<8>(0);
                        for b in 0..8 {
                            let coil_off: Bits<16> = bits::<16>(b as u128);
                            let coil_idx: Bits<16> = coil_base + coil_off;
                            let in_range: bool = coil_idx < count_v;
                            let safe_idx: Bits<16> = if in_range { coil_idx } else { bits::<16>(0) };
                            let bit_val: bool = q.write_coils[safe_idx];
                            if in_range && bit_val {
                                packed = packed | (bits::<8>(1) << bits::<8>(b as u128));
                            }
                        }
                        d.req_buf[buf_idx] = packed;
                    } else {
                        // 0x10: each register fills 2 data bytes (hi, lo).
                        let reg_idx16: Bits<16> = (data_idx >> bits::<8>(1)).resize();
                        let in_range: bool = reg_idx16 < count_v;
                        let safe_idx: Bits<16> = if in_range { reg_idx16 } else { bits::<16>(0) };
                        let reg_val: Bits<16> = q.write_regs[safe_idx];
                        let is_hi: bool = (data_idx & bits::<8>(1)) == bits::<8>(0);
                        let byte_val: Bits<8> = if is_hi {
                            (reg_val >> bits::<16>(8)).resize()
                        } else {
                            (reg_val & bits::<16>(0xFF)).resize()
                        };
                        d.req_buf[buf_idx] = byte_val;
                    }
                }
                e.build_idx = bi + bits::<8>(1);
            }
        }
        MasterState::BuildReqCrc => {
            // Fold req_buf[0..req_len] into running_crc, then append.
            let bi: Bits<8> = q.extras.build_idx;
            if bi >= q.extras.req_len {
                // Done.  Append crc_lo, crc_hi.
                let crc_lo: Bits<8> = (q.extras.running_crc & bits::<16>(0xFF)).resize();
                let crc_hi: Bits<8> = (q.extras.running_crc >> bits::<16>(8)).resize();
                d.req_buf[q.extras.req_len] = crc_lo;
                d.req_buf[q.extras.req_len + bits::<8>(1)] = crc_hi;
                e.req_len = q.extras.req_len + bits::<8>(2);
                e.build_idx = bits::<8>(0);
                d.state = MasterState::Sending;
            } else {
                let byte: Bits<8> = q.req_buf[bi];
                e.running_crc = crc16_step(q.extras.running_crc, byte);
                e.build_idx = bi + bits::<8>(1);
            }
        }
        MasterState::Sending => {
            let bi: Bits<8> = q.extras.build_idx;
            tx_byte = q.req_buf[bi];
            tx_valid = true;
            if i.tx_ready {
                if bi + bits::<8>(1) >= q.extras.req_len {
                    // Last byte just consumed.  Move to RxWait.
                    e.build_idx = bits::<8>(0);
                    e.running_crc = bits::<16>(0xFFFF);
                    e.resp_len = bits::<8>(0);
                    e.t35_counter = bits::<16>(0);
                    d.state = MasterState::RxWait;
                } else {
                    e.build_idx = bi + bits::<8>(1);
                }
            }
        }
        MasterState::RxWait => {
            if i.rx_valid {
                // First response byte arrived.
                d.resp_buf[bits::<8>(0)] = i.rx_byte;
                e.running_crc = crc16_step(bits::<16>(0xFFFF), i.rx_byte);
                e.resp_len = bits::<8>(1);
                e.t35_counter = bits::<16>(0);
                d.state = MasterState::Receiving;
            }
        }
        MasterState::Receiving => {
            if i.rx_valid {
                let resp_idx: Bits<8> = q.extras.resp_len;
                if (resp_idx.raw() as usize) < BUF_LEN {
                    d.resp_buf[resp_idx] = i.rx_byte;
                    e.resp_len = q.extras.resp_len + bits::<8>(1);
                }
                e.running_crc = crc16_step(q.extras.running_crc, i.rx_byte);
                e.t35_counter = bits::<16>(0);
            } else {
                if q.extras.t35_counter >= q.t35_threshold {
                    // End of frame.
                    e.build_idx = bits::<8>(0);
                    d.state = MasterState::ValidateResp;
                } else {
                    e.t35_counter = q.extras.t35_counter + bits::<16>(1);
                }
            }
        }
        MasterState::ValidateResp => {
            // Check CRC, addr, exception bit.
            let crc_ok: bool = q.extras.running_crc == bits::<16>(0);
            let resp_addr: Bits<8> = q.resp_buf[bits::<8>(0)];
            let addr_match: bool = resp_addr == q.extras.slave_addr;
            let resp_fc: Bits<8> = q.resp_buf[bits::<8>(1)];
            let is_exception: bool = (resp_fc & bits::<8>(0x80)) != bits::<8>(0);

            if !crc_ok {
                e.error = true;
                e.error_code = bits::<8>(0x80); // CRC_MISMATCH
                d.state = MasterState::Done;
            } else if !addr_match {
                e.error = true;
                e.error_code = bits::<8>(0x81); // ADDR_MISMATCH
                d.state = MasterState::Done;
            } else if is_exception {
                e.error = true;
                e.error_code = q.resp_buf[bits::<8>(2)];
                d.state = MasterState::Done;
            } else {
                let fc: Bits<8> = q.extras.fc;
                if fc == bits::<8>(0x01)
                    || fc == bits::<8>(0x02)
                    || fc == bits::<8>(0x03)
                    || fc == bits::<8>(0x04)
                {
                    // Read response: walk body and decode.
                    e.build_idx = bits::<8>(0);
                    d.state = MasterState::DecodeRead;
                } else {
                    // Write response — nothing more to do.
                    d.state = MasterState::Done;
                }
            }
        }
        MasterState::DecodeRead => {
            // Walk count_or_value items, unpacking them from
            // resp_buf[3..] into read_regs / read_coils.
            let bi: Bits<8> = q.extras.build_idx;
            let fc: Bits<8> = q.extras.fc;
            let count_v: Bits<16> = q.extras.count_or_value;
            let bi16: Bits<16> = bi.resize();

            if bi16 >= count_v {
                d.state = MasterState::Done;
            } else if fc == bits::<8>(0x01) || fc == bits::<8>(0x02) {
                // Coil read: each bit packed LSB-first.
                let byte_off: Bits<16> = bi16 >> bits::<16>(3);
                let bit_off: Bits<16> = bi16 & bits::<16>(7);
                let src_idx: Bits<8> = bits::<8>(3) + byte_off.resize();
                let src_byte: Bits<8> = q.resp_buf[src_idx];
                let bit_off8: Bits<8> = bit_off.resize();
                let mask: Bits<8> = bits::<8>(1) << bit_off8;
                let bit_val: bool = (src_byte & mask) != bits::<8>(0);
                d.read_coils[bi16] = bit_val;
                e.build_idx = bi + bits::<8>(1);
            } else {
                // 0x03 / 0x04: register read.  Each register is 2 bytes (hi, lo).
                let src_idx: Bits<8> = bits::<8>(3) + (bi << bits::<8>(1));
                let hi: Bits<8> = q.resp_buf[src_idx];
                let lo: Bits<8> = q.resp_buf[src_idx + bits::<8>(1)];
                let hi16: Bits<16> = hi.resize();
                let lo16: Bits<16> = lo.resize();
                let val: Bits<16> = (hi16 << bits::<16>(8)) | lo16;
                d.read_regs[bi16] = val;
                e.build_idx = bi + bits::<8>(1);
            }
        }
        MasterState::Done => {
            e.done_pulse = true;
            d.state = MasterState::Idle;
        }
    }

    if cr.reset.any() {
        d.state = MasterState::Idle;
        e = MasterExtras::default();
        for k in 0..BUF_LEN {
            d.req_buf[k] = bits::<8>(0);
            d.resp_buf[k] = bits::<8>(0);
        }
        for k in 0..NREG {
            d.write_regs[k] = bits::<16>(0);
            d.read_regs[k] = bits::<16>(0);
        }
        for k in 0..NCOIL {
            d.write_coils[k] = false;
            d.read_coils[k] = false;
        }
    }

    d.extras = e;

    let mut o = Out::<NREG, NCOIL>::dont_care();
    o.tx_byte = tx_byte;
    o.tx_valid = tx_valid;
    o.busy = q.state != MasterState::Idle;
    o.done = q.extras.done_pulse;
    o.error = q.extras.error;
    o.error_code = q.extras.error_code;
    for k in 0..NREG {
        o.read_regs[k] = q.read_regs[k];
    }
    for k in 0..NCOIL {
        o.read_coils[k] = q.read_coils[k];
    }
    o.read_count = if q.extras.fc == bits::<8>(0x01)
        || q.extras.fc == bits::<8>(0x02)
        || q.extras.fc == bits::<8>(0x03)
        || q.extras.fc == bits::<8>(0x04)
    {
        q.extras.count_or_value
    } else {
        bits::<16>(0)
    };
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    /// Reference Modbus CRC-16 (polynomial 0xA001).
    fn ref_crc(payload: &[u8]) -> u16 {
        let mut crc: u16 = 0xFFFF;
        for &b in payload {
            crc ^= b as u16;
            for _ in 0..8 {
                if crc & 1 == 1 {
                    crc = (crc >> 1) ^ 0xA001;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc
    }

    fn idle_in<const NREG: usize, const NCOIL: usize>() -> In<NREG, NCOIL> {
        In {
            slave_addr: bits(0),
            fc: bits(0),
            addr: bits(0),
            count_or_value: bits(0),
            write_regs: [bits(0); NREG],
            write_coils: [false; NCOIL],
            start: false,
            rx_byte: bits(0),
            rx_valid: false,
            tx_ready: false,
        }
    }

    /// Drive a single full request cycle:
    /// 1. Strobe `start`.
    /// 2. Drain TX (16 tx_ready strobes).
    /// 3. Push the response back as rx_valid bytes.
    /// 4. Idle until done.
    fn drive_request<const NREG: usize, const NCOIL: usize>(
        slave_addr: u8,
        fc: u8,
        addr: u16,
        count_or_value: u16,
        write_regs: [Bits<16>; NREG],
        write_coils: [bool; NCOIL],
        response: &[u8],
    ) -> Vec<In<NREG, NCOIL>> {
        let mut out: Vec<In<NREG, NCOIL>> = Vec::new();
        // Strobe start.
        out.push(In {
            slave_addr: bits(slave_addr as u128),
            fc: bits(fc as u128),
            addr: bits(addr as u128),
            count_or_value: bits(count_or_value as u128),
            write_regs,
            write_coils,
            start: true,
            rx_byte: bits(0),
            rx_valid: false,
            tx_ready: false,
        });
        // Idle for many cycles to let BuildReq + BuildReqCrc run.
        for _ in 0..200 {
            out.push(idle_in::<NREG, NCOIL>());
        }
        // Drain TX.
        for _ in 0..32 {
            out.push(In {
                tx_ready: true,
                ..idle_in::<NREG, NCOIL>()
            });
            out.push(idle_in::<NREG, NCOIL>());
        }
        // Now push response bytes.
        for &b in response {
            out.push(In {
                rx_byte: bits(b as u128),
                rx_valid: true,
                ..idle_in::<NREG, NCOIL>()
            });
            out.push(idle_in::<NREG, NCOIL>());
        }
        // Wait for t3.5 silence + decode.
        for _ in 0..400 {
            out.push(idle_in::<NREG, NCOIL>());
        }
        out
    }

    /// Run and capture (tx_bytes_emitted, final_out_at_done).
    fn run_request<const NREG: usize, const NCOIL: usize>(
        uut: &ModbusRtuMaster<NREG, NCOIL>,
        stream_in: Vec<In<NREG, NCOIL>>,
    ) -> (Vec<u8>, Option<Out<NREG, NCOIL>>)
    where
        [(); NREG]: Sized,
        [(); NCOIL]: Sized,
    {
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let tx: Vec<u8> = outputs
            .iter()
            .filter(|s| s.output.tx_valid && s.input.1.tx_ready)
            .map(|s| s.output.tx_byte.raw() as u8)
            .collect();
        let done_out = outputs
            .iter()
            .find(|s| s.output.done)
            .map(|s| s.output);
        (tx, done_out)
    }

    fn build_response(addr: u8, fc: u8, body: &[u8]) -> Vec<u8> {
        let mut frame = vec![addr, fc];
        frame.extend_from_slice(body);
        let crc = ref_crc(&frame);
        frame.push((crc & 0xFF) as u8);
        frame.push((crc >> 8) as u8);
        frame
    }

    #[test]
    fn test_idle_no_tx_valid() -> miette::Result<()> {
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let stream = std::iter::repeat_n(idle_in::<8, 8>(), 100)
            .with_reset(1)
            .clock_pos_edge(100);
        let any = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .any(|s| s.output.tx_valid);
        assert!(!any);
        Ok(())
    }

    #[test]
    fn test_fc03_request_frame() -> miette::Result<()> {
        // FC 0x03: read 5 holding registers from slave 1 starting at addr 0.
        // Canonical Modbus example: 01 03 00 00 00 05 85 0A.
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = build_response(0x01, 0x03, &[0x0A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let stream_in =
            drive_request::<8, 8>(0x01, 0x03, 0x0000, 5, [bits(0); 8], [false; 8], &response);
        let (tx, done) = run_request(&uut, stream_in);
        let mut expected = vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x05];
        let crc = ref_crc(&expected);
        expected.push((crc & 0xFF) as u8);
        expected.push((crc >> 8) as u8);
        assert_eq!(tx, expected, "request frame mismatch");
        let done = done.expect("no done pulse");
        assert!(!done.error, "unexpected error: {:?}", done.error_code);
        Ok(())
    }

    #[test]
    fn test_fc06_request_frame() -> miette::Result<()> {
        // FC 0x06: write reg[5] = 0x1234.  Slave echo response.
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = build_response(0x01, 0x06, &[0x00, 0x05, 0x12, 0x34]);
        let stream_in = drive_request::<8, 8>(
            0x01,
            0x06,
            0x0005,
            0x1234,
            [bits(0); 8],
            [false; 8],
            &response,
        );
        let (tx, done) = run_request(&uut, stream_in);
        let mut expected = vec![0x01, 0x06, 0x00, 0x05, 0x12, 0x34];
        let crc = ref_crc(&expected);
        expected.push((crc & 0xFF) as u8);
        expected.push((crc >> 8) as u8);
        assert_eq!(tx, expected);
        let done = done.expect("no done pulse");
        assert!(!done.error);
        Ok(())
    }

    #[test]
    fn test_fc10_request_frame() -> miette::Result<()> {
        // FC 0x10: write 3 registers starting at 0 with [0xAAAA, 0xBBBB, 0xCCCC].
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let mut wr: [Bits<16>; 8] = [bits(0); 8];
        wr[0] = bits(0xAAAA);
        wr[1] = bits(0xBBBB);
        wr[2] = bits(0xCCCC);
        let response = build_response(0x01, 0x10, &[0x00, 0x00, 0x00, 0x03]);
        let stream_in = drive_request::<8, 8>(0x01, 0x10, 0x0000, 3, wr, [false; 8], &response);
        let (tx, done) = run_request(&uut, stream_in);
        let mut expected = vec![
            0x01, 0x10, 0x00, 0x00, 0x00, 0x03, 0x06, 0xAA, 0xAA, 0xBB, 0xBB, 0xCC, 0xCC,
        ];
        let crc = ref_crc(&expected);
        expected.push((crc & 0xFF) as u8);
        expected.push((crc >> 8) as u8);
        assert_eq!(tx, expected);
        let done = done.expect("no done pulse");
        assert!(!done.error);
        Ok(())
    }

    #[test]
    fn test_fc03_response_decode() -> miette::Result<()> {
        // FC 0x03: read 3 regs.  Slave response includes 3 regs of data.
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = build_response(
            0x01,
            0x03,
            &[0x06, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
        );
        let stream_in =
            drive_request::<8, 8>(0x01, 0x03, 0x0000, 3, [bits(0); 8], [false; 8], &response);
        let (_, done) = run_request(&uut, stream_in);
        let done = done.expect("no done pulse");
        assert!(!done.error);
        assert_eq!(done.read_count.raw(), 3);
        assert_eq!(done.read_regs[0].raw(), 0x1122);
        assert_eq!(done.read_regs[1].raw(), 0x3344);
        assert_eq!(done.read_regs[2].raw(), 0x5566);
        Ok(())
    }

    #[test]
    fn test_fc01_response_decode() -> miette::Result<()> {
        // FC 0x01: read 5 coils.  Slave response: byte 0b10110 = coils [0,1,1,0,1].
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = build_response(0x01, 0x01, &[0x01, 0b10110]);
        let stream_in =
            drive_request::<8, 8>(0x01, 0x01, 0x0000, 5, [bits(0); 8], [false; 8], &response);
        let (_, done) = run_request(&uut, stream_in);
        let done = done.expect("no done pulse");
        assert!(!done.error);
        assert_eq!(done.read_count.raw(), 5);
        assert!(!done.read_coils[0]);
        assert!(done.read_coils[1]);
        assert!(done.read_coils[2]);
        assert!(!done.read_coils[3]);
        assert!(done.read_coils[4]);
        Ok(())
    }

    #[test]
    fn test_exception_response() -> miette::Result<()> {
        // FC 0x03 returns exception 0x02 (ILLEGAL_DATA_ADDRESS).
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = build_response(0x01, 0x83, &[0x02]);
        let stream_in =
            drive_request::<8, 8>(0x01, 0x03, 0x00FF, 1, [bits(0); 8], [false; 8], &response);
        let (_, done) = run_request(&uut, stream_in);
        let done = done.expect("no done pulse");
        assert!(done.error);
        assert_eq!(done.error_code.raw(), 0x02);
        Ok(())
    }

    #[test]
    fn test_bad_crc_response() -> miette::Result<()> {
        // Response with deliberately wrong CRC.
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = vec![0x01, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00];
        let stream_in =
            drive_request::<8, 8>(0x01, 0x03, 0x0000, 1, [bits(0); 8], [false; 8], &response);
        let (_, done) = run_request(&uut, stream_in);
        let done = done.expect("no done pulse");
        assert!(done.error);
        assert_eq!(done.error_code.raw(), 0x80); // CRC_MISMATCH
        Ok(())
    }

    #[test]
    fn test_fc0f_request_frame() -> miette::Result<()> {
        // FC 0x0F: write 5 coils starting at 0 = [0,1,1,0,1] = 0b10110.
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let mut wc: [bool; 8] = [false; 8];
        wc[1] = true;
        wc[2] = true;
        wc[4] = true;
        let response = build_response(0x01, 0x0F, &[0x00, 0x00, 0x00, 0x05]);
        let stream_in = drive_request::<8, 8>(0x01, 0x0F, 0x0000, 5, [bits(0); 8], wc, &response);
        let (tx, done) = run_request(&uut, stream_in);
        let mut expected = vec![0x01, 0x0F, 0x00, 0x00, 0x00, 0x05, 0x01, 0b10110];
        let crc = ref_crc(&expected);
        expected.push((crc & 0xFF) as u8);
        expected.push((crc >> 8) as u8);
        assert_eq!(tx, expected);
        let done = done.expect("no done pulse");
        assert!(!done.error);
        Ok(())
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["380775"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    #[test]
    fn test_modbus_rtu_master_hdl_works() -> miette::Result<()> {
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = build_response(0x01, 0x03, &[0x02, 0x12, 0x34]);
        let stream_in =
            drive_request::<8, 8>(0x01, 0x03, 0x0000, 1, [bits(0); 8], [false; 8], &response);
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_modbus_rtu_master_trace() -> miette::Result<()> {
        let uut: ModbusRtuMaster<8, 8> = ModbusRtuMaster::default();
        let response = build_response(0x01, 0x06, &[0x00, 0x02, 0xCA, 0xFE]);
        let stream_in = drive_request::<8, 8>(
            0x01,
            0x06,
            0x0002,
            0xCAFE,
            [bits(0); 8],
            [false; 8],
            &response,
        );
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("modbus_rtu_master");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["d2d93563f055466de19609e847af3e6eadbeddd8e0031afe522207ae40b59e25"];
        let digest = vcd
            .dump_to_file(root.join("modbus_rtu_master.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    // ===========================================================
    // Closed-loop round-trip tests — wire master.tx_byte/tx_valid
    // into the slave's rx_byte/rx_valid and the slave's tx back to
    // the master.  This validates that the master and slave agree
    // on the wire format end-to-end (not just via the shared
    // `ref_crc` test helper).
    // ===========================================================

    use crate::serial_bus::modbus_rtu_slave::{
        In as SlaveIn, ModbusRtuSlave, Out as SlaveOut,
    };

    /// Run a full request-response round-trip: master starts a
    /// request, master TX bytes flow into slave RX, slave processes
    /// + responds, slave TX bytes flow back to master RX, master
    /// decodes.  Returns `(tx_master_bytes, slave_holding_regs,
    /// slave_coils, master_done_out)`.  The "wire" model is naive:
    /// every cycle, if either side has `tx_valid` it's asserted on
    /// the other side's `rx_valid` AND the sender's `tx_ready` is
    /// pulsed — i.e. one byte per cycle, instantaneous.
    fn run_round_trip<const NREG: usize, const NCOIL: usize>(
        slave_addr: u8,
        fc: u8,
        addr: u16,
        count_or_value: u16,
        write_regs: [Bits<16>; NREG],
        write_coils: [bool; NCOIL],
        slave_input_regs: [Bits<16>; NREG],
        slave_discrete_inputs: [bool; NCOIL],
        n_cycles: usize,
    ) -> (
        Vec<u8>,
        Option<Out<NREG, NCOIL>>,
        Option<SlaveOut<NREG, NCOIL>>,
    )
    where
        [(); NREG]: Sized,
        [(); NCOIL]: Sized,
    {
        // Build master + slave streams that depend on each other.
        // Step them in lockstep, feeding outputs back as inputs.
        let mut stream_m: Vec<In<NREG, NCOIL>> = Vec::with_capacity(n_cycles + 4);
        let mut stream_s: Vec<SlaveIn<NREG, NCOIL>> = Vec::with_capacity(n_cycles + 4);

        // Two reset cycles + then the start strobe.
        for _ in 0..2 {
            stream_m.push(idle_in::<NREG, NCOIL>());
            stream_s.push(SlaveIn {
                rx_byte: bits(0),
                rx_valid: false,
                tx_ready: true,
                input_regs: slave_input_regs,
                discrete_inputs: slave_discrete_inputs,
            });
        }

        // Use the standard `run` API on each, but generate the
        // streams *one cycle at a time* using a simple state-tracking
        // wire: we run master with a stream that has its rx fed from
        // the slave's previous tx (1-cycle delay), and vice versa.
        // To do this without manually instantiating Q states, we use
        // a two-pass approach: first run master with no slave
        // response, capture its tx; then run slave with master's tx
        // as rx, capture its tx; then run master again with slave's
        // tx as rx, get the done state.

        // ------ Pass 1: run master alone, capture tx_master ------
        let mut s_m: Vec<In<NREG, NCOIL>> = Vec::new();
        s_m.push(In {
            slave_addr: bits(slave_addr as u128),
            fc: bits(fc as u128),
            addr: bits(addr as u128),
            count_or_value: bits(count_or_value as u128),
            write_regs,
            write_coils,
            start: true,
            rx_byte: bits(0),
            rx_valid: false,
            tx_ready: true,
        });
        for _ in 0..n_cycles {
            s_m.push(In {
                tx_ready: true,
                ..idle_in::<NREG, NCOIL>()
            });
        }
        let master_p1: ModbusRtuMaster<NREG, NCOIL> = ModbusRtuMaster::default();
        let stream_p1 = s_m.clone().into_iter().with_reset(1).clock_pos_edge(100);
        let outputs_m1: Vec<_> = master_p1
            .run(stream_p1)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        // Capture tx bytes per cycle (with cycle index).
        let tx_m_pairs: Vec<(usize, u8)> = outputs_m1
            .iter()
            .enumerate()
            .filter(|(_, s)| s.output.tx_valid && s.input.1.tx_ready)
            .map(|(i, s)| (i, s.output.tx_byte.raw() as u8))
            .collect();
        let tx_m: Vec<u8> = tx_m_pairs.iter().map(|(_, b)| *b).collect();

        // ------ Pass 2: feed master's tx to slave, capture its tx ------
        // Find the cycle index of the *last* master byte.
        let last_m_cycle = tx_m_pairs.last().map(|(i, _)| *i).unwrap_or(0);

        // Build slave input stream: at each cycle, present rx_byte =
        // master_tx if that cycle is in the tx_m_pairs, else idle.
        let mut s_s: Vec<SlaveIn<NREG, NCOIL>> = Vec::with_capacity(n_cycles);
        let mut tx_iter = tx_m_pairs.iter().peekable();
        for cyc in 0..n_cycles {
            let rx_now = tx_iter.peek().is_some_and(|(i, _)| *i == cyc);
            let (rx_byte, rx_valid) = if rx_now {
                let (_, b) = tx_iter.next().unwrap();
                (bits(*b as u128), true)
            } else {
                (bits(0), false)
            };
            s_s.push(SlaveIn {
                rx_byte,
                rx_valid,
                tx_ready: true,
                input_regs: slave_input_regs,
                discrete_inputs: slave_discrete_inputs,
            });
        }
        let slave_uut: ModbusRtuSlave<NREG, NCOIL> = ModbusRtuSlave::default();
        let stream_s = s_s.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs_s: Vec<_> = slave_uut
            .run(stream_s)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let tx_s_pairs: Vec<(usize, u8)> = outputs_s
            .iter()
            .enumerate()
            .filter(|(_, s)| s.output.tx_valid && s.input.1.tx_ready)
            .map(|(i, s)| (i, s.output.tx_byte.raw() as u8))
            .collect();
        let slave_done_out = outputs_s
            .iter()
            .find(|s| s.output.resp_done)
            .map(|s| s.output);

        // ------ Pass 3: feed slave's tx back to master, get done ------
        // Build a fresh master stream with rx_byte from slave's tx.
        let mut s_m2: Vec<In<NREG, NCOIL>> = s_m.clone();
        // Extend if needed.
        while s_m2.len() < n_cycles {
            s_m2.push(In {
                tx_ready: true,
                ..idle_in::<NREG, NCOIL>()
            });
        }
        // We need to inject slave's tx bytes into master's rx at the
        // *correct cycles*.  A real wire would be one-cycle delayed
        // from when the slave asserted tx_valid.  Inject at the same
        // cycle as the slave emitted (as if the wire is zero-delay).
        for &(cyc, b) in &tx_s_pairs {
            if cyc < s_m2.len() {
                s_m2[cyc].rx_byte = bits(b as u128);
                s_m2[cyc].rx_valid = true;
            }
        }
        let master_p3: ModbusRtuMaster<NREG, NCOIL> = ModbusRtuMaster::default();
        let stream_p3 = s_m2.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs_m3: Vec<_> = master_p3
            .run(stream_p3)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        let master_done_out = outputs_m3
            .iter()
            .find(|s| s.output.done)
            .map(|s| s.output);

        let _ = last_m_cycle;
        let _ = stream_m;
        let _ = stream_s;
        (tx_m, master_done_out, slave_done_out)
    }

    #[test]
    fn test_round_trip_fc06_write_single_register() -> miette::Result<()> {
        // Master writes reg[2] = 0xCAFE; slave receives, applies,
        // echoes the request.  Master decodes the response and
        // pulses done with no error.
        let (tx_m, m_done, s_done) =
            run_round_trip::<8, 8>(1, 0x06, 0x0002, 0xCAFE, [bits(0); 8], [false; 8], [bits(0); 8], [false; 8], 600);
        // Verify the master sent the canonical FC 0x06 frame.
        let mut expected_req = vec![0x01, 0x06, 0x00, 0x02, 0xCA, 0xFE];
        let crc = ref_crc(&expected_req);
        expected_req.push((crc & 0xFF) as u8);
        expected_req.push((crc >> 8) as u8);
        assert_eq!(tx_m, expected_req, "master TX mismatch");
        // Slave should have responded.
        let s_done = s_done.expect("slave did not produce a response");
        assert_eq!(s_done.holding_regs[2].raw(), 0xCAFE, "slave didn't apply write");
        // Master should have decoded the response and pulsed done.
        let m_done = m_done.expect("master did not pulse done");
        assert!(!m_done.error, "master saw error: code 0x{:02x}", m_done.error_code.raw());
        Ok(())
    }

    #[test]
    fn test_round_trip_fc03_read_holding_registers() -> miette::Result<()> {
        // Master reads regs[0..3].  Slave responds with all-zero
        // (default).  Master decodes; read_regs should be zeros.
        let (tx_m, m_done, s_done) =
            run_round_trip::<8, 8>(1, 0x03, 0x0000, 3, [bits(0); 8], [false; 8], [bits(0); 8], [false; 8], 600);
        let mut expected_req = vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x03];
        let crc = ref_crc(&expected_req);
        expected_req.push((crc & 0xFF) as u8);
        expected_req.push((crc >> 8) as u8);
        assert_eq!(tx_m, expected_req);
        assert!(s_done.is_some(), "slave produced no response");
        let m_done = m_done.expect("master no done");
        assert!(!m_done.error);
        assert_eq!(m_done.read_count.raw(), 3);
        for i in 0..3 {
            assert_eq!(m_done.read_regs[i].raw(), 0, "reg {} not zero", i);
        }
        Ok(())
    }

    #[test]
    fn test_round_trip_fc10_write_multiple_registers() -> miette::Result<()> {
        let mut wr: [Bits<16>; 8] = [bits(0); 8];
        wr[0] = bits(0x1111);
        wr[1] = bits(0x2222);
        wr[2] = bits(0x3333);
        let (tx_m, m_done, s_done) =
            run_round_trip::<8, 8>(1, 0x10, 0x0000, 3, wr, [false; 8], [bits(0); 8], [false; 8], 600);
        let mut expected_req = vec![
            0x01, 0x10, 0x00, 0x00, 0x00, 0x03, 0x06, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33,
        ];
        let crc = ref_crc(&expected_req);
        expected_req.push((crc & 0xFF) as u8);
        expected_req.push((crc >> 8) as u8);
        assert_eq!(tx_m, expected_req);
        let s_done = s_done.expect("slave did not respond");
        assert_eq!(s_done.holding_regs[0].raw(), 0x1111);
        assert_eq!(s_done.holding_regs[1].raw(), 0x2222);
        assert_eq!(s_done.holding_regs[2].raw(), 0x3333);
        let m_done = m_done.expect("master no done");
        assert!(!m_done.error);
        Ok(())
    }

    #[test]
    fn test_round_trip_exception_for_oor_read() -> miette::Result<()> {
        // Master reads addr 99 from slave with NREG=8 → exception 0x02.
        let (_, m_done, _) =
            run_round_trip::<8, 8>(1, 0x03, 99, 1, [bits(0); 8], [false; 8], [bits(0); 8], [false; 8], 600);
        let m_done = m_done.expect("no done");
        assert!(m_done.error, "master should see exception");
        assert_eq!(m_done.error_code.raw(), 0x02);
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = ModbusRtuMaster::<8, 8>::fsm_descriptor();
        assert_eq!(desc.widget_name, "ModbusRtuMaster");
        assert_eq!(desc.variants().len(), 9);
        assert_eq!(desc.initial_index(), 0);
    }
}
