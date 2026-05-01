//! Pilot rewrite #3 — a simple UART-style protocol PHY as a
//! genuinely multi-rule rule kernel.
//!
//! Validates the design plan §15 / §16 / §21 commitment for a
//! protocol PHY where rules represent **distinct state transitions**
//! and at most one fires per cycle.  This is the rule-kernel pattern
//! that pays off: each rule corresponds to a transition condition,
//! the priority chain decides which transition wins on edge cases,
//! and the source reads as a state diagram.
//!
//! ## Why a fresh widget rather than rewriting `serial_bus::uart_tx`
//!
//! The existing `uart_tx` uses a `Constant<T>` sub-circuit for the
//! baud divisor.  Rule kernels (today) only handle DFF-shaped
//! sub-circuits because the macro generates `D::<…> { field: …}`
//! constructors and doesn't know about the per-sub-circuit input
//! shapes for non-DFF sub-circuits.  Using a const generic for the
//! divisor sidesteps the issue.  The shipped UART TX would lower
//! cleanly through this same rule pattern once `Constant<T>` is
//! supported (see follow-up note in this file).
//!
//! ## Rule decomposition
//!
//! State register: `Bits<4>` bit_counter (0 = idle; 1..=10 = sending
//! start + 8 data + stop bit).  Plus `data_reg: Bits<8>` (latched
//! data).
//!
//! Rules (all write `bit_counter` — write-write conflicts handled
//! by the priority chain; at most one fires per cycle):
//!
//! 1. **`load`** — guard: `bit_counter == 0 && i.send`.  Latch
//!    data, set bit_counter = 1 (start bit).
//! 2. **`advance`** — guard: `bit_counter > 0 && bit_counter < 10`.
//!    Increment bit_counter.
//! 3. **`finish`** — guard: `bit_counter == 10`.  Reset bit_counter
//!    = 0 (return to idle).
//!
//! These are pairwise mutually exclusive by construction (each
//! guard is a distinct `bit_counter` predicate); we declare that
//! to the macro so the priority chain elides the redundant
//! suppressors per Phase-2 `mutually_exclusive`.
//!
//! `data_reg` is written only by `load`, which has no read
//! contention from the other rules — so no conflict there.
//!
//! No baud divider in this pilot — the UART runs at one bit per
//! clock.  Production UART TX uses an additional baud counter
//! (which is itself a single state register) gating the
//! `advance`/`finish` rules.  The pattern extends directly.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct UartTxIn {
    /// Byte to transmit (latched when `send` is asserted in idle).
    pub data: Bits<8>,
    /// Strobe to start transmission.  Ignored while not idle.
    pub send: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct UartTxOut {
    /// The serial line (idle high).
    pub tx: bool,
    /// High while a frame is being transmitted.
    pub busy: bool,
}

rule_kernel! {
    /// Simple 1-bit-per-clock UART transmitter as a rule kernel.
    /// Three rules (load / advance / finish), all write
    /// `bit_counter`.  Mutually exclusive guards.
    pub struct RuleSimpleUartTx {
        bit_counter: dff::DFF<Bits<4>>,
        data_reg: dff::DFF<Bits<8>>,
    }

    impl RuleSimpleUartTx {
        /// Idle → start: latch data and arm.
        #[rule(mutually_exclusive = "advance", mutually_exclusive = "finish")]
        fn load(ctx: &mut RuleCtx<Self>, i: UartTxIn) {
            guard!(*ctx.bit_counter == bits::<4>(0));
            guard!(i.send);
            ctx.bit_counter = bits::<4>(1);
            ctx.data_reg = i.data;
        }

        /// Mid-frame: advance to the next bit.
        #[rule(mutually_exclusive = "load", mutually_exclusive = "finish")]
        fn advance(ctx: &mut RuleCtx<Self>, i: UartTxIn) {
            let _ = i;
            guard!(*ctx.bit_counter >= bits::<4>(1));
            guard!(*ctx.bit_counter < bits::<4>(10));
            ctx.bit_counter = *ctx.bit_counter + bits::<4>(1);
        }

        /// Stop bit complete → return to idle.
        #[rule(mutually_exclusive = "load", mutually_exclusive = "advance")]
        fn finish(ctx: &mut RuleCtx<Self>, i: UartTxIn) {
            let _ = i;
            guard!(*ctx.bit_counter == bits::<4>(10));
            ctx.bit_counter = bits::<4>(0);
        }

        /// `tx` line:
        ///   0  → idle, line high
        ///   1  → start bit, line low
        ///   2-9 → data[0..=7], LSB first
        ///   10 → stop bit, line high
        #[output]
        fn output(self_q: &Self, _i: UartTxIn) -> UartTxOut {
            let bc = *self_q.bit_counter;
            let one_b4: Bits<4> = bits::<4>(1);
            let ten_b4: Bits<4> = bits::<4>(10);
            let mask_b4: Bits<4> = bits::<4>(0b111);
            let zero_b8: Bits<8> = bits::<8>(0);
            let one_b8: Bits<8> = bits::<8>(1);

            // bit_idx = bc - 2 saturated to [0, 7] (only meaningful for bc in 2..=9)
            let bit_idx_raw: Bits<4> = bc - bits::<4>(2);
            let bit_idx_safe: Bits<4> = bit_idx_raw & mask_b4;
            let data_bit: Bits<8> = (*self_q.data_reg >> bit_idx_safe) & one_b8;
            let data_bit_b: bool = data_bit != zero_b8;

            let is_idle = bc == bits::<4>(0);
            let is_start = bc == one_b4;
            let is_stop = bc == ten_b4;

            let tx = if is_idle {
                true
            } else if is_start {
                false
            } else if is_stop {
                true
            } else {
                data_bit_b
            };

            UartTxOut {
                tx,
                busy: !is_idle,
            }
        }
    }
}

