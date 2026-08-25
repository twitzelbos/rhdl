//! IEEE 1284 ECP (Extended Capabilities Port) master, fully bidirectional
//!
//! ECP is IEEE 1284 mode 5 — bidirectional, RLE-compressed,
//! interlocked-handshake parallel I/O.  This widget implements
//! **both** the forward channel (host → device) and the reverse
//! channel (device → host), each with run-length compression
//! provided by the composed [super::super::core::rle_encoder::RleEncoder]
//! and [super::super::core::rle_decoder::RleDecoder].
//!
//! **Forward channel** (host → device):
//!
//! 1. Host streams uncompressed bytes via `in_data` / `in_valid`.
//! 2. Internal `RleEncoder` compresses runs into `(count, byte)`
//!    pairs.
//! 3. Each beat (literal or run-encoding) goes through a 4-state
//!    interlocked handshake with the device:
//!    - Drive D + HostClk + nStrobe low.
//!    - Wait for PeriphAck (device acknowledges).
//!    - Release nStrobe.
//!    - Wait for PeriphAck to deassert.
//! 4. `HostClk` distinguishes byte type (high = command/count,
//!    low = data).
//!
//! **Reverse channel** (device → host):
//!
//! 1. Host asserts `nReverseRequest` low to request reverse mode.
//! 2. Device drives D + asserts `PeriphClk` low to clock a byte.
//! 3. Host samples D, asserts `nAckReverse` low to acknowledge.
//! 4. Device deasserts `PeriphClk`.
//! 5. Host deasserts `nAckReverse`.
//! 6. Internal `RleDecoder` expands runs back into a flat byte
//!    stream on `rev_out_data` / `rev_out_valid`.
//!
//! Direction is selected by the `dir_request` input — the FSM
//! gates forward vs. reverse based on this single bit.  Switching
//! direction mid-byte is not allowed (host must wait for the
//! current beat to drain).
//!
//! **All-features-implemented:** forward + reverse channels, RLE
//! compression in both directions (via the reusable encoder /
//! decoder widgets), interlocked handshakes both ways, no FIFO
//! (the encoder/decoder both have built-in back-pressure via
//! `out_ready`/`stalled`).
//!
//! Composes [super::super::core::rle_encoder::RleEncoder] and
//! [super::super::core::rle_decoder::RleDecoder].
//!
//! Here is the schematic symbol
#![doc = badascii_doc::badascii_formal!(r"
     +-----+ParallelPortEcp+----------+
     |                                |
B<8> |                                | B<8>
+--->| in_data                  d_out +--->
bool |                                | bool
+--->| in_valid                  d_oe +--->
bool |                                | bool
+--->| flush               host_clk   +--->
bool |                                | bool
+--->| periph_ack_in       n_strobe   +--->
B<8> |                                | bool
+--->| d_in_rev      n_reverse_req    +--->
bool |                                | bool
+--->| periph_clk_in      n_ack_rev   +--->
bool |                                | B<8>
+--->| dir_request       rev_out_data +--->
     |                  rev_out_valid +--->
     |                          busy  +--->
     |                          done  +--->
     +--------------------------------+
")]
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/parallel_port_ecp.rs")]
//!```
//!
//! The trace below demonstrates the result.
#![doc = include_str!("../../doc/parallel_port_ecp.md")]
//!
//! And the auto-generated FSM diagram for the bidirectional FSM:
#![doc = include_str!("../../doc/parallel_port_ecp_fsm.md")]
use rhdl::prelude::*;

use super::super::core::rle_decoder::{RleDecoder, rle_decoder as rle_decoder_kernel};
use super::super::core::rle_encoder::{RleEncoder, rle_encoder as rle_encoder_kernel};
use crate::core::dff;

#[allow(unused_imports)]
use rle_decoder_kernel as _;
#[allow(unused_imports)]
use rle_encoder_kernel as _;

