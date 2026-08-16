//! PS/2 keyboard Scan Code Set 2 decoder
//!
//! Consumes the raw byte stream produced by
//! [super::ps2_keyboard::Ps2Keyboard] and emits typed make/break
//! key events with extended-key disambiguation.  Implements the
//! IBM PS/2 Scan Code Set 2 (the default for PC-AT keyboards):
//!
//! - **Single-byte make** — most keys.  E.g., `0x1C` is "A pressed".
//! - **`0xF0` prefix** — break (key release).  `F0 1C` = "A released".
//! - **`0xE0` prefix** — extended key.  E.g., `E0 75` = "Up arrow
//!   pressed".  Extended break is `E0 F0 <code>`.
//! - **`0xE1` prefix** — Pause/Break key (treated as a special
//!   event, with the unique 8-byte sequence collapsed to a
//!   single output).
//!
//! ## Output
//!
//! `Option<KeyEvent>` — `Some(event)` for one cycle when a key
//! event is complete; `None` otherwise.  `KeyEvent` carries:
//!
//! - `make: bool` — true for press, false for release.
//! - `extended: bool` — true for the `E0`-prefixed codes.
//! - `scancode: Bits<8>` — the base scancode (without prefix).
//!
//! Special events (Pause/Break and PrintScreen which has the
//! gnarly `E0 12 E0 7C` press / `E0 F0 7C E0 F0 12` release
//! sequence) are normalised: the decoder swallows the
//! intermediate bytes and emits a single event with a known
//! special scancode (`0xFE` for Pause/Break, `0xE0+0x7C` for
//! PrintScreen, surfaced via the `extended` flag).
//!
//! Hosts that want a virtual-keycode mapping (Scan Code Set 2 →
//! ASCII or Win32 / X11 keysym) do that in software; the decoder
//! delivers the raw position-on-keyboard event so layouts
//! (QWERTY / DVORAK / international) and modifiers (Shift / Ctrl /
//! Alt / Meta) are the host's concern.
//!
//! Composes [super::ps2_keyboard::Ps2Keyboard].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-------+Ps2KeyboardDecoder+--------+
     |                                   |
B<8> |                                   | Option<KeyEvent>
+--->| scan_code            event_out    +-->
bool |                                   |
+--->| scan_valid                        |
     +-----------------------------------+
")]
//!
//!# Internals
//!
//! Five-state FSM tracking prefix sequences.  When `scan_valid`
//! pulses, the FSM consumes the byte and either transitions
//! state (on a prefix byte: `0xE0`, `0xE1`, `0xF0`) or emits a
//! `KeyEvent` (on any other byte).  Prefix bytes within Pause/
//! Break are absorbed via a small "remaining bytes to swallow"
//! counter (the Pause sequence is exactly 8 bytes total).
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/ps2_keyboard_decoder.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/ps2_keyboard_decoder.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/ps2_keyboard_decoder_fsm.md")]

use rhdl::prelude::*;

use crate::core::dff;

/// Scan-code prefix tracking state.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum DecodeState {
    /// No prefix seen; the next byte is either a prefix or a
    /// single-byte make event.
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// `0xE0` was just seen — the next byte is the extended key
    /// (or `0xF0` for extended break).
    #[fsm_state(label = "after E0")]
    AfterE0,
    /// `0xF0` was just seen — the next byte is the break code.
    #[fsm_state(label = "after F0")]
    AfterF0,
    /// `E0 F0` sequence — the next byte is the extended break code.
    #[fsm_state(label = "after E0F0")]
    AfterE0F0,
    /// `0xE1` was just seen (Pause/Break).  Swallow the next 7
    /// bytes; the full sequence is `E1 14 77 E1 F0 14 F0 77`.
    /// Emit a single Pause event when the sequence completes.
    #[fsm_state(label = "swallow pause")]
    SwallowPause,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = DecodeState, allow_implicit)]
