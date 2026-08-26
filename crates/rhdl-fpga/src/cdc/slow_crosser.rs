//!# Multi-bit handshake bridge (slow CDC)
//!
//! Crosses an arbitrary `T: Digital` signal from a source clock
//! domain `W` to a destination clock domain `R` using a textbook
//! 4-phase request/acknowledge handshake.  The data register is
//! held stable in the `W` domain while `req` is asserted, so the
//! `R` domain can sample its bits directly without per-bit
//! synchronization (the metastability protection lives in the
//! single-bit `req`/`ack` synchronizers).
//!
//! Use this when:
//! - You need to move a multi-bit value (configuration register,
//!   status word, command code) between clock domains.
//! - The transfer rate is *much* slower than either clock — every
//!   crossing takes ~6–8 destination cycles plus ~6–8 source
//!   cycles.  For high-throughput multi-bit crossings, use
//!   [super::super::fifo::asynchronous::AsyncFIFO] instead.
//! - You can guarantee the source side waits for `busy` to drop
//!   before issuing a new value.
//!
//!# Schematic Symbol
//!
#![doc = badascii_doc::badascii_formal!(r"
        +-+SlowCrosser+-+
        |               |
   T    |               |    T
+------>| src_data      | data +--->
        |          dst  |
   bool |               |    bool
+------>| src_send  busy+----->
        |          src  |
        |               |
        |        src_cr |<---+ ClockReset (W)
        |               |
        |        dst_cr |<---+ ClockReset (R)
        +---------------+
")]
//!
//!# Internals
//!
//! Two halves of a state machine, one per clock domain, plus a pair
//! of single-bit synchronizers (one for `req` going W→R, one for
//! `ack` going R→W).  The data signal is a plain wire from
//! `data_reg` (W-domain register) into the destination's sample
//! mux; it is *only* sampled when the synchronized `req` is high,
//! by which point the data has been stable for several W cycles.
//!
//!# 4-phase handshake
//!
//! 1. Source `Idle`, `src_send` asserted → latch `data_reg`,
//!    drive `req=1`, transition to `WaitForAck`.
//! 2. Destination `Idle`, sees synchronized `req=1` → sample
//!    `data_reg` into `data_out`, drive `ack=1`, transition to
//!    `WaitForReqClear`.
//! 3. Source sees synchronized `ack=1` → drive `req=0`, transition
//!    to `WaitForAckClear`.
//! 4. Destination sees synchronized `req=0` → drive `ack=0`,
//!    transition back to `Idle`.
//! 5. Source sees synchronized `ack=0` → transition back to
//!    `Idle`.  `busy` drops; ready for next send.
//!
//!# Domain assumptions
//!
//! - `data_reg` is held stable from step 1 through step 5; the
//!   `R` domain only samples it after step 2's `req_sync_2 = 1`,
//!   by which point it has been stable for at least two `R` clock
//!   periods (the 2-FF synchronizer's settling window).
//! - The cross-domain wires `req` (W→R) and `ack` (R→W) each
//!   pass through a 2-FF synchronizer in the destination domain.
//!   Same caveats as [super::synchronizer::Sync1Bit] apply: the
//!   first FF in each chain is the metastability-resolution stage.
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/slow_crosser.rs")]
//!```
//!
//! With an output trace
#![doc = include_str!("../../doc/slow_crosser.md")]

use quote::format_ident;
use rhdl::{
    core::{ScopedName, circuit::descriptor::AsyncKind},
    prelude::*,
};
use syn::parse_quote;

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
#[doc(hidden)]
pub enum SrcState {
    #[default]
    Idle,
    WaitForAck,
    WaitForAckClear,
}

#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
#[doc(hidden)]
pub enum DstState {
    #[default]
    Idle,
    WaitForReqClear,
}

