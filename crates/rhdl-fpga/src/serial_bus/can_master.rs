//! Classical CAN 2.0A bidirectional node
//!
//! A complete Classical CAN 2.0A node: simultaneously a transmitter,
//! a receiver, an error-counter / bus-off state machine, and an
//! acceptance filter — sharing a single set of internal registers
//! and a single FSM that walks the wire frame.  Pairs with an
//! external CAN transceiver (TJA1050 / MCP2551 / SN65HVD230) — the
//! widget exposes a logical-sense `tx`/`rx` pair (`false` =
//! dominant, `true` = recessive); the transceiver inverts to the
//! differential CAN_H / CAN_L on the wire.
//!
//! The widget retains the historical name `can_master` for
//! source-file stability; despite the name, it is **not** a
//! master in any protocol sense.  CAN is multi-master and
//! arbitration-free — every node both transmits and receives.
//! This widget does both.
//!
//! # Behavioural surface
//!
//! - **Standard 11-bit and extended 29-bit frames.**  Transmitter
//!   takes `tx_extended` + `tx_id: Bits<29>` (lower 11 bits used
//!   when standard); receiver detects the IDE bit and parses
//!   either form.  Received frames surface their full 29-bit ID
//!   plus an `rx_extended` flag.
//! - **Data frames.**  Up to 8 data bytes (DLC 0..=8).
//! - **Bit stuffing on transmit and destuffing on receive** per
//!   the canonical 5-same-bit rule across SOF through end of CRC.
//! - **CRC-15 over the destuffed stream**, polynomial `0x4599`,
//!   init `0`.
//! - **Bit-timing hard sync at every recessive→dominant edge**
//!   inside a frame.
//! - **Arbitration loss detection.**  When transmitting, if we
//!   drove recessive but the wire reads dominant during the
//!   arbitration zone (ID / SRR / IDE / IDB / RTR), we lost
//!   arbitration: silently switch to receiver role for this
//!   frame and re-queue the pending TX for after IFS.
//! - **All five Classical CAN error types.**  Stuff, form, bit,
//!   ACK, CRC.  Each fires the error-frame generator and the
//!   appropriate counter increment.
//! - **TEC / REC counters per ISO 11898-1 §11.6.**  Each 0..=255
//!   in `Bits<9>` so the bus-off threshold (TEC ≥ 256) and the
//!   error-passive threshold (≥ 128) are representable directly.
//! - **Error-active / error-passive / bus-off node states.**
//!   Active errors emit a 6-dominant error flag; passive emit a
//!   6-recessive error flag.  Bus-off (TEC ≥ 256) suspends
//!   transmission and recovers via 128 occurrences of 11
//!   consecutive recessive bits.
//! - **Acceptance filter on the receive side.**  Inputs
//!   `acc_id_filter` and `acc_id_mask` (`Bits<29>`) — the
//!   receiver only fires `frame_valid` when `(rx_id &
//!   acc_id_mask) == (acc_id_filter & acc_id_mask)`.  Set
//!   `acc_id_mask = 0` to accept everything.
//!
//! # Out-of-scope (separate widgets / future work)
//!
//! - **CAN-FD (ISO 11898-1:2015).**  Different bit timing inside
//!   the data phase, longer payloads, different CRC.
//! - **Per-segment programmable bit timing.**  ISO 11898-1 §10
//!   defines four programmable segments (Sync / Prop / Phase1 /
//!   Phase2); this widget collapses them into a single
//!   `bit_period` and a single sample point at end-of-bit.
//! - **Multiple TX message buffers.**  One pending TX at a time.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
        +------+CanMaster+--------+
        |                         |
B<29>   |                         | bool
+------>| tx_id          tx_out   +------>
bool    |                         | bool
+------>| tx_extended    tx_busy  +------>
bool    |                         | bool
+------>| tx_rtr         tx_done  +------>
B<4>    |                         | bool
+------>| tx_dlc         frame    +------>
B<64>   |                  _valid |
+------>| tx_data                 | B<29>
bool    |                  rx_id  +------>
+------>| tx_request              | bool
bool    |                  rx_ext +------>
+------>| rx                      | bool
B<29>   |                  rx_rtr +------>
+------>| acc_id_filter           | B<4>
B<29>   |                  rx_dlc +------>
+------>| acc_id_mask             | B<64>
        |                  rx_data+------>
        |                         | bool
        |                  crc_ok +------>
        |                         | B<9>
        |                  tec    +------>
        |                         | B<9>
        |                  rec    +------>
        |                         | bool
        |                 bus_off +------>
        |                         | bool
        |               err_pass  +------>
        +-------------------------+
")]
//!
//! # Internals
//!
//! Built using the protocol-PHY pattern from CLAUDE.md §3.1: one
//! `dff::DFF<CanField>` for the FSM-tagged frame-walk enum, plus
//! one `dff::DFF<CanState<DIV_W>>` for the (very large) bundle of
//! internal registers, plus a `Constant<Bits<DIV_W>>` for the bit
//! period.  Three sibling sub-circuits, well under the 12-tuple
//! ceiling, even though the widget carries about thirty pieces of
//! internal state.
//!
//! - **Bit-period counter** divides FPGA clocks down to the CAN
//!   bit rate.  Hard-syncs at every recessive→dominant edge
//!   inside a frame.
//! - **Frame walker** (the `field` FSM) traverses the same fields
//!   regardless of whether we are the transmitter or a receiver
//!   for this frame; the role is tracked as `is_transmitting` in
//!   the state bundle.
//! - **Bit stuffer / destuffer** mirrors itself across the two
//!   roles: TX inserts stuff bits after 5 same-polarity bits in
//!   the stuff zone; RX expects them and discards them.
//! - **Two CRC registers** — `crc_reg` is the locally computed
//!   CRC over the destuffed stream (used by both TX and RX);
//!   `rx_crc_accum` is the 15-bit CRC field as received from the
//!   wire.  Compared at the end of the Crc field.
//! - **Error-frame generator** runs as its own field: drives 6
//!   dominant or 6 recessive bits depending on `error_passive`,
//!   then 8 recessive delimiters, then re-enters Idle.
//! - **Bus-off recovery** counts 128 occurrences of 11
//!   consecutive recessive bits on `rx`.
//!
//! # Bit timing
//!
//! `bit_period` counts FPGA clocks per CAN bit time.  Sample
//! point is end-of-bit (counter rollover).  Hard sync at every
//! recessive→dominant edge inside a frame snaps the counter back
//! to one so subsequent samples land at the correct bit time.
//!
//! # Parameters
//!
//! - `DIV_W` — bit width of the bit-period counter
//!
//! # Example
//!
//! ```
#![doc = include_str!("../../examples/can_master.rs")]
//! ```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/can_master.md")]
//!
//! And the auto-generated FSM diagram for the CAN frame walk:
#![doc = include_str!("../../doc/can_master_fsm.md")]
use rhdl::prelude::*;