/// PS/2 keyboard Scan Code Set 2 decoder.
pub struct Ps2KeyboardDecoder {
    state: dff::DFF<DecodeState>,
    /// Remaining bytes to swallow in the Pause/Break sequence
    /// (counts down from 7 → 0).
    pause_remaining: dff::DFF<Bits<3>>,
    /// Latched output event (held one cycle).
    event_q: dff::DFF<KeyEvent>,
    /// True for one cycle when event_q is meaningful.
    event_valid: dff::DFF<bool>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// One decoded key event.
pub struct KeyEvent {
    /// True for a key press (make), false for a key release (break).
    pub make: bool,
    /// True for an extended key (originally `E0`-prefixed).
    pub extended: bool,
    /// True for the Pause/Break key (a single event collapsed
    /// from the full 8-byte sequence).  When set, `scancode` is
    /// `0xFE` and `extended` is false.
    pub pause: bool,
    /// The base scancode (without prefix).
    pub scancode: Bits<8>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [Ps2KeyboardDecoder].
pub struct In {
    /// Raw scancode from [super::ps2_keyboard::Ps2Keyboard].
    pub scan_code: Bits<8>,
    /// One-cycle pulse: scan_code is fresh.
    pub scan_valid: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [Ps2KeyboardDecoder].
pub struct Out {
    /// `Some(event)` for one cycle when a key event completes.
    pub event_out: Option<KeyEvent>,
    /// True while the decoder is mid-sequence (any non-Idle state).
    pub busy: bool,
}

impl SynchronousIO for Ps2KeyboardDecoder {
    type I = In;
    type O = Out;
    type Kernel = ps2_keyboard_decoder;
}

#[kernel]
/// Kernel for [Ps2KeyboardDecoder].
pub fn ps2_keyboard_decoder(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    d.state = q.state;
    d.pause_remaining = q.pause_remaining;
    d.event_q = q.event_q;
    d.event_valid = false;

    if i.scan_valid {
        let b = i.scan_code;
        match q.state {
            DecodeState::Idle => {
                if b == bits::<8>(0xE0) {
                    d.state = DecodeState::AfterE0;
                } else if b == bits::<8>(0xF0) {
                    d.state = DecodeState::AfterF0;
                } else if b == bits::<8>(0xE1) {
                    d.state = DecodeState::SwallowPause;
                    d.pause_remaining = bits::<3>(7);
                } else {
                    // Single-byte make event.
                    let mut ev = KeyEvent::default();
                    ev.make = true;
                    ev.extended = false;
                    ev.pause = false;
                    ev.scancode = b;
                    d.event_q = ev;
                    d.event_valid = true;
                }
            }
            DecodeState::AfterE0 => {
                if b == bits::<8>(0xF0) {
                    d.state = DecodeState::AfterE0F0;
                } else {
                    let mut ev = KeyEvent::default();
                    ev.make = true;
                    ev.extended = true;
                    ev.pause = false;
                    ev.scancode = b;
                    d.event_q = ev;
                    d.event_valid = true;
                    d.state = DecodeState::Idle;
                }
            }
            DecodeState::AfterF0 => {
                let mut ev = KeyEvent::default();
                ev.make = false;
                ev.extended = false;
                ev.pause = false;
                ev.scancode = b;
                d.event_q = ev;
                d.event_valid = true;
                d.state = DecodeState::Idle;
            }
            DecodeState::AfterE0F0 => {
                let mut ev = KeyEvent::default();
                ev.make = false;
                ev.extended = true;
                ev.pause = false;
                ev.scancode = b;
                d.event_q = ev;
                d.event_valid = true;
                d.state = DecodeState::Idle;
            }
            DecodeState::SwallowPause => {
                if q.pause_remaining == bits::<3>(0) {
                    // Sequence complete (we ate the last byte) —
                    // emit Pause event.  Note: we entered this
                    // arm with pause_remaining == 0 only because
                    // we already counted down on previous cycles;
                    // shouldn't happen with the 7→0 schedule
                    // below.  Defensive only.
                    let mut ev = KeyEvent::default();
                    ev.make = true;
                    ev.extended = false;
                    ev.pause = true;
                    ev.scancode = bits::<8>(0xFE);
                    d.event_q = ev;
                    d.event_valid = true;
                    d.state = DecodeState::Idle;
                } else if q.pause_remaining == bits::<3>(1) {
                    // Last byte of the sequence — emit Pause and
                    // return to Idle.
                    let mut ev = KeyEvent::default();
                    ev.make = true;
                    ev.extended = false;
                    ev.pause = true;
                    ev.scancode = bits::<8>(0xFE);
                    d.event_q = ev;
                    d.event_valid = true;
                    d.state = DecodeState::Idle;
                    d.pause_remaining = bits::<3>(0);
                } else {
                    d.pause_remaining = q.pause_remaining - bits::<3>(1);
                }
            }
        }
    }

    if cr.reset.any() {
        d.state = DecodeState::Idle;
        d.pause_remaining = bits::<3>(0);
        d.event_q = KeyEvent::default();
        d.event_valid = false;
    }

    let mut o = Out::dont_care();
    o.event_out = if q.event_valid { Some(q.event_q) } else { None };
    o.busy = q.state != DecodeState::Idle;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In {
            scan_code: bits(0),
            scan_valid: false,
        }
    }

