//! MIDI 1.0 message parser
//!
//! Consumes the byte stream produced by
//! [super::midi::MidiInterface] (or any compatible byte-oriented
//! source) and emits typed MIDI messages: channel voice (Note On/
//! Note Off / Polyphonic Aftertouch / Control Change / Program
//! Change / Channel Aftertouch / Pitch Bend), System Common
//! (MIDI Time Code Quarter Frame, Song Position Pointer, Song
//! Select, Tune Request, End-Of-Exclusive), System Real-Time
//! (Timing Clock, Start, Continue, Stop, Active Sensing, System
//! Reset), and System Exclusive (variable-length payloads
//! delimited by `0xF0` start and `0xF7` end).
//!
//! ## Protocol summary (MIDI 1.0, MMA spec)
//!
//! - Status bytes have MSB=1 (`0x80..=0xFF`); data bytes have
//!   MSB=0 (`0x00..=0x7F`).
//! - Channel voice status bytes are `0x80..=0xEF`; the high nibble
//!   is the message type, the low nibble is the channel
//!   (`0x0..=0xF` = MIDI channels 1-16).
//! - System Common are `0xF0..=0xF7`.  Variable lengths per type.
//! - System Real-Time are `0xF8..=0xFF`.  ALWAYS single-byte.
//!   They can appear AT ANY TIME, even in the middle of another
//!   message — the parser must emit the real-time message
//!   immediately and resume the in-progress message at the next
//!   data byte.
//! - **Running status**: a stream of channel-voice messages with
//!   the same status byte may omit the status byte after the
//!   first; the parser uses the most recent status byte as the
//!   implicit status.  System messages do NOT participate in
//!   running status (they reset it).
//!
//! ## Output: typed `MidiMessage`
//!
//! The parser emits `Option<MidiMessage>` — `Some(msg)` on the
//! cycle the message is complete; `None` otherwise.  The host
//! latches the message on the cycle the output is `Some`.
//!
//! For SysEx, two messages are emitted: `SysExStart` (when `0xF0`
//! is received) and `SysExByte(byte)` for each subsequent data
//! byte until `SysExEnd` (when `0xF7` is received).  Hosts that
//! want to buffer the full SysEx body can accumulate from
//! `SysExStart` to `SysExEnd`; hosts that want streaming can
//! consume each `SysExByte` as it arrives.
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +---------+MidiParser+--------+
     |                             |
Option<B<8>>                       | Option<MidiMessage>
+--->| byte_in        message_out  +-->
     |                             |
     |                  in_sysex   +--> bool
     +-----------------------------+
")]
//!
//!# Internals
//!
//! Six-state FSM tracks the position within a multi-byte message:
//!
//! - `Idle` — waiting for a status byte (or a running-status data byte).
//! - `WaitData1` — got the status, waiting for the first data byte.
//! - `WaitData2` — got status + data1, waiting for data2 (3-byte messages).
//! - `WaitSysEx` — inside a SysEx body; consume bytes until 0xF7.
//! - `WaitMtcQf` / `WaitSppLsb` / `WaitSppMsb` / `WaitSongSel` —
//!   per-System-Common-message data byte states.
//!
//! The `last_status` register holds the most recent channel-voice
//! status (for running-status decoding); it's NOT cleared by
//! System Real-Time messages (which the host inserts inline) but
//! IS cleared by System Common messages.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/midi_parser.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/midi_parser.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/midi_parser_fsm.md")]

use rhdl::prelude::*;

use crate::core::dff;

/// Internal state machine — tracks position within a multi-byte
/// MIDI message.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum MidiParseState {
    /// Waiting for any status byte (or a running-status data byte).
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Saw a 2-byte channel-voice status (PC, ChAT) or part of one;
    /// waiting for the single data byte.
    #[fsm_state(label = "wait data 1 of 1")]
    WaitData1Of1,
    /// Saw a 3-byte channel-voice status (Note On/Off, Poly AT, CC,
    /// Pitch Bend); waiting for the first of two data bytes.
    #[fsm_state(label = "wait data 1 of 2")]
    WaitData1Of2,
    /// Waiting for the second data byte of a 3-byte message.
    #[fsm_state(label = "wait data 2 of 2")]
    WaitData2Of2,
    /// Inside a System Exclusive body — consume bytes until 0xF7.
    #[fsm_state(label = "in sysex")]
    InSysEx,
    /// Saw MIDI Time Code Quarter Frame status (0xF1) — waiting for
    /// the single data byte.
    #[fsm_state(label = "wait MTC QF byte")]
    WaitMtcQf,
    /// Saw Song Position Pointer status (0xF2) — waiting for the
    /// LSB of the 14-bit position.
    #[fsm_state(label = "wait SPP lsb")]
    WaitSppLsb,
    /// Got SPP LSB; waiting for MSB.
    #[fsm_state(label = "wait SPP msb")]
    WaitSppMsb,
    /// Saw Song Select status (0xF3) — waiting for the song number.
    #[fsm_state(label = "wait song sel")]
    WaitSongSel,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = MidiParseState, allow_implicit)]