/// Multi-bit slow-CDC bridge from domain `W` to domain `R`.
///
/// `T` is the data type to be carried; it must be `Digital`.
#[derive(PartialEq, Debug, Clone, Default)]
pub struct SlowCrosser<T: Digital, W: Domain, R: Domain> {
    _t: std::marker::PhantomData<T>,
    _w: std::marker::PhantomData<W>,
    _r: std::marker::PhantomData<R>,
}

#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
/// Inputs to the [SlowCrosser].
pub struct In<T: Digital, W: Domain, R: Domain> {
    /// The data value to send (latched at the cycle `src_send` is asserted).
    pub src_data: Signal<T, W>,
    /// Strobe to start a new crossing.  Must only be asserted when `busy` is low.
    pub src_send: Signal<bool, W>,
    /// Clock and reset for the source `W` domain.
    pub src_cr: Signal<ClockReset, W>,
    /// Clock and reset for the destination `R` domain.
    pub dst_cr: Signal<ClockReset, R>,
}

#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
/// Outputs from the [SlowCrosser].
pub struct Out<T: Digital, W: Domain, R: Domain> {
    /// The latest received value, in the `R` domain.  Holds its
    /// previous value until a new crossing completes.
    pub data: Signal<T, R>,
    /// High in the `W` domain while a crossing is in progress.
    /// Source must wait for this to drop before next `src_send`.
    pub busy: Signal<bool, W>,
}

impl<T: Digital, W: Domain, R: Domain> CircuitDQ for SlowCrosser<T, W, R> {
    type D = ();
    type Q = ();
}

impl<T: Digital, W: Domain, R: Domain> CircuitIO for SlowCrosser<T, W, R> {
    type I = In<T, W, R>;
    type O = Out<T, W, R>;
    type Kernel = NoCircuitKernel<Self::I, (), (Self::O, ())>;
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
#[doc(hidden)]
pub struct S<T: Digital> {
    // Last-seen clocks (for edge detection)
    src_clock: Clock,
    dst_clock: Clock,
    // W-domain registers (current and next)
    data_reg: T,
    data_reg_next: T,
    req: bool,
    req_next: bool,
    src_state: SrcState,
    src_state_next: SrcState,
    ack_sync_1: bool,
    ack_sync_1_next: bool,
    ack_sync_2: bool,
    ack_sync_2_next: bool,
    // R-domain registers (current and next)
    data_out: T,
    data_out_next: T,
    ack: bool,
    ack_next: bool,
    dst_state: DstState,
    dst_state_next: DstState,
    req_sync_1: bool,
    req_sync_1_next: bool,
    req_sync_2: bool,
    req_sync_2_next: bool,
}

impl<T: Digital, W: Domain, R: Domain> Circuit for SlowCrosser<T, W, R> {
    type S = S<T>;

    fn init(&self) -> Self::S {
        S {
            src_clock: Clock::dont_care(),
            dst_clock: Clock::dont_care(),
            data_reg: T::dont_care(),
            data_reg_next: T::dont_care(),
            req: false,
            req_next: false,
            src_state: SrcState::Idle,
            src_state_next: SrcState::Idle,
            ack_sync_1: false,
            ack_sync_1_next: false,
            ack_sync_2: false,
            ack_sync_2_next: false,
            data_out: T::dont_care(),
            data_out_next: T::dont_care(),
            ack: false,
            ack_next: false,
            dst_state: DstState::Idle,
            dst_state_next: DstState::Idle,
            req_sync_1: false,
            req_sync_1_next: false,
            req_sync_2: false,
            req_sync_2_next: false,
        }
    }