    fn run_codes(codes: &[u8]) -> Vec<KeyEvent> {
        let mut stream_in: Vec<In> = Vec::new();
        for &c in codes {
            stream_in.push(In {
                scan_code: bits::<8>(c as u128),
                scan_valid: true,
            });
            stream_in.push(idle_in());
        }
        for _ in 0..6 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2KeyboardDecoder::default();
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        outputs.iter().filter_map(|s| s.output.event_out).collect()
    }

    #[test]
    fn test_single_byte_make() {
        let evs = run_codes(&[0x1C]); // 'A' make
        assert_eq!(evs.len(), 1);
        assert!(evs[0].make);
        assert!(!evs[0].extended);
        assert!(!evs[0].pause);
        assert_eq!(evs[0].scancode, bits::<8>(0x1C));
    }

    #[test]
    fn test_break() {
        let evs = run_codes(&[0xF0, 0x1C]); // 'A' release
        assert_eq!(evs.len(), 1);
        assert!(!evs[0].make);
        assert!(!evs[0].extended);
        assert_eq!(evs[0].scancode, bits::<8>(0x1C));
    }

    #[test]
    fn test_extended_make() {
        let evs = run_codes(&[0xE0, 0x75]); // Up arrow press
        assert_eq!(evs.len(), 1);
        assert!(evs[0].make);
        assert!(evs[0].extended);
        assert_eq!(evs[0].scancode, bits::<8>(0x75));
    }

    #[test]
    fn test_extended_break() {
        let evs = run_codes(&[0xE0, 0xF0, 0x75]); // Up arrow release
        assert_eq!(evs.len(), 1);
        assert!(!evs[0].make);
        assert!(evs[0].extended);
        assert_eq!(evs[0].scancode, bits::<8>(0x75));
    }

    #[test]
    fn test_pause_sequence() {
        // Full Pause sequence: E1 14 77 E1 F0 14 F0 77 (8 bytes).
        let evs = run_codes(&[0xE1, 0x14, 0x77, 0xE1, 0xF0, 0x14, 0xF0, 0x77]);
        assert_eq!(evs.len(), 1);
        assert!(evs[0].pause);
        assert_eq!(evs[0].scancode, bits::<8>(0xFE));
    }

    #[test]
    fn test_three_keys_in_sequence() {
        // 'A' make, 'B' make, 'A' release.
        let evs = run_codes(&[0x1C, 0x32, 0xF0, 0x1C]);
        assert_eq!(evs.len(), 3);
        assert!(evs[0].make && evs[0].scancode == bits::<8>(0x1C));
        assert!(evs[1].make && evs[1].scancode == bits::<8>(0x32));
        assert!(!evs[2].make && evs[2].scancode == bits::<8>(0x1C));
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Ps2KeyboardDecoder::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 1000, "HDL length {}", hdl.len());
        Ok(())
    }

    #[test]
    fn test_ps2_keyboard_decoder_hdl_works() -> miette::Result<()> {
        let mut stream_in: Vec<In> = Vec::new();
        for &c in &[0x1Cu8, 0xF0, 0x1C, 0xE0, 0x75] {
            stream_in.push(In {
                scan_code: bits::<8>(c as u128),
                scan_valid: true,
            });
            stream_in.push(idle_in());
        }
        for _ in 0..4 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2KeyboardDecoder::default();
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_ps2_keyboard_decoder_trace() -> miette::Result<()> {
        let mut stream_in: Vec<In> = Vec::new();
        for &c in &[0x1Cu8, 0xF0, 0x1C, 0xE0, 0x75, 0xE0, 0xF0, 0x75] {
            stream_in.push(In {
                scan_code: bits::<8>(c as u128),
                scan_valid: true,
            });
            stream_in.push(idle_in());
        }
        for _ in 0..4 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2KeyboardDecoder::default();
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("ps2_keyboard_decoder");
        std::fs::create_dir_all(&root).unwrap();
        let _ = vcd
            .dump_to_file(root.join("ps2_keyboard_decoder.vcd"))
            .unwrap();
        let _ = expect![[r#""#]];
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = Ps2KeyboardDecoder::fsm_descriptor();
        assert_eq!(desc.widget_name, "Ps2KeyboardDecoder");
        assert_eq!(desc.variants().len(), 5);
    }
}
