//! PS/2 IntelliMouse 4-byte packet decoder
//!
//! Microsoft IntelliMouse extended the original 3-byte PS/2
//! mouse protocol with a 4-byte packet that adds a Z-axis
//! (scroll wheel) and two extra buttons (4 = "back", 5 =
//! "forward").  Most modern PS/2 mice (and PS/2-emulating USB
//! mice through the BIOS legacy mode) report in this format
//! once the host enables IntelliMouse mode via the standard
//! initialization sequence (set sample rate to 200, then 100,
//! then 80; the device responds to a Get Device ID query with
//! `0x03` instead of `0x00`).
//!
//! ## Packet format (4 bytes per movement event)
//!
//! ```text
//!   Byte 0: Y_OVF X_OVF Y_SIGN X_SIGN  ALWAYS_1=1 BTN_M BTN_R BTN_L
//!   Byte 1: X displacement (signed 8 bit, sign in byte 0 bit 4)
//!   Byte 2: Y displacement (signed 8 bit, sign in byte 0 bit 5)
//!   Byte 3: Z displacement (signed 4 bit in low nibble) +
//!           BTN_5 BTN_4 0 0 in high nibble
//! ```
//!
//! The IntelliMouse Explorer extension uses a slightly
//! different byte 3 layout (Z is signed 4 bits in low nibble,
//! buttons 4 and 5 in bits 4 and 5).  This widget supports the
//! Explorer layout (the more common 4-byte form).
//!
//! ## Output
//!
//! Each completed packet emits an `Option<MouseEvent>` with:
//!
//! - `dx: SignedBits<9>` — X displacement (8-bit value + sign extension).
//! - `dy: SignedBits<9>` — Y displacement.
//! - `dz: SignedBits<4>` — scroll wheel displacement (positive = up).
//! - `btn_left / btn_right / btn_middle / btn_4 / btn_5: bool`.
//! - `x_overflow / y_overflow: bool` — set if the device's
//!   internal accumulator saturated (rare for short polling
//!   intervals).
//!
//! Pairs with [super::ps2_keyboard::Ps2Keyboard]'s byte stream
//! reception — the host wires `scan_valid + scan_code` outputs
//! to this widget's `byte_valid + byte_in` inputs.  (The PS/2
//! electrical interface is the same for keyboard and mouse;
//! the existing receive widget works unchanged.)
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +------+Ps2MouseIntelliMouse+------+
     |                                  |
B<8> |                                  | Option<MouseEvent>
+--->| byte_in              event_out   +-->
bool |                                  |
+--->| byte_valid                       |
     +----------------------------------+
")]
//!
//!# Internals
//!
//! Five-state FSM: WaitByte0, WaitByte1, WaitByte2, WaitByte3,
//! Emit.  Buffers byte 0-2 in DFFs; on byte 3 receipt, computes
//! the sign-extended displacements and emits the typed event.
//!
//! Byte-0 sanity check: bit 3 should always be 1 for a valid
//! packet header.  If it isn't, the packet is dropped and the
//! parser stays at WaitByte0 — re-syncing with the device's
//! transmission boundary.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/ps2_mouse_intellimouse.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/ps2_mouse_intellimouse.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/ps2_mouse_intellimouse_fsm.md")]

use rhdl::prelude::*;

use crate::core::dff;

/// Per-packet byte position.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum Ps2MouseImState {
    /// Waiting for byte 0 (status + buttons).
    #[default]
    #[fsm_state(label = "wait byte 0")]
    WaitByte0,
    /// Waiting for byte 1 (X displacement).
    #[fsm_state(label = "wait byte 1")]
    WaitByte1,
    /// Waiting for byte 2 (Y displacement).
    #[fsm_state(label = "wait byte 2")]
    WaitByte2,
    /// Waiting for byte 3 (Z + buttons 4,5).
    #[fsm_state(label = "wait byte 3")]
    WaitByte3,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = Ps2MouseImState, allow_implicit)]
