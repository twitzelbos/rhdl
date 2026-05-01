//! Widget-corpus regression tests for the RHIF spec's property
//! oracles — semantic preservation across passes, and lowering
//! correctness (RHIF VM ↔ RTL VM).
//!
//! Per `rhif-formalization-plan.md` §5.1, two of the four major
//! Phase 2 properties are:
//!
//! - **Semantic preservation across passes.** Every pass is
//!   observation-equivalent: for any input, the VM output before
//!   and after the pass is bit-identical.  Stronger than
//!   well-formedness alone — a pass could rewrite the IR to a
//!   well-formed but semantically-different program; this catches
//!   that.
//!
//! - **Lowering correctness.** The RHIF VM and the RTL VM produce
//!   the same output for any well-typed input to any kernel.  This
//!   pins the `lower_rhif_to_rtl` translation as observation-
//!   equivalent.
//!
//! The well-formedness corpus shadow lives in
//! [`super::widget_well_formedness`].
//!
//! ## Test pacing
//!
//! Each widget runs with a small fixed number of randomly-sampled
//! inputs (`SAMPLES_PER_WIDGET`).  This is a cost / coverage
//! tradeoff: random fuzzing of widget inputs will not exercise
//! every code path in every kernel, but it's enough to catch
//! "this pass corrupts the result for some inputs" regressions
//! that the corpus existed-but-passed before.
//!
//! Seeds are deterministic — failing tests are reproducible.

#![cfg(test)]

use rhdl::core::rhif::property_tests::{
    check_lowering_correctness, check_semantic_preservation, random_arguments,
    seeded_rng, structured_synchronous_arguments, LoweringCorrectnessOutcome,
    SemanticPreservationOutcome,
};

const SAMPLES_PER_WIDGET: usize = 4;

fn assert_semantic_preservation_synchronous<W>(seed: u64)
where
    W: rhdl::core::circuit::synchronous::SynchronousIO,
{
    let mut rng = seeded_rng(seed);
    // We need a representative Object to compute argument kinds —
    // compile once, then sample inputs from those kinds.
    let obj = rhdl::core::compiler::driver::compile_design_stage1::<W::Kernel>(
        rhdl::core::CompilationMode::Synchronous,
    )
    .unwrap_or_else(|e| panic!("compile {} failed: {e:?}", std::any::type_name::<W>()));
    for sample in 0..SAMPLES_PER_WIDGET {
        let args = random_arguments(&obj, &mut rng);
        let outcome = check_semantic_preservation::<W::Kernel>(
            rhdl::core::CompilationMode::Synchronous,
            args.clone(),
        )
        .unwrap_or_else(|e| panic!("compile {} failed: {e:?}", std::any::type_name::<W>()));
        match outcome {
            SemanticPreservationOutcome::Preserved { .. } => continue,
            other => panic!(
                "widget {} failed semantic preservation on sample #{sample}: {other:?}",
                std::any::type_name::<W>(),
            ),
        }
    }
}

/// Sample inputs for the kernel.  `Random` uses
/// [`random_arguments`] (all three kernel args are random bits);
/// `StructuredFirstCycle` uses [`structured_synchronous_arguments`]
/// (cr=zero, q=zero, i=random) — useful for widgets whose `q` has
/// dynamic-index reads that ICE on random states.
#[derive(Clone, Copy)]
enum InputStrategy {
    Random,
    StructuredFirstCycle,
}

fn assert_lowering_correctness_synchronous<W>(seed: u64)
where
    W: rhdl::core::circuit::synchronous::SynchronousIO,
{
    assert_lowering_correctness_synchronous_with_strategy::<W>(seed, InputStrategy::Random);
}

fn assert_lowering_correctness_synchronous_with_strategy<W>(seed: u64, strategy: InputStrategy)
where
    W: rhdl::core::circuit::synchronous::SynchronousIO,
{
    // Some kernels reject many random inputs as out-of-domain (e.g.,
    // a runtime shift by a value that exceeds the operand width
    // produces a `ShiftAmountMustBeLessThan` error).  Sample
    // generously and skip those; require at least
    // `MIN_IN_DOMAIN_SAMPLES` samples to actually exercise the VMs
    // — otherwise the test passes vacuously.
    const MAX_ATTEMPTS: usize = 64;
    const MIN_IN_DOMAIN_SAMPLES: usize = 1;

    let mut rng = seeded_rng(seed);
    let obj = rhdl::core::compiler::driver::compile_design_stage1::<W::Kernel>(
        rhdl::core::CompilationMode::Synchronous,
    )
    .unwrap_or_else(|e| panic!("compile {} failed: {e:?}", std::any::type_name::<W>()));
    let mut in_domain = 0usize;
    for attempt in 0..MAX_ATTEMPTS {
        if in_domain >= SAMPLES_PER_WIDGET {
            break;
        }
        let args = match strategy {
            InputStrategy::Random => random_arguments(&obj, &mut rng),
            InputStrategy::StructuredFirstCycle => {
                structured_synchronous_arguments(&obj, &mut rng)
            }
        };
        let outcome = check_lowering_correctness::<W::Kernel>(
            rhdl::core::CompilationMode::Synchronous,
            args.clone(),
        )
        .unwrap_or_else(|e| panic!("compile {} failed: {e:?}", std::any::type_name::<W>()));
        match outcome {
            LoweringCorrectnessOutcome::Equal => {
                in_domain += 1;
            }
            LoweringCorrectnessOutcome::RhifError { .. } => {
                // Skip out-of-domain inputs.
                continue;
            }
            other => panic!(
                "widget {} failed lowering correctness on attempt #{attempt}: {other:?}",
                std::any::type_name::<W>(),
            ),
        }
    }
    assert!(
        in_domain >= MIN_IN_DOMAIN_SAMPLES,
        "widget {} produced no in-domain inputs in {MAX_ATTEMPTS} attempts — \
         the input sampler can't exercise this kernel's runtime constraints; \
         consider switching strategies or tightening the input distribution",
        std::any::type_name::<W>(),
    );
}

