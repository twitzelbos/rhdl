#![warn(missing_docs)]
//! Present a widget's struct-typed I/O as individually-named Verilog
//! ports.
//!
//! # The problem
//!
//! RHDL emits a widget's `I` and `O` as **single packed vectors**:
//!
//! ```verilog
//! module dut(input wire [1:0] clock_reset, input wire [7:0] i, output reg [7:0] o);
//! ```
//!
//! That is the right default — it needs no naming convention and it
//! cannot disagree with the Rust types. But it makes a widget
//! unusable from any Verilog that binds **by name**, which is every
//! vendor IP testbench, every third-party protocol checker, and any
//! hand-written monitor that would otherwise have to slice the packed
//! vector at offsets transcribed from RHDL's `Digital` layout.
//!
//! This generates the missing wrapper.
//!
//! # Why the names are supplied rather than derived
//!
//! [`PortSpec`] pairs a field [`Path`] with a port name the caller
//! chooses. It would be easy to derive `aw_valid` from `.aw.valid`
//! automatically — and wrong, because the name a standard requires is
//! not a transformation of the field name. AMBA wants `awvalid`;
//! `aresetn` is active-low where RHDL's reset is active-high; a vendor
//! core may want `S_AXI_AWVALID`. None of that falls out of a mangling
//! rule, so the caller states it.
//!
//! What the caller cannot get wrong silently is *coverage*:
//! [`wrapper_module`] computes each path's bit range and **refuses to
//! emit unless the specs tile the struct exactly** — no gaps, no
//! overlaps, no bits left over. A forgotten field or a duplicated one
//! is an error, not a wrapper that compiles and misroutes a signal.
//!
//! # Why this is not in `axi4`
//!
//! It was built for AXI4's two protocol monitors, and it is not AXI4's.
//! Vendor primitives (`xilinx-primitive-library.md`), declared black
//! boxes (`black-box-connectivity.md`) and the MAC frontend `net` is
//! waiting for all need a named boundary for the same reason. Keeping
//! it here means a second consumer is a use, not a refactor.
//!
//! # Built through the AST, not by templating
//!
//! `architecture.md` requires Verilog to be constructed through
//! `rhdl-vlog`'s AST rather than string-formatted, and a five-channel
//! AXI4 boundary is far too many slices to hand-write safely in any
//! case.
//!
//! The route is `parse_quote!` into a [`vlog::ModuleDef`], with bit
//! indices interpolated as `syn::Index` — the same mechanism
//! [`crate::cdc::synchronizer`] and [`crate::cdc::slow_crosser`]
//! already use for their hand-written Verilog. The result is parsed,
//! so a malformed wrapper fails here rather than in iverilog.

use quote::{format_ident, quote};
use rhdl::core::types::path::{Path, bit_range};
use rhdl::prelude::*;
use syn::parse_quote;

/// One named port and the field it maps to.
#[derive(Clone, Debug)]
pub struct PortSpec {
    /// The field inside the widget's `I` or `O` struct.
    pub path: Path,
    /// The Verilog port name to present it as.
    pub name: String,
}

impl PortSpec {
    /// A spec for a top-level field named `field`, presented as `name`.
    pub fn field(field: &str, name: &str) -> Self {
        Self {
            path: Path::default().field(field),
            name: name.to_string(),
        }
    }

    /// A spec for a nested field, presented as `name`.
    ///
    /// `PortSpec::nested(&["aw", "valid"], "awvalid")` is the AXI4
    /// shape: a channel struct inside the widget's `I`.
    pub fn nested(fields: &[&str], name: &str) -> Self {
        let mut path = Path::default();
        for f in fields {
            path = path.field(f);
        }
        Self {
            path,
            name: name.to_string(),
        }
    }
}

/// How a wrapper treats the clock and reset.
#[derive(Clone, Debug)]
pub struct ClockResetNaming {
    /// The clock port's name. AMBA calls it `aclk`.
    pub clock: String,
    /// The reset port's name.
    pub reset: String,
    /// Whether the reset port is active-low.
    ///
    /// AMBA's `aresetn` is, and RHDL's reset is active-high, so the
    /// wrapper inverts. This is the clearest example of the wrapper
    /// being more than slicing: a convention mismatch that would
    /// otherwise be a silent, total failure — a design held in reset
    /// for ever, or never reset at all.
    pub reset_active_low: bool,
}

impl Default for ClockResetNaming {
    fn default() -> Self {
        Self {
            clock: "aclk".to_string(),
            reset: "aresetn".to_string(),
            reset_active_low: true,
        }
    }
}