/// Fixture: drive a single byte through the transmitter and collect
/// the per-cycle (tx, busy) outputs.  Returns the post-reset stream
/// only.
fn drive_byte(byte: u128, idle_padding: usize) -> Vec<(bool, bool)> {
    let send_seq: Vec<UartTxIn> = std::iter::once(UartTxIn {
        data: bits::<8>(byte),
        send: true,
    })
    .chain(std::iter::repeat_n(
        UartTxIn {
            data: bits::<8>(0),
            send: false,
        },
        14 + idle_padding,
    ))
    .collect();

    let uut: RuleSimpleUartTx = RuleSimpleUartTx::default();
    uut.run(send_seq.into_iter().with_reset(2).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| (s.output.tx, s.output.busy))
        .collect()
}

#[test]
fn rule_uart_idle_line_is_high() {
    let stream: Vec<UartTxIn> = std::iter::repeat_n(
        UartTxIn {
            data: bits::<8>(0xff),
            send: false,
        },
        4,
    )
    .collect();
    let uut: RuleSimpleUartTx = RuleSimpleUartTx::default();
    let outputs: Vec<UartTxOut> = uut
        .run(stream.into_iter().with_reset(2).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();
    assert!(
        outputs.iter().all(|o| o.tx && !o.busy),
        "idle: tx must be high and busy low; got {outputs:?}",
    );
}

#[test]
fn rule_uart_transmits_byte_with_correct_frame() {
    // 0x55 = 0b01010101 → LSB first: 1, 0, 1, 0, 1, 0, 1, 0
    // Frame: start=0, data[0..=7]=10101010, stop=1
    let outputs = drive_byte(0x55, 4);
    let tx: Vec<bool> = outputs.iter().map(|(t, _)| *t).collect();
    let busy: Vec<bool> = outputs.iter().map(|(_, b)| *b).collect();

    // Find where busy first goes high — that's the cycle the load
    // rule fired.  The bit on the line at that cycle reflects the
    // post-load state (bit_counter = 1 → start bit → tx low).
    let start = busy
        .iter()
        .position(|b| *b)
        .expect("transmission should start");

    // Frame check: tx[start..start+10] should be:
    //   start_bit (low), bit0, bit1, ..., bit7, stop_bit (high)
    // For 0x55: start=0, 1, 0, 1, 0, 1, 0, 1, 0, stop=1
    let expected_frame = vec![
        false, // start
        true, false, true, false, true, false, true, false, // data, LSB first
        true, // stop
    ];
    assert!(
        start + expected_frame.len() <= tx.len(),
        "trace too short to capture full frame",
    );
    let actual_frame = &tx[start..start + expected_frame.len()];
    assert_eq!(
        actual_frame, expected_frame,
        "frame for 0x55 mismatch.  tx sequence: {tx:?}",
    );

    // After the frame, line should return to idle high and busy low.
    let after = start + expected_frame.len();
    if after < tx.len() {
        assert!(tx[after], "post-frame line should be high");
        assert!(!busy[after], "post-frame busy should drop");
    }
}

#[test]
fn rule_uart_back_to_back_bytes() {
    // Two bytes in succession: 0x01 then 0x80.
    let inputs: Vec<UartTxIn> = vec![
        UartTxIn {
            data: bits::<8>(0x01),
            send: true,
        },
    ]
    .into_iter()
    .chain(std::iter::repeat_n(
        UartTxIn {
            data: bits::<8>(0),
            send: false,
        },
        12,
    ))
    .chain(std::iter::once(UartTxIn {
        data: bits::<8>(0x80),
        send: true,
    }))
    .chain(std::iter::repeat_n(
        UartTxIn {
            data: bits::<8>(0),
            send: false,
        },
        14,
    ))
    .collect();

    let uut: RuleSimpleUartTx = RuleSimpleUartTx::default();
    let outputs: Vec<UartTxOut> = uut
        .run(inputs.into_iter().with_reset(2).clock_pos_edge(100))
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    // Should observe two distinct busy windows of 10 cycles each.
    let busy_windows: Vec<usize> = {
        let mut out = Vec::new();
        let mut current = 0usize;
        for o in &outputs {
            if o.busy {
                current += 1;
            } else if current > 0 {
                out.push(current);
                current = 0;
            }
        }
        if current > 0 {
            out.push(current);
        }
        out
    };
    assert_eq!(
        busy_windows,
        vec![10, 10],
        "expected two 10-cycle busy windows; got {busy_windows:?}",
    );
}

#[test]
fn rule_uart_iverilog_round_trip() -> Result<(), RHDLError> {
    let uut: RuleSimpleUartTx = RuleSimpleUartTx::default();
    let inputs: Vec<UartTxIn> = vec![
        UartTxIn {
            data: bits::<8>(0xa5),
            send: true,
        },
    ]
    .into_iter()
    .chain(std::iter::repeat_n(
        UartTxIn {
            data: bits::<8>(0),
            send: false,
        },
        12,
    ))
    .collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
    let tm = test_bench.rtl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    let tm = test_bench.ntl(&uut, &Default::default())?;
    tm.run_iverilog()?;
    Ok(())
}
