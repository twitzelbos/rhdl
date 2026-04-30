//! Widget-corpus regression test for the RHIF well-formedness invariants.
//!
//! For every widget in the corpus, this test compiles the kernel through
//! `compile_design_stage1` (which runs the full RHIF-pass pipeline) and
//! asserts that the resulting [`Object`] satisfies every invariant
//! documented in `doc/rhif-spec/invariants/object.md`.  Per
//! `rhif-formalization-plan.md` Phase 2, the widget corpus serves as
//! the primary "real-world" property oracle: any pass that introduces
//! an invariant violation will fail every widget that exercises that
//! pass, surfacing the regression at PR review.
//!
//! The well-formedness checkers themselves are unit-tested in
//! [`rhdl_core::rhif::well_formedness::tests`].  This file provides the
//! corpus shadow.
//!
//! ## What's checked
//!
//! Per [`rhdl_core::rhif::well_formedness::check_object`]:
//!
//! - **single-assignment** — every register is the `lhs` of at most one opcode
//! - **definition-before-use** — every read of a register precedes its definition
//! - **symbol-table completeness** — every referenced slot is in the symtab
//! - **literal-read-only** — no opcode has `lhs = Slot::Literal(_)`
//! - **no nested `Signal`** — `Signal(Signal(_, _), _)` is rejected
//! - **externals consistency** — every `Exec` references a present callee with the right arg count
//! - **no unresolved holes** — `Cast`/`Retime`/`Wrap` have their inferred fields resolved
//! - **valid arguments and return** — `Object::arguments` and `Object::return_slot` are in the symtab
//!
//! ## Adding a widget
//!
//! For every new widget added to `crates/rhdl-fpga`, add a corresponding
//! `well_formed_<widget>` test below.  The widget list mirrors
//! `fsm_corpus_regression.rs` plus a few non-FSM widgets to broaden
//! coverage to non-FSM-tagged kernels.
//!
//! ## When this fails
//!
//! A violation here is one of:
//!
//! - A pass introduced a violation it didn't intend (compiler bug).
//! - A widget's kernel hits a corner case the front-end mis-lowers
//!   (front-end bug).
//! - The well-formedness checker has a false positive (checker bug).
//!
//! The right resolution depends on the diagnostic; see
//! `doc/rhif-spec/invariants/object.md` for the per-invariant
//! semantics.

#![cfg(test)]

use rhdl::core::rhif::well_formedness::{
    check_widget_well_formed_asynchronous, check_widget_well_formed_synchronous,
};

/// Helper for synchronous widgets.
fn assert_synchronous<W>()
where
    W: rhdl::core::circuit::synchronous::SynchronousIO,
{
    check_widget_well_formed_synchronous::<W>().unwrap_or_else(|err| {
        panic!(
            "widget {} produced a non-well-formed RHIF Object:\n{err}",
            std::any::type_name::<W>()
        )
    });
}

/// Helper for asynchronous widgets.  Currently unused but kept for
/// future async-widget tests.
#[allow(dead_code)]
fn assert_asynchronous<W>()
where
    W: rhdl::core::circuit::circuit_impl::CircuitIO,
{
    check_widget_well_formed_asynchronous::<W>().unwrap_or_else(|err| {
        panic!(
            "widget {} produced a non-well-formed RHIF Object:\n{err}",
            std::any::type_name::<W>()
        )
    });
}

// === audio ===

#[test]
fn well_formed_i2s_tx() {
    use crate::audio::i2s_tx::I2sTx;
    assert_synchronous::<I2sTx>();
}

#[test]
fn well_formed_stereo_audio_pwm() {
    use crate::audio::audio_pwm::StereoAudioPwm;
    assert_synchronous::<StereoAudioPwm<8, 8>>();
}

#[test]
fn well_formed_dtmf_generator() {
    use crate::audio::dtmf_generator::DtmfGenerator;
    assert_synchronous::<DtmfGenerator<8>>();
}

// === core ===
//
// Note: framework primitives like `DFF`, `Constant`, and `Sync1Bit`
// do not have user-visible RHIF kernels — they are "primitive"
// circuits whose lowering is hand-written, not derived from a
// `#[kernel]`.  They are deliberately excluded from this corpus.

#[test]
fn well_formed_counter() {
    use crate::core::counter::Counter;
    assert_synchronous::<Counter<8>>();
}

#[test]
fn well_formed_delay() {
    use crate::core::delay::Delay;
    use rhdl::prelude::*;
    assert_synchronous::<Delay<Bits<8>, 4>>();
}

#[test]
fn well_formed_register_file() {
    use crate::core::register_file::RegisterFile;
    use rhdl::prelude::*;
    assert_synchronous::<RegisterFile<Bits<8>, 4, 2>>();
}

#[test]
fn well_formed_rle_decoder() {
    use crate::core::rle_decoder::RleDecoder;
    assert_synchronous::<RleDecoder>();
}

#[test]
fn well_formed_rle_encoder() {
    use crate::core::rle_encoder::RleEncoder;
    assert_synchronous::<RleEncoder>();
}

// === fifo ===

#[test]
fn well_formed_synchronous_fifo() {
    use crate::fifo::synchronous::SyncFIFO;
    use rhdl::prelude::*;
    assert_synchronous::<SyncFIFO<Bits<8>, 4>>();
}

// === serial_bus ===
// (mirrors the FSM-corpus list)