/// A port's resolved position inside the packed vector.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPort {
    /// The Verilog port name.
    pub name: String,
    /// Its bit range inside the packed `i` or `o` vector.
    pub lsb: usize,
    /// One past the top bit.
    pub msb_exclusive: usize,
}

impl ResolvedPort {
    /// The port's width in bits.
    pub fn width(&self) -> usize {
        self.msb_exclusive - self.lsb
    }
}

/// Resolve each spec against `kind`, and verify the set tiles it.
///
/// This is the check that makes a supplied naming safe: a forgotten
/// field leaves bits uncovered, a duplicated one overlaps, and either
/// would otherwise produce a wrapper that compiles and misroutes.
pub fn resolve_ports(kind: Kind, specs: &[PortSpec]) -> Result<Vec<ResolvedPort>, RHDLError> {
    let mut out = Vec::with_capacity(specs.len());
    let mut covered = vec![false; kind.bits()];
    for spec in specs {
        let (range, _k) = bit_range(kind, &spec.path)?;
        for b in range.clone() {
            if covered[b] {
                return Err(anyhow::anyhow!(
                    "port `{}` overlaps an earlier port at bit {b} of {} -- \
                     two specs name the same field, or one is a parent of \
                     another",
                    spec.name,
                    kind.bits()
                )
                .into());
            }
            covered[b] = true;
        }
        out.push(ResolvedPort {
            name: spec.name.clone(),
            lsb: range.start,
            msb_exclusive: range.end,
        });
    }
    let missing: Vec<usize> = covered
        .iter()
        .enumerate()
        .filter(|(_, c)| !**c)
        .map(|(b, _)| b)
        .collect();
    if !missing.is_empty() {
        return Err(anyhow::anyhow!(
            "the port specs leave {} of {} bits uncovered (first at bit {}) \
             -- a field was forgotten, and a wrapper emitted now would \
             leave a signal unconnected",
            missing.len(),
            kind.bits(),
            missing[0]
        )
        .into());
    }
    Ok(out)
}

