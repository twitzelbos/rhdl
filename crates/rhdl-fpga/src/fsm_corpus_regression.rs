//! FSM extractor corpus regression test.
//!
//! This is the corpus-wide regression oracle for the principled FSM
//! extractor.  Each widget in the corpus has an `expect_test`
//! snapshot of its derived transition graph — set equality at every
//! commit.  When the extractor changes (a bug fix, an algorithm
//! improvement), every snapshot diff must be reviewed before
//! re-blessing.
//!
//! ## Why snapshots
//!
//! The principled algorithm produces a *sound over-approximation*
//! per `fsm-architecture.md` §5.4 #5: every transition the kernel
//! can produce is in the graph (zero false negatives), but the
//! graph may include edges that won't fire under reasonable inputs
//! due to cross-DFF invariants the extractor can't see.  The
//! algorithm's correctness is pinned by the Tier-1 unit tests in
//! `crates/rhdl-core/src/fsm/extraction.rs`; the corpus snapshots
//! catch regressions across the whole widget surface.  When a
//! snapshot changes, the reviewer reads the diff and the kernel to
//! confirm the change is intentional.
//!
//! ## What we explicitly do NOT do
//!
//! We do NOT compare against the (deleted) manual `FSM_TRANSITIONS`
//! consts.  Those consts were author-best-effort and contained
//! both omissions (missing implicit-hold self-loops) and outright
//! errors (e.g., `ws2812` listed a spurious `Sending → Latching`
//! edge that didn't exist in the kernel).  Treating them as the
//! regression oracle would force the extractor to either match
//! author errors or under-approximate.  The cleanup PR that
//! introduces this file also deletes those manual consts.
//!
//! ## Refresh
//!
//! `UPDATE_EXPECT=1 cargo test --package rhdl-fpga --lib fsm_corpus_regression`

#![cfg(test)]

use expect_test::{expect, Expect};
use rhdl::core::fsm::extract_widget_transitions;

/// Drive the extractor against a widget and snapshot the derived
/// transition graph.  Asserts no `Unanalyzable` diagnostics — the
/// principled algorithm should handle every kernel shape in the
/// corpus.
fn snapshot_corpus_widget<W>(expected: Expect)
where
    W: rhdl::core::fsm::FsmWidget + rhdl::core::circuit::synchronous::SynchronousIO,
{
    let result = extract_widget_transitions::<W>().expect("compile + extract");
    assert!(
        result.unanalyzable.is_empty(),
        "extractor produced Unanalyzable diagnostics: {:?}",
        result.unanalyzable
    );
    let mut derived = result.transitions;
    derived.sort();
    let formatted = derived
        .iter()
        .map(|t| format!("{} -> {}", t.source_index, t.target_index))
        .collect::<Vec<_>>()
        .join("\n");
    expected.assert_eq(&formatted);
}

// === audio ===

#[test]
fn corpus_i2s_tx() {
    use crate::audio::i2s_tx::I2sTx;
    snapshot_corpus_widget::<I2sTx>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 0
        1 -> 1"#]]);
}

// === core ===

#[test]
fn corpus_rle_decoder() {
    use crate::core::rle_decoder::RleDecoder;
    snapshot_corpus_widget::<RleDecoder>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 0
        2 -> 2"#]]);
}