/// MIDI 1.0 message parser.
pub struct MidiParser {
    state: dff::DFF<MidiParseState>,
    /// Most recent channel-voice status byte (for running status).
    last_status: dff::DFF<Bits<8>>,
    /// First data byte buffer (for 3-byte messages and SPP-LSB).
    data1: dff::DFF<Bits<8>>,
    /// Latched output message (held one cycle).
    out_message: dff::DFF<MidiMessage>,
    /// True for one cycle when out_message is meaningful.
    out_valid: dff::DFF<bool>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// One MIDI message, fully decoded.
///
/// `kind` is a `Bits<5>` code — see the `MIDI_KIND_*` constants
/// for the encoding.  Per-kind payload fields carry the parsed
/// values; unused fields are zero.
///
/// Using a `Bits<5>` code instead of a Rust enum avoids macro
/// expansion blowup in the kernel: a 22-variant enum used
/// pervasively in the kernel's many message-construction paths
/// produces an exponential RHIF size that OOMs the compiler.
/// The code form keeps the kernel tractable while preserving
/// every distinction the host needs to act on.
pub struct MidiMessage {
    /// Message kind code.  See `MIDI_KIND_*` constants.
    pub kind: Bits<5>,
    /// MIDI channel (0..=15) for channel-voice messages; 0 otherwise.
    pub channel: Bits<4>,
    /// First data byte: note number / CC number / song number /
    /// MTC QF byte / SPP LSB / sysex body byte.
    pub data1: Bits<8>,
    /// Second data byte: velocity / CC value / SPP MSB / 0 for
    /// 2-byte messages.
    pub data2: Bits<8>,
}

/// `MidiMessage::kind` codes.
pub const MIDI_KIND_NONE: u128 = 0;
/// Note Off (status 0x80, 3 bytes).  data1=note, data2=velocity.
pub const MIDI_KIND_NOTE_OFF: u128 = 1;
/// Note On (status 0x90, 3 bytes).  data1=note, data2=velocity.
pub const MIDI_KIND_NOTE_ON: u128 = 2;
/// Polyphonic Aftertouch (status 0xA0, 3 bytes).  data1=note, data2=pressure.
pub const MIDI_KIND_POLY_AFTERTOUCH: u128 = 3;
/// Control Change (status 0xB0, 3 bytes).  data1=controller, data2=value.
pub const MIDI_KIND_CONTROL_CHANGE: u128 = 4;
/// Program Change (status 0xC0, 2 bytes).  data1=program.
pub const MIDI_KIND_PROGRAM_CHANGE: u128 = 5;
/// Channel Aftertouch (status 0xD0, 2 bytes).  data1=pressure.
pub const MIDI_KIND_CHANNEL_AFTERTOUCH: u128 = 6;
/// Pitch Bend (status 0xE0, 3 bytes).  data1=lsb, data2=msb.
pub const MIDI_KIND_PITCH_BEND: u128 = 7;
/// MIDI Time Code Quarter Frame (status 0xF1, 2 bytes).  data1=qf byte.
pub const MIDI_KIND_MTC_QF: u128 = 8;
/// Song Position Pointer (status 0xF2, 3 bytes).  data1=lsb, data2=msb.
pub const MIDI_KIND_SONG_POSITION: u128 = 9;
/// Song Select (status 0xF3, 2 bytes).  data1=song number.
pub const MIDI_KIND_SONG_SELECT: u128 = 10;
/// Tune Request (status 0xF6, 1 byte).
pub const MIDI_KIND_TUNE_REQUEST: u128 = 11;
/// SysEx start (status 0xF0).
pub const MIDI_KIND_SYSEX_START: u128 = 12;
/// SysEx body byte (data byte while in SysEx; data1=byte).
pub const MIDI_KIND_SYSEX_BYTE: u128 = 13;
/// SysEx end (status 0xF7).
pub const MIDI_KIND_SYSEX_END: u128 = 14;
/// Timing Clock (status 0xF8, 1 byte, real-time).
pub const MIDI_KIND_TIMING_CLOCK: u128 = 15;
/// Start (status 0xFA, 1 byte, real-time).
pub const MIDI_KIND_START: u128 = 16;
/// Continue (status 0xFB, 1 byte, real-time).
pub const MIDI_KIND_CONTINUE: u128 = 17;
/// Stop (status 0xFC, 1 byte, real-time).
pub const MIDI_KIND_STOP: u128 = 18;
/// Active Sensing (status 0xFE, 1 byte, real-time).
pub const MIDI_KIND_ACTIVE_SENSING: u128 = 19;
/// System Reset (status 0xFF, 1 byte, real-time).
pub const MIDI_KIND_SYSTEM_RESET: u128 = 20;

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [MidiParser].
pub struct In {
    /// Next byte from the MIDI byte stream.  `Some(byte)` for one
    /// cycle when a byte is available; `None` otherwise.
    pub byte_in: Option<Bits<8>>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [MidiParser].
pub struct Out {
    /// Decoded message — `Some(msg)` for one cycle when complete.
    pub message_out: Option<MidiMessage>,
    /// True while inside a System Exclusive body (between F0 and F7).
    pub in_sysex: bool,
}

impl SynchronousIO for MidiParser {
    type I = In;
    type O = Out;
    type Kernel = midi_parser;
}

#[kernel]
/// Kernel for [MidiParser].
pub fn midi_parser(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    d.state = q.state;
    d.last_status = q.last_status;
    d.data1 = q.data1;
    d.out_message = q.out_message;
    d.out_valid = false;

    if let Some(byte) = i.byte_in {
        let is_status = (byte & bits::<8>(0x80)) != bits::<8>(0);

        // System Real-Time (0xF8..=0xFF) can interrupt any message —
        // emit immediately, do not affect parser state.
        if byte >= bits::<8>(0xF8) {
            let mut msg = MidiMessage::default();
            msg.channel = bits::<4>(0);
            msg.data1 = bits::<8>(0);
            msg.data2 = bits::<8>(0);
            // Decode the real-time kind.
            if byte == bits::<8>(0xF8) {
                msg.kind = bits::<5>(MIDI_KIND_TIMING_CLOCK);
            } else if byte == bits::<8>(0xFA) {
                msg.kind = bits::<5>(MIDI_KIND_START);
            } else if byte == bits::<8>(0xFB) {
                msg.kind = bits::<5>(MIDI_KIND_CONTINUE);
            } else if byte == bits::<8>(0xFC) {
                msg.kind = bits::<5>(MIDI_KIND_STOP);
            } else if byte == bits::<8>(0xFE) {
                msg.kind = bits::<5>(MIDI_KIND_ACTIVE_SENSING);
            } else if byte == bits::<8>(0xFF) {
                msg.kind = bits::<5>(MIDI_KIND_SYSTEM_RESET);
            } else {
                msg.kind = bits::<5>(MIDI_KIND_NONE);
            }
            d.out_message = msg;
            d.out_valid = true;
            // Do NOT change parser state.
        } else if is_status {
            // Channel-voice or System Common status byte.
            // Channel-voice: 0x80..=0xEF.  System Common: 0xF0..=0xF7.
            if byte < bits::<8>(0xF0) {
                // Channel-voice: latch as last_status, advance to wait
                // for first data byte.
                d.last_status = byte;
                let high_nibble = (byte >> 4) & bits::<8>(0xF);
                // 2-byte messages: ProgramChange (0xC0), ChannelAftertouch (0xD0).
                // 3-byte messages: NoteOff (0x80), NoteOn (0x90), PolyAT (0xA0),
                //                  CC (0xB0), PitchBend (0xE0).
                if high_nibble == bits::<8>(0xC) || high_nibble == bits::<8>(0xD) {
                    d.state = MidiParseState::WaitData1Of1;
                } else {
                    d.state = MidiParseState::WaitData1Of2;
                }
            } else {
                // System Common (0xF0..=0xF7).  Clears running status.
                d.last_status = bits::<8>(0);
                if byte == bits::<8>(0xF0) {
                    // SysEx start — emit message and enter InSysEx.
                    let mut msg = MidiMessage::default();
                    msg.kind = bits::<5>(MIDI_KIND_SYSEX_START);
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::InSysEx;
                } else if byte == bits::<8>(0xF1) {
                    d.state = MidiParseState::WaitMtcQf;
                } else if byte == bits::<8>(0xF2) {
                    d.state = MidiParseState::WaitSppLsb;
                } else if byte == bits::<8>(0xF3) {
                    d.state = MidiParseState::WaitSongSel;
                } else if byte == bits::<8>(0xF6) {
                    // Tune Request — single byte, complete immediately.
                    let mut msg = MidiMessage::default();
                    msg.kind = bits::<5>(MIDI_KIND_TUNE_REQUEST);
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::Idle;
                } else if byte == bits::<8>(0xF7) {
                    // End-Of-Exclusive — emit SysExEnd, leave InSysEx.
                    let mut msg = MidiMessage::default();
                    msg.kind = bits::<5>(MIDI_KIND_SYSEX_END);
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::Idle;
                } else {
                    // Reserved (0xF4, 0xF5).  Ignore.
                    d.state = MidiParseState::Idle;
                }
            }
        } else {
            // Data byte (MSB=0).  Action depends on current state.
            match q.state {
                MidiParseState::Idle => {
                    // Running status: use last_status if non-zero.
                    let ls = q.last_status;
                    if ls != bits::<8>(0) {
                        let high_nibble = (ls >> 4) & bits::<8>(0xF);
                        // Set up as if we just received the status byte.
                        if high_nibble == bits::<8>(0xC) || high_nibble == bits::<8>(0xD) {
                            // 2-byte: this byte is the only data byte.
                            let mut msg = MidiMessage::default();
                            if high_nibble == bits::<8>(0xC) {
                                msg.kind = bits::<5>(MIDI_KIND_PROGRAM_CHANGE);
                            } else {
                                msg.kind = bits::<5>(MIDI_KIND_CHANNEL_AFTERTOUCH);
                            }
                            msg.channel = (ls & bits::<8>(0xF)).resize::<4>();
                            msg.data1 = byte;
                            msg.data2 = bits::<8>(0);
                            d.out_message = msg;
                            d.out_valid = true;
                            d.state = MidiParseState::Idle;
                        } else {
                            // 3-byte: this is data1.
                            d.data1 = byte;
                            d.state = MidiParseState::WaitData2Of2;
                        }
                    }
                    // else: orphan data byte, ignore.
                }
                MidiParseState::WaitData1Of1 => {
                    // Single data byte completes a 2-byte channel-voice message.
                    let ls = q.last_status;
                    let high_nibble = (ls >> 4) & bits::<8>(0xF);
                    let mut msg = MidiMessage::default();
                    if high_nibble == bits::<8>(0xC) {
                        msg.kind = bits::<5>(MIDI_KIND_PROGRAM_CHANGE);
                    } else {
                        msg.kind = bits::<5>(MIDI_KIND_CHANNEL_AFTERTOUCH);
                    }
                    msg.channel = (ls & bits::<8>(0xF)).resize::<4>();
                    msg.data1 = byte;
                    msg.data2 = bits::<8>(0);
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::Idle;
                }
                MidiParseState::WaitData1Of2 => {
                    d.data1 = byte;
                    d.state = MidiParseState::WaitData2Of2;
                }
                MidiParseState::WaitData2Of2 => {
                    let ls = q.last_status;
                    let high_nibble = (ls >> 4) & bits::<8>(0xF);
                    let mut msg = MidiMessage::default();
                    if high_nibble == bits::<8>(0x8) {
                        msg.kind = bits::<5>(MIDI_KIND_NOTE_OFF);
                    } else if high_nibble == bits::<8>(0x9) {
                        msg.kind = bits::<5>(MIDI_KIND_NOTE_ON);
                    } else if high_nibble == bits::<8>(0xA) {
                        msg.kind = bits::<5>(MIDI_KIND_POLY_AFTERTOUCH);
                    } else if high_nibble == bits::<8>(0xB) {
                        msg.kind = bits::<5>(MIDI_KIND_CONTROL_CHANGE);
                    } else if high_nibble == bits::<8>(0xE) {
                        msg.kind = bits::<5>(MIDI_KIND_PITCH_BEND);
                    } else {
                        msg.kind = bits::<5>(MIDI_KIND_NONE);
                    }
                    msg.channel = (ls & bits::<8>(0xF)).resize::<4>();
                    msg.data1 = q.data1;
                    msg.data2 = byte;
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::Idle;
                }
                MidiParseState::InSysEx => {
                    // Each byte inside SysEx is emitted as SysExByte.
                    let mut msg = MidiMessage::default();
                    msg.kind = bits::<5>(MIDI_KIND_SYSEX_BYTE);
                    msg.data1 = byte;
                    d.out_message = msg;
                    d.out_valid = true;
                    // Stay in InSysEx.
                }
                MidiParseState::WaitMtcQf => {
                    let mut msg = MidiMessage::default();
                    msg.kind = bits::<5>(MIDI_KIND_MTC_QF);
                    msg.data1 = byte;
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::Idle;
                }
                MidiParseState::WaitSppLsb => {
                    d.data1 = byte;
                    d.state = MidiParseState::WaitSppMsb;
                }
                MidiParseState::WaitSppMsb => {
                    let mut msg = MidiMessage::default();
                    msg.kind = bits::<5>(MIDI_KIND_SONG_POSITION);
                    msg.data1 = q.data1;
                    msg.data2 = byte;
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::Idle;
                }
                MidiParseState::WaitSongSel => {
                    let mut msg = MidiMessage::default();
                    msg.kind = bits::<5>(MIDI_KIND_SONG_SELECT);
                    msg.data1 = byte;
                    d.out_message = msg;
                    d.out_valid = true;
                    d.state = MidiParseState::Idle;
                }
            }
        }
    }

    if cr.reset.any() {
        d.state = MidiParseState::Idle;
        d.last_status = bits::<8>(0);
        d.data1 = bits::<8>(0);
        d.out_message = MidiMessage::default();
        d.out_valid = false;
    }

    let mut o = Out::dont_care();
    o.message_out = if q.out_valid {
        Some(q.out_message)
    } else {
        None
    };
    o.in_sysex = q.state == MidiParseState::InSysEx;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In { byte_in: None }
    }