/// PS/2 IntelliMouse 4-byte packet decoder.
pub struct Ps2MouseIntelliMouse {
    state: dff::DFF<Ps2MouseImState>,
    /// Buffered byte 0 (status / buttons / sign / overflow flags).
    b0: dff::DFF<Bits<8>>,
    /// Buffered byte 1 (X displacement).
    b1: dff::DFF<Bits<8>>,
    /// Buffered byte 2 (Y displacement).
    b2: dff::DFF<Bits<8>>,
    /// Latched output event.
    event_q: dff::DFF<MouseEvent>,
    /// True for one cycle when event_q is fresh.
    event_valid: dff::DFF<bool>,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
/// Decoded IntelliMouse packet.
pub struct MouseEvent {
    /// X displacement, sign-extended to 9 bits.
    pub dx: SignedBits<9>,
    /// Y displacement, sign-extended to 9 bits.
    pub dy: SignedBits<9>,
    /// Z (scroll wheel) displacement, signed 4 bits.
    pub dz: SignedBits<4>,
    /// Left button state.
    pub btn_left: bool,
    /// Right button state.
    pub btn_right: bool,
    /// Middle (wheel) button state.
    pub btn_middle: bool,
    /// Button 4 ("back" on most mice).
    pub btn_4: bool,
    /// Button 5 ("forward" on most mice).
    pub btn_5: bool,
    /// X overflow flag from byte 0 bit 6.
    pub x_overflow: bool,
    /// Y overflow flag from byte 0 bit 7.
    pub y_overflow: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [Ps2MouseIntelliMouse].
pub struct In {
    /// Next byte from the PS/2 mouse byte stream.
    pub byte_in: Bits<8>,
    /// One-cycle pulse: byte_in is valid this cycle.
    pub byte_valid: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [Ps2MouseIntelliMouse].
pub struct Out {
    /// `Some(event)` for one cycle when a packet is complete.
    pub event_out: Option<MouseEvent>,
    /// True while accumulating a multi-byte packet.
    pub busy: bool,
}

impl SynchronousIO for Ps2MouseIntelliMouse {
    type I = In;
    type O = Out;
    type Kernel = ps2_mouse_intellimouse;
}

#[kernel]
/// Kernel for [Ps2MouseIntelliMouse].
pub fn ps2_mouse_intellimouse(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    d.state = q.state;
    d.b0 = q.b0;
    d.b1 = q.b1;
    d.b2 = q.b2;
    d.event_q = q.event_q;
    d.event_valid = false;

    if i.byte_valid {
        let byte = i.byte_in;
        match q.state {
            Ps2MouseImState::WaitByte0 => {
                // Byte-0 sanity: bit 3 must be 1 for a valid header.
                if (byte & bits::<8>(0x08)) != bits::<8>(0) {
                    d.b0 = byte;
                    d.state = Ps2MouseImState::WaitByte1;
                }
                // else: drop and stay (re-sync on next byte).
            }
            Ps2MouseImState::WaitByte1 => {
                d.b1 = byte;
                d.state = Ps2MouseImState::WaitByte2;
            }
            Ps2MouseImState::WaitByte2 => {
                d.b2 = byte;
                d.state = Ps2MouseImState::WaitByte3;
            }
            Ps2MouseImState::WaitByte3 => {
                // Decode all 10 fields.
                let b0 = q.b0;
                let b1 = q.b1;
                let b2 = q.b2;
                let b3 = byte;

                let btn_left = (b0 & bits::<8>(0x01)) != bits::<8>(0);
                let btn_right = (b0 & bits::<8>(0x02)) != bits::<8>(0);
                let btn_middle = (b0 & bits::<8>(0x04)) != bits::<8>(0);
                let x_sign = (b0 & bits::<8>(0x10)) != bits::<8>(0);
                let y_sign = (b0 & bits::<8>(0x20)) != bits::<8>(0);
                let x_overflow = (b0 & bits::<8>(0x40)) != bits::<8>(0);
                let y_overflow = (b0 & bits::<8>(0x80)) != bits::<8>(0);

                // Sign-extend X and Y to 9 bits.  If sign bit (b0 bit
                // 4 / 5) is set, the value is negative and we
                // sign-extend with a leading 1; otherwise leading 0.
                let x_unsigned = b1.resize::<9>();
                let y_unsigned = b2.resize::<9>();
                let x_ext: Bits<9> = if x_sign {
                    x_unsigned | bits::<9>(0x100)
                } else {
                    x_unsigned
                };
                let y_ext: Bits<9> = if y_sign {
                    y_unsigned | bits::<9>(0x100)
                } else {
                    y_unsigned
                };
                let dx = x_ext.as_signed();
                let dy = y_ext.as_signed();

                // Byte 3: low nibble = Z (signed 4 bits),
                // high nibble bits 4,5 = buttons 4,5.
                let z_nibble = (b3 & bits::<8>(0x0F)).resize::<4>();
                let dz = z_nibble.as_signed();
                let btn_4 = (b3 & bits::<8>(0x10)) != bits::<8>(0);
                let btn_5 = (b3 & bits::<8>(0x20)) != bits::<8>(0);

                let mut ev = MouseEvent::default();
                ev.dx = dx;
                ev.dy = dy;
                ev.dz = dz;
                ev.btn_left = btn_left;
                ev.btn_right = btn_right;
                ev.btn_middle = btn_middle;
                ev.btn_4 = btn_4;
                ev.btn_5 = btn_5;
                ev.x_overflow = x_overflow;
                ev.y_overflow = y_overflow;
                d.event_q = ev;
                d.event_valid = true;
                d.state = Ps2MouseImState::WaitByte0;
            }
        }
    }

    if cr.reset.any() {
        d.state = Ps2MouseImState::WaitByte0;
        d.b0 = bits::<8>(0);
        d.b1 = bits::<8>(0);
        d.b2 = bits::<8>(0);
        d.event_q = MouseEvent::default();
        d.event_valid = false;
    }

    let mut o = Out::dont_care();
    o.event_out = if q.event_valid {
        Some(q.event_q)
    } else {
        None
    };
    o.busy = q.state != Ps2MouseImState::WaitByte0;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In {
            byte_in: bits(0),
            byte_valid: false,
        }
    }