#[test]
fn corpus_rle_encoder() {
    use crate::core::rle_encoder::RleEncoder;
    snapshot_corpus_widget::<RleEncoder>(expect![[r#"
        0 -> 0
        0 -> 1
        0 -> 2
        1 -> 1
        1 -> 2
        2 -> 0
        2 -> 2"#]]);
}

// === serial_bus ===

#[test]
fn corpus_battery_monitor() {
    use crate::serial_bus::battery_monitor::BatteryMonitor;
    snapshot_corpus_widget::<BatteryMonitor<10, 8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 2
        2 -> 2
        2 -> 3
        3 -> 4
        4 -> 4
        4 -> 5
        5 -> 6
        6 -> 0
        6 -> 6"#]]);
}

#[test]
fn corpus_can_master() {
    use crate::serial_bus::can_master::CanMaster;
    snapshot_corpus_widget::<CanMaster<5>>(expect![[r#"
        0 -> 0
        0 -> 1
        0 -> 17
        0 -> 20
        1 -> 1
        1 -> 2
        1 -> 17
        1 -> 20
        2 -> 2
        2 -> 3
        2 -> 17
        2 -> 20
        3 -> 3
        3 -> 4
        3 -> 17
        3 -> 20
        4 -> 4
        4 -> 5
        4 -> 8
        4 -> 17
        4 -> 20
        5 -> 5
        5 -> 6
        5 -> 17
        5 -> 20
        6 -> 6
        6 -> 7
        6 -> 17
        6 -> 20
        7 -> 7
        7 -> 8
        7 -> 17
        7 -> 20
        8 -> 8
        8 -> 9
        8 -> 17
        8 -> 20
        9 -> 9
        9 -> 10
        9 -> 11
        9 -> 17
        9 -> 20
        10 -> 10
        10 -> 11
        10 -> 17
        10 -> 20
        11 -> 11
        11 -> 12
        11 -> 17
        11 -> 20
        12 -> 12
        12 -> 13
        12 -> 17
        12 -> 20
        13 -> 13
        13 -> 14
        13 -> 17
        13 -> 20
        14 -> 14
        14 -> 15
        14 -> 17
        14 -> 20
        15 -> 15
        15 -> 16
        15 -> 17
        15 -> 20
        16 -> 0
        16 -> 16
        16 -> 17
        16 -> 20
        17 -> 17
        17 -> 18
        17 -> 20
        18 -> 0
        18 -> 17
        18 -> 18
        18 -> 19
        18 -> 20
        19 -> 0
        19 -> 17
        19 -> 19
        19 -> 20
        20 -> 0
        20 -> 17
        20 -> 20"#]]);
}

#[test]
fn corpus_dht22() {
    use crate::serial_bus::dht22::Dht22Reader;
    snapshot_corpus_widget::<Dht22Reader<10>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 0
        2 -> 2
        2 -> 3
        3 -> 0
        3 -> 3
        3 -> 4
        4 -> 0
        4 -> 4
        4 -> 5
        5 -> 0
        5 -> 5
        5 -> 6
        6 -> 0
        6 -> 6
        6 -> 7
        7 -> 0
        7 -> 6
        7 -> 7"#]]);
}

#[test]
fn corpus_half_spi_master() {
    use crate::serial_bus::half_spi_master::HalfSpiMaster;
    snapshot_corpus_widget::<HalfSpiMaster<8, 4>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        1 -> 3
        2 -> 2
        2 -> 3
        3 -> 0
        3 -> 3"#]]);
}

#[test]
fn corpus_hd44780() {
    use crate::serial_bus::hd44780::Hd44780;
    snapshot_corpus_widget::<Hd44780<10>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 2
        2 -> 3
        3 -> 3
        3 -> 4
        4 -> 4
        4 -> 5
        5 -> 0
        5 -> 5"#]]);
}

#[test]
fn corpus_i2c_master() {
    use crate::serial_bus::i2c_master::I2cMaster;
    snapshot_corpus_widget::<I2cMaster<4>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 1
        2 -> 2
        2 -> 3
        3 -> 1
        3 -> 3
        3 -> 4
        4 -> 1
        4 -> 4
        4 -> 5
        5 -> 1
        5 -> 5
        5 -> 6
        6 -> 0
        6 -> 1
        6 -> 6"#]]);
}

#[test]
fn corpus_ieee1284_negotiator() {
    use crate::serial_bus::ieee1284_negotiator::Ieee1284Negotiator;
    snapshot_corpus_widget::<Ieee1284Negotiator<8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 2
        2 -> 2
        2 -> 3
        2 -> 11
        3 -> 3
        3 -> 4
        4 -> 4
        4 -> 5
        4 -> 11
        5 -> 6
        5 -> 7
        5 -> 12
        6 -> 7
        7 -> 8
        8 -> 1
        8 -> 8
        8 -> 9
        9 -> 10
        10 -> 0
        10 -> 10
        10 -> 13
        11 -> 9
        12 -> 9
        13 -> 0"#]]);
}

#[test]
fn corpus_ir_nec_rx() {
    use crate::serial_bus::ir_nec_rx::IrNecRx;
    snapshot_corpus_widget::<IrNecRx<14>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 0
        1 -> 1
        1 -> 2
        2 -> 0
        2 -> 2
        2 -> 3
        3 -> 3
        3 -> 4
        4 -> 0
        4 -> 3
        4 -> 4
        4 -> 5
        5 -> 0
        5 -> 5"#]]);
}

#[test]
fn corpus_lin_master() {
    use crate::serial_bus::lin_master::LinMaster;
    snapshot_corpus_widget::<LinMaster<6, 8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 3
        3 -> 3
        3 -> 4
        4 -> 5
        5 -> 5
        5 -> 6
        6 -> 7
        7 -> 7
        7 -> 8
        8 -> 9
        9 -> 0
        9 -> 9"#]]);
}

#[test]
fn corpus_mfm_encoder() {
    use crate::serial_bus::mfm_encoder::MfmEncoder;
    snapshot_corpus_widget::<MfmEncoder>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 2
        2 -> 0
        2 -> 1"#]]);
}

#[test]
fn corpus_mipi_dbi_type_b() {
    use crate::serial_bus::mipi_dbi_type_b::MipiDbiTypeB;
    snapshot_corpus_widget::<MipiDbiTypeB<8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 2
        2 -> 3
        3 -> 0
        3 -> 3"#]]);
}

#[test]
fn corpus_modbus_rtu_master() {
    use crate::serial_bus::modbus_rtu_master::ModbusRtuMaster;
    snapshot_corpus_widget::<ModbusRtuMaster<8, 8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 2
        2 -> 3
        3 -> 3
        3 -> 4
        4 -> 4
        4 -> 5
        5 -> 5
        5 -> 6
        6 -> 7
        6 -> 8
        7 -> 7
        7 -> 8
        8 -> 0"#]]);
}

#[test]
fn corpus_modbus_rtu_slave() {
    use crate::serial_bus::modbus_rtu_slave::ModbusRtuSlave;
    snapshot_corpus_widget::<ModbusRtuSlave<8, 8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 0
        2 -> 3
        2 -> 4
        3 -> 3
        3 -> 4
        4 -> 4
        4 -> 5
        5 -> 0
        5 -> 5"#]]);
}

#[test]
fn corpus_nand_flash_async() {
    use crate::serial_bus::nand_flash_async::NandFlashAsync;
    snapshot_corpus_widget::<NandFlashAsync<8>>(expect![[r#"
        0 -> 0
        0 -> 1
        0 -> 3
        1 -> 1
        1 -> 2
        2 -> 2
        2 -> 5
        3 -> 3
        3 -> 4
        4 -> 5
        5 -> 0"#]]);
}

#[test]
fn corpus_one_wire_master() {
    use crate::serial_bus::one_wire_master::OneWireMaster;
    snapshot_corpus_widget::<OneWireMaster<10>>(expect![[r#"
        0 -> 0
        0 -> 1
        0 -> 3
        0 -> 5
        1 -> 1
        1 -> 2
        2 -> 2
        2 -> 7
        3 -> 3
        3 -> 4
        4 -> 3
        4 -> 4
        4 -> 7
        5 -> 5
        5 -> 6
        6 -> 5
        6 -> 6
        6 -> 7
        7 -> 0"#]]);
}

#[test]
fn corpus_parallel_port_centronics() {
    use crate::serial_bus::parallel_port_centronics::ParallelPortCentronics;
    snapshot_corpus_widget::<ParallelPortCentronics<8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 2
        2 -> 3
        3 -> 0
        3 -> 3"#]]);
}

#[test]
fn corpus_parallel_port_ecp() {
    use crate::serial_bus::parallel_port_ecp::ParallelPortEcp;
    snapshot_corpus_widget::<ParallelPortEcp>(expect![[r#"
        0 -> 0
        0 -> 1
        0 -> 4
        1 -> 2
        2 -> 2
        2 -> 3
        3 -> 0
        3 -> 3
        4 -> 0
        4 -> 4
        4 -> 5
        5 -> 6
        6 -> 0
        6 -> 6"#]]);
}

#[test]
fn corpus_parallel_port_epp() {
    use crate::serial_bus::parallel_port_epp::ParallelPortEpp;
    snapshot_corpus_widget::<ParallelPortEpp<8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 2
        2 -> 2
        2 -> 3
        2 -> 6
        3 -> 4
        4 -> 4
        4 -> 5
        4 -> 6
        5 -> 0
        6 -> 0"#]]);
}

#[test]
fn corpus_ps2_keyboard() {
    use crate::serial_bus::ps2_keyboard::Ps2Keyboard;
    snapshot_corpus_widget::<Ps2Keyboard>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 0
        1 -> 1"#]]);
}

#[test]
fn corpus_ps2_mouse() {
    use crate::serial_bus::ps2_mouse::Ps2Mouse;
    snapshot_corpus_widget::<Ps2Mouse>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 0
        2 -> 2"#]]);
}

#[test]
fn corpus_rs485_master() {
    use crate::serial_bus::rs485_master::Rs485Master;
    snapshot_corpus_widget::<Rs485Master<6, 4, 8>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 0
        2 -> 2"#]]);
}

#[test]
fn corpus_sent_rx() {
    use crate::serial_bus::sent_rx::SentRx;
    snapshot_corpus_widget::<SentRx<10>>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 0
        1 -> 1"#]]);
}

#[test]
fn corpus_smpte_ltc_encoder() {
    use crate::serial_bus::smpte_ltc_encoder::SmpteLtcEncoder;
    snapshot_corpus_widget::<SmpteLtcEncoder>(expect![[r#"
        0 -> 0
        0 -> 1
        1 -> 1
        1 -> 2
        2 -> 1
        2 -> 2"#]]);
}

#[test]
fn corpus_ti_hdq() {
    use crate::serial_bus::ti_hdq::TiHdqMaster;
    snapshot_corpus_widget::<TiHdqMaster<10>>(expect![[r#"
        0 -> 0
        0 -> 1
        0 -> 3
        0 -> 5
        1 -> 1
        1 -> 2
        2 -> 2
        2 -> 7
        3 -> 3
        3 -> 4
        4 -> 3
        4 -> 4
        4 -> 7
        5 -> 5
        5 -> 6
        6 -> 5
        6 -> 6
        6 -> 7
        7 -> 0"#]]);
}

#[test]
fn corpus_ws2812() {
    use crate::serial_bus::ws2812::Ws2812Driver;
    snapshot_corpus_widget::<Ws2812Driver<8>>(expect![[r#"
        0 -> 0
        0 -> 1
        0 -> 2
        1 -> 0
        1 -> 1
        2 -> 0
        2 -> 2"#]]);
}