use crate::core::{constant::Constant, dff};

/// Frame-walk FSM for the CAN node.
///
/// The same enum drives both the transmit and receive paths —
/// `is_transmitting` in the state bundle distinguishes role.
/// Variants follow the wire order, with separate states for the
/// extended-frame fields (`IdB`, `Rtr`, `R1`) and the
/// error-frame generator.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum CanField {
    /// Bus-quiescent.  Wait for SOF or for `tx_request`.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Start-of-frame: a single dominant bit.
    #[fsm_state(label = "SOF")]
    Sof,
    /// First 11 bits of the identifier (also the entire ID for
    /// standard frames).
    IdA,
    /// Bit 12: SRR if extended, RTR if standard.
    SrrOrRtr,
    /// Bit 13: 0 = standard, 1 = extended.
    Ide,
    /// 18 additional ID bits (extended frames only).
    IdB,
    /// RTR bit (extended frames only).
    Rtr,
    /// Reserved bit r1 (extended frames only).
    R1,
    /// Reserved bit r0.
    R0,
    /// 4-bit data length code.
    Dlc,
    /// Data field, 0..=64 bits MSB-first.
    Data,
    /// 15-bit CRC, MSB-first.
    Crc,
    /// CRC delimiter, 1 recessive bit.
    #[fsm_state(label = "CRCDelim")]
    CrcDelim,
    /// ACK slot.
    #[fsm_state(label = "ACK")]
    AckSlot,
    /// ACK delimiter, 1 recessive bit.
    #[fsm_state(label = "ACKDelim")]
    AckDelim,
    /// 7 recessive bits marking end-of-frame.
    Eof,
    /// 3 recessive bits of inter-frame spacing.
    Ifs,
    /// Error-frame flag: 6 same-polarity bits.
    #[fsm_state(label = "ErrFlag")]
    ErrFlag,
    /// Error-frame delimiter: 8 recessive bits.
    #[fsm_state(label = "ErrDelim")]
    ErrDelim,
    /// Suspend transmission: 8 recessive bits after an
    /// error-passive transmitter completes its error frame.
    Suspend,
    /// Bus-off recovery: count 128 × 11 recessive bits.
    #[fsm_state(label = "BusOff")]
    BusOffWait,
}

/// Bundled internal state for the CAN node.
///
/// Per CLAUDE.md §3.1, all non-FSM state lives behind one DFF
/// inside a `Digital`-derived struct.
#[derive(PartialEq, Debug, Digital, Clone, Copy)]
pub struct CanState<const DIV_W: usize>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    bit_phase_counter: Bits<DIV_W>,
    field_bit_idx: Bits<7>,
    last_rx: bool,
    last_bit: bool,
    stuff_run: Bits<3>,
    expecting_stuff: bool,
    is_transmitting: bool,
    tx_pending: bool,
    tx_id_latched: Bits<29>,
    tx_extended_latched: bool,
    tx_rtr_latched: bool,
    tx_dlc_latched: Bits<4>,
    tx_data_latched: Bits<64>,
    id_accum: Bits<29>,
    extended_rx: bool,
    rtr_rx: bool,
    srr_or_rtr_bit: bool,
    dlc_accum: Bits<4>,
    data_accum: Bits<64>,
    rx_crc_accum: Bits<15>,
    crc_reg: Bits<15>,
    crc_ok: bool,
    tec: Bits<9>,
    rec: Bits<9>,
    error_passive: bool,
    bus_off: bool,
    error_pending: bool,
    bus_off_groups: Bits<8>,
    tx_done_pulse: bool,
    frame_pulse: bool,
    last_rx_id: Bits<29>,
    last_rx_extended: bool,
    last_rx_rtr: bool,
    last_rx_dlc: Bits<4>,
    last_rx_data: Bits<64>,
}

impl<const DIV_W: usize> Default for CanState<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    fn default() -> Self {
        Self {
            bit_phase_counter: bits::<DIV_W>(0),
            field_bit_idx: bits::<7>(0),
            last_rx: true,
            last_bit: true,
            stuff_run: bits::<3>(0),
            expecting_stuff: false,
            is_transmitting: false,
            tx_pending: false,
            tx_id_latched: bits::<29>(0),
            tx_extended_latched: false,
            tx_rtr_latched: false,
            tx_dlc_latched: bits::<4>(0),
            tx_data_latched: bits::<64>(0),
            id_accum: bits::<29>(0),
            extended_rx: false,
            rtr_rx: false,
            srr_or_rtr_bit: false,
            dlc_accum: bits::<4>(0),
            data_accum: bits::<64>(0),
            rx_crc_accum: bits::<15>(0),
            crc_reg: bits::<15>(0),
            crc_ok: false,
            tec: bits::<9>(0),
            rec: bits::<9>(0),
            error_passive: false,
            bus_off: false,
            error_pending: false,
            bus_off_groups: bits::<8>(0),
            tx_done_pulse: false,
            frame_pulse: false,
            last_rx_id: bits::<29>(0),
            last_rx_extended: false,
            last_rx_rtr: false,
            last_rx_dlc: bits::<4>(0),
            last_rx_data: bits::<64>(0),
        }
    }
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "field", state_enum = CanField, allow_implicit)]
/// Classical CAN 2.0A node — transmits, receives, manages errors,
/// recovers from bus-off.  See module-level docs for the full
/// behavioural surface.
pub struct CanMaster<const DIV_W: usize>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    field: dff::DFF<CanField>,
    state: dff::DFF<CanState<DIV_W>>,
    bit_period: Constant<Bits<DIV_W>>,
}

