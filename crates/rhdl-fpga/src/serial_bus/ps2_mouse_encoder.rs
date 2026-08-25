//! PS/2 mouse encoder (FPGA emulates a PS/2 IntelliMouse)
//!
//! Wraps [super::ps2_device_tx::Ps2DeviceTx] with the 4-byte
//! IntelliMouse packet builder so the host can drive the FPGA at
//! the higher level "send a movement event with these dx/dy/dz +
//! buttons" without packing the bytes manually.
//!
//! ## 4-byte packet
//!
//! Same layout as [super::ps2_mouse_intellimouse::MouseEvent],
//! emitted in transmission order:
//!
//! - Byte 0: `Y_OVF X_OVF Y_SIGN X_SIGN  ALWAYS_1=1 BTN_M BTN_R BTN_L`
//! - Byte 1: X displacement (8-bit, sign in byte 0 bit 4).
//! - Byte 2: Y displacement (8-bit, sign in byte 0 bit 5).
//! - Byte 3: Z displacement (low nibble, signed 4-bit) +
//!   BTN_5, BTN_4 (high nibble bits 4, 5).
//!
//! Composes [super::ps2_device_tx::Ps2DeviceTx].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +--------+Ps2MouseEncoder+--------+
     |                                 |
S<9> |                                 | bool
+--->| dx                  clk_oe      +-->
S<9> |                                 | bool
+--->| dy                  data_oe     +-->
S<4> |                                 | bool
+--->| dz                  tx_busy     +-->
bool |                                 | bool
+--->| btn_left            ready       +-->
bool |                                 |
+--->| btn_right                       |
bool |                                 |
+--->| btn_middle                      |
bool |                                 |
+--->| btn_4                           |
bool |                                 |
+--->| btn_5                           |
bool |                                 |
+--->| send                            |
bool |                                 |
+--->| clk_in                          |
     +---------------------------------+
")]
//!
//!# Internals
//!
//! Five-state FSM: Idle → SendByte0 → SendByte1 → SendByte2 →
//! SendByte3 → WaitTxDone (one WaitTxDone per byte) → next
//! byte's send-state.  Byte values are computed combinationally
//! from the inputs at the moment `send` strobes; the latched
//! event is held in a buffer of four 8-bit registers until the
//! sequence completes.
//!
//!# Parameters
//!
//! - `DIV_W` — bit width of the wire-layer half-period divisor.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/ps2_mouse_encoder.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/ps2_mouse_encoder.md")]
//!
//! And the auto-generated FSM diagram:
#![doc = include_str!("../../doc/ps2_mouse_encoder_fsm.md")]

use rhdl::prelude::*;

use super::ps2_device_tx::Ps2DeviceTx;
use crate::core::dff;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum MouseEncState {
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    #[fsm_state(label = "send byte 0")]
    SendByte0,
    #[fsm_state(label = "send byte 1")]
    SendByte1,
    #[fsm_state(label = "send byte 2")]
    SendByte2,
    #[fsm_state(label = "send byte 3")]
    SendByte3,
    #[fsm_state(label = "wait tx done")]
    WaitTxDone,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = MouseEncState, allow_implicit)]
/// PS/2 mouse encoder (FPGA-as-IntelliMouse).
pub struct Ps2MouseEncoder<const DIV_W: usize>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    state: dff::DFF<MouseEncState>,
    /// Latched packet bytes (computed at strobe time).
    b0_q: dff::DFF<Bits<8>>,
    b1_q: dff::DFF<Bits<8>>,
    b2_q: dff::DFF<Bits<8>>,
    b3_q: dff::DFF<Bits<8>>,
    /// Where to go after WaitTxDone.
    pending_state: dff::DFF<MouseEncState>,
    /// Wire-layer TX (composed sub-circuit).
    tx: Ps2DeviceTx<DIV_W>,
}