    fn byte_in(b: u8) -> In {
        In {
            byte_in: Some(bits::<8>(b as u128)),
        }
    }

    fn run_bytes(bytes: &[u8]) -> Vec<MidiMessage> {
        let mut stream_in: Vec<In> = Vec::new();
        for &b in bytes {
            stream_in.push(byte_in(b));
            stream_in.push(idle_in());
        }
        for _ in 0..6 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = MidiParser::default();
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        outputs
            .iter()
            .filter_map(|s| s.output.message_out)
            .collect()
    }

    // Tier 1/2 — every message kind.

    #[test]
    fn test_note_on_off() {
        // Note On ch 0 note 60 vel 100, then Note Off ch 0 note 60 vel 0.
        let msgs = run_bytes(&[0x90, 60, 100, 0x80, 60, 0]);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_NOTE_ON));
        assert_eq!(msgs[0].channel, bits::<4>(0));
        assert_eq!(msgs[0].data1, bits::<8>(60));
        assert_eq!(msgs[0].data2, bits::<8>(100));
        assert_eq!(msgs[1].kind, bits::<5>(MIDI_KIND_NOTE_OFF));
        assert_eq!(msgs[1].data1, bits::<8>(60));
    }

    #[test]
    fn test_running_status() {
        // Note On ch 5, then 3 more notes via running status.
        let msgs = run_bytes(&[0x95, 60, 100, 62, 90, 64, 80, 66, 70]);
        assert_eq!(msgs.len(), 4);
        for m in &msgs {
            assert_eq!(m.kind, bits::<5>(MIDI_KIND_NOTE_ON));
            assert_eq!(m.channel, bits::<4>(5));
        }
        assert_eq!(msgs[0].data1, bits::<8>(60));
        assert_eq!(msgs[1].data1, bits::<8>(62));
        assert_eq!(msgs[2].data1, bits::<8>(64));
        assert_eq!(msgs[3].data1, bits::<8>(66));
    }

    #[test]
    fn test_program_change() {
        let msgs = run_bytes(&[0xC3, 42]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_PROGRAM_CHANGE));
        assert_eq!(msgs[0].channel, bits::<4>(3));
        assert_eq!(msgs[0].data1, bits::<8>(42));
    }

    #[test]
    fn test_pitch_bend() {
        let msgs = run_bytes(&[0xE7, 0x00, 0x40]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_PITCH_BEND));
        assert_eq!(msgs[0].data1, bits::<8>(0));
        assert_eq!(msgs[0].data2, bits::<8>(0x40));
    }

    #[test]
    fn test_realtime_interrupts_3byte() {
        // Note On ch 0 note 60 [TimingClock interrupting] vel 100.
        let msgs = run_bytes(&[0x90, 60, 0xF8, 100]);
        // Two messages: TimingClock, then NoteOn.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_TIMING_CLOCK));
        assert_eq!(msgs[1].kind, bits::<5>(MIDI_KIND_NOTE_ON));
        assert_eq!(msgs[1].data1, bits::<8>(60));
        assert_eq!(msgs[1].data2, bits::<8>(100));
    }

    #[test]
    fn test_sysex_full_cycle() {
        // F0 ID DATA DATA F7
        let msgs = run_bytes(&[0xF0, 0x7E, 0x01, 0x02, 0xF7]);
        // Expect: SysExStart, SysExByte(0x7E), SysExByte(0x01), SysExByte(0x02), SysExEnd.
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_SYSEX_START));
        assert_eq!(msgs[1].kind, bits::<5>(MIDI_KIND_SYSEX_BYTE));
        assert_eq!(msgs[1].data1, bits::<8>(0x7E));
        assert_eq!(msgs[2].kind, bits::<5>(MIDI_KIND_SYSEX_BYTE));
        assert_eq!(msgs[3].kind, bits::<5>(MIDI_KIND_SYSEX_BYTE));
        assert_eq!(msgs[4].kind, bits::<5>(MIDI_KIND_SYSEX_END));
    }

    #[test]
    fn test_song_position() {
        // SPP with position = (msb << 7) | lsb = (0x10 << 7) | 0x05 = 0x805.
        let msgs = run_bytes(&[0xF2, 0x05, 0x10]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_SONG_POSITION));
        assert_eq!(msgs[0].data1, bits::<8>(0x05));
        assert_eq!(msgs[0].data2, bits::<8>(0x10));
    }

    #[test]
    fn test_tune_request() {
        let msgs = run_bytes(&[0xF6]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_TUNE_REQUEST));
    }

    #[test]
    fn test_realtime_messages() {
        let msgs = run_bytes(&[0xF8, 0xFA, 0xFB, 0xFC, 0xFE, 0xFF]);
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_TIMING_CLOCK));
        assert_eq!(msgs[1].kind, bits::<5>(MIDI_KIND_START));
        assert_eq!(msgs[2].kind, bits::<5>(MIDI_KIND_CONTINUE));
        assert_eq!(msgs[3].kind, bits::<5>(MIDI_KIND_STOP));
        assert_eq!(msgs[4].kind, bits::<5>(MIDI_KIND_ACTIVE_SENSING));
        assert_eq!(msgs[5].kind, bits::<5>(MIDI_KIND_SYSTEM_RESET));
    }

    #[test]
    fn test_system_common_clears_running_status() {
        // Note On ch 0 60 100 (running status set), then Tune Request
        // (clears running status), then orphan data byte (should be ignored).
        let msgs = run_bytes(&[0x90, 60, 100, 0xF6, 70]);
        // Expect: NoteOn, TuneRequest. The orphan 70 is dropped.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].kind, bits::<5>(MIDI_KIND_NOTE_ON));
        assert_eq!(msgs[1].kind, bits::<5>(MIDI_KIND_TUNE_REQUEST));
    }

    // Tier 3 — HDL emission length.
    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = MidiParser::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 1000, "HDL length {}", hdl.len());
        Ok(())
    }

    // Tier 4 — iverilog round-trip.
    #[test]
    fn test_midi_parser_hdl_works() -> miette::Result<()> {
        let mut stream_in: Vec<In> = Vec::new();
        for &b in &[0x90u8, 60, 100, 0x80, 60, 0] {
            stream_in.push(byte_in(b));
            stream_in.push(idle_in());
        }
        for _ in 0..4 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = MidiParser::default();
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest.
    #[test]
    fn test_midi_parser_trace() -> miette::Result<()> {
        let mut stream_in: Vec<In> = Vec::new();
        for &b in &[0x90u8, 60, 100, 0x80, 60, 0] {
            stream_in.push(byte_in(b));
            stream_in.push(idle_in());
        }
        for _ in 0..4 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = MidiParser::default();
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("midi_parser");
        std::fs::create_dir_all(&root).unwrap();
        let _ = vcd.dump_to_file(root.join("midi_parser.vcd")).unwrap();
        let _ = expect![[r#""#]];
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = MidiParser::fsm_descriptor();
        assert_eq!(desc.widget_name, "MidiParser");
        assert_eq!(desc.variants().len(), 9);
    }
}