// ===========================================================
// Semantic preservation
// ===========================================================

mod semantic_preservation {
    use super::*;

    #[test]
    fn i2s_tx() {
        use crate::audio::i2s_tx::I2sTx;
        assert_semantic_preservation_synchronous::<I2sTx>(101);
    }

    #[test]
    fn dtmf_generator() {
        use crate::audio::dtmf_generator::DtmfGenerator;
        assert_semantic_preservation_synchronous::<DtmfGenerator<8>>(102);
    }

    #[test]
    fn counter() {
        use crate::core::counter::Counter;
        assert_semantic_preservation_synchronous::<Counter<8>>(103);
    }

    #[test]
    fn delay() {
        use crate::core::delay::Delay;
        use rhdl::prelude::*;
        assert_semantic_preservation_synchronous::<Delay<Bits<8>, 4>>(104);
    }

    #[test]
    fn rle_decoder() {
        use crate::core::rle_decoder::RleDecoder;
        assert_semantic_preservation_synchronous::<RleDecoder>(105);
    }

    #[test]
    fn rle_encoder() {
        use crate::core::rle_encoder::RleEncoder;
        assert_semantic_preservation_synchronous::<RleEncoder>(106);
    }

    #[test]
    fn synchronous_fifo() {
        use crate::fifo::synchronous::SyncFIFO;
        use rhdl::prelude::*;
        assert_semantic_preservation_synchronous::<SyncFIFO<Bits<8>, 4>>(107);
    }

    #[test]
    fn battery_monitor() {
        use crate::serial_bus::battery_monitor::BatteryMonitor;
        assert_semantic_preservation_synchronous::<BatteryMonitor<10, 8>>(108);
    }

    #[test]
    fn can_master() {
        use crate::serial_bus::can_master::CanMaster;
        assert_semantic_preservation_synchronous::<CanMaster<5>>(109);
    }

    #[test]
    fn dht22() {
        use crate::serial_bus::dht22::Dht22Reader;
        assert_semantic_preservation_synchronous::<Dht22Reader<10>>(110);
    }

    #[test]
    fn half_spi_master() {
        use crate::serial_bus::half_spi_master::HalfSpiMaster;
        assert_semantic_preservation_synchronous::<HalfSpiMaster<8, 4>>(111);
    }

    #[test]
    fn hd44780() {
        use crate::serial_bus::hd44780::Hd44780;
        assert_semantic_preservation_synchronous::<Hd44780<10>>(112);
    }

    #[test]
    fn i2c_master() {
        use crate::serial_bus::i2c_master::I2cMaster;
        assert_semantic_preservation_synchronous::<I2cMaster<4>>(113);
    }

    #[test]
    fn ir_nec_rx() {
        use crate::serial_bus::ir_nec_rx::IrNecRx;
        assert_semantic_preservation_synchronous::<IrNecRx<14>>(114);
    }

    #[test]
    fn lin_master() {
        use crate::serial_bus::lin_master::LinMaster;
        assert_semantic_preservation_synchronous::<LinMaster<6, 8>>(115);
    }

    #[test]
    fn modbus_rtu_master() {
        use crate::serial_bus::modbus_rtu_master::ModbusRtuMaster;
        assert_semantic_preservation_synchronous::<ModbusRtuMaster<8, 8>>(116);
    }

    #[test]
    fn modbus_rtu_slave() {
        use crate::serial_bus::modbus_rtu_slave::ModbusRtuSlave;
        assert_semantic_preservation_synchronous::<ModbusRtuSlave<8, 8>>(117);
    }

    #[test]
    fn one_wire_master() {
        use crate::serial_bus::one_wire_master::OneWireMaster;
        assert_semantic_preservation_synchronous::<OneWireMaster<10>>(118);
    }

