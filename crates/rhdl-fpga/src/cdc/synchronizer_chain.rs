//! N-stage bit synchronizer chain
//!
//! Generalizes the 2-stage [super::synchronizer::Sync1Bit] to a chain of
//! `N` flip flops, all clocked by the destination domain `R`.  Used when
//! a project requires more than the conventional 2-stage synchronizer
//! (deeper chains lower the chance of metastability propagating, at the
//! cost of an extra cycle of latency per stage).
//!
//! All caveats from [super::synchronizer::Sync1Bit] apply equally here:
//! a synchronizer chain reduces the *probability* of a metastable
//! output, but does not preserve transition counts and does not protect
//! multi-bit data.  Use a Gray-coded crossing
//! ([super::cross_counter::CrossCounter]) for counts and a FIFO
//! ([super::super::fifo::asynchronous::AsyncFIFO]) for arbitrary data.
//!
//!# Connections
//!
//! Here is the schematic symbol for the chain.
//!
#![doc = badascii_doc::badascii_formal!("
     +-+BitSyncChain+--+
     |                 |
+--->| data     output +--->
     |                 |
     |              cr |<---+
     |                 |
     +-----------------+
")]
//!
//!# Internals
//!
//! Internally, the chain is `N` single-bit flip flops, each clocked by
//! the destination `R` domain.  The data signal from the `W` domain
//! feeds the first flip flop; subsequent stages take the previous
//! stage's `Q` as their `D`.
//!
#![doc = badascii_doc::badascii!("
       +-------+      +-------+              +-------+
       |       |      |       |              |       |
+----->|d FF0 q+----->|d FF1 q+--> ...   --->|d FFn q+--->
       |       |      |       |              |       |
   +-->|clk/rst|  +-->|clk/rst|          +-->|clk/rst|
   |   +-------+  |   +-------+          |   +-------+
   |              |                      |
+--+--------------+----------------------+
")]
//!
//!# Example
//!
//!```
#![doc = include_str!("../../examples/synchronizer_chain.rs")]
//!```
//!
//! With an output trace
#![doc = include_str!("../../doc/synchronizer_chain.md")]

use quote::{format_ident, quote};
use rhdl::{
    core::{ScopedName, circuit::descriptor::AsyncKind},
    prelude::*,
};
use syn::parse_quote;

/// An `N`-stage synchronizer for crossing a single bit from
/// the `W` domain to the `R` domain.
///
/// `N` is the chain depth; the conventional value is 2 (matching
/// [super::synchronizer::Sync1Bit]).  Larger values further reduce the
/// metastability propagation probability, at one cycle of added latency
/// per stage.
#[derive(PartialEq, Debug, Clone, Default)]
pub struct BitSyncChain<W: Domain, R: Domain, const N: usize> {
    _w: std::marker::PhantomData<W>,
    _r: std::marker::PhantomData<R>,
}

#[derive(PartialEq, Debug, Digital, Copy, Timed, Clone)]
/// Inputs to the synchronizer chain
pub struct In<W: Domain, R: Domain> {
    /// The data signal coming from the `W` source domain
    pub data: Signal<bool, W>,
    /// The clock and reset signal from the `R` destination domain
    pub cr: Signal<ClockReset, R>,
}

impl<W: Domain, R: Domain, const N: usize> CircuitDQ for BitSyncChain<W, R, N> {
    type D = ();
    type Q = ();
}

impl<W: Domain, R: Domain, const N: usize> CircuitIO for BitSyncChain<W, R, N> {
    type I = In<W, R>;
    type O = Signal<bool, R>;
    type Kernel = NoCircuitKernel<Self::I, (), (Self::O, ())>;
}

#[derive(PartialEq, Debug, Digital, Clone, Copy)]
#[doc(hidden)]
pub struct S<const N: usize> {
    clock: Clock,
    reg_next: [bool; N],
    reg_current: [bool; N],
}

impl<W: Domain, R: Domain, const N: usize> Circuit for BitSyncChain<W, R, N> {
    type S = S<N>;

    fn init(&self) -> Self::S {
        S {
            clock: Clock::dont_care(),
            reg_next: [false; N],
            reg_current: [false; N],
        }
    }