    fn sim(&self, input: Self::I, state: &mut Self::S) -> Self::O {
        let src_clock = input.src_cr.val().clock;
        let src_reset = input.src_cr.val().reset;
        let dst_clock = input.dst_cr.val().clock;
        let dst_reset = input.dst_cr.val().reset;
        trace("src_clock", &src_clock);
        trace("dst_clock", &dst_clock);
        trace("src_data", &input.src_data);
        trace("src_send", &input.src_send);

        // === W-domain combinational pre-edge computation ===
        if !src_clock.raw() {
            let ack_seen = state.ack_sync_2;
            // src_state machine
            match state.src_state {
                SrcState::Idle => {
                    if input.src_send.val() {
                        state.data_reg_next = input.src_data.val();
                        state.req_next = true;
                        state.src_state_next = SrcState::WaitForAck;
                    } else {
                        state.data_reg_next = state.data_reg;
                        state.req_next = state.req;
                        state.src_state_next = SrcState::Idle;
                    }
                }
                SrcState::WaitForAck => {
                    state.data_reg_next = state.data_reg;
                    if ack_seen {
                        state.req_next = false;
                        state.src_state_next = SrcState::WaitForAckClear;
                    } else {
                        state.req_next = state.req;
                        state.src_state_next = SrcState::WaitForAck;
                    }
                }
                SrcState::WaitForAckClear => {
                    state.data_reg_next = state.data_reg;
                    state.req_next = state.req;
                    if !ack_seen {
                        state.src_state_next = SrcState::Idle;
                    } else {
                        state.src_state_next = SrcState::WaitForAckClear;
                    }
                }
            }
            // ack synchronizer (R→W)
            state.ack_sync_1_next = state.ack;
            state.ack_sync_2_next = state.ack_sync_1;
        }

        // === R-domain combinational pre-edge computation ===
        if !dst_clock.raw() {
            let req_seen = state.req_sync_2;
            // dst_state machine
            match state.dst_state {
                DstState::Idle => {
                    if req_seen {
                        // Sample W-domain data (it has been stable while req was high
                        // long enough for the 2-FF synchronizer to settle).
                        state.data_out_next = state.data_reg;
                        state.ack_next = true;
                        state.dst_state_next = DstState::WaitForReqClear;
                    } else {
                        state.data_out_next = state.data_out;
                        state.ack_next = state.ack;
                        state.dst_state_next = DstState::Idle;
                    }
                }
                DstState::WaitForReqClear => {
                    state.data_out_next = state.data_out;
                    if !req_seen {
                        state.ack_next = false;
                        state.dst_state_next = DstState::Idle;
                    } else {
                        state.ack_next = state.ack;
                        state.dst_state_next = DstState::WaitForReqClear;
                    }
                }
            }
            // req synchronizer (W→R)
            state.req_sync_1_next = state.req;
            state.req_sync_2_next = state.req_sync_1;
        }

        // === Reset overrides (per-domain) ===
        if src_reset.raw() {
            state.data_reg_next = T::dont_care();
            state.req_next = false;
            state.src_state_next = SrcState::Idle;
            state.ack_sync_1_next = false;
            state.ack_sync_2_next = false;
        }
        if dst_reset.raw() {
            state.data_out_next = T::dont_care();
            state.ack_next = false;
            state.dst_state_next = DstState::Idle;
            state.req_sync_1_next = false;
            state.req_sync_2_next = false;
        }

        // === Edge-triggered latching ===
        if src_clock.raw() && !state.src_clock.raw() {
            state.data_reg = state.data_reg_next;
            state.req = state.req_next;
            state.src_state = state.src_state_next;
            state.ack_sync_1 = state.ack_sync_1_next;
            state.ack_sync_2 = state.ack_sync_2_next;
        }
        if dst_clock.raw() && !state.dst_clock.raw() {
            state.data_out = state.data_out_next;
            state.ack = state.ack_next;
            state.dst_state = state.dst_state_next;
            state.req_sync_1 = state.req_sync_1_next;
            state.req_sync_2 = state.req_sync_2_next;
        }

        state.src_clock = src_clock;
        state.dst_clock = dst_clock;

        let busy = state.src_state != SrcState::Idle;
        trace("data_out", &state.data_out);
        trace("busy", &busy);
        Out {
            data: signal(state.data_out),
            busy: signal(busy),
        }
    }