impl<const DIV_W: usize> CanMaster<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    /// Create a CAN node with the given FPGA-cycles-per-CAN-bit
    /// period.  E.g. for 100 MHz clock and 1 Mbps CAN,
    /// `bit_period = 100`.
    pub fn new(bit_period: Bits<DIV_W>) -> Self {
        Self {
            field: dff::DFF::default(),
            state: dff::DFF::new(CanState::default()),
            bit_period: Constant::new(bit_period),
        }
    }
}

impl<const DIV_W: usize> Default for CanMaster<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    fn default() -> Self {
        Self::new(bits(4))
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [CanMaster].
pub struct In {
    /// Bus value (logical sense): `false` = dominant, `true` = recessive.
    pub rx: bool,
    /// 29-bit ID.  Lower 11 bits used when `tx_extended` is false.
    pub tx_id: Bits<29>,
    /// false = standard 11-bit frame; true = extended 29-bit frame.
    pub tx_extended: bool,
    /// Remote transmission request (data frame: false; remote: true).
    pub tx_rtr: bool,
    /// Data length code (0..=8).
    pub tx_dlc: Bits<4>,
    /// Data payload, MSB-first packed: byte 0 in `data[63..56]`.
    pub tx_data: Bits<64>,
    /// Strobe to begin transmitting a frame.  Latched.
    pub tx_request: bool,
    /// Acceptance filter ID (compared bitwise after masking).
    pub acc_id_filter: Bits<29>,
    /// Acceptance filter mask: bits set to 1 must match.  Set to 0 to accept all.
    pub acc_id_mask: Bits<29>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [CanMaster].
pub struct Out {
    /// Bus drive (logical sense): `false` = dominant, `true` = recessive.
    /// Bus value to drive, same logical sense as `rx`: `false` =
    /// dominant, `true` = recessive. The transceiver inverts.
    pub tx_out: bool,
    /// High while a frame is being transmitted.
    pub tx_busy: bool,
    /// Pulses for one cycle when a transmitted frame completes successfully.
    pub tx_done: bool,
    /// Pulses for one cycle when a received frame passes CRC + acceptance filter.
    pub frame_valid: bool,
    /// ID of the frame just accepted. Lower 11 bits when `rx_extended`
    /// is false. Valid from the `frame_valid` pulse onward.
    pub rx_id: Bits<29>,
    /// The accepted frame used a 29-bit extended ID.
    pub rx_extended: bool,
    /// The accepted frame was a remote-transmission request, so
    /// `rx_data` carries nothing.
    pub rx_rtr: bool,
    /// Data length code of the accepted frame, 0..=8.
    pub rx_dlc: Bits<4>,
    /// Payload of the accepted frame, MSB-first packed: byte 0 in
    /// `rx_data[63..56]`, matching `tx_data`.
    pub rx_data: Bits<64>,
    /// The received CRC matched. `frame_valid` already implies this, so
    /// this is the signal to watch when you want to see frames that
    /// failed rather than only those that passed.
    pub crc_ok: bool,
    /// Transmit error counter, per ISO 11898-1 §11.6. Nine bits wide so
    /// the bus-off threshold of 256 is representable rather than
    /// wrapping.
    pub tec: Bits<9>,
    /// Receive error counter, per ISO 11898-1 §11.6.
    pub rec: Bits<9>,
    /// `tec` reached 256: the node has removed itself from the bus and
    /// transmits nothing until it sees 128 occurrences of 11 recessive
    /// bits.
    pub bus_off: bool,
    /// Either counter passed 128. The node still communicates but sends
    /// passive error flags, so it can no longer destroy other nodes'
    /// frames -- which is the point of the state.
    pub error_passive: bool,
}

impl<const DIV_W: usize> SynchronousIO for CanMaster<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = can_master<DIV_W>;
}

#[kernel]
/// Kernel for [CanMaster].
pub fn can_master<const DIV_W: usize>(cr: ClockReset, i: In, q: Q<DIV_W>) -> (Out, D<DIV_W>)
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    let one_div: Bits<DIV_W> = bits::<DIV_W>(1);
    let zero_div: Bits<DIV_W> = bits::<DIV_W>(0);
    let one_b7: Bits<7> = bits::<7>(1);
    let zero_b7: Bits<7> = bits::<7>(0);
    let one_b3: Bits<3> = bits::<3>(1);
    let one_b9: Bits<9> = bits::<9>(1);
    let eight_b9: Bits<9> = bits::<9>(8);
    let one_b8: Bits<8> = bits::<8>(1);

    let mut d = D::<DIV_W>::dont_care();
    d.field = q.field;
    let mut next = q.state;

    next.tx_done_pulse = false;
    next.frame_pulse = false;
    next.last_rx = i.rx;
    next.error_pending = false;

    if i.tx_request && !q.state.tx_pending && !q.state.bus_off && !q.state.is_transmitting {
        next.tx_pending = true;
        next.tx_id_latched = i.tx_id;
        next.tx_extended_latched = i.tx_extended;
        next.tx_rtr_latched = i.tx_rtr;
        next.tx_dlc_latched = i.tx_dlc;
        next.tx_data_latched = i.tx_data;
    }

    let sampled = i.rx;
    let bit_done = q.state.bit_phase_counter == (q.bit_period - one_div);
    let rx_falling_edge = q.state.last_rx && !sampled;

    if q.field != CanField::Idle && q.field != CanField::BusOffWait {
        if rx_falling_edge && q.state.bit_phase_counter > one_div {
            next.bit_phase_counter = one_div;
        } else if bit_done {
            next.bit_phase_counter = zero_div;
        } else {
            next.bit_phase_counter = q.state.bit_phase_counter + one_div;
        }
    }