/// ECP bidirectional FSM state.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default, Fsm)]
pub enum EcpState {
    #[default]
    #[fsm_state(label = "idle")]
    Idle,
    /// Forward — driving D + HostClk + nStrobe low.
    #[fsm_state(label = "fwd: drive")]
    FwdDrive,
    /// Forward — waiting for PeriphAck assertion.
    #[fsm_state(label = "fwd: wait ack")]
    FwdWaitAck,
    /// Forward — releasing nStrobe; waiting for PeriphAck deassertion.
    #[fsm_state(label = "fwd: release")]
    FwdRelease,
    /// Reverse — waiting for device's PeriphClk assertion.
    #[fsm_state(label = "rev: wait clk")]
    RevWaitClk,
    /// Reverse — sampled D; asserting nAckReverse low.
    #[fsm_state(label = "rev: sample")]
    RevSample,
    /// Reverse — waiting for device to release PeriphClk.
    #[fsm_state(label = "rev: wait clk high")]
    RevAckHigh,
}

/// Bundled internal state for the ECP master (CLAUDE.md §3.1).
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct ParallelPortEcpExtras {
    pub fwd_beat_data: Bits<8>,
    pub fwd_beat_is_count: bool,
    pub rev_sample: Bits<8>,
    pub rev_is_count: bool,
    pub rev_byte_valid_pulse: bool,
    pub done_pulse: bool,
}