    fn descriptor(&self, scoped_name: ScopedName) -> Result<Descriptor<AsyncKind>, RHDLError> {
        let name = scoped_name.to_string();
        Descriptor::<AsyncKind> {
            combinational_reachability: Default::default(),
            name: scoped_name,
            input_kind: <<Self as CircuitIO>::I as Digital>::static_kind(),
            output_kind: <<Self as CircuitIO>::O as Digital>::static_kind(),
            d_kind: <<Self as CircuitDQ>::D as Digital>::static_kind(),
            q_kind: <<Self as CircuitDQ>::Q as Digital>::static_kind(),
            kernel: None,
            netlist: None,
            hdl: Some(self.hdl(&name)?),
            _phantom: std::marker::PhantomData,
        }
        // Nothing feeds through: a registered handshake across the domain crossing.
        .with_netlist_black_box(BlackBoxConnectivity::None)
    }
}

impl<T: Digital, W: Domain, R: Domain> SlowCrosser<T, W, R> {
    fn hdl(&self, name: &str) -> Result<HDLDescriptor, RHDLError> {
        let module_name = name.to_owned();
        let module_ident = format_ident!("{}", module_name);
        let i_kind = <<Self as CircuitIO>::I as Digital>::static_kind();
        let o_kind = <<Self as CircuitIO>::O as Digital>::static_kind();
        let i = <Self as CircuitIO>::I::dont_care();
        let o = <Self as CircuitIO>::O::dont_care();
        let i_bits = i_kind.bits();
        let o_bits = o_kind.bits();
        let t_bits = T::BITS;
        let i_bus: vlog::BitRange = (0..i_bits).into();
        let o_bus: vlog::BitRange = (0..o_bits).into();
        let data_bus: vlog::BitRange = (0..t_bits).into();
        // Input bit positions
        let src_data_range = bit_range(i_kind, &path!(i.src_data))?;
        let src_data_lo = syn::Index::from(src_data_range.0.start);
        let src_data_hi = syn::Index::from(src_data_range.0.end - 1);
        let src_send_idx = syn::Index::from(bit_range(i_kind, &path!(i.src_send))?.0.start);
        let src_clk_idx =
            syn::Index::from(bit_range(i_kind, &path!(i.src_cr.val().clock))?.0.start);
        let src_rst_idx =
            syn::Index::from(bit_range(i_kind, &path!(i.src_cr.val().reset))?.0.start);
        let dst_clk_idx =
            syn::Index::from(bit_range(i_kind, &path!(i.dst_cr.val().clock))?.0.start);
        let dst_rst_idx =
            syn::Index::from(bit_range(i_kind, &path!(i.dst_cr.val().reset))?.0.start);
        // Output bit positions
        let data_out_range = bit_range(o_kind, &path!(o.data))?;
        let data_out_lo = syn::Index::from(data_out_range.0.start);
        let data_out_hi = syn::Index::from(data_out_range.0.end - 1);
        let busy_idx = syn::Index::from(bit_range(o_kind, &path!(o.busy))?.0.start);
        let one_bit: vlog::BitRange = (0..1).into();
        let module: vlog::ModuleDef = parse_quote! {
            module #module_ident(input wire [#i_bus] i, output wire [#o_bus] o);
                wire [#data_bus] src_data;
                wire [#one_bit] send_in;
                wire [#one_bit] src_clock;
                wire [#one_bit] src_reset;
                wire [#one_bit] dst_clock;
                wire [#one_bit] dst_reset;
                assign src_data = i[#src_data_hi:#src_data_lo];
                assign send_in = i[#src_send_idx];
                assign src_clock = i[#src_clk_idx];
                assign src_reset = i[#src_rst_idx];
                assign dst_clock = i[#dst_clk_idx];
                assign dst_reset = i[#dst_rst_idx];

                // W-domain registers
                reg [#data_bus] data_reg;
                reg [0:0] req;
                reg [1:0] src_state;
                reg [0:0] ack_sync_1;
                reg [0:0] ack_sync_2;
                // R-domain registers
                reg [#data_bus] data_out;
                reg [0:0] ack;
                reg [0:0] dst_state;
                reg [0:0] req_sync_1;
                reg [0:0] req_sync_2;

                // src_state encoding: 0=Idle, 1=WaitForAck, 2=WaitForAckClear
                // dst_state encoding: 0=Idle, 1=WaitForReqClear

                initial begin
                    data_reg = 0;
                    req = 1'b0;
                    src_state = 2'd0;
                    ack_sync_1 = 1'b0;
                    ack_sync_2 = 1'b0;
                    data_out = 0;
                    ack = 1'b0;
                    dst_state = 1'b0;
                    req_sync_1 = 1'b0;
                    req_sync_2 = 1'b0;
                end

                // === W-domain logic (src_clock) ===
                always @(posedge src_clock) begin
                    if (src_reset) begin
                        data_reg <= 0;
                        req <= 1'b0;
                        src_state <= 2'd0;
                        ack_sync_1 <= 1'b0;
                        ack_sync_2 <= 1'b0;
                    end else begin
                        // ack synchronizer (R→W)
                        ack_sync_1 <= ack;
                        ack_sync_2 <= ack_sync_1;
                        // src state machine
                        case (src_state)
                            2'd0: begin // Idle
                                if (send_in) begin
                                    data_reg <= src_data;
                                    req <= 1'b1;
                                    src_state <= 2'd1;
                                end
                            end
                            2'd1: begin // WaitForAck
                                if (ack_sync_2) begin
                                    req <= 1'b0;
                                    src_state <= 2'd2;
                                end
                            end
                            2'd2: begin // WaitForAckClear
                                if (!ack_sync_2) begin
                                    src_state <= 2'd0;
                                end
                            end
                            default: src_state <= 2'd0;
                        endcase
                    end
                end

                // === R-domain logic (dst_clock) ===
                always @(posedge dst_clock) begin
                    if (dst_reset) begin
                        data_out <= 0;
                        ack <= 1'b0;
                        dst_state <= 1'b0;
                        req_sync_1 <= 1'b0;
                        req_sync_2 <= 1'b0;
                    end else begin
                        // req synchronizer (W→R)
                        req_sync_1 <= req;
                        req_sync_2 <= req_sync_1;
                        // dst state machine
                        case (dst_state)
                            1'b0: begin // Idle
                                if (req_sync_2) begin
                                    data_out <= data_reg;
                                    ack <= 1'b1;
                                    dst_state <= 1'b1;
                                end
                            end
                            1'b1: begin // WaitForReqClear
                                if (!req_sync_2) begin
                                    ack <= 1'b0;
                                    dst_state <= 1'b0;
                                end
                            end
                        endcase
                    end
                end

                // Pack output: { busy, data }
                assign o[#data_out_hi:#data_out_lo] = data_out;
                assign o[#busy_idx] = (src_state != 2'd0);
            endmodule
        };
        Ok(HDLDescriptor {
            name: module_name,
            modules: module.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use expect_test::expect;
    use rhdl::prelude::vlog::Pretty;

    use super::*;

    /// Build a stream that sends a sequence of values across the bridge.
    /// W clock period = 100 ps, R clock period = 79 ps so the two domains
    /// are not phase-aligned.
    fn cross_stream() -> impl Iterator<Item = TimedSample<In<Bits<8>, Red, Blue>>> {
        // The W side issues a `src_send` pulse for one cycle, then waits
        // many cycles before issuing the next.  This guarantees the
        // crossing has time to complete.
        let mut src_pattern: Vec<(Bits<8>, bool)> = Vec::new();
        let values = [bits::<8>(0xA5), bits(0x5A), bits(0xFF), bits(0x10)];
        for &v in &values {
            // send for one cycle, then 30 idle cycles.
            src_pattern.push((v, true));
            for _ in 0..30 {
                src_pattern.push((v, false));
            }
        }
        let red = src_pattern.into_iter().with_reset(2).clock_pos_edge(100);
        let blue = std::iter::repeat(false).with_reset(2).clock_pos_edge(79);
        red.merge_map(blue, |r, b| In {
            src_data: signal(r.1.0),
            src_send: signal(r.1.1),
            src_cr: signal(r.0),
            dst_cr: signal(b.0),
        })
    }

    // Tier 2 — iterator simulation, end-to-end functional check.
    //
    // Issue 4 distinct values; the destination should observe each
    // one in order (with 1-cycle hold for the busy strobe and a few
    // R cycles of latency per crossing).
    #[test]
    fn test_crossings_arrive_in_order() -> miette::Result<()> {
        let uut = SlowCrosser::<Bits<8>, Red, Blue>::default();
        // Sample data on R-domain rising edge; collect distinct values.
        let outputs = uut
            .run(cross_stream())
            .sample_at_neg_edge(|t| t.input.dst_cr.val().clock)
            .map(|t| t.output.data.val().raw())
            .collect::<Vec<_>>();
        // The first observation should be 0 (initial dont_care prints as 0
        // in the Rust simulator).  After that, the four sent values should
        // appear in order — possibly each repeated for several cycles
        // before the next crossing completes.
        let mut distinct: Vec<u128> = Vec::new();
        for v in &outputs {
            if distinct.last() != Some(v) {
                distinct.push(*v);
            }
        }
        // Expect the trailing sequence to contain our four values in order.
        let want = [0xA5u128, 0x5A, 0xFF, 0x10];
        // Find each `want[i]` in `distinct` after the previous one.
        let mut idx = 0;
        for w in want {
            while idx < distinct.len() && distinct[idx] != w {
                idx += 1;
            }
            assert!(
                idx < distinct.len(),
                "expected {w:#x} in distinct trail, got {distinct:?}"
            );
            idx += 1;
        }
        Ok(())
    }

    // Tier 3 — HDL emission length sanity check.
    //
    // The HDL is large (~2 KB) and parameterized by `T::BITS` per
    // instantiation; a length proxy catches accidental codegen drift
    // without the snapshot brittleness of capturing the full text.
    #[test]
    fn test_hdl_generation_length() -> miette::Result<()> {
        let uut = SlowCrosser::<Bits<8>, Red, Blue>::default();
        let hdl = uut.hdl("top")?.modules.pretty();
        let expect = expect!["2520"];
        expect.assert_eq(&hdl.len().to_string());
        Ok(())
    }

    // Tier 4 — iverilog elaboration.
    //
    // Per the existing `cdc::synchronizer` and
    // `cdc::synchronizer_chain` convention, the asynchronous
    // testbench cannot align cycle-for-cycle with hand-written
    // multi-domain widgets; we run iverilog with `skip(!0)` to get
    // elaboration coverage without per-sample comparison.  Functional
    // correctness is covered by [test_crossings_arrive_in_order] and
    // by the VCD digest.
    #[test]
    fn test_crosser_hdl_works() -> miette::Result<()> {
        let uut = SlowCrosser::<Bits<8>, Red, Blue>::default();
        let test_bench = uut.run(cross_stream()).collect::<TestBench<_, _>>();
        let test_mod = test_bench.rtl(&uut, &TestBenchOptions::default().skip(!0))?;
        test_mod.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest.
    #[test]
    fn test_crosser_trace() -> miette::Result<()> {
        let uut = SlowCrosser::<Bits<8>, Red, Blue>::default();
        let vcd = uut.run(cross_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("slow_crosser");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["6a1042f30742df31ccb26dec825bff77b54e5bca489dc14e8b12798fea2e8242"];
        let digest = vcd.dump_to_file(root.join("slow_crosser.vcd")).unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