impl<const DIV_W: usize> Ps2MouseEncoder<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    pub fn new(half_period: Bits<DIV_W>) -> Self {
        Self {
            state: dff::DFF::default(),
            b0_q: dff::DFF::default(),
            b1_q: dff::DFF::default(),
            b2_q: dff::DFF::default(),
            b3_q: dff::DFF::default(),
            pending_state: dff::DFF::default(),
            tx: Ps2DeviceTx::new(half_period),
        }
    }
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [Ps2MouseEncoder].
pub struct In {
    /// X displacement (signed 9 bits — value in low 8 bits, sign in bit 8).
    pub dx: SignedBits<9>,
    /// Y displacement (signed 9 bits).
    pub dy: SignedBits<9>,
    /// Z (scroll wheel) displacement (signed 4 bits).
    pub dz: SignedBits<4>,
    pub btn_left: bool,
    pub btn_right: bool,
    pub btn_middle: bool,
    pub btn_4: bool,
    pub btn_5: bool,
    /// Pulse for one cycle to start the packet sequence.
    pub send: bool,
    /// Sampled CLK input from the wire.
    pub clk_in: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [Ps2MouseEncoder].
pub struct Out {
    pub clk_oe: bool,
    pub data_oe: bool,
    pub tx_busy: bool,
    pub ready: bool,
}

impl<const DIV_W: usize> SynchronousIO for Ps2MouseEncoder<DIV_W>
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    type I = In;
    type O = Out;
    type Kernel = ps2_mouse_encoder<DIV_W>;
}

