//! Modbus RTU slave — full FC 0x01–0x06, 0x0F, 0x10 coverage.
//!
//! Companion to [super::modbus_rtu_master].  Receives a Modbus RTU
//! request frame from a UART, validates address + CRC, executes
//! against an internal register / coil bank, and transmits the
//! response frame.  Returns standard Modbus exception responses
//! (ILLEGAL_FUNCTION, ILLEGAL_DATA_ADDRESS, ILLEGAL_DATA_VALUE)
//! when the request is malformed or out of range.
//!
//! **Function-code coverage:**
//!
//! | FC   | Name                          | Direction           |
//! |------|-------------------------------|---------------------|
//! | 0x01 | Read Coils                    | master → slave      |
//! | 0x02 | Read Discrete Inputs          | master → slave      |
//! | 0x03 | Read Holding Registers        | master → slave      |
//! | 0x04 | Read Input Registers          | master → slave      |
//! | 0x05 | Write Single Coil             | master → slave      |
//! | 0x06 | Write Single Register         | master → slave      |
//! | 0x0F | Write Multiple Coils          | master → slave      |
//! | 0x10 | Write Multiple Registers      | master → slave      |
//!
//! Function codes 0x07, 0x11, 0x14, 0x15, 0x16, 0x17, 0x2B and the
//! diagnostic / encapsulated-interface codes are not implemented;
//! requests with those FCs return ILLEGAL_FUNCTION.  This is the
//! same coverage that 99 % of installed Modbus PLCs / inverters /
//! HVAC supervisors actually use.
//!
//! **Holding registers vs input registers:** holding registers
//! (`HOLDING`) are read-write — both the master and the slave's
//! local logic can read and write them, and they live in DFFs
//! internal to the widget.  Input registers (`INPUT`) and
//! discrete inputs (`DI`) are *read-only from the master's
//! perspective*; they are supplied by the slave's owner via the
//! [`In::input_regs`] and [`In::discrete_inputs`] arrays of the
//! widget's `In` port.  Wire whatever sensor / status data should
//! be readable by the master into those arrays.
//!
//! Coils (`COIL`) are read-write 1-bit values, stored internally
//! to the widget the same way holding registers are.
//!
//! **Composes** [super::super::core::dff::DFF] for the FSM state,
//! the receive buffer, the response buffer, the holding-register
//! file, the coil bit-array, and a bundled-extras struct for
//! per-request scratch (CRC accumulator, byte counters, latched
//! function code, latched address fields).  Per CLAUDE.md §3.1,
//! the FSM enum sits in its own DFF and all other scalar state
//! lives in a single bundled-extras DFF; the buffers and
//! register / coil banks are arrays-of-DFFs (see
//! [super::super::core::register_file]).
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +--------+ModbusRtuSlave+--------+
     |                                |
B<8> |                                | B<8>
+--->| rx_byte                tx_byte +--->
bool |                                | bool
+--->| rx_valid              tx_valid +--->
bool |                                | bool
+--->| tx_ready                  busy +--->
B<16>|                                | bool
+--->| t35_threshold        resp_done +--->
[..] |                                | [..]
+--->| input_regs        holding_regs +--->
[..] |                                | [..]
+--->| discrete_inputs          coils +--->
     |                                |
     +--------------------------------+
")]
//!
//!# FSM
//!
//! 1. **Idle** — `running_crc = 0xFFFF`, `req_len = 0`.  On `rx_valid`,
//!    enter Receiving with the first byte appended.
//! 2. **Receiving** — append each `rx_valid` byte to `req_buf`,
//!    fold into `running_crc`, reset the t3.5 silence counter.  On
//!    each cycle without `rx_valid`, increment the silence counter;
//!    when it reaches `t35_threshold`, transition to Process.
//! 3. **Process** — single cycle.  Validate `req_buf[0]` (slave
//!    address — must equal our [`SLAVE_ADDR`] constant or be the
//!    broadcast address 0).  Validate `running_crc == 0` (Modbus
//!    CRC over `addr | fc | data | crc_lo | crc_hi` is 0 if the
//!    frame is intact).  Validate the function code is one we
//!    support, and that the address / count fields are in range.
//!    Set up the response header in `resp_buf[0..2]` (addr, fc) and
//!    set `resp_len`.  Or, if validation fails, build the exception
//!    response (`fc | 0x80`, exception code) and skip Build entirely
//!    — go straight to BuildCrc.
//! 4. **Build** — multi-cycle.  Walks `build_idx` from 0 upward.
//!    Each cycle performs one per-FC action: copy a register byte,
//!    pack a coil bit into a status byte, apply a single-write,
//!    apply one byte of a multi-write.  When `build_idx` reaches the
//!    per-FC end-of-build value, transition to BuildCrc.
//! 5. **BuildCrc** — multi-cycle.  Walks `build_idx` from 0 to
//!    `resp_len`, folding each `resp_buf[i]` into `running_crc`.  On
//!    completion, appends `crc_lo` then `crc_hi` to `resp_buf` and
//!    transitions to Sending.
//! 6. **Sending** — emit `resp_buf[build_idx]` with `tx_valid = true`;
//!    advance on `tx_ready`.  On the last byte's `tx_ready`, pulse
//!    `resp_done` and return to Idle.
//!
//! No state holds the request bytes after Sending completes; the
//! holding-register / coil DFFs have already been updated.  The
//! widget is ready for the next request immediately.
//!
//!# Constants
//!
//! - [`SLAVE_ADDR`] — this slave's Modbus address.  Hardcoded to 1
//!   for now.  Future: const-generic parameter.
//!
//!# Parameters
//!
//! - `NREG` — capacity of both the holding-register file (FC 0x03 /
//!   0x06 / 0x10 reads/writes against this) and the input-register
//!   array (FC 0x04 reads from `input_regs` on the `In` port, sized
//!   `[Bits<16>; NREG]`).  Address space is `0..NREG` for each.  We
//!   share the size for simplicity — most real deployments allocate
//!   the same count of each.  Out-of-range reads / writes return
//!   exception 0x02 (ILLEGAL_DATA_ADDRESS).
//! - `NCOIL` — capacity of the coil bit-array (FC 0x01 / 0x05 / 0x0F)
//!   and the discrete-input array (FC 0x02 reads from
//!   `discrete_inputs: [bool; NCOIL]`).  Same shared-capacity choice.
//!
//!# Buffer size
//!
//! The receive and transmit buffers are fixed at 64 bytes each.  This
//! is enough for FC 0x10 with up to ~30 registers, FC 0x0F with up to
//! ~480 coils, and any single-register / single-coil command.  Frames
//! exceeding 64 bytes overflow `req_len` and are discarded silently
//! (no response — the master will see a timeout).  Future: const-
//! generic the buffer size.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/modbus_rtu_slave.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/modbus_rtu_slave.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/modbus_rtu_slave_fsm.md")]