#[derive(Clone, Debug, Synchronous, SynchronousDQ, Default, FsmWidget)]
#[rhdl(dq_no_prefix)]
#[fsm(state_field = "state", state_enum = EcpState, allow_implicit)]
/// IEEE 1284 ECP master, fully bidirectional with RLE in both directions.
pub struct ParallelPortEcp {
    state: dff::DFF<EcpState>,
    extras: dff::DFF<ParallelPortEcpExtras>,
    /// RLE encoder owns the forward-path compression.
    rle_enc: RleEncoder,
    /// RLE decoder owns the reverse-path decompression.
    rle_dec: RleDecoder,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Inputs to [ParallelPortEcp].
pub struct In {
    /// Forward — next input byte from the host.
    pub in_data: Bits<8>,
    /// Forward — strobe to consume `in_data`.
    pub in_valid: bool,
    /// Forward — strobe when input stream ends.
    pub flush: bool,
    /// Forward — sampled `PeriphAck` from the device.
    pub periph_ack_in: bool,
    /// Reverse — sampled D[7:0] from the device.
    pub d_in_rev: Bits<8>,
    /// Reverse — sampled `PeriphClk` from the device.
    pub periph_clk_in: bool,
    /// Reverse — host pulses this to claim the next decoded byte.
    pub rev_out_ready: bool,
    /// Direction selector: `false` = forward, `true` = reverse.
    pub dir_request: bool,
    /// Reverse — sampled `host_clk_in_rev` (or equivalent) — distinguishes
    /// data vs count byte in the device's reverse stream.
    pub rev_is_count_in: bool,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
/// Outputs from [ParallelPortEcp].
pub struct Out {
    /// Forward — 8-bit data driven on D[7:0] (only meaningful while `d_oe == 1`).
    pub d_out: Bits<8>,
    /// True ⇒ host drives the D bus (forward direction).
    pub d_oe: bool,
    /// `HostClk` line — distinguishes data (low) vs command/RLE-count (high).
    pub host_clk: bool,
    /// `nStrobe` (active-low data-valid strobe).
    pub n_strobe: bool,
    /// `nReverseRequest` — host asserts low to request reverse mode.
    pub n_reverse_req: bool,
    /// `nAckReverse` — host asserts low to acknowledge a reverse byte.
    pub n_ack_rev: bool,
    /// Reverse — decoded output byte (after run-length expansion).
    pub rev_out_data: Bits<8>,
    /// Reverse — true while a fresh decoded byte is on `rev_out_data`.
    pub rev_out_valid: bool,
    /// True while compressing, transmitting, or receiving (any non-Idle state).
    pub busy: bool,
    /// Pulses for one cycle when a forward beat completes.
    pub done: bool,
}

impl SynchronousIO for ParallelPortEcp {
    type I = In;
    type O = Out;
    type Kernel = parallel_port_ecp;
}

#[kernel]
/// Kernel for [ParallelPortEcp].
pub fn parallel_port_ecp(cr: ClockReset, i: In, q: Q) -> (Out, D) {
    let mut d = D::dont_care();
    d.state = q.state;
    let mut next = q.extras;
    next.rev_byte_valid_pulse = false;
    next.done_pulse = false;

    let mut rle_enc_out_ready = false;
    let mut rle_dec_in_valid = false;

    let beat_data = q.rle_enc.out_data;
    let beat_is_count = q.rle_enc.out_is_count;
    let beat_valid = q.rle_enc.out_valid;

    match q.state {
        EcpState::Idle => {
            if !i.dir_request {
                if beat_valid {
                    next.fwd_beat_data = beat_data;
                    next.fwd_beat_is_count = beat_is_count;
                    d.state = EcpState::FwdDrive;
                    rle_enc_out_ready = true;
                }
            } else {
                d.state = EcpState::RevWaitClk;
            }
        }
        EcpState::FwdDrive => {
            d.state = EcpState::FwdWaitAck;
        }
        EcpState::FwdWaitAck => {
            if i.periph_ack_in {
                d.state = EcpState::FwdRelease;
            }
        }
        EcpState::FwdRelease => {
            if !i.periph_ack_in {
                d.state = EcpState::Idle;
                next.done_pulse = true;
            }
        }
        EcpState::RevWaitClk => {
            if !i.periph_clk_in {
                next.rev_sample = i.d_in_rev;
                next.rev_is_count = i.rev_is_count_in;
                d.state = EcpState::RevSample;
            } else if !i.dir_request {
                d.state = EcpState::Idle;
            }
        }
        EcpState::RevSample => {
            rle_dec_in_valid = true;
            next.rev_byte_valid_pulse = true;
            d.state = EcpState::RevAckHigh;
        }
        EcpState::RevAckHigh => {
            if i.periph_clk_in {
                d.state = EcpState::Idle;
                next.done_pulse = true;
            }
        }
    }

    d.rle_enc = super::super::core::rle_encoder::In {
        in_data: i.in_data,
        in_valid: i.in_valid,
        flush: i.flush,
        out_ready: rle_enc_out_ready,
    };

    d.rle_dec = super::super::core::rle_decoder::In {
        in_data: q.extras.rev_sample,
        in_is_count: q.extras.rev_is_count,
        in_valid: rle_dec_in_valid,
        out_ready: i.rev_out_ready,
    };

    if cr.reset.any() {
        d.state = EcpState::Idle;
        next = ParallelPortEcpExtras::default();
    }

    d.extras = next;

    let busy = q.state != EcpState::Idle;

    let fwd_drive_active = match q.state {
        EcpState::FwdDrive | EcpState::FwdWaitAck => true,
        _ => false,
    };

    let rev_ack_active = match q.state {
        EcpState::RevSample | EcpState::RevAckHigh => true,
        _ => false,
    };
    let n_reverse_req = !i.dir_request;

    let mut o = Out::dont_care();
    o.d_out = q.extras.fwd_beat_data;
    o.d_oe = fwd_drive_active;
    o.host_clk = q.extras.fwd_beat_is_count;
    o.n_strobe = !fwd_drive_active;
    o.n_reverse_req = n_reverse_req;
    o.n_ack_rev = !rev_ack_active;
    o.rev_out_data = q.rle_dec.out_data;
    o.rev_out_valid = q.rle_dec.out_valid;
    o.busy = busy;
    o.done = q.extras.done_pulse;
    (o, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use std::path::PathBuf;

    fn idle_in() -> In {
        In {
            in_data: bits(0),
            in_valid: false,
            flush: false,
            periph_ack_in: false,
            d_in_rev: bits(0),
            periph_clk_in: true,
            rev_out_ready: false,
            dir_request: false,
            rev_is_count_in: false,
        }
    }

    /// Drive a sequence of forward bytes and capture the (d_out, host_clk)
    /// tuples.
    fn drive_forward(input: &[u8]) -> Vec<(u8, bool)> {
        let uut = ParallelPortEcp::default();
        let mut stream_in: Vec<In> = Vec::new();
        // Push all input bytes back-to-back (forward direction).
        for &b in input {
            stream_in.push(In {
                in_data: bits(b as u128),
                in_valid: true,
                flush: false,
                periph_ack_in: false,
                d_in_rev: bits(0),
                periph_clk_in: true,
                rev_out_ready: false,
                dir_request: false,
                rev_is_count_in: false,
            });
            stream_in.push(idle_in());
        }
        // Flush.
        stream_in.push(In {
            in_data: bits(0),
            in_valid: false,
            flush: true,
            periph_ack_in: false,
            d_in_rev: bits(0),
            periph_clk_in: true,
            rev_out_ready: false,
            dir_request: false,
            rev_is_count_in: false,
        });
        // Drain.
        for _ in 0..40 {
            stream_in.push(idle_in());
            stream_in.push(idle_in());
            stream_in.push(In {
                in_data: bits(0),
                in_valid: false,
                flush: false,
                periph_ack_in: true,
                d_in_rev: bits(0),
                periph_clk_in: true,
                rev_out_ready: false,
                dir_request: false,
                rev_is_count_in: false,
            });
            stream_in.push(In {
                in_data: bits(0),
                in_valid: false,
                flush: false,
                periph_ack_in: true,
                d_in_rev: bits(0),
                periph_clk_in: true,
                rev_out_ready: false,
                dir_request: false,
                rev_is_count_in: false,
            });
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        outputs
            .iter()
            .filter(|s| s.output.done && !s.input.1.dir_request)
            .map(|s| (s.output.d_out.raw() as u8, s.output.host_clk))
            .collect()
    }

    /// Drive a reverse beat sequence (device-side encoded) and capture
    /// the decoded output bytes.
    fn drive_reverse(beats: &[(u8, bool)]) -> Vec<u8> {
        let uut = ParallelPortEcp::default();
        let mut stream_in: Vec<In> = Vec::new();
        // Set direction = reverse for the entire run.
        for &(b, is_count) in beats {
            // Wait for FSM to enter RevWaitClk.
            stream_in.push(In {
                in_data: bits(0),
                in_valid: false,
                flush: false,
                periph_ack_in: false,
                d_in_rev: bits(b as u128),
                periph_clk_in: true,
                rev_out_ready: true,
                dir_request: true,
                rev_is_count_in: is_count,
            });
            // Device asserts periph_clk low.
            for _ in 0..3 {
                stream_in.push(In {
                    in_data: bits(0),
                    in_valid: false,
                    flush: false,
                    periph_ack_in: false,
                    d_in_rev: bits(b as u128),
                    periph_clk_in: false,
                    rev_out_ready: true,
                    dir_request: true,
                    rev_is_count_in: is_count,
                });
            }
            // Device releases periph_clk.
            for _ in 0..6 {
                stream_in.push(In {
                    in_data: bits(0),
                    in_valid: false,
                    flush: false,
                    periph_ack_in: false,
                    d_in_rev: bits(b as u128),
                    periph_clk_in: true,
                    rev_out_ready: true,
                    dir_request: true,
                    rev_is_count_in: is_count,
                });
            }
        }
        // Trailing settle.
        for _ in 0..16 {
            stream_in.push(In {
                in_data: bits(0),
                in_valid: false,
                flush: false,
                periph_ack_in: false,
                d_in_rev: bits(0),
                periph_clk_in: true,
                rev_out_ready: true,
                dir_request: true,
                rev_is_count_in: false,
            });
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        uut.run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .filter(|s| s.output.rev_out_valid && s.input.1.rev_out_ready)
            .map(|s| s.output.rev_out_data.raw() as u8)
            .collect()
    }

    #[test]
    fn test_idle_no_strobe() -> miette::Result<()> {
        let uut = ParallelPortEcp::default();
        let stream = std::iter::repeat_n(idle_in(), 32)
            .with_reset(1)
            .clock_pos_edge(100);
        let any_strobe = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .any(|s| !s.output.n_strobe);
        assert!(!any_strobe);
        Ok(())
    }

    #[test]
    fn test_forward_single_literal() -> miette::Result<()> {
        let captured = drive_forward(&[0x42]);
        assert_eq!(captured, vec![(0x42, false)]);
        Ok(())
    }

    #[test]
    fn test_forward_run_compresses() -> miette::Result<()> {
        let captured = drive_forward(&[0xAA, 0xAA, 0xAA]);
        assert_eq!(captured, vec![(2, true), (0xAA, false)]);
        Ok(())
    }

    /// Reverse-direction corner case: device sends a literal, host decodes it.
    #[test]
    fn test_reverse_single_literal() -> miette::Result<()> {
        let captured = drive_reverse(&[(0x55, false)]);
        assert_eq!(captured, vec![0x55]);
        Ok(())
    }

    /// Reverse-direction corner case: device sends a run, host decodes
    /// it via the composed `RleDecoder`.
    #[test]
    fn test_reverse_run_decompresses() -> miette::Result<()> {
        // Run of 3: count=2, data=0xAA.
        let captured = drive_reverse(&[(2, true), (0xAA, false)]);
        assert_eq!(captured, vec![0xAA, 0xAA, 0xAA]);
        Ok(())
    }

    /// Reverse direction asserts n_reverse_req while dir_request is high.
    #[test]
    fn test_reverse_request_line() -> miette::Result<()> {
        let uut = ParallelPortEcp::default();
        let stream_in: Vec<In> = vec![
            In {
                in_data: bits(0),
                in_valid: false,
                flush: false,
                periph_ack_in: false,
                d_in_rev: bits(0),
                periph_clk_in: true,
                rev_out_ready: false,
                dir_request: true,
                rev_is_count_in: false,
            };
            16
        ];
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let outputs: Vec<_> = uut
            .run(stream)
            .synchronous_sample()
            .filter(|s| !s.input.0.reset.any())
            .collect();
        // n_reverse_req must be low (asserted) throughout.
        assert!(outputs.iter().all(|s| !s.output.n_reverse_req));
        Ok(())
    }

    #[test]
    fn test_vlog_generation() -> miette::Result<()> {
        let uut = ParallelPortEcp::default();
        let desc = uut.descriptor("top".into())?;
        let hdl = desc.hdl()?.modules.pretty();
        let expect = expect!["32453"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    #[test]
    fn test_parallel_port_ecp_hdl_works() -> miette::Result<()> {
        let uut = ParallelPortEcp::default();
        let mut stream_in: Vec<In> = vec![In {
            in_data: bits(0x42),
            in_valid: true,
            flush: false,
            periph_ack_in: false,
            d_in_rev: bits(0),
            periph_clk_in: true,
            rev_out_ready: false,
            dir_request: false,
            rev_is_count_in: false,
        }];
        for _ in 0..40 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let test_bench = uut.run(stream).collect::<SynchronousTestBench<_, _>>();
        let tm = test_bench.rtl(&uut, &Default::default())?;
        tm.run_iverilog()?;
        Ok(())
    }

    #[test]
    fn test_parallel_port_ecp_trace() -> miette::Result<()> {
        let uut = ParallelPortEcp::default();
        let mut stream_in: Vec<In> = Vec::new();
        for &b in &[0x42u8, 0xAA, 0xAA, 0xAA] {
            stream_in.push(In {
                in_data: bits(b as u128),
                in_valid: true,
                flush: false,
                periph_ack_in: false,
                d_in_rev: bits(0),
                periph_clk_in: true,
                rev_out_ready: false,
                dir_request: false,
                rev_is_count_in: false,
            });
            stream_in.push(idle_in());
        }
        stream_in.push(In {
            in_data: bits(0),
            in_valid: false,
            flush: true,
            periph_ack_in: false,
            d_in_rev: bits(0),
            periph_clk_in: true,
            rev_out_ready: false,
            dir_request: false,
            rev_is_count_in: false,
        });
        for _ in 0..40 {
            stream_in.push(idle_in());
        }
        let stream = stream_in.into_iter().with_reset(1).clock_pos_edge(100);
        let vcd = uut.run(stream).collect::<VcdFile>();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("parallel_port_ecp");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["95884a1fe3e5460e0409ed83bee682479bf5593b0c983ee25bb9d16c767bd31f"];
        let digest = vcd
            .dump_to_file(root.join("parallel_port_ecp.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }

    #[test]
    fn test_fsm_descriptor_round_trip() {
        let desc = ParallelPortEcp::fsm_descriptor();
        assert_eq!(desc.widget_name, "ParallelPortEcp");
        assert_eq!(desc.variants().len(), 7);
        assert_eq!(desc.initial_index(), 0);
    }
}