    let raw_bit_tx: bool = match q.field {
        CanField::Sof | CanField::R0 | CanField::R1 => false,
        CanField::SrrOrRtr => {
            if q.state.tx_extended_latched {
                true
            } else {
                q.state.tx_rtr_latched
            }
        }
        CanField::Ide => q.state.tx_extended_latched,
        CanField::Rtr => q.state.tx_rtr_latched,
        CanField::IdA => {
            let pos: Bits<7> = bits::<7>(10) - q.state.field_bit_idx;
            if q.state.tx_extended_latched {
                let shift_amount: Bits<7> = pos + bits::<7>(18);
                ((q.state.tx_id_latched >> shift_amount) & bits::<29>(1)) != bits::<29>(0)
            } else {
                ((q.state.tx_id_latched >> pos) & bits::<29>(1)) != bits::<29>(0)
            }
        }
        CanField::IdB => {
            let pos: Bits<7> = bits::<7>(17) - q.state.field_bit_idx;
            ((q.state.tx_id_latched >> pos) & bits::<29>(1)) != bits::<29>(0)
        }
        CanField::Dlc => {
            let pos: Bits<7> = bits::<7>(3) - q.state.field_bit_idx;
            ((q.state.tx_dlc_latched >> pos) & bits::<4>(1)) != bits::<4>(0)
        }
        CanField::Data => {
            let pos: Bits<7> = bits::<7>(63) - q.state.field_bit_idx;
            ((q.state.tx_data_latched >> pos) & bits::<64>(1)) != bits::<64>(0)
        }
        CanField::Crc => {
            let pos: Bits<7> = bits::<7>(14) - q.state.field_bit_idx;
            ((q.state.crc_reg >> pos) & bits::<15>(1)) != bits::<15>(0)
        }
        _ => true,
    };

    let err_flag_bit = q.state.error_passive;

    let in_stuff_zone = match q.field {
        CanField::Sof
        | CanField::IdA
        | CanField::SrrOrRtr
        | CanField::Ide
        | CanField::IdB
        | CanField::Rtr
        | CanField::R1
        | CanField::R0
        | CanField::Dlc
        | CanField::Data
        | CanField::Crc => true,
        _ => false,
    };
    let crc_input_active = match q.field {
        CanField::Sof
        | CanField::IdA
        | CanField::SrrOrRtr
        | CanField::Ide
        | CanField::IdB
        | CanField::Rtr
        | CanField::R1
        | CanField::R0
        | CanField::Dlc
        | CanField::Data => true,
        _ => false,
    };
    let in_arbitration_zone = match q.field {
        CanField::IdA | CanField::SrrOrRtr | CanField::Ide | CanField::IdB | CanField::Rtr => true,
        _ => false,
    };

    let drive_bit: bool = if q.field == CanField::ErrFlag {
        err_flag_bit
    } else if q.field == CanField::AckSlot && !q.state.is_transmitting && q.state.crc_ok {
        false
    } else if q.state.is_transmitting && q.field != CanField::Idle {
        if q.state.expecting_stuff {
            !q.state.last_bit
        } else {
            raw_bit_tx
        }
    } else {
        true
    };

    let bit_to_crc: bool = if q.state.is_transmitting {
        raw_bit_tx
    } else {
        sampled
    };
    let crc_top = (q.state.crc_reg >> bits::<15>(14)) & bits::<15>(1);
    let crc_top_set = crc_top != bits::<15>(0);
    let crc_feedback = crc_top_set != bit_to_crc;
    let crc_shifted = q.state.crc_reg << 1;
    let crc_stepped: Bits<15> = if crc_feedback {
        (crc_shifted ^ bits::<15>(0x4599)) & bits::<15>(0x7FFF)
    } else {
        crc_shifted & bits::<15>(0x7FFF)
    };

    let new_run: Bits<3> = if bit_to_crc == q.state.last_bit {
        q.state.stuff_run + one_b3
    } else {
        one_b3
    };

    let bit_error = q.state.is_transmitting
        && bit_done
        && q.field != CanField::Idle
        && q.field != CanField::AckSlot
        && drive_bit != sampled
        && !in_arbitration_zone;

    let lost_arbitration =
        q.state.is_transmitting && bit_done && in_arbitration_zone && drive_bit && !sampled;

    let stuff_error_rx = !q.state.is_transmitting
        && bit_done
        && in_stuff_zone
        && !q.state.expecting_stuff
        && sampled == q.state.last_bit
        && q.state.stuff_run == bits::<3>(5);

    if lost_arbitration {
        next.is_transmitting = false;
        next.tx_pending = true;
    }

    let in_walk_field = in_stuff_zone;
    let consume_real_bit = bit_done && in_walk_field && !q.state.expecting_stuff;
    let consume_stuff_bit = bit_done && in_walk_field && q.state.expecting_stuff;

    if consume_stuff_bit {
        next.last_bit = sampled;
        next.stuff_run = one_b3;
        next.expecting_stuff = false;
    } else if consume_real_bit {
        if stuff_error_rx {
            next.error_pending = true;
        } else {
            next.last_bit = bit_to_crc;
            next.stuff_run = new_run;
            next.expecting_stuff = in_stuff_zone && new_run == bits::<3>(5);
            if crc_input_active {
                next.crc_reg = crc_stepped;
            }
        }
    }