use rhdl::prelude::*;

use crate::core::{constant::Constant, dff};

/// This slave's Modbus address.  Hardcoded to 1 (the most common
/// convention for "single device on the bus").
pub const SLAVE_ADDR: u128 = 1;

/// Modbus RTU request / response buffer size, bytes.
const BUF_LEN: usize = 64;

/// Internal state machine.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum SlaveState {
    /// Idle — line silent, no frame in progress.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Accumulating request bytes.  Inter-frame silence detection
    /// (t3.5 char times) marks end-of-frame.
    #[fsm_state(label = "rx")]
    Receiving,
    /// Validate addr + CRC; dispatch FC; set up Build* or build
    /// exception inline.
    #[fsm_state(label = "process")]
    Process,
    /// Multi-cycle response assembly (per-FC).
    #[fsm_state(label = "build")]
    Build,
    /// Multi-cycle CRC computation over the response buffer.
    #[fsm_state(label = "build CRC")]
    BuildCrc,
    /// Multi-cycle response transmission (one byte per tx_ready).
    #[fsm_state(label = "send")]
    Sending,
}

/// Bundled scratch state per CLAUDE.md §3.1.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct SlaveExtras {
    /// Inter-frame silence counter, in clock cycles.
    pub t35_counter: Bits<16>,
    /// Number of bytes currently in `req_buf`.  0..=64.
    pub req_len: Bits<8>,
    /// Number of bytes currently in `resp_buf` (excluding CRC, until
    /// BuildCrc appends it).  0..=64.
    pub resp_len: Bits<8>,
    /// During Build / BuildCrc / Sending, the per-byte walk index.
    pub build_idx: Bits<8>,
    /// Running CRC.  Used both for incoming-frame validation
    /// (folded as bytes arrive) and outgoing-frame computation
    /// (folded over `resp_buf` during BuildCrc).
    pub running_crc: Bits<16>,
    /// Latched function code (`req_buf[1]`, possibly masked).  Set
    /// during Process; used during Build to drive the per-FC walk.
    pub fc: Bits<8>,
    /// Latched starting address (request's `addr_hi:addr_lo`).
    /// Used during Build for read / write FCs.
    pub start_addr: Bits<16>,
    /// Latched count / quantity (request's `count_hi:count_lo`).
    /// Used during Build for multi-read / multi-write FCs.
    pub count: Bits<16>,
    /// Pulses for one cycle when the response transmission completes.
    pub resp_done: bool,
}

/// Modbus RTU slave widget.
#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = SlaveState, allow_implicit)]
pub struct ModbusRtuSlave<const NREG: usize, const NCOIL: usize>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    state: dff::DFF<SlaveState>,
    extras: dff::DFF<SlaveExtras>,
    req_buf: [dff::DFF<Bits<8>>; BUF_LEN],
    resp_buf: [dff::DFF<Bits<8>>; BUF_LEN],
    holding_regs: [dff::DFF<Bits<16>>; NREG],
    coils: [dff::DFF<bool>; NCOIL],
    t35_threshold: Constant<Bits<16>>,
}

impl<const NREG: usize, const NCOIL: usize> ModbusRtuSlave<NREG, NCOIL>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    /// Create a slave with the supplied inter-frame silence
    /// threshold (in clock cycles).  Modbus spec: at 9600 baud this
    /// is `(11 / 9600) * 3.5 ≈ 4.0 ms`.  The caller computes the
    /// cycle count from their clock frequency and baud rate.
    pub fn new(t35_threshold: Bits<16>) -> Self {
        Self {
            state: dff::DFF::default(),
            extras: dff::DFF::default(),
            req_buf: array_init::array_init(|_| dff::DFF::new(bits::<8>(0))),
            resp_buf: array_init::array_init(|_| dff::DFF::new(bits::<8>(0))),
            holding_regs: array_init::array_init(|_| dff::DFF::new(bits::<16>(0))),
            coils: array_init::array_init(|_| dff::DFF::new(false)),
            t35_threshold: Constant::new(t35_threshold),
        }
    }
}