    #[test]
    fn ps2_keyboard() {
        use crate::serial_bus::ps2_keyboard::Ps2Keyboard;
        assert_semantic_preservation_synchronous::<Ps2Keyboard>(119);
    }

    #[test]
    fn sent_rx() {
        use crate::serial_bus::sent_rx::SentRx;
        assert_semantic_preservation_synchronous::<SentRx<10>>(120);
    }

    #[test]
    fn smpte_ltc_encoder() {
        use crate::serial_bus::smpte_ltc_encoder::SmpteLtcEncoder;
        assert_semantic_preservation_synchronous::<SmpteLtcEncoder>(121);
    }

    #[test]
    fn uart_rx() {
        use crate::serial_bus::uart_rx::UartRx;
        assert_semantic_preservation_synchronous::<UartRx<10>>(122);
    }

    #[test]
    fn uart_tx() {
        use crate::serial_bus::uart_tx::UartTx;
        assert_semantic_preservation_synchronous::<UartTx<10>>(123);
    }
}

// ===========================================================
// Lowering correctness (RHIF VM ↔ RTL VM)
// ===========================================================

mod lowering_correctness {
    use super::*;

    #[test]
    fn i2s_tx() {
        use crate::audio::i2s_tx::I2sTx;
        assert_lowering_correctness_synchronous::<I2sTx>(201);
    }

    #[test]
    fn dtmf_generator() {
        use crate::audio::dtmf_generator::DtmfGenerator;
        assert_lowering_correctness_synchronous::<DtmfGenerator<8>>(202);
    }

    #[test]
    fn counter() {
        use crate::core::counter::Counter;
        assert_lowering_correctness_synchronous::<Counter<8>>(203);
    }

    #[test]
    fn delay() {
        use crate::core::delay::Delay;
        use rhdl::prelude::*;
        assert_lowering_correctness_synchronous::<Delay<Bits<8>, 4>>(204);
    }

    #[test]
    fn rle_decoder() {
        use crate::core::rle_decoder::RleDecoder;
        assert_lowering_correctness_synchronous::<RleDecoder>(205);
    }

    #[test]
    fn rle_encoder() {
        use crate::core::rle_encoder::RleEncoder;
        assert_lowering_correctness_synchronous::<RleEncoder>(206);
    }

    #[test]
    fn synchronous_fifo() {
        use crate::fifo::synchronous::SyncFIFO;
        use rhdl::prelude::*;
        assert_lowering_correctness_synchronous::<SyncFIFO<Bits<8>, 4>>(207);
    }

    #[test]
    fn battery_monitor() {
        use crate::serial_bus::battery_monitor::BatteryMonitor;
        assert_lowering_correctness_synchronous::<BatteryMonitor<10, 8>>(208);
    }

    #[test]
    fn can_master() {
        use crate::serial_bus::can_master::CanMaster;
        assert_lowering_correctness_synchronous::<CanMaster<5>>(209);
    }

    #[test]
    fn i2c_master() {
        use crate::serial_bus::i2c_master::I2cMaster;
        assert_lowering_correctness_synchronous::<I2cMaster<4>>(210);
    }

    // ModbusRtuMaster / ModbusRtuSlave use the StructuredFirstCycle
    // strategy: cr=zero, q=zero (initial state), i=random.  Random-
    // bits q would put `q.extras.build_idx` at an arbitrary value
    // that the kernel uses as a runtime array index, ICEing on
    // ArrayIndexOutOfBounds nearly 100 % of the time.  Zeroing q
    // gives the kernel its post-reset state, which is well-defined
    // for any random `i`.

    #[test]
    fn modbus_rtu_master() {
        use crate::serial_bus::modbus_rtu_master::ModbusRtuMaster;
        assert_lowering_correctness_synchronous_with_strategy::<ModbusRtuMaster<8, 8>>(
            220,
            InputStrategy::StructuredFirstCycle,
        );
    }

    #[test]
    fn modbus_rtu_slave() {
        use crate::serial_bus::modbus_rtu_slave::ModbusRtuSlave;
        assert_lowering_correctness_synchronous_with_strategy::<ModbusRtuSlave<8, 8>>(
            221,
            InputStrategy::StructuredFirstCycle,
        );
    }

    #[test]
    fn ps2_keyboard() {
        use crate::serial_bus::ps2_keyboard::Ps2Keyboard;
        assert_lowering_correctness_synchronous::<Ps2Keyboard>(213);
    }

    #[test]
    fn uart_rx() {
        use crate::serial_bus::uart_rx::UartRx;
        assert_lowering_correctness_synchronous::<UartRx<10>>(214);
    }

    #[test]
    fn uart_tx() {
        use crate::serial_bus::uart_tx::UartTx;
        assert_lowering_correctness_synchronous::<UartTx<10>>(215);
    }
}