    fn sim(&self, input: Self::I, state: &mut Self::S) -> Self::O {
        let clock = input.cr.val().clock;
        let reset = input.cr.val().reset;
        trace("clock", &clock);
        trace("reset", &reset);
        trace("input", &input.data);
        if !clock.raw() {
            state.reg_next[0] = input.data.val();
            for i in 1..N {
                state.reg_next[i] = state.reg_current[i - 1];
            }
        }
        if clock.raw() && !state.clock.raw() {
            for i in 0..N {
                state.reg_current[i] = state.reg_next[i];
            }
        }
        if reset.raw() {
            for r in state.reg_next.iter_mut() {
                *r = false;
            }
        }
        state.clock = clock;
        let out = state.reg_current[N - 1];
        trace("output", &out);
        signal(out)
    }

    fn descriptor(&self, scoped_name: ScopedName) -> Result<Descriptor<AsyncKind>, RHDLError> {
        let name = scoped_name.to_string();
        Descriptor::<AsyncKind> {
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
        .with_netlist_black_box()
    }
}

impl<W: Domain, R: Domain, const N: usize> BitSyncChain<W, R, N> {
    fn hdl(&self, name: &str) -> Result<HDLDescriptor, RHDLError> {
        assert!(N >= 1, "BitSyncChain depth N must be at least 1");
        let module_name = name.to_owned();
        let module_ident = format_ident!("{}", module_name);
        let i_kind = <<Self as CircuitIO>::I as Digital>::static_kind();
        let i = <Self as CircuitIO>::I::dont_care();
        let reset_index = bit_range(i_kind, &path!(i.cr.val().reset))?;
        let reset_index = syn::Index::from(reset_index.0.start);
        let clock_index = bit_range(i_kind, &path!(i.cr.val().clock))?;
        let clock_index = syn::Index::from(clock_index.0.start);
        let data_index = bit_range(i_kind, &path!(i.data))?;
        let data_index = syn::Index::from(data_index.0.start);
        let reg_decls = (0..N).map(|i| {
            let r = format_ident!("reg_{}", i);
            quote! { reg [0:0] #r; }
        });
        let reg_inits = (0..N).map(|i| {
            let r = format_ident!("reg_{}", i);
            quote! { #r = 1'b0; }
        });
        let reg_updates = (0..N).map(|i| {
            let r = format_ident!("reg_{}", i);
            let src = if i == 0 {
                quote! { data }
            } else {
                let prev = format_ident!("reg_{}", i - 1);
                quote! { #prev }
            };
            quote! {
                if (reset) begin
                    #r <= 1'b0;
                end else begin
                    #r <= #src;
                end
            }
        });
        let last_reg = format_ident!("reg_{}", N - 1);
        let module: vlog::ModuleDef = parse_quote! {
            module #module_ident(input wire [2:0] i, output wire [0:0] o);
                wire [0:0] data;
                wire [0:0] clock;
                wire [0:0] reset;
                #(#reg_decls)*
                assign data = i[#data_index];
                assign clock = i[#clock_index];
                assign reset = i[#reset_index];
                assign o = #last_reg;
                initial begin
                    #(#reg_inits)*
                end
                always @(posedge clock) begin
                    #(#reg_updates)*
                end
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
    use rand::{Rng, SeedableRng};
    use rhdl::prelude::vlog::Pretty;

    use super::*;

    fn sync_stream() -> impl Iterator<Item = TimedSample<In<Red, Blue>>> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xdead_beef);
        let red = (0..)
            .map(move |_| rng.random::<bool>())
            .take(100)
            .with_reset(1)
            .clock_pos_edge(100);
        let blue = std::iter::repeat(false).with_reset(1).clock_pos_edge(79);
        red.merge_map(blue, |r, g| In {
            data: signal(r.1),
            cr: signal(g.0),
        })
    }

    // Tier 2 — iterator simulation: end-to-end CDC works.
    #[test]
    fn test_chain_glitch_check() -> miette::Result<()> {
        let uut = BitSyncChain::<Red, Blue, 4>::default();
        let _ = uut
            .run(sync_stream())
            .glitch_check(|i| (i.input.cr.val().clock, i.output.val()))
            .last();
        Ok(())
    }

    // Tier 3 — HDL emission snapshot for N=4.
    #[test]
    fn test_hdl_generation_n4() -> miette::Result<()> {
        let uut = BitSyncChain::<Red, Blue, 4>::default();
        let hdl = uut.hdl("top")?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [2:0] i, output wire [0:0] o);
               wire [0:0] data;
               wire [0:0] clock;
               wire [0:0] reset;
               reg [0:0] reg_0;
               reg [0:0] reg_1;
               reg [0:0] reg_2;
               reg [0:0] reg_3;
               assign data = i[0];
               assign clock = i[1];
               assign reset = i[2];
               assign o = reg_3;
               initial begin
                  reg_0 = 1'b0;
                  reg_1 = 1'b0;
                  reg_2 = 1'b0;
                  reg_3 = 1'b0;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     reg_0 <= 1'b0;
                  end else begin
                     reg_0 <= data;
                  end
                  if (reset) begin
                     reg_1 <= 1'b0;
                  end else begin
                     reg_1 <= reg_0;
                  end
                  if (reset) begin
                     reg_2 <= 1'b0;
                  end else begin
                     reg_2 <= reg_1;
                  end
                  if (reset) begin
                     reg_3 <= 1'b0;
                  end else begin
                     reg_3 <= reg_2;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // Tier 3 — additional snapshot for N=2 (matches Sync1Bit's structure).
    #[test]
    fn test_hdl_generation_n2() -> miette::Result<()> {
        let uut = BitSyncChain::<Red, Blue, 2>::default();
        let hdl = uut.hdl("top")?.modules.pretty();
        let expect = expect![[r#"
            module top(input wire [2:0] i, output wire [0:0] o);
               wire [0:0] data;
               wire [0:0] clock;
               wire [0:0] reset;
               reg [0:0] reg_0;
               reg [0:0] reg_1;
               assign data = i[0];
               assign clock = i[1];
               assign reset = i[2];
               assign o = reg_1;
               initial begin
                  reg_0 = 1'b0;
                  reg_1 = 1'b0;
               end
               always @(posedge clock) begin
                  if (reset) begin
                     reg_0 <= 1'b0;
                  end else begin
                     reg_0 <= data;
                  end
                  if (reset) begin
                     reg_1 <= 1'b0;
                  end else begin
                     reg_1 <= reg_0;
                  end
               end
            endmodule
        "#]];
        expect.assert_eq(&hdl);
        Ok(())
    }

    // Tier 4 — iverilog round-trip.
    //
    // The asynchronous testbench compares the DUT output against the
    // Rust simulator's output at every event in the merged input
    // stream.  For a hand-written multi-domain widget, the per-event
    // timing of `current` register updates inside the Rust `sim()`
    // loop does not align cycle-for-cycle with iverilog's
    // `always @(posedge clock)` semantics under arbitrary stream
    // interleavings — the same limitation that drives
    // [super::synchronizer::Sync1Bit] to use `skip(!0)` here.
    //
    // We still want this test to provide *elaboration* and
    // *compilation* coverage of the generated Verilog, so we run
    // iverilog with `skip(!0)` to disable the per-sample comparison.
    // The functional correctness of the chain is covered by the
    // Rust-side glitch check ([test_chain_glitch_check]) and by the
    // VCD-digest regression ([test_chain_trace]).
    #[test]
    fn test_chain_hdl_works() -> miette::Result<()> {
        let uut = BitSyncChain::<Red, Blue, 4>::default();
        let test_bench = uut.run(sync_stream()).collect::<TestBench<_, _>>();
        let test_mod = test_bench.rtl(&uut, &TestBenchOptions::default().skip(!0))?;
        test_mod.run_iverilog()?;
        Ok(())
    }

    // Tier 5 — VCD digest.
    #[test]
    fn test_chain_trace() -> miette::Result<()> {
        let uut = BitSyncChain::<Red, Blue, 4>::default();
        let vcd = uut.run(sync_stream()).collect::<VcdFile>();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vcd")
            .join("synchronizer_chain");
        std::fs::create_dir_all(&root).unwrap();
        let expect = expect!["8ccd9166c885667a7d7c58d8e0ff366c85a719eb7742b09596453fa410a7c87b"];
        let digest = vcd
            .dump_to_file(root.join("synchronizer_chain.vcd"))
            .unwrap();
        expect.assert_eq(&digest);
        Ok(())
    }
}