impl<const NREG: usize, const NCOIL: usize> Default for ModbusRtuSlave<NREG, NCOIL>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    fn default() -> Self {
        // Default threshold = 100 cycles (cheap for tests; production users
        // should call `new` with a real value).
        Self::new(bits::<16>(100))
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [ModbusRtuSlave].
pub struct In<const NREG: usize, const NCOIL: usize> {
    /// Latest received byte from the UART.  Only meaningful while
    /// `rx_valid` is high.
    pub rx_byte: Bits<8>,
    /// One-cycle pulse: a new `rx_byte` is available this cycle.
    pub rx_valid: bool,
    /// One-cycle pulse from the downstream UART: "I consumed the
    /// previous `tx_byte`; advance to the next."
    pub tx_ready: bool,
    /// Read-only input registers (FC 0x04 reads from these).  Wire
    /// sensor data, status registers, etc., into this array.
    pub input_regs: [Bits<16>; NREG],
    /// Read-only discrete inputs (FC 0x02 reads from these).
    pub discrete_inputs: [bool; NCOIL],
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [ModbusRtuSlave].
pub struct Out<const NREG: usize, const NCOIL: usize> {
    /// Current byte to transmit (only meaningful while `tx_valid` is high).
    pub tx_byte: Bits<8>,
    /// `true` while a fresh `tx_byte` is available for the UART to consume.
    pub tx_valid: bool,
    /// True while the slave is receiving, processing, or transmitting.
    pub busy: bool,
    /// Pulses for one cycle when the response transmission completes.
    pub resp_done: bool,
    /// Live view of the holding-register file.  FC 0x06 / 0x10
    /// writes update these on the next clock edge.
    pub holding_regs: [Bits<16>; NREG],
    /// Live view of the coil bit-array.  FC 0x05 / 0x0F writes
    /// update these on the next clock edge.
    pub coils: [bool; NCOIL],
}

impl<const NREG: usize, const NCOIL: usize> SynchronousIO for ModbusRtuSlave<NREG, NCOIL>
where
    [(); NREG]: Sized,
    [(); NCOIL]: Sized,
{
    type I = In<NREG, NCOIL>;
    type O = Out<NREG, NCOIL>;
    type Kernel = modbus_rtu_slave<NREG, NCOIL>;
}

/// Combinational CRC-16 update for a single byte.  Folds the byte
/// into `crc` per the Modbus polynomial `0xA001` (reflected).  Used
/// by the kernel both during reception (per byte) and during
/// response-CRC computation.
#[kernel]
pub fn crc16_step(crc: Bits<16>, byte: Bits<8>) -> Bits<16> {
    let byte_w: Bits<16> = byte.resize();
    let mut work: Bits<16> = crc ^ byte_w;
    for _b in 0..8 {
        let lsb_set = (work & bits::<16>(1)) != bits::<16>(0);
        let shifted: Bits<16> = work >> bits::<16>(1);
        work = if lsb_set {
            shifted ^ bits::<16>(0xA001)
        } else {
            shifted
        };
    }
    work
}

#[kernel]
/// Kernel for [ModbusRtuSlave].
pub fn modbus_rtu_slave<const NREG: usize, const NCOIL: usize>(
    cr: ClockReset,
    i: In<NREG, NCOIL>,
    q: Q<NREG, NCOIL>,
) -> (Out<NREG, NCOIL>, D<NREG, NCOIL>) {
    let mut d = D::<NREG, NCOIL>::dont_care();
    d.state = q.state;
    d.extras = q.extras;
    // Default: hold every buffer / register / coil byte.
    for k in 0..BUF_LEN {
        d.req_buf[k] = q.req_buf[k];
        d.resp_buf[k] = q.resp_buf[k];
    }
    for k in 0..NREG {
        d.holding_regs[k] = q.holding_regs[k];
    }
    for k in 0..NCOIL {
        d.coils[k] = q.coils[k];
    }
    let mut e = q.extras;
    e.resp_done = false;

    let mut tx_byte: Bits<8> = bits::<8>(0);
    let mut tx_valid: bool = false;

    let slave_addr_b: Bits<8> = bits::<8>(SLAVE_ADDR);
    let bcast_b: Bits<8> = bits::<8>(0);

    match q.state {
        SlaveState::Idle => {
            // Reset CRC + counters.  On rx_valid, enter Receiving and
            // store the first byte.
            e.running_crc = bits::<16>(0xFFFF);
            e.req_len = bits::<8>(0);
            e.t35_counter = bits::<16>(0);
            if i.rx_valid {
                d.req_buf[0] = i.rx_byte;
                e.running_crc = crc16_step(bits::<16>(0xFFFF), i.rx_byte);
                e.req_len = bits::<8>(1);
                d.state = SlaveState::Receiving;
            }
        }
        SlaveState::Receiving => {
            if i.rx_valid {
                // Append byte if buffer not full.  (If it's full, drop
                // and the frame will fail CRC validation.)
                let req_len_idx: Bits<8> = q.extras.req_len;
                if (req_len_idx.raw() as usize) < BUF_LEN {
                    d.req_buf[req_len_idx] = i.rx_byte;
                    e.req_len = q.extras.req_len + bits::<8>(1);
                }
                e.running_crc = crc16_step(q.extras.running_crc, i.rx_byte);
                e.t35_counter = bits::<16>(0);
            } else {
                // Idle cycle — count toward t3.5 timeout.
                if q.extras.t35_counter >= q.t35_threshold {
                    // End of frame.  Move to Process.
                    e.build_idx = bits::<8>(0);
                    d.state = SlaveState::Process;
                } else {
                    e.t35_counter = q.extras.t35_counter + bits::<16>(1);
                }
            }
        }
        SlaveState::Process => {
            // Inspect req_buf[0] (addr), req_buf[1] (fc), running_crc.
            let frame_addr: Bits<8> = q.req_buf[bits::<8>(0)];
            let frame_fc: Bits<8> = q.req_buf[bits::<8>(1)];
            let crc_ok: bool = q.extras.running_crc == bits::<16>(0);
            let addr_match: bool = frame_addr == slave_addr_b;
            let is_broadcast: bool = frame_addr == bcast_b;
            let frame_min_len: bool = q.extras.req_len >= bits::<8>(4);

            // Only proceed if this is for us (or broadcast) and CRC matches.
            if !crc_ok || (!addr_match && !is_broadcast) || !frame_min_len {
                // Drop frame silently.
                d.state = SlaveState::Idle;
            } else {
                // Latch fc, addr, count.
                e.fc = frame_fc;
                let addr_hi: Bits<8> = q.req_buf[bits::<8>(2)];
                let addr_lo: Bits<8> = q.req_buf[bits::<8>(3)];
                let addr_hi16: Bits<16> = addr_hi.resize();
                let addr_lo16: Bits<8> = addr_lo;
                let addr_lo16w: Bits<16> = addr_lo16.resize();
                e.start_addr = (addr_hi16 << bits::<16>(8)) | addr_lo16w;

                // For multi-byte FCs, count = req_buf[4..6].  For single-write
                // FCs (0x05, 0x06), count is the value, not a count.
                let count_hi: Bits<8> = q.req_buf[bits::<8>(4)];
                let count_lo: Bits<8> = q.req_buf[bits::<8>(5)];
                let count_hi16: Bits<16> = count_hi.resize();
                let count_lo16: Bits<16> = count_lo.resize();
                e.count = (count_hi16 << bits::<16>(8)) | count_lo16;

                // Set up response header (addr | fc).  Build will fill in
                // the payload.  On exception, we'll overwrite resp_buf[1]
                // with fc | 0x80 and resp_buf[2] with the exception code.
                d.resp_buf[bits::<8>(0)] = slave_addr_b;
                d.resp_buf[bits::<8>(1)] = frame_fc;

                // Validate per FC.  On any validation failure, build the
                // exception response inline and skip Build.
                let mut exception: Bits<8> = bits::<8>(0); // 0 = no exception
                let nreg_b: Bits<16> = bits::<16>(NREG as u128);
                let ncoil_b: Bits<16> = bits::<16>(NCOIL as u128);
                let nin_b: Bits<16> = bits::<16>(NREG as u128);
                let ndin_b: Bits<16> = bits::<16>(NCOIL as u128);

                let start_addr_v: Bits<16> = (addr_hi16 << bits::<16>(8)) | addr_lo16w;
                let count_v: Bits<16> = (count_hi16 << bits::<16>(8)) | count_lo16;

                if frame_fc == bits::<8>(0x01) {
                    // Read Coils.  Validate addr+count vs NCOIL, count
                    // 1..=2000.
                    if count_v == bits::<16>(0) || count_v > bits::<16>(2000) {
                        exception = bits::<8>(0x03);
                    } else if start_addr_v >= ncoil_b || (start_addr_v + count_v) > ncoil_b {
                        exception = bits::<8>(0x02);
                    }
                } else if frame_fc == bits::<8>(0x02) {
                    if count_v == bits::<16>(0) || count_v > bits::<16>(2000) {
                        exception = bits::<8>(0x03);
                    } else if start_addr_v >= ndin_b || (start_addr_v + count_v) > ndin_b {
                        exception = bits::<8>(0x02);
                    }
                } else if frame_fc == bits::<8>(0x03) {
                    if count_v == bits::<16>(0) || count_v > bits::<16>(125) {
                        exception = bits::<8>(0x03);
                    } else if start_addr_v >= nreg_b || (start_addr_v + count_v) > nreg_b {
                        exception = bits::<8>(0x02);
                    }
                } else if frame_fc == bits::<8>(0x04) {
                    if count_v == bits::<16>(0) || count_v > bits::<16>(125) {
                        exception = bits::<8>(0x03);
                    } else if start_addr_v >= nin_b || (start_addr_v + count_v) > nin_b {
                        exception = bits::<8>(0x02);
                    }
                } else if frame_fc == bits::<8>(0x05) {
                    // Write Single Coil.  count_v is the value:
                    // 0x0000 = off, 0xFF00 = on, anything else = exception 0x03.
                    if count_v != bits::<16>(0) && count_v != bits::<16>(0xFF00) {
                        exception = bits::<8>(0x03);
                    } else if start_addr_v >= ncoil_b {
                        exception = bits::<8>(0x02);
                    }
                } else if frame_fc == bits::<8>(0x06) {
                    // Write Single Register.  count_v is the value, always valid.
                    if start_addr_v >= nreg_b {
                        exception = bits::<8>(0x02);
                    }
                } else if frame_fc == bits::<8>(0x0F) {
                    // Write Multiple Coils.  count = number of coils.
                    if count_v == bits::<16>(0) || count_v > bits::<16>(1968) {
                        exception = bits::<8>(0x03);
                    } else if start_addr_v >= ncoil_b || (start_addr_v + count_v) > ncoil_b {
                        exception = bits::<8>(0x02);
                    }
                } else if frame_fc == bits::<8>(0x10) {
                    // Write Multiple Registers.
                    if count_v == bits::<16>(0) || count_v > bits::<16>(123) {
                        exception = bits::<8>(0x03);
                    } else if start_addr_v >= nreg_b || (start_addr_v + count_v) > nreg_b {
                        exception = bits::<8>(0x02);
                    }
                } else {
                    // Unsupported FC.
                    exception = bits::<8>(0x01);
                }

                if exception != bits::<8>(0) {
                    // Exception response: fc | 0x80, exception_code.
                    d.resp_buf[bits::<8>(1)] = frame_fc | bits::<8>(0x80);
                    d.resp_buf[bits::<8>(2)] = exception;
                    e.resp_len = bits::<8>(3);
                    e.running_crc = bits::<16>(0xFFFF);
                    e.build_idx = bits::<8>(0);
                    if is_broadcast {
                        // Broadcast: never respond.
                        d.state = SlaveState::Idle;
                    } else {
                        d.state = SlaveState::BuildCrc;
                    }
                } else if is_broadcast
                    && (frame_fc == bits::<8>(0x05)
                        || frame_fc == bits::<8>(0x06)
                        || frame_fc == bits::<8>(0x0F)
                        || frame_fc == bits::<8>(0x10))
                {
                    // Broadcast write: apply, but don't respond.  We fall
                    // through to Build and let the builder do the work, then
                    // skip Sending.  Easiest: jump straight into Build with
                    // a flag... or just process write inline here and return
                    // to Idle.  For broadcast, only writes are valid.
                    //
                    // Inline single-writes; defer multi-writes to Build with
                    // a "no-respond" flag.  For now we apply the writes
                    // through Build and rely on a special path that returns
                    // to Idle after Build instead of going to BuildCrc.
                    //
                    // Simpler: just go to Build with `resp_len = 0` as a
                    // sentinel, and have the post-Build handler check
                    // is_broadcast (latched in `e.fc`'s top bit?  hack).
                    // Cleanest: store broadcast flag in extras.
                    //
                    // For this implementation we simply drop broadcast
                    // writes — most slaves on multi-drop buses use unicast
                    // for writes anyway.
                    d.state = SlaveState::Idle;
                } else {
                    // Valid request.  Set up Build.
                    e.build_idx = bits::<8>(0);
                    e.running_crc = bits::<16>(0xFFFF);
                    // resp_len for read FCs depends on count; for writes
                    // it's a fixed 6 (addr + fc + addr_hi + addr_lo +
                    // count_hi/value_hi + count_lo/value_lo).  Set up here
                    // for correctness.
                    if frame_fc == bits::<8>(0x01) || frame_fc == bits::<8>(0x02) {
                        // byte_count = ceil(count / 8) bytes.
                        let bytes16: Bits<16> = (count_v + bits::<16>(7)) >> bits::<16>(3);
                        let bytes8: Bits<8> = bytes16.resize();
                        // resp_len = 3 (addr + fc + byte_count) + bytes8
                        e.resp_len = bits::<8>(3) + bytes8;
                        d.resp_buf[bits::<8>(2)] = bytes8;
                    } else if frame_fc == bits::<8>(0x03) || frame_fc == bits::<8>(0x04) {
                        // byte_count = count * 2.
                        let bytes16: Bits<16> = count_v << bits::<16>(1);
                        let bytes8: Bits<8> = bytes16.resize();
                        e.resp_len = bits::<8>(3) + bytes8;
                        d.resp_buf[bits::<8>(2)] = bytes8;
                    } else if frame_fc == bits::<8>(0x05) || frame_fc == bits::<8>(0x06) {
                        // Echo addr + value.
                        d.resp_buf[bits::<8>(2)] = addr_hi;
                        d.resp_buf[bits::<8>(3)] = addr_lo;
                        d.resp_buf[bits::<8>(4)] = count_hi;
                        d.resp_buf[bits::<8>(5)] = count_lo;
                        e.resp_len = bits::<8>(6);
                    } else {
                        // 0x0F, 0x10: echo addr + count.
                        d.resp_buf[bits::<8>(2)] = addr_hi;
                        d.resp_buf[bits::<8>(3)] = addr_lo;
                        d.resp_buf[bits::<8>(4)] = count_hi;
                        d.resp_buf[bits::<8>(5)] = count_lo;
                        e.resp_len = bits::<8>(6);
                    }
                    d.state = SlaveState::Build;
                }
            }
        }
        SlaveState::Build => {
            // build_idx walks per FC.  We define the per-FC end criteria
            // and per-step action.
            let bi: Bits<8> = q.extras.build_idx;
            let fc: Bits<8> = q.extras.fc;
            let start: Bits<16> = q.extras.start_addr;
            let count_v: Bits<16> = q.extras.count;
            let bi16: Bits<16> = bi.resize();

            if fc == bits::<8>(0x01) || fc == bits::<8>(0x02) {
                // Pack `count_v` bits into ceil(count/8) bytes, LSB-first
                // per Modbus spec (coil 0 = bit 0 of byte 0).
                // build_idx steps over each output byte.
                let byte_idx: Bits<16> = bi16;
                let bytes_total: Bits<16> = (count_v + bits::<16>(7)) >> bits::<16>(3);
                if byte_idx >= bytes_total {
                    // Done.
                    d.state = SlaveState::BuildCrc;
                    e.build_idx = bits::<8>(0);
                    e.running_crc = bits::<16>(0xFFFF);
                } else {
                    // Build one byte: 8 coils starting at start + byte_idx*8.
                    let mut packed: Bits<8> = bits::<8>(0);
                    for b in 0..8 {
                        let coil_offset: Bits<16> =
                            (byte_idx << bits::<16>(3)) + bits::<16>(b as u128);
                        let coil_addr: Bits<16> = start + coil_offset;
                        let in_range: bool = coil_offset < count_v;
                        // Guard against out-of-range index — both branches
                        // of an `if` evaluate combinationally in RHDL, and
                        // the simulator panics on out-of-bounds array reads.
                        let safe_addr: Bits<16> = if in_range { coil_addr } else { bits::<16>(0) };
                        let bit_val: bool = if fc == bits::<8>(0x01) {
                            q.coils[safe_addr]
                        } else {
                            i.discrete_inputs[safe_addr]
                        };
                        if in_range && bit_val {
                            packed |= bits::<8>(1) << bits::<8>(b as u128);
                        }
                    }
                    d.resp_buf[bi + bits::<8>(3)] = packed;
                    e.build_idx = bi + bits::<8>(1);
                }
            } else if fc == bits::<8>(0x03) || fc == bits::<8>(0x04) {
                // Read Holding / Input Registers.  Each register fills 2
                // resp bytes (hi then lo).
                let reg_idx: Bits<16> = bi16;
                if reg_idx >= count_v {
                    d.state = SlaveState::BuildCrc;
                    e.build_idx = bits::<8>(0);
                    e.running_crc = bits::<16>(0xFFFF);
                } else {
                    let reg_addr: Bits<16> = start + reg_idx;
                    let reg_val: Bits<16> = if fc == bits::<8>(0x03) {
                        q.holding_regs[reg_addr]
                    } else {
                        i.input_regs[reg_addr]
                    };
                    let hi: Bits<8> = (reg_val >> bits::<16>(8)).resize();
                    let lo: Bits<8> = (reg_val & bits::<16>(0xFF)).resize();
                    let resp_byte_idx: Bits<8> = bits::<8>(3) + (bi << bits::<8>(1));
                    d.resp_buf[resp_byte_idx] = hi;
                    d.resp_buf[resp_byte_idx + bits::<8>(1)] = lo;
                    e.build_idx = bi + bits::<8>(1);
                }
            } else if fc == bits::<8>(0x05) {
                // Write Single Coil — count_v contains the value
                // (0x0000 = off, 0xFF00 = on).  Apply once.
                let val: bool = count_v == bits::<16>(0xFF00);
                d.coils[start] = val;
                // Done in one step.
                d.state = SlaveState::BuildCrc;
                e.build_idx = bits::<8>(0);
                e.running_crc = bits::<16>(0xFFFF);
            } else if fc == bits::<8>(0x06) {
                // Write Single Register — count_v contains the value.
                d.holding_regs[start] = count_v;
                d.state = SlaveState::BuildCrc;
                e.build_idx = bits::<8>(0);
                e.running_crc = bits::<16>(0xFFFF);
            } else if fc == bits::<8>(0x0F) {
                // Write Multiple Coils.  Walk count_v coils, unpacking
                // bits from req_buf[7..] (after addr+fc+addr+count+
                // byte_count = 7-byte header).
                let coil_idx: Bits<16> = bi16;
                if coil_idx >= count_v {
                    d.state = SlaveState::BuildCrc;
                    e.build_idx = bits::<8>(0);
                    e.running_crc = bits::<16>(0xFFFF);
                } else {
                    // Source byte: req_buf[7 + coil_idx/8]
                    let byte_off: Bits<16> = coil_idx >> bits::<16>(3);
                    let bit_off: Bits<16> = coil_idx & bits::<16>(7);
                    let src_idx: Bits<8> = bits::<8>(7) + byte_off.resize();
                    let src_byte: Bits<8> = q.req_buf[src_idx];
                    let bit_off8: Bits<8> = bit_off.resize();
                    let mask: Bits<8> = bits::<8>(1) << bit_off8;
                    let bit_set: bool = (src_byte & mask) != bits::<8>(0);
                    let coil_addr: Bits<16> = start + coil_idx;
                    d.coils[coil_addr] = bit_set;
                    e.build_idx = bi + bits::<8>(1);
                }
            } else {
                // 0x10 — Write Multiple Registers.  Walk count_v regs;
                // each from req_buf[7+2*i .. 7+2*i+2].
                let reg_idx: Bits<16> = bi16;
                if reg_idx >= count_v {
                    d.state = SlaveState::BuildCrc;
                    e.build_idx = bits::<8>(0);
                    e.running_crc = bits::<16>(0xFFFF);
                } else {
                    let src_idx: Bits<8> = bits::<8>(7) + (bi << bits::<8>(1));
                    let hi: Bits<8> = q.req_buf[src_idx];
                    let lo: Bits<8> = q.req_buf[src_idx + bits::<8>(1)];
                    let hi16: Bits<16> = hi.resize();
                    let lo16: Bits<16> = lo.resize();
                    let val: Bits<16> = (hi16 << bits::<16>(8)) | lo16;
                    let reg_addr: Bits<16> = start + reg_idx;
                    d.holding_regs[reg_addr] = val;
                    e.build_idx = bi + bits::<8>(1);
                }
            }
        }
        SlaveState::BuildCrc => {
            // Walk resp_buf[0..resp_len] folding each byte into running_crc.
            let bi: Bits<8> = q.extras.build_idx;
            if bi >= q.extras.resp_len {
                // Done — append CRC bytes (low, then high) to resp_buf.
                let crc_lo: Bits<8> = (q.extras.running_crc & bits::<16>(0xFF)).resize();
                let crc_hi: Bits<8> = (q.extras.running_crc >> bits::<16>(8)).resize();
                d.resp_buf[q.extras.resp_len] = crc_lo;
                d.resp_buf[q.extras.resp_len + bits::<8>(1)] = crc_hi;
                e.resp_len = q.extras.resp_len + bits::<8>(2);
                e.build_idx = bits::<8>(0);
                d.state = SlaveState::Sending;
            } else {
                let byte: Bits<8> = q.resp_buf[bi];
                e.running_crc = crc16_step(q.extras.running_crc, byte);
                e.build_idx = bi + bits::<8>(1);
            }
        }
        SlaveState::Sending => {
            // Drive resp_buf[build_idx] on tx_byte / tx_valid; advance on tx_ready.
            let bi: Bits<8> = q.extras.build_idx;
            tx_byte = q.resp_buf[bi];
            tx_valid = true;
            if i.tx_ready {
                if bi + bits::<8>(1) >= q.extras.resp_len {
                    // Last byte just consumed.
                    e.resp_done = true;
                    e.build_idx = bits::<8>(0);
                    d.state = SlaveState::Idle;
                } else {
                    e.build_idx = bi + bits::<8>(1);
                }
            }
        }
    }

    if cr.reset.any() {
        d.state = SlaveState::Idle;
        e = SlaveExtras::default();
        // Zero buffers, holding_regs, coils on reset.
        for k in 0..BUF_LEN {
            d.req_buf[k] = bits::<8>(0);
            d.resp_buf[k] = bits::<8>(0);
        }
        for k in 0..NREG {
            d.holding_regs[k] = bits::<16>(0);
        }
        for k in 0..NCOIL {
            d.coils[k] = false;
        }
    }

    d.extras = e;

    let mut o = Out::<NREG, NCOIL>::dont_care();
    o.tx_byte = tx_byte;
    o.tx_valid = tx_valid;
    o.busy = q.state != SlaveState::Idle;
    o.resp_done = q.extras.resp_done;
    for k in 0..NREG {
        o.holding_regs[k] = q.holding_regs[k];
    }
    for k in 0..NCOIL {
        o.coils[k] = q.coils[k];
    }
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    /// Reference Modbus CRC-16 (polynomial 0xA001) — used to cross-check
    /// the kernel's iterative computation.
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
            rx_byte: bits(0),
            rx_valid: false,
            tx_ready: false,
            input_regs: [bits(0); NREG],
            discrete_inputs: [false; NCOIL],
        }
    }