#[test]
fn well_formed_battery_monitor() {
    use crate::serial_bus::battery_monitor::BatteryMonitor;
    assert_synchronous::<BatteryMonitor<10, 8>>();
}

#[test]
fn well_formed_can_master() {
    use crate::serial_bus::can_master::CanMaster;
    assert_synchronous::<CanMaster<5>>();
}

#[test]
fn well_formed_dht22() {
    use crate::serial_bus::dht22::Dht22Reader;
    assert_synchronous::<Dht22Reader<10>>();
}

#[test]
fn well_formed_half_spi_master() {
    use crate::serial_bus::half_spi_master::HalfSpiMaster;
    assert_synchronous::<HalfSpiMaster<8, 4>>();
}

#[test]
fn well_formed_hd44780() {
    use crate::serial_bus::hd44780::Hd44780;
    assert_synchronous::<Hd44780<10>>();
}

#[test]
fn well_formed_i2c_master() {
    use crate::serial_bus::i2c_master::I2cMaster;
    assert_synchronous::<I2cMaster<4>>();
}

#[test]
fn well_formed_ieee1284_negotiator() {
    use crate::serial_bus::ieee1284_negotiator::Ieee1284Negotiator;
    assert_synchronous::<Ieee1284Negotiator<8>>();
}

#[test]
fn well_formed_ir_nec_rx() {
    use crate::serial_bus::ir_nec_rx::IrNecRx;
    assert_synchronous::<IrNecRx<14>>();
}

#[test]
fn well_formed_lin_master() {
    use crate::serial_bus::lin_master::LinMaster;
    assert_synchronous::<LinMaster<6, 8>>();
}

#[test]
fn well_formed_mfm_encoder() {
    use crate::serial_bus::mfm_encoder::MfmEncoder;
    assert_synchronous::<MfmEncoder>();
}

#[test]
fn well_formed_mipi_dbi_type_b() {
    use crate::serial_bus::mipi_dbi_type_b::MipiDbiTypeB;
    assert_synchronous::<MipiDbiTypeB<8>>();
}

#[test]
fn well_formed_modbus_rtu_master() {
    use crate::serial_bus::modbus_rtu_master::ModbusRtuMaster;
    assert_synchronous::<ModbusRtuMaster<8, 8>>();
}

#[test]
fn well_formed_modbus_rtu_slave() {
    use crate::serial_bus::modbus_rtu_slave::ModbusRtuSlave;
    assert_synchronous::<ModbusRtuSlave<8, 8>>();
}

#[test]
fn well_formed_nand_flash_async() {
    use crate::serial_bus::nand_flash_async::NandFlashAsync;
    assert_synchronous::<NandFlashAsync<8>>();
}

#[test]
fn well_formed_one_wire_master() {
    use crate::serial_bus::one_wire_master::OneWireMaster;
    assert_synchronous::<OneWireMaster<10>>();
}

#[test]
fn well_formed_parallel_port_centronics() {
    use crate::serial_bus::parallel_port_centronics::ParallelPortCentronics;
    assert_synchronous::<ParallelPortCentronics<8>>();
}

#[test]
fn well_formed_parallel_port_ecp() {
    use crate::serial_bus::parallel_port_ecp::ParallelPortEcp;
    assert_synchronous::<ParallelPortEcp>();
}

#[test]
fn well_formed_parallel_port_epp() {
    use crate::serial_bus::parallel_port_epp::ParallelPortEpp;
    assert_synchronous::<ParallelPortEpp<8>>();
}

#[test]
fn well_formed_ps2_keyboard() {
    use crate::serial_bus::ps2_keyboard::Ps2Keyboard;
    assert_synchronous::<Ps2Keyboard>();
}

#[test]
fn well_formed_ps2_mouse() {
    use crate::serial_bus::ps2_mouse::Ps2Mouse;
    assert_synchronous::<Ps2Mouse>();
}

#[test]
fn well_formed_rs485_master() {
    use crate::serial_bus::rs485_master::Rs485Master;
    assert_synchronous::<Rs485Master<6, 4, 8>>();
}

#[test]
fn well_formed_sent_rx() {
    use crate::serial_bus::sent_rx::SentRx;
    assert_synchronous::<SentRx<10>>();
}

#[test]
fn well_formed_smpte_ltc_encoder() {
    use crate::serial_bus::smpte_ltc_encoder::SmpteLtcEncoder;
    assert_synchronous::<SmpteLtcEncoder>();
}

#[test]
fn well_formed_ti_hdq() {
    use crate::serial_bus::ti_hdq::TiHdqMaster;
    assert_synchronous::<TiHdqMaster<10>>();
}

#[test]
fn well_formed_ws2812() {
    use crate::serial_bus::ws2812::Ws2812Driver;
    assert_synchronous::<Ws2812Driver<8>>();
}

// === serial_bus — additional non-FSM widgets ===

#[test]
fn well_formed_uart_rx() {
    use crate::serial_bus::uart_rx::UartRx;
    assert_synchronous::<UartRx<10>>();
}

#[test]
fn well_formed_uart_tx() {
    use crate::serial_bus::uart_tx::UartTx;
    assert_synchronous::<UartTx<10>>();
}

#[test]
fn well_formed_spi_master() {
    use crate::serial_bus::spi_master::SpiMaster;
    assert_synchronous::<SpiMaster<8, 4>>();
}