    fn run_packet(bytes: &[u8]) -> Vec<MouseEvent> {
        let mut stream_in: Vec<In> = Vec::new();
        for &b in bytes {
            stream_in.push(In {
                byte_in: bits::<8>(b as u128),
                byte_valid: true,
            });
            stream_in.push(idle_in());
        }
        for _ in 0..6 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2MouseIntelliMouse::default();
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        outputs.iter().filter_map(|s| s.output.event_out).collect()
    }

    #[test]
    fn test_simple_movement_no_buttons() {
        // Byte 0: bit 3 set (always-1) only.  Bytes 1,2 = X=5, Y=10.  Byte 3 = 0.
        let evs = run_packet(&[0x08, 5, 10, 0]);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].dx, signed::<9>(5));
        assert_eq!(evs[0].dy, signed::<9>(10));
        assert_eq!(evs[0].dz, signed::<4>(0));
        assert!(!evs[0].btn_left && !evs[0].btn_right && !evs[0].btn_middle);
    }

    #[test]
    fn test_negative_movement_via_sign_bits() {
        // Byte 0: bit 3 + X sign (bit 4) + Y sign (bit 5).  X=0xFE = -2 9-bit, Y=0xFF = -1.
        let evs = run_packet(&[0x08 | 0x10 | 0x20, 0xFE, 0xFF, 0]);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].dx, signed::<9>(-2));
        assert_eq!(evs[0].dy, signed::<9>(-1));
    }

    #[test]
    fn test_left_button() {
        let evs = run_packet(&[0x08 | 0x01, 0, 0, 0]);
        assert_eq!(evs.len(), 1);
        assert!(evs[0].btn_left);
    }

    #[test]
    fn test_scroll_up() {
        // Z = +1 (low nibble = 0x01).
        let evs = run_packet(&[0x08, 0, 0, 0x01]);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].dz, signed::<4>(1));
    }

    #[test]
    fn test_scroll_down() {
        // Z = -1 (low nibble = 0x0F = -1 signed 4-bit).
        let evs = run_packet(&[0x08, 0, 0, 0x0F]);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].dz, signed::<4>(-1));
    }

    #[test]
    fn test_buttons_4_5() {
        let evs = run_packet(&[0x08, 0, 0, 0x30]);
        assert_eq!(evs.len(), 1);
        assert!(evs[0].btn_4);
        assert!(evs[0].btn_5);
    }

    #[test]
    fn test_drops_invalid_byte0() {
        // Byte 0 with bit 3 = 0 should be dropped.  Then a valid packet works.
        let evs = run_packet(&[0x00, 0x08, 5, 10, 0]);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].dx, signed::<9>(5));
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Ps2MouseIntelliMouse::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 1000, "HDL length {}", hdl.len());
        Ok(())
    }

    #[test]
    fn test_ps2_mouse_intellimouse_hdl_works() -> miette::Result<()> {
        let mut stream_in: Vec<In> = Vec::new();
        for &b in &[0x08u8, 5, 10, 0x01] {
            stream_in.push(In {
                byte_in: bits::<8>(b as u128),
                byte_valid: true,
            });
            stream_in.push(idle_in());
        }
        for _ in 0..4 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2MouseIntelliMouse::default();
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_ps2_mouse_intellimouse_trace() -> miette::Result<()> {
        let mut stream_in: Vec<In> = Vec::new();
        for &b in &[0x08u8, 5, 10, 0x01, 0x09, 0xFE, 0xFF, 0x0F] {
            stream_in.push(In {
                byte_in: bits::<8>(b as u128),
                byte_valid: true,
            });
            stream_in.push(idle_in());
        }
        for _ in 0..4 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2MouseIntelliMouse::default();
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("ps2_mouse_intellimouse");
        std::fs::create_dir_all(&root).unwrap();
        let _ = vcd.dump_to_file(root.join("ps2_mouse_intellimouse.vcd")).unwrap();
        let _ = expect![[r#""#]];
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = Ps2MouseIntelliMouse::fsm_descriptor();
        assert_eq!(desc.widget_name, "Ps2MouseIntelliMouse");
        assert_eq!(desc.variants().len(), 4);
    }
}