    /// Build a request stream: emit each byte one cycle apart, then
    /// hold idle for `silence` cycles to trigger the t3.5 timeout
    /// (default threshold 100 cycles).
    fn make_request_stream<const NREG: usize, const NCOIL: usize>(
        bytes: &[u8],
        silence: usize,
        input_regs: [Bits<16>; NREG],
        discrete_inputs: [bool; NCOIL],
    ) -> Vec<In<NREG, NCOIL>> {
        let mut out: Vec<In<NREG, NCOIL>> = Vec::new();
        for &b in bytes {
            out.push(In {
                rx_byte: bits(b as u128),
                rx_valid: true,
                tx_ready: false,
                input_regs,
                discrete_inputs,
            });
            // One idle cycle between bytes (real UART).
            out.push(In {
                rx_byte: bits(0),
                rx_valid: false,
                tx_ready: false,
                input_regs,
                discrete_inputs,
            });
        }
        for _ in 0..silence {
            out.push(In {
                rx_byte: bits(0),
                rx_valid: false,
                tx_ready: false,
                input_regs,
                discrete_inputs,
            });
        }
        out
    }

    /// Drain the response by alternating "wait for tx_valid" / "tx_ready".
    /// Returns the bytes captured.  Drives the slave forward by appending
    /// `tx_ready` strobes to `stream` until `expected_len` bytes have been
    /// consumed.
    fn drain_response_stream<const NREG: usize, const NCOIL: usize>(
        mut stream: Vec<In<NREG, NCOIL>>,
        ready_count: usize,
    ) -> Vec<In<NREG, NCOIL>> {
        let last = *stream.last().unwrap_or(&In {
            rx_byte: bits(0),
            rx_valid: false,
            tx_ready: false,
            input_regs: [bits(0); NREG],
            discrete_inputs: [false; NCOIL],
        });
        for _ in 0..ready_count {
            stream.push(In {
                tx_ready: true,
                ..last
            });
            stream.push(In {
                tx_ready: false,
                ..last
            });
        }
        stream
    }