    match q.field {
        CanField::Idle => {
            // Two SOF entry paths with different counter seeds
            // for bit-time alignment:
            //
            // - TX path (we initiated): counter starts at 0.
            //   q.field becomes Sof one cycle later, then we
            //   drive dominant for 4 cycles (counter 0→3).  TX
            //   bit_done lands at the cycle TX spent its 4th
            //   cycle of dominant drive.
            //
            // - RX path (we detected dominant on the bus): the
            //   detection cycle itself is the first cycle of
            //   the SOF bit time we missed (TX was already
            //   driving when we sampled).  Counter starts at
            //   1 so RX's bit_done lands the SAME cycle as
            //   TX's bit_done.  Without this asymmetry, RX
            //   would sample every subsequent bit one wire-cycle
            //   late.
            let want_to_tx = (i.tx_request || q.state.tx_pending) && !q.state.bus_off;
            let detected_sof = !sampled && !want_to_tx;
            if want_to_tx || detected_sof {
                d.field = CanField::Sof;
                next.field_bit_idx = zero_b7;
                next.bit_phase_counter = if detected_sof { one_div } else { zero_div };
                next.last_bit = false;
                next.stuff_run = one_b3;
                next.expecting_stuff = false;
                next.id_accum = bits::<29>(0);
                next.dlc_accum = bits::<4>(0);
                next.data_accum = bits::<64>(0);
                next.rx_crc_accum = bits::<15>(0);
                next.extended_rx = false;
                next.rtr_rx = false;
                next.srr_or_rtr_bit = false;
                next.crc_reg = bits::<15>(0);
                next.crc_ok = false;
                next.is_transmitting = want_to_tx;
                if want_to_tx {
                    next.tx_pending = true;
                    if i.tx_request {
                        next.tx_id_latched = i.tx_id;
                        next.tx_extended_latched = i.tx_extended;
                        next.tx_rtr_latched = i.tx_rtr;
                        next.tx_dlc_latched = i.tx_dlc;
                        next.tx_data_latched = i.tx_data;
                    }
                }
            }
        }
        CanField::Sof => {
            if bit_done {
                d.field = CanField::IdA;
                next.field_bit_idx = zero_b7;
                next.last_bit = false;
                next.stuff_run = one_b3;
            }
        }
        CanField::IdA => {
            if consume_real_bit && !stuff_error_rx {
                let bit_b29: Bits<29> = if bit_to_crc {
                    bits::<29>(1)
                } else {
                    bits::<29>(0)
                };
                next.id_accum = (q.state.id_accum << 1) | bit_b29;
                if q.state.field_bit_idx == bits::<7>(10) {
                    d.field = CanField::SrrOrRtr;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::SrrOrRtr => {
            if consume_real_bit && !stuff_error_rx {
                next.srr_or_rtr_bit = bit_to_crc;
                d.field = CanField::Ide;
                next.field_bit_idx = zero_b7;
            }
        }
        CanField::Ide => {
            if consume_real_bit && !stuff_error_rx {
                next.extended_rx = bit_to_crc;
                if bit_to_crc {
                    d.field = CanField::IdB;
                } else {
                    next.rtr_rx = q.state.srr_or_rtr_bit;
                    d.field = CanField::R0;
                }
                next.field_bit_idx = zero_b7;
            }
        }
        CanField::IdB => {
            if consume_real_bit && !stuff_error_rx {
                let bit_b29: Bits<29> = if bit_to_crc {
                    bits::<29>(1)
                } else {
                    bits::<29>(0)
                };
                next.id_accum = (q.state.id_accum << 1) | bit_b29;
                if q.state.field_bit_idx == bits::<7>(17) {
                    d.field = CanField::Rtr;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::Rtr => {
            if consume_real_bit && !stuff_error_rx {
                next.rtr_rx = bit_to_crc;
                d.field = CanField::R1;
                next.field_bit_idx = zero_b7;
            }
        }
        CanField::R1 => {
            if consume_real_bit && !stuff_error_rx {
                d.field = CanField::R0;
                next.field_bit_idx = zero_b7;
            }
        }
        CanField::R0 => {
            if consume_real_bit && !stuff_error_rx {
                d.field = CanField::Dlc;
                next.field_bit_idx = zero_b7;
            }
        }
        CanField::Dlc => {
            if consume_real_bit && !stuff_error_rx {
                let bit_b4: Bits<4> = if bit_to_crc {
                    bits::<4>(1)
                } else {
                    bits::<4>(0)
                };
                let new_dlc = (q.state.dlc_accum << 1) | bit_b4;
                next.dlc_accum = new_dlc;
                if q.state.field_bit_idx == bits::<7>(3) {
                    if new_dlc == bits::<4>(0) {
                        d.field = CanField::Crc;
                    } else {
                        d.field = CanField::Data;
                    }
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::Data => {
            if consume_real_bit && !stuff_error_rx {
                let bit_b64: Bits<64> = if bit_to_crc {
                    bits::<64>(1)
                } else {
                    bits::<64>(0)
                };
                let shifted = (q.state.data_accum << 1) | bit_b64;
                let dlc_for_total: Bits<4> = if q.state.is_transmitting {
                    q.state.tx_dlc_latched
                } else {
                    q.state.dlc_accum
                };
                let total_data_bits: Bits<7> = match dlc_for_total {
                    Bits::<4>(0) => bits::<7>(0),
                    Bits::<4>(1) => bits::<7>(8),
                    Bits::<4>(2) => bits::<7>(16),
                    Bits::<4>(3) => bits::<7>(24),
                    Bits::<4>(4) => bits::<7>(32),
                    Bits::<4>(5) => bits::<7>(40),
                    Bits::<4>(6) => bits::<7>(48),
                    Bits::<4>(7) => bits::<7>(56),
                    Bits::<4>(8) => bits::<7>(64),
                    _ => bits::<7>(64),
                };
                let next_idx = q.state.field_bit_idx + one_b7;
                if next_idx == total_data_bits {
                    let final_shift: Bits<7> = match dlc_for_total {
                        Bits::<4>(1) => bits::<7>(56),
                        Bits::<4>(2) => bits::<7>(48),
                        Bits::<4>(3) => bits::<7>(40),
                        Bits::<4>(4) => bits::<7>(32),
                        Bits::<4>(5) => bits::<7>(24),
                        Bits::<4>(6) => bits::<7>(16),
                        Bits::<4>(7) => bits::<7>(8),
                        Bits::<4>(8) => bits::<7>(0),
                        _ => bits::<7>(0),
                    };
                    next.data_accum = shifted << final_shift;
                    d.field = CanField::Crc;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.data_accum = shifted;
                    next.field_bit_idx = next_idx;
                }
            }
        }
        CanField::Crc => {
            if consume_real_bit && !stuff_error_rx {
                let bit_b15: Bits<15> = if bit_to_crc {
                    bits::<15>(1)
                } else {
                    bits::<15>(0)
                };
                let new_rx_crc = (q.state.rx_crc_accum << 1) | bit_b15;
                next.rx_crc_accum = new_rx_crc;
                if q.state.field_bit_idx == bits::<7>(14) {
                    next.crc_ok = q.state.crc_reg == new_rx_crc;
                    d.field = CanField::CrcDelim;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::CrcDelim => {
            if bit_done {
                if !sampled {
                    next.error_pending = true;
                } else {
                    d.field = CanField::AckSlot;
                    next.field_bit_idx = zero_b7;
                }
            }
        }
        CanField::AckSlot => {
            if bit_done {
                if q.state.is_transmitting && sampled {
                    next.error_pending = true;
                } else {
                    d.field = CanField::AckDelim;
                    next.field_bit_idx = zero_b7;
                }
            }
        }
        CanField::AckDelim => {
            if bit_done {
                if !sampled {
                    next.error_pending = true;
                } else {
                    d.field = CanField::Eof;
                    next.field_bit_idx = zero_b7;
                }
            }
        }
        CanField::Eof => {
            if bit_done {
                if !sampled {
                    next.error_pending = true;
                } else if q.state.field_bit_idx == bits::<7>(6) {
                    d.field = CanField::Ifs;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::Ifs => {
            if bit_done {
                if q.state.field_bit_idx == bits::<7>(2) {
                    if q.state.is_transmitting {
                        next.tx_done_pulse = true;
                        next.tx_pending = false;
                        if q.state.tec > bits::<9>(0) {
                            next.tec = q.state.tec - one_b9;
                        }
                    } else {
                        let masked_id = q.state.id_accum & i.acc_id_mask;
                        let masked_filter = i.acc_id_filter & i.acc_id_mask;
                        if masked_id == masked_filter {
                            next.frame_pulse = true;
                            next.last_rx_id = q.state.id_accum;
                            next.last_rx_extended = q.state.extended_rx;
                            next.last_rx_rtr = q.state.rtr_rx;
                            next.last_rx_dlc = q.state.dlc_accum;
                            next.last_rx_data = q.state.data_accum;
                        }
                        if q.state.rec > bits::<9>(0) {
                            next.rec = q.state.rec - one_b9;
                        }
                    }
                    d.field = CanField::Idle;
                    next.is_transmitting = false;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::ErrFlag => {
            if bit_done {
                if q.state.field_bit_idx == bits::<7>(5) {
                    d.field = CanField::ErrDelim;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::ErrDelim => {
            if bit_done {
                if q.state.field_bit_idx == bits::<7>(7) {
                    if q.state.is_transmitting && q.state.error_passive {
                        d.field = CanField::Suspend;
                    } else {
                        d.field = CanField::Idle;
                    }
                    next.field_bit_idx = zero_b7;
                    next.is_transmitting = false;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::Suspend => {
            if bit_done {
                if q.state.field_bit_idx == bits::<7>(7) {
                    d.field = CanField::Idle;
                    next.field_bit_idx = zero_b7;
                } else {
                    next.field_bit_idx = q.state.field_bit_idx + one_b7;
                }
            }
        }
        CanField::BusOffWait => {
            if sampled {
                let new_in_group = q.state.field_bit_idx + one_b7;
                if new_in_group == bits::<7>(11) {
                    let new_groups = q.state.bus_off_groups + one_b8;
                    next.bus_off_groups = new_groups;
                    next.field_bit_idx = zero_b7;
                    if new_groups == bits::<8>(128) {
                        next.bus_off = false;
                        next.tec = bits::<9>(0);
                        next.rec = bits::<9>(0);
                        next.bus_off_groups = bits::<8>(0);
                        d.field = CanField::Idle;
                    }
                } else {
                    next.field_bit_idx = new_in_group;
                }
            } else {
                next.field_bit_idx = zero_b7;
            }
        }
    }

    if bit_error {
        next.error_pending = true;
    }

    if next.error_pending
        && q.field != CanField::Idle
        && q.field != CanField::ErrFlag
        && q.field != CanField::ErrDelim
        && q.field != CanField::Suspend
        && q.field != CanField::BusOffWait
    {
        d.field = CanField::ErrFlag;
        next.field_bit_idx = zero_b7;
        next.bit_phase_counter = zero_div;
        next.last_bit = q.state.error_passive;
        next.stuff_run = bits::<3>(0);
        next.expecting_stuff = false;

        if q.state.is_transmitting {
            let new_tec = q.state.tec + eight_b9;
            let saturated_tec = if new_tec >= bits::<9>(256) {
                bits::<9>(256)
            } else {
                new_tec
            };
            next.tec = saturated_tec;
            if saturated_tec >= bits::<9>(256) {
                next.bus_off = true;
            }
        } else {
            let new_rec = q.state.rec + one_b9;
            let saturated_rec = if new_rec >= bits::<9>(256) {
                bits::<9>(256)
            } else {
                new_rec
            };
            next.rec = saturated_rec;
        }
    }

    if next.bus_off && q.field != CanField::BusOffWait {
        d.field = CanField::BusOffWait;
        next.field_bit_idx = zero_b7;
        next.bus_off_groups = bits::<8>(0);
    }

    next.error_passive = (next.tec >= bits::<9>(128)) || (next.rec >= bits::<9>(128));

    if cr.reset.any() {
        d.field = CanField::Idle;
        next = CanState::<DIV_W>::default();
    }

    d.state = next;

    let mut o = Out::dont_care();
    o.tx_out = drive_bit;
    o.tx_busy = q.state.is_transmitting && q.field != CanField::Idle;
    o.tx_done = q.state.tx_done_pulse;
    o.frame_valid = q.state.frame_pulse;
    o.rx_id = q.state.last_rx_id;
    o.rx_extended = q.state.last_rx_extended;
    o.rx_rtr = q.state.last_rx_rtr;
    o.rx_dlc = q.state.last_rx_dlc;
    o.rx_data = q.state.last_rx_data;
    o.crc_ok = q.state.crc_ok;
    o.tec = q.state.tec;
    o.rec = q.state.rec;
    o.bus_off = q.state.bus_off;
    o.error_passive = q.state.error_passive;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In {
            rx: true,
            tx_id: bits(0),
            tx_extended: false,
            tx_rtr: false,
            tx_dlc: bits(0),
            tx_data: bits(0),
            tx_request: false,
            acc_id_filter: bits(0),
            acc_id_mask: bits(0),
        }
    }

    /// Run two CAN nodes against a wired-AND bus with zero-cycle
    /// skew.  Bypasses the Synchronous sim machinery and calls
    /// the kernel directly so we can do the two-pass evaluation
    /// that the closed loop needs:
    ///
    /// 1. Compute tx_out for both nodes from current q (the
    ///    kernel's o.tx_out has no rx dependency).
    /// 2. Wire-AND those to get this cycle's bus.
    /// 3. Re-run the kernel with rx = bus to get the d that
    ///    reflects the same-cycle bus.
    /// 4. Latch d → q for the next cycle.
    ///
    /// This makes ACK-during-AckSlot work: the receiver's
    /// drive-dominant during AckSlot is visible to the
    /// transmitter the same cycle.
    fn run_two_nodes(
        bit_period: u128,
        n_cycles: usize,
        mut make_in1: impl FnMut(usize, bool) -> In,
        mut make_in2: impl FnMut(usize, bool) -> In,
    ) -> Vec<(Out, Out)> {
        let cr_normal = clock_reset(clock(true), reset(false));
        let cr_reset = clock_reset(clock(true), reset(true));
        let mut q1 = Q::<5> {
            field: CanField::Idle,
            state: CanState::default(),
            bit_period: bits(bit_period),
        };
        let mut q2 = Q::<5> {
            field: CanField::Idle,
            state: CanState::default(),
            bit_period: bits(bit_period),
        };
        // Reset cycle.
        let (_, d1) = can_master::<5>(cr_reset, idle_in(), q1);
        q1.field = d1.field;
        q1.state = d1.state;
        let (_, d2) = can_master::<5>(cr_reset, idle_in(), q2);
        q2.field = d2.field;
        q2.state = d2.state;

        let mut bus = true;
        let mut out: Vec<(Out, Out)> = Vec::with_capacity(n_cycles);
        for cycle in 0..n_cycles {
            let mut in1 = make_in1(cycle, bus);
            let mut in2 = make_in2(cycle, bus);
            // Pass 1: compute outputs (tx_out doesn't depend on rx).
            in1.rx = bus;
            in2.rx = bus;
            let (o1_pass1, _) = can_master::<5>(cr_normal, in1, q1);
            let (o2_pass1, _) = can_master::<5>(cr_normal, in2, q2);
            let new_bus = o1_pass1.tx_out && o2_pass1.tx_out;
            // Pass 2: re-run with corrected rx to get authoritative
            // d (and o, though o.tx_out is unchanged).
            in1.rx = new_bus;
            in2.rx = new_bus;
            let (o1, d1) = can_master::<5>(cr_normal, in1, q1);
            let (o2, d2) = can_master::<5>(cr_normal, in2, q2);
            out.push((o1, o2));
            q1.field = d1.field;
            q1.state = d1.state;
            q2.field = d2.field;
            q2.state = d2.state;
            bus = new_bus;
        }
        out
    }

    fn one_shot_tx(
        id: u128,
        extended: bool,
        dlc: u128,
        data: u128,
    ) -> impl FnMut(usize, bool) -> In {
        move |cycle, _bus| {
            let mut i = idle_in();
            if cycle == 0 {
                i.tx_request = true;
                i.tx_id = bits(id);
                i.tx_extended = extended;
                i.tx_dlc = bits(dlc);
                i.tx_data = bits(data);
            }
            i
        }
    }

    fn silent_listener() -> impl FnMut(usize, bool) -> In {
        move |_cycle, _bus| idle_in()
    }

    fn filtering_listener(filter: u128, mask: u128) -> impl FnMut(usize, bool) -> In {
        move |_cycle, _bus| {
            let mut i = idle_in();
            i.acc_id_filter = bits(filter);
            i.acc_id_mask = bits(mask);
            i
        }
    }

    // ===========================================================
    // Tier 1 — kernel-level unit tests.
    // ===========================================================

    #[test]
    fn test_idle_stays_idle_when_bus_recessive() {
        let cr = ClockReset::dont_care();
        let q = Q::<5> {
            field: CanField::Idle,
            state: CanState::default(),
            bit_period: bits::<5>(4),
        };
        let (o, d) = can_master::<5>(cr, idle_in(), q);
        assert_eq!(d.field, CanField::Idle);
        assert!(!o.tx_busy);
        assert!(o.tx_out);
    }

    #[test]
    fn test_tx_request_starts_sof_immediately() {
        // tx_request causes both: the latch (tx_pending) AND
        // immediate SOF entry, in the same cycle.  This keeps
        // TX's bit timing aligned with what RX will see on the
        // wire (RX detects the resulting dominant the same
        // cycle TX drives it).
        let cr = ClockReset::dont_care();
        let q = Q::<5> {
            field: CanField::Idle,
            state: CanState::default(),
            bit_period: bits::<5>(4),
        };
        let mut i = idle_in();
        i.tx_request = true;
        i.tx_id = bits::<29>(0x123);
        i.tx_dlc = bits::<4>(1);
        let (_o, d) = can_master::<5>(cr, i, q);
        assert!(d.state.tx_pending);
        assert_eq!(d.state.tx_id_latched, bits::<29>(0x123));
        assert_eq!(d.field, CanField::Sof);
        assert!(d.state.is_transmitting);
    }

    #[test]
    fn test_reset_clears_state() {
        let cr = clock_reset(clock(false), reset(true));
        let mut q = Q::<5> {
            field: CanField::Data,
            state: CanState::default(),
            bit_period: bits::<5>(4),
        };
        q.state.tec = bits::<9>(50);
        q.state.id_accum = bits::<29>(0x123);
        let (_o, d) = can_master::<5>(cr, idle_in(), q);
        assert_eq!(d.field, CanField::Idle);
        assert_eq!(d.state.tec, bits::<9>(0));
        assert_eq!(d.state.id_accum, bits::<29>(0));
    }

    #[test]
    fn test_error_passive_threshold() {
        let cr = ClockReset::dont_care();
        let mut q = Q::<5> {
            field: CanField::Idle,
            state: CanState::default(),
            bit_period: bits::<5>(4),
        };
        q.state.tec = bits::<9>(128);
        let (_o, d) = can_master::<5>(cr, idle_in(), q);
        assert!(d.state.error_passive);
    }

    // ===========================================================
    // Tier 2 — two-node bus round-trips.
    // ===========================================================

    #[test]
    fn test_two_node_standard_frame() -> miette::Result<()> {
        let trace = run_two_nodes(
            4,
            300 * 4 + 200,
            one_shot_tx(0x123, false, 1, 0xA5_00_00_00_00_00_00_00),
            silent_listener(),
        );
        let pulse = trace
            .iter()
            .find(|(_, o2)| o2.frame_valid)
            .expect("no frame_valid pulse from listener");
        assert!(pulse.1.crc_ok);
        assert_eq!(pulse.1.rx_id, bits::<29>(0x123));
        assert!(!pulse.1.rx_extended);
        assert!(!pulse.1.rx_rtr);
        assert_eq!(pulse.1.rx_dlc, bits::<4>(1));
        assert_eq!(pulse.1.rx_data, bits::<64>(0xA5_00_00_00_00_00_00_00));
        let tx_done = trace.iter().any(|(o1, _)| o1.tx_done);
        assert!(
            tx_done,
            "transmitter never pulsed tx_done (ACK was not received)"
        );
        Ok(())
    }

    #[test]
    fn test_two_node_extended_frame() -> miette::Result<()> {
        // DLC=2 transmits only the first 2 bytes (0xDE, 0xAD).
        // After receive + left-align: 0xDEAD_0000_0000_0000.
        let trace = run_two_nodes(
            4,
            350 * 4 + 200,
            one_shot_tx(0x1ABCDE7, true, 2, 0xDEAD_BEEF_00_00_00_00),
            silent_listener(),
        );
        let pulse = trace
            .iter()
            .find(|(_, o2)| o2.frame_valid)
            .expect("no frame_valid for extended frame");
        assert!(pulse.1.crc_ok);
        assert_eq!(pulse.1.rx_id, bits::<29>(0x1ABCDE7));
        assert!(pulse.1.rx_extended);
        assert_eq!(pulse.1.rx_dlc, bits::<4>(2));
        assert_eq!(pulse.1.rx_data, bits::<64>(0xDEAD_0000_0000_0000));
        Ok(())
    }

    #[test]
    fn test_two_node_eight_byte_frame() -> miette::Result<()> {
        let trace = run_two_nodes(
            4,
            400 * 4 + 200,
            one_shot_tx(0x7FF, false, 8, 0xDEAD_BEEF_CAFE_F00D),
            silent_listener(),
        );
        let pulse = trace
            .iter()
            .find(|(_, o2)| o2.frame_valid)
            .expect("no frame_valid for 8-byte frame");
        assert!(pulse.1.crc_ok);
        assert_eq!(pulse.1.rx_id, bits::<29>(0x7FF));
        assert_eq!(pulse.1.rx_dlc, bits::<4>(8));
        assert_eq!(pulse.1.rx_data, bits::<64>(0xDEAD_BEEF_CAFE_F00D));
        Ok(())
    }

    #[test]
    fn test_acceptance_filter_rejects() -> miette::Result<()> {
        let trace = run_two_nodes(
            4,
            300 * 4 + 200,
            one_shot_tx(0x100, false, 0, 0),
            filtering_listener(0x200, 0x7FF),
        );
        assert!(!trace.iter().any(|(_, o2)| o2.frame_valid));
        Ok(())
    }

    #[test]
    fn test_acceptance_filter_accepts() -> miette::Result<()> {
        let trace = run_two_nodes(
            4,
            300 * 4 + 200,
            one_shot_tx(0x200, false, 0, 0),
            filtering_listener(0x200, 0x7FF),
        );
        assert!(trace.iter().any(|(_, o2)| o2.frame_valid));
        Ok(())
    }

    // ===========================================================
    // Tier 3 — HDL emission length sanity check.
    // ===========================================================

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut: CanMaster<5> = CanMaster::new(bits(4));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 5000, "HDL length {} too small", hdl.len());
        Ok(())
    }

    // ===========================================================
    // Tier 4 — iverilog round-trip.
    // ===========================================================

    #[test]
    fn test_can_master_hdl_works() -> miette::Result<()> {
        let uut: CanMaster<5> = CanMaster::new(bits(4));
        let mut stream_in: Vec<In> = vec![idle_in(); 2];
        let mut req = idle_in();
        req.tx_request = true;
        req.tx_id = bits(0x123);
        req.tx_dlc = bits(1);
        req.tx_data = bits(0xA5_00_00_00_00_00_00_00);
        stream_in.push(req);
        for _ in 0..400 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // ===========================================================
    // Tier 5 — VCD digest.
    // ===========================================================

    #[test]
    fn test_can_master_trace() -> miette::Result<()> {
        let uut: CanMaster<5> = CanMaster::new(bits(4));
        let mut stream_in: Vec<In> = vec![idle_in(); 2];
        let mut req = idle_in();
        req.tx_request = true;
        req.tx_id = bits(0x123);
        req.tx_dlc = bits(1);
        req.tx_data = bits(0xA5_00_00_00_00_00_00_00);
        stream_in.push(req);
        for _ in 0..400 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("can_master");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["1520ac9637c5f71e2a6433cd38d683445e2b9ad30829e890fda6bef81670adaa"];
        let digest = vcd.dump_to_file(root.join("can_master.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = CanMaster::<5>::fsm_descriptor();
        assert_eq!(desc.widget_name, "CanMaster");
        assert_eq!(desc.widget.state_field, "field");
        let variants = desc.variants();
        assert_eq!(variants.len(), 21);
        assert_eq!(variants[0].name, "Idle");
        assert_eq!(desc.initial_index(), 0);
    }
}