/// Build a wrapper module presenting `inner`'s packed I/O as named
/// ports.
///
/// `inner` is the module name the widget was emitted under; the wrapper
/// instantiates it and nothing else. The widget's own Verilog is
/// untouched, so a design that does not need named ports pays nothing.
pub fn wrapper_module<T: Synchronous>(
    _uut: &T,
    inner: &str,
    wrapper: &str,
    inputs: &[PortSpec],
    outputs: &[PortSpec],
    naming: &ClockResetNaming,
) -> Result<vlog::ModuleDef, RHDLError> {
    let in_kind = <T::I as Digital>::static_kind();
    let out_kind = <T::O as Digital>::static_kind();
    let ins = resolve_ports(in_kind, inputs)?;
    let outs = resolve_ports(out_kind, outputs)?;

    let wrapper_ident = format_ident!("{}", wrapper);
    let inner_ident = format_ident!("{}", inner);
    let clk = format_ident!("{}", naming.clock);
    let rst = format_ident!("{}", naming.reset);

    let i_bus: vlog::BitRange = (0..in_kind.bits()).into();
    let o_bus: vlog::BitRange = (0..out_kind.bits()).into();
    let cr_bus: vlog::BitRange = (0..2).into();

    let mut ports = vec![quote! { input wire #clk }, quote! { input wire #rst }];
    let mut assigns = Vec::new();

    for p in &ins {
        let id = format_ident!("{}", p.name);
        let w: vlog::BitRange = (0..p.width()).into();
        ports.push(quote! { input wire [#w] #id });
        let lo = syn::Index::from(p.lsb);
        let hi = syn::Index::from(p.msb_exclusive - 1);
        assigns.push(quote! { assign i_packed[#hi:#lo] = #id; });
    }
    for p in &outs {
        let id = format_ident!("{}", p.name);
        let w: vlog::BitRange = (0..p.width()).into();
        ports.push(quote! { output wire [#w] #id });
        let lo = syn::Index::from(p.lsb);
        let hi = syn::Index::from(p.msb_exclusive - 1);
        assigns.push(quote! { assign #id = o_packed[#hi:#lo]; });
    }

    // RHDL packs the clock/reset pair as `{reset, clock}`. AMBA's reset
    // is active-low where RHDL's is active-high, so the inversion
    // happens here -- the one place a reader can see it. Getting it
    // wrong is a total failure, not a subtle one: a design held in
    // reset for ever, or never reset at all.
    let reset_term = if naming.reset_active_low {
        quote! { ~#rst }
    } else {
        quote! { #rst }
    };

    let module: vlog::ModuleDef = parse_quote! {
        module #wrapper_ident( #(#ports),* );
            wire [#i_bus] i_packed;
            wire [#o_bus] o_packed;
            wire [#cr_bus] cr_packed;
            assign cr_packed = {#reset_term, #clk};
            #(#assigns)*
            #inner_ident inner(.clock_reset(cr_packed), .i(i_packed), .o(o_packed));
        endmodule
    };
    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::dff;
    use rhdl::prelude::vlog::Pretty;

    /// A widget with a struct-typed boundary, so the wrapper has
    /// something to name.
    #[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
    pub struct In {
        pub valid: bool,
        pub addr: b16,
        pub len: b8,
    }

    #[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
    pub struct Out {
        pub ready: bool,
        pub echo: b16,
    }

    #[derive(Clone, Debug, Synchronous, SynchronousDQ, Default)]
    #[rhdl(dq_no_prefix)]
    pub struct Echo {
        reg: dff::DFF<b16>,
    }

    impl SynchronousIO for Echo {
        type I = In;
        type O = Out;
        type Kernel = echo_kernel;
    }

    #[kernel]
    #[doc(hidden)]
    pub fn echo_kernel(_cr: ClockReset, i: In, q: Q) -> (Out, D) {
        let mut d = D::dont_care();
        d.reg = if i.valid { i.addr } else { q.reg };
        let mut o = Out::dont_care();
        o.ready = true;
        o.echo = q.reg;
        (o, d)
    }

    fn in_specs() -> Vec<PortSpec> {
        vec![
            PortSpec::field("valid", "s_valid"),
            PortSpec::field("addr", "s_addr"),
            PortSpec::field("len", "s_len"),
        ]
    }

    fn out_specs() -> Vec<PortSpec> {
        vec![
            PortSpec::field("ready", "s_ready"),
            PortSpec::field("echo", "s_echo"),
        ]
    }

    // ------------------------------------------ the coverage check

    /// **A forgotten field is an error, not a silently unconnected
    /// wire.** This is the property that makes caller-supplied naming
    /// safe.
    #[test]
    fn a_missing_field_is_refused() {
        let kind = In::static_kind();
        let partial = vec![
            PortSpec::field("valid", "s_valid"),
            PortSpec::field("addr", "s_addr"),
            // `len` omitted.
        ];
        let err = resolve_ports(kind, &partial).expect_err("must refuse");
        let msg = format!("{err}");
        assert!(
            msg.contains("uncovered"),
            "the error should say bits were left uncovered: {msg}"
        );
    }

    /// And a duplicated field is refused too — including the subtler
    /// case of naming a parent and a child, which overlaps rather than
    /// repeats.
    #[test]
    fn an_overlapping_field_is_refused() {
        let kind = In::static_kind();
        let dup = vec![
            PortSpec::field("valid", "s_valid"),
            PortSpec::field("valid", "again"),
            PortSpec::field("addr", "s_addr"),
            PortSpec::field("len", "s_len"),
        ];
        let err = resolve_ports(kind, &dup).expect_err("must refuse");
        assert!(
            format!("{err}").contains("overlaps"),
            "the error should say the ports overlap: {err}"
        );
    }

    #[test]
    fn a_complete_set_resolves_and_tiles_the_struct() {
        let kind = In::static_kind();
        let ports = resolve_ports(kind, &in_specs()).expect("resolves");
        assert_eq!(ports.len(), 3);
        let total: usize = ports.iter().map(|p| p.width()).sum();
        assert_eq!(total, kind.bits(), "the ports must tile the struct");
        // And the widths are the field widths, not guesses.
        let by_name = |n: &str| ports.iter().find(|p| p.name == n).unwrap().width();
        assert_eq!(by_name("s_valid"), 1);
        assert_eq!(by_name("s_addr"), 16);
        assert_eq!(by_name("s_len"), 8);
    }

    // --------------------------------------- the generated wrapper

    #[test]
    fn the_wrapper_has_the_ports_it_was_asked_for() -> miette::Result<()> {
        let uut = Echo::default();
        let m = wrapper_module(
            &uut,
            "echo_inner",
            "echo_named",
            &in_specs(),
            &out_specs(),
            &ClockResetNaming::default(),
        )?;
        let text = m.pretty();
        for want in [
            "module echo_named",
            "aclk",
            "aresetn",
            "s_valid",
            "s_addr",
            "s_len",
            "s_ready",
            "s_echo",
            "echo_inner inner",
        ] {
            assert!(text.contains(want), "wrapper is missing {want:?}:\n{text}");
        }
        Ok(())
    }

    /// **The active-low reset is inverted**, which is the convention
    /// mismatch that would otherwise be a total and silent failure.
    #[test]
    fn an_active_low_reset_is_inverted_and_an_active_high_one_is_not() -> miette::Result<()> {
        let uut = Echo::default();
        let low = wrapper_module(
            &uut,
            "inner",
            "w",
            &in_specs(),
            &out_specs(),
            &ClockResetNaming::default(),
        )?
        .pretty();
        assert!(
            low.contains("~aresetn"),
            "an active-low reset must be inverted into RHDL's active-high \
             one:\n{low}"
        );
        let high = wrapper_module(
            &uut,
            "inner",
            "w",
            &in_specs(),
            &out_specs(),
            &ClockResetNaming {
                clock: "clk".into(),
                reset: "rst".into(),
                reset_active_low: false,
            },
        )?
        .pretty();
        assert!(
            !high.contains("~rst"),
            "an active-high reset must not be inverted:\n{high}"
        );
        Ok(())
    }

    /// **The decisive test: iverilog elaborates the wrapper around the
    /// real widget and the named ports behave.**
    ///
    /// Everything above checks the generated text. This checks that the
    /// text is Verilog, that the slices land on the right bits, and
    /// that the reset polarity is right — none of which text inspection
    /// can establish. It is also the mechanism vendor-IP interop needs,
    /// per `axi4-architecture.md` §7.5, so proving it here proves it
    /// there.
    #[test]
    fn iverilog_drives_the_named_ports() -> miette::Result<()> {
        let uut = Echo::default();
        let inner = uut.descriptor("echo_inner".into())?.hdl()?.modules.pretty();
        let wrapper = wrapper_module(
            &uut,
            "echo_inner",
            "echo_named",
            &in_specs(),
            &out_specs(),
            &ClockResetNaming::default(),
        )?
        .pretty();

        // A hand-written testbench that binds the wrapper *by name* --
        // which is the whole point, and which cannot be done against
        // the packed boundary.
        let tb = r#"
module tb;
   reg aclk = 0;
   reg aresetn = 0;
   reg s_valid = 0;
   reg [15:0] s_addr = 0;
   reg [7:0] s_len = 0;
   wire s_ready;
   wire [15:0] s_echo;
   integer errors = 0;

   echo_named dut(.aclk(aclk), .aresetn(aresetn), .s_valid(s_valid),
                  .s_addr(s_addr), .s_len(s_len),
                  .s_ready(s_ready), .s_echo(s_echo));

   always #5 aclk = ~aclk;

   initial begin
      // Held in reset: aresetn low means RHDL reset high.
      @(posedge aclk);
      if (s_ready !== 1'b1) begin
         $display("FAILED: s_ready should be constant high");
         errors = errors + 1;
      end
      aresetn = 1;
      @(posedge aclk);
      // Load 0xBEEF through the named address port.
      s_valid = 1; s_addr = 16'hBEEF;
      @(posedge aclk);
      s_valid = 0;
      @(posedge aclk);
      if (s_echo !== 16'hBEEF) begin
         $display("FAILED: s_echo was %h, expected beef", s_echo);
         errors = errors + 1;
      end
      // A second value, to show the slice is not accidentally right.
      s_valid = 1; s_addr = 16'h1234;
      @(posedge aclk);
      s_valid = 0;
      @(posedge aclk);
      if (s_echo !== 16'h1234) begin
         $display("FAILED: s_echo was %h, expected 1234", s_echo);
         errors = errors + 1;
      end
      if (errors == 0) $display("TESTBENCH OK");
      $finish;
   end
endmodule
"#;

        let dir = std::env::temp_dir().join("rhdl_hdl_wrap_probe");
        std::fs::create_dir_all(&dir).expect("creating the scratch directory");
        let path = dir.join("wrap.v");
        std::fs::write(&path, format!("{inner}\n{wrapper}\n{tb}")).expect("writing wrap.v");
        let exe = dir.join("wrap");
        let status = std::process::Command::new("iverilog")
            .arg("-o")
            .arg(&exe)
            .arg(&path)
            .status()
            .expect("iverilog must be installed and on PATH");
        assert!(
            status.success(),
            "the generated wrapper did not compile; the emitted file was:\n{}",
            std::fs::read_to_string(&path).unwrap_or_default()
        );
        let out = std::process::Command::new("vvp")
            .arg(&exe)
            .output()
            .expect("vvp must be installed and on PATH");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains("TESTBENCH OK"),
            "the named-port wrapper misbehaved under iverilog:\n{text}"
        );
        Ok(())
    }
}