#[kernel]
/// Kernel for [Ps2MouseEncoder].
pub fn ps2_mouse_encoder<const DIV_W: usize>(cr: ClockReset, i: In, q: Q<DIV_W>) -> (Out, D<DIV_W>)
where
    rhdl::bits::W<DIV_W>: BitWidth,
{
    let mut d = D::<DIV_W>::dont_care();
    d.state = q.state;
    d.b0_q = q.b0_q;
    d.b1_q = q.b1_q;
    d.b2_q = q.b2_q;
    d.b3_q = q.b3_q;
    d.pending_state = q.pending_state;

    let mut tx_in = super::ps2_device_tx::In {
        tx_byte: bits::<8>(0),
        tx_strobe: false,
        clk_in: i.clk_in,
    };

    match q.state {
        MouseEncState::Idle => {
            if i.send {
                // Pack the 4 bytes from the typed inputs.
                let dx_unsigned: Bits<9> = i.dx.as_unsigned();
                let dy_unsigned: Bits<9> = i.dy.as_unsigned();
                let dz_unsigned: Bits<4> = i.dz.as_unsigned();
                // Sign bits are bit 8 of the 9-bit two's-complement value.
                let x_sign = (dx_unsigned & bits::<9>(0x100)) != bits::<9>(0);
                let y_sign = (dy_unsigned & bits::<9>(0x100)) != bits::<9>(0);
                // X/Y displacement low byte.
                let x_byte: Bits<8> = (dx_unsigned & bits::<9>(0xFF)).resize::<8>();
                let y_byte: Bits<8> = (dy_unsigned & bits::<9>(0xFF)).resize::<8>();
                // Byte 0: bit 3 = 1 (always), bits 0-2 = L/R/M, bit 4-5 = sign,
                // bit 6 = X overflow (we never set), bit 7 = Y overflow.
                let mut b0: Bits<8> = bits::<8>(0x08);
                if i.btn_left {
                    b0 |= bits::<8>(0x01);
                }
                if i.btn_right {
                    b0 |= bits::<8>(0x02);
                }
                if i.btn_middle {
                    b0 |= bits::<8>(0x04);
                }
                if x_sign {
                    b0 |= bits::<8>(0x10);
                }
                if y_sign {
                    b0 |= bits::<8>(0x20);
                }
                // Byte 3: low nibble = Z (signed 4 bits), bits 4/5 = btn 4/5.
                let mut b3: Bits<8> = dz_unsigned.resize::<8>();
                if i.btn_4 {
                    b3 |= bits::<8>(0x10);
                }
                if i.btn_5 {
                    b3 |= bits::<8>(0x20);
                }
                d.b0_q = b0;
                d.b1_q = x_byte;
                d.b2_q = y_byte;
                d.b3_q = b3;
                d.state = MouseEncState::SendByte0;
            }
        }
        MouseEncState::SendByte0 => {
            tx_in.tx_byte = q.b0_q;
            tx_in.tx_strobe = true;
            d.pending_state = MouseEncState::SendByte1;
            d.state = MouseEncState::WaitTxDone;
        }
        MouseEncState::SendByte1 => {
            tx_in.tx_byte = q.b1_q;
            tx_in.tx_strobe = true;
            d.pending_state = MouseEncState::SendByte2;
            d.state = MouseEncState::WaitTxDone;
        }
        MouseEncState::SendByte2 => {
            tx_in.tx_byte = q.b2_q;
            tx_in.tx_strobe = true;
            d.pending_state = MouseEncState::SendByte3;
            d.state = MouseEncState::WaitTxDone;
        }
        MouseEncState::SendByte3 => {
            tx_in.tx_byte = q.b3_q;
            tx_in.tx_strobe = true;
            d.pending_state = MouseEncState::Idle;
            d.state = MouseEncState::WaitTxDone;
        }
        MouseEncState::WaitTxDone => {
            if q.tx.tx_done {
                d.state = q.pending_state;
            }
        }
    }

    d.tx = tx_in;

    if cr.reset.any() {
        d.state = MouseEncState::Idle;
        d.b0_q = bits::<8>(0);
        d.b1_q = bits::<8>(0);
        d.b2_q = bits::<8>(0);
        d.b3_q = bits::<8>(0);
        d.pending_state = MouseEncState::Idle;
    }

    let mut o = Out::dont_care();
    o.clk_oe = q.tx.clk_oe;
    o.data_oe = q.tx.data_oe;
    o.tx_busy = q.state != MouseEncState::Idle;
    o.ready = q.state == MouseEncState::Idle;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In {
            dx: signed::<9>(0),
            dy: signed::<9>(0),
            dz: signed::<4>(0),
            btn_left: false,
            btn_right: false,
            btn_middle: false,
            btn_4: false,
            btn_5: false,
            send: false,
            clk_in: true,
        }
    }

    #[test]
    fn test_send_strobe_starts_busy() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 2];
        let mut start = idle_in();
        start.dx = signed::<9>(5);
        start.dy = signed::<9>(-3);
        start.btn_left = true;
        start.send = true;
        stream_in.push(start);
        for _ in 0..1500 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2MouseEncoder::<8>::new(bits(4));
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        assert!(outputs.iter().any(|s| s.output.tx_busy));
        Ok(())
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = Ps2MouseEncoder::<13>::new(bits(4000));
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        assert!(hdl.len() > 1000, "HDL length {}", hdl.len());
        Ok(())
    }

    #[test]
    fn test_ps2_mouse_encoder_hdl_works() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 2];
        let mut start = idle_in();
        start.send = true;
        stream_in.push(start);
        for _ in 0..40 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2MouseEncoder::<8>::new(bits(4));
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_ps2_mouse_encoder_trace() -> miette::Result<()> {
        let mut stream_in = vec![idle_in(); 2];
        let mut start = idle_in();
        start.dx = signed::<9>(7);
        start.dy = signed::<9>(-2);
        start.dz = signed::<4>(1);
        start.btn_middle = true;
        start.send = true;
        stream_in.push(start);
        for _ in 0..400 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(2).clock_pos_edge(100);
        let uut = Ps2MouseEncoder::<8>::new(bits(4));
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("ps2_mouse_encoder");
        std::fs::create_dir_all(&root).unwrap();
        let _ = vcd
            .dump_to_file(root.join("ps2_mouse_encoder.vcd"))
            .unwrap();
        let _ = expect![[r#""#]];
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = Ps2MouseEncoder::<13>::fsm_descriptor();
        assert_eq!(desc.widget_name, "Ps2MouseEncoder");
        assert_eq!(desc.variants().len(), 6);
    }
}