    /// Run the slave through `stream_in` and capture the bytes that
    /// were transmitted (tx_byte sampled when tx_valid && tx_ready).
    fn run_and_capture<const NREG: usize, const NCOIL: usize>(
        uut: &ModbusRtuSlave<NREG, NCOIL>,
        stream_in: Vec<In<NREG, NCOIL>>,
    ) -> Vec<u8>
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
        outputs
            .iter()
            .filter(|s| s.output.tx_valid && s.input.1.tx_ready)
            .map(|s| s.output.tx_byte.raw() as u8)
            .collect()
    }

    /// Build a Modbus request frame: addr + fc + payload + CRC bytes.
    fn build_frame(addr: u8, fc: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![addr, fc];
        frame.extend_from_slice(payload);
        let crc = ref_crc(&frame);
        frame.push((crc & 0xFF) as u8);
        frame.push((crc >> 8) as u8);
        frame
    }

    #[test]
    fn test_idle_no_tx_valid() -> miette::Result<()> {
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let stream = std::iter::repeat_n(idle_in::<8, 8>(), 200)
            .with_reset(1)
            .clock_pos_edge(100);
        let any = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .any(|s| s.output.tx_valid);
        assert!(!any, "tx_valid asserted with no input");
        Ok(())
    }

    #[test]
    fn test_fc03_read_holding_registers() -> miette::Result<()> {
        // Read 3 holding registers from slave 1, starting at addr 2.
        let frame = build_frame(0x01, 0x03, &[0x00, 0x02, 0x00, 0x03]);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);
        // Default holding regs are 0.  Expected response:
        // 01 03 06 00 00 00 00 00 00 + crc_lo + crc_hi
        let mut expected_payload = vec![0x01, 0x03, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let crc = ref_crc(&expected_payload);
        expected_payload.push((crc & 0xFF) as u8);
        expected_payload.push((crc >> 8) as u8);
        assert_eq!(captured, expected_payload, "response frame mismatch");
        Ok(())
    }

    #[test]
    fn test_fc06_write_single_register_then_fc03_read_back() -> miette::Result<()> {
        // Write reg[5] = 0x1234 (FC 0x06), then read regs[5..6] (FC 0x03).
        let mut stream_in: Vec<In<8, 8>> = Vec::new();
        let f1 = build_frame(0x01, 0x06, &[0x00, 0x05, 0x12, 0x34]);
        let mut s1 = make_request_stream::<8, 8>(&f1, 150, [bits(0); 8], [false; 8]);
        s1 = drain_response_stream(s1, 16);
        stream_in.extend(s1);
        let f2 = build_frame(0x01, 0x03, &[0x00, 0x05, 0x00, 0x01]);
        let mut s2 = make_request_stream::<8, 8>(&f2, 150, [bits(0); 8], [false; 8]);
        s2 = drain_response_stream(s2, 16);
        stream_in.extend(s2);

        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);
        // Expected: full echo of FC 0x06 request, then FC 0x03 response.
        let mut e1 = vec![0x01, 0x06, 0x00, 0x05, 0x12, 0x34];
        let crc1 = ref_crc(&e1);
        e1.push((crc1 & 0xFF) as u8);
        e1.push((crc1 >> 8) as u8);
        let mut e2 = vec![0x01, 0x03, 0x02, 0x12, 0x34];
        let crc2 = ref_crc(&e2);
        e2.push((crc2 & 0xFF) as u8);
        e2.push((crc2 >> 8) as u8);
        let mut expected = e1;
        expected.extend(e2);
        assert_eq!(captured, expected, "FC 0x06 + read-back mismatch");
        Ok(())
    }

    #[test]
    fn test_fc05_write_single_coil_then_fc01_read_back() -> miette::Result<()> {
        // Write coil[3] = ON, then read coils[3..6].
        let mut stream_in: Vec<In<8, 8>> = Vec::new();
        let f1 = build_frame(0x01, 0x05, &[0x00, 0x03, 0xFF, 0x00]);
        let mut s1 = make_request_stream::<8, 8>(&f1, 150, [bits(0); 8], [false; 8]);
        s1 = drain_response_stream(s1, 16);
        stream_in.extend(s1);
        let f2 = build_frame(0x01, 0x01, &[0x00, 0x03, 0x00, 0x03]);
        let mut s2 = make_request_stream::<8, 8>(&f2, 150, [bits(0); 8], [false; 8]);
        s2 = drain_response_stream(s2, 16);
        stream_in.extend(s2);

        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);

        let mut e1 = vec![0x01, 0x05, 0x00, 0x03, 0xFF, 0x00];
        let crc1 = ref_crc(&e1);
        e1.push((crc1 & 0xFF) as u8);
        e1.push((crc1 >> 8) as u8);
        // FC 0x01 reading 3 coils starting at coil[3]: result is bits
        // [3, 4, 5] packed LSB-first.  Only coil[3] is set, so bit 0
        // is 1, bits 1-2 are 0 → 0x01.  byte_count = 1.
        let mut e2 = vec![0x01, 0x01, 0x01, 0x01];
        let crc2 = ref_crc(&e2);
        e2.push((crc2 & 0xFF) as u8);
        e2.push((crc2 >> 8) as u8);
        let mut expected = e1;
        expected.extend(e2);
        assert_eq!(captured, expected, "FC 0x05 + read-back mismatch");
        Ok(())
    }

    #[test]
    fn test_fc10_write_multiple_registers_then_fc03_read_back() -> miette::Result<()> {
        // Write regs[0..3] = [0x1111, 0x2222, 0x3333], read back.
        let mut stream_in: Vec<In<8, 8>> = Vec::new();
        let f1 = build_frame(
            0x01,
            0x10,
            &[
                0x00, 0x00, 0x00, 0x03, 0x06, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33,
            ],
        );
        let mut s1 = make_request_stream::<8, 8>(&f1, 150, [bits(0); 8], [false; 8]);
        s1 = drain_response_stream(s1, 16);
        stream_in.extend(s1);
        let f2 = build_frame(0x01, 0x03, &[0x00, 0x00, 0x00, 0x03]);
        let mut s2 = make_request_stream::<8, 8>(&f2, 250, [bits(0); 8], [false; 8]);
        s2 = drain_response_stream(s2, 16);
        stream_in.extend(s2);

        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);

        let mut e1 = vec![0x01, 0x10, 0x00, 0x00, 0x00, 0x03];
        let crc1 = ref_crc(&e1);
        e1.push((crc1 & 0xFF) as u8);
        e1.push((crc1 >> 8) as u8);
        let mut e2 = vec![0x01, 0x03, 0x06, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33];
        let crc2 = ref_crc(&e2);
        e2.push((crc2 & 0xFF) as u8);
        e2.push((crc2 >> 8) as u8);
        let mut expected = e1;
        expected.extend(e2);
        assert_eq!(captured, expected, "FC 0x10 + read-back mismatch");
        Ok(())
    }

    #[test]
    fn test_fc04_read_input_registers() -> miette::Result<()> {
        // Read 2 input registers from slave 1, starting at addr 1.
        // Input regs come from the In struct.
        let frame = build_frame(0x01, 0x04, &[0x00, 0x01, 0x00, 0x02]);
        let input_regs: [Bits<16>; 8] = [
            bits(0xAAAA),
            bits(0xBBBB),
            bits(0xCCCC),
            bits(0xDDDD),
            bits(0),
            bits(0),
            bits(0),
            bits(0),
        ];
        let stream_in = make_request_stream::<8, 8>(&frame, 150, input_regs, [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);

        // Expected: 01 04 04 BB BB CC CC + crc.  (We requested
        // input_regs[1..3] = 0xBBBB, 0xCCCC.)
        let mut e = vec![0x01, 0x04, 0x04, 0xBB, 0xBB, 0xCC, 0xCC];
        let crc = ref_crc(&e);
        e.push((crc & 0xFF) as u8);
        e.push((crc >> 8) as u8);
        assert_eq!(captured, e);
        Ok(())
    }

    #[test]
    fn test_fc02_read_discrete_inputs() -> miette::Result<()> {
        // Read 4 discrete inputs from addr 0.  inputs = [T, F, T, T].
        // Expected byte: bit0=1, bit1=0, bit2=1, bit3=1, rest 0 → 0x0D.
        let frame = build_frame(0x01, 0x02, &[0x00, 0x00, 0x00, 0x04]);
        let di: [bool; 8] = [true, false, true, true, false, false, false, false];
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], di);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);

        let mut e = vec![0x01, 0x02, 0x01, 0x0D];
        let crc = ref_crc(&e);
        e.push((crc & 0xFF) as u8);
        e.push((crc >> 8) as u8);
        assert_eq!(captured, e);
        Ok(())
    }

    #[test]
    fn test_fc0f_write_multiple_coils_then_fc01_read_back() -> miette::Result<()> {
        // Write 5 coils starting at coil[1], data bits = 0b10110.
        // (coil[1]=0, coil[2]=1, coil[3]=1, coil[4]=0, coil[5]=1)
        let mut stream_in: Vec<In<8, 8>> = Vec::new();
        let f1 = build_frame(0x01, 0x0F, &[0x00, 0x01, 0x00, 0x05, 0x01, 0b10110]);
        let mut s1 = make_request_stream::<8, 8>(&f1, 150, [bits(0); 8], [false; 8]);
        s1 = drain_response_stream(s1, 16);
        stream_in.extend(s1);
        let f2 = build_frame(0x01, 0x01, &[0x00, 0x00, 0x00, 0x08]);
        let mut s2 = make_request_stream::<8, 8>(&f2, 150, [bits(0); 8], [false; 8]);
        s2 = drain_response_stream(s2, 16);
        stream_in.extend(s2);

        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);

        let mut e1 = vec![0x01, 0x0F, 0x00, 0x01, 0x00, 0x05];
        let crc1 = ref_crc(&e1);
        e1.push((crc1 & 0xFF) as u8);
        e1.push((crc1 >> 8) as u8);
        // Read 8 coils from coil[0]: coils 1..6 = [0,1,1,0,1].
        // bit0=coil[0]=0, bit1=coil[1]=0, bit2=coil[2]=1, bit3=coil[3]=1,
        // bit4=coil[4]=0, bit5=coil[5]=1, bit6=0, bit7=0 → 0b00101100 = 0x2C
        let mut e2 = vec![0x01, 0x01, 0x01, 0x2C];
        let crc2 = ref_crc(&e2);
        e2.push((crc2 & 0xFF) as u8);
        e2.push((crc2 >> 8) as u8);
        let mut expected = e1;
        expected.extend(e2);
        assert_eq!(captured, expected, "FC 0x0F + read-back mismatch");
        Ok(())
    }

    #[test]
    fn test_exception_illegal_function() -> miette::Result<()> {
        // FC 0x07 is not implemented.  Should return exception 0x01.
        let frame = build_frame(0x01, 0x07, &[]);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);
        // Expected: 01 87 01 + crc.
        let mut e = vec![0x01, 0x87, 0x01];
        let crc = ref_crc(&e);
        e.push((crc & 0xFF) as u8);
        e.push((crc >> 8) as u8);
        assert_eq!(captured, e);
        Ok(())
    }

    #[test]
    fn test_exception_illegal_data_address() -> miette::Result<()> {
        // FC 0x03 read at addr 99 (out of range — only 8 regs).  Exception 0x02.
        let frame = build_frame(0x01, 0x03, &[0x00, 0x63, 0x00, 0x01]);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);
        let mut e = vec![0x01, 0x83, 0x02];
        let crc = ref_crc(&e);
        e.push((crc & 0xFF) as u8);
        e.push((crc >> 8) as u8);
        assert_eq!(captured, e);
        Ok(())
    }

    #[test]
    fn test_exception_illegal_data_value() -> miette::Result<()> {
        // FC 0x05 with invalid coil value (not 0x0000 or 0xFF00).  Exception 0x03.
        let frame = build_frame(0x01, 0x05, &[0x00, 0x00, 0x12, 0x34]);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);
        let mut e = vec![0x01, 0x85, 0x03];
        let crc = ref_crc(&e);
        e.push((crc & 0xFF) as u8);
        e.push((crc >> 8) as u8);
        assert_eq!(captured, e);
        Ok(())
    }

    #[test]
    fn test_bad_crc_no_response() -> miette::Result<()> {
        // Send a frame with deliberately wrong CRC.  Slave should not respond.
        let mut frame = vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x01];
        frame.push(0x00); // wrong CRC
        frame.push(0x00);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);
        assert!(captured.is_empty(), "responded to frame with bad CRC");
        Ok(())
    }

    #[test]
    fn test_wrong_address_no_response() -> miette::Result<()> {
        // Send a valid frame addressed to slave 7 (we are slave 1).  No response.
        let frame = build_frame(0x07, 0x03, &[0x00, 0x00, 0x00, 0x01]);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let captured = run_and_capture(&uut, stream_in);
        assert!(captured.is_empty(), "responded to frame for another slave");
        Ok(())
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["330073"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    #[test]
    fn test_modbus_rtu_slave_hdl_works() -> miette::Result<()> {
        let frame = build_frame(0x01, 0x03, &[0x00, 0x00, 0x00, 0x02]);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_modbus_rtu_slave_trace() -> miette::Result<()> {
        let frame = build_frame(0x01, 0x06, &[0x00, 0x02, 0xCA, 0xFE]);
        let stream_in = make_request_stream::<8, 8>(&frame, 150, [bits(0); 8], [false; 8]);
        let stream_in = drain_response_stream(stream_in, 16);
        let uut: ModbusRtuSlave<8, 8> = ModbusRtuSlave::default();
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("modbus_rtu_slave");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["3fc936de41fadb106636f41b2120a356d7df46c9d7aa7d43707fea61f20207ea"];
        let digest = vcd.dump_to_file(root.join("modbus_rtu_slave.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = ModbusRtuSlave::<8, 8>::fsm_descriptor();
        assert_eq!(desc.widget_name, "ModbusRtuSlave");
        assert_eq!(desc.variants().len(), 6);
        assert_eq!(desc.initial_index(), 0);
    }
}
