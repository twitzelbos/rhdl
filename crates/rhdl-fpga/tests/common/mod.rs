//! Shared helpers for the AXI4 integration tests.
//!
//! The Tier B Verilog monitor is driven by a generated testbench, and
//! two test binaries now need to do that: `axi4_tier_b_monitor`, which
//! checks the monitors against hand-written transcripts, and
//! `axi4_phase2_write_burst`, which checks them against a manager's own
//! traffic. The generator lives here so the two cannot drift — a
//! divergence between them would show up as one suite passing and the
//! other failing on the same bus behaviour.
//!
//! This is `tests/common/mod.rs` rather than `tests/common.rs` on
//! purpose: a file directly in `tests/` is compiled as its own test
//! binary, and this one has no tests in it.

#![allow(dead_code)]

use rhdl_fpga::axi4::check::rules::Fault;
use rhdl_fpga::axi4::check::traffic::WriteCycle;
use rhdl_fpga::axi4::types::encode_burst;

pub const A: usize = 32;
pub const DW: usize = 32;
pub const STRB: usize = 4;
pub const ID: usize = 4;

/// The fault-code contract shared with `axi4_write_monitor.v`.
///
/// Written out rather than taken from the derived `Digital`
/// discriminant, so that a reordering of the Rust enum cannot silently
/// re-point the Verilog's `\`define`s at different faults.
pub fn code_of(f: Fault) -> u32 {
    match f {
        Fault::None => 0,
        Fault::AwValidDropped => 1,
        Fault::AwPayloadChanged => 2,
        Fault::WValidDropped => 3,
        Fault::WPayloadChanged => 4,
        Fault::BValidDropped => 5,
        Fault::BPayloadChanged => 6,
        Fault::WLastEarly => 7,
        Fault::WLastMissing => 8,
        Fault::BrespUnexpected => 9,
        Fault::IncrCrosses4K => 10,
        Fault::WrapBadLength => 11,
        Fault::WrapUnaligned => 12,
        Fault::SizeExceedsBus => 13,
        Fault::StrobeOutsideLanes => 14,
        // The read-channel codes start at 15 rather than following the
        // enum's declaration order. That is the point of writing this
        // mapping out by hand: adding the read variants inserted them
        // into the MIDDLE of the shared `Fault` enum, and a mapping
        // derived from the discriminant would have silently re-pointed
        // the write monitor's existing `\`define`s at different faults.
        // Assigning above the write codes instead meant
        // `axi4_write_monitor.v` never had to change.
        Fault::ArValidDropped => 15,
        Fault::ArPayloadChanged => 16,
        Fault::RValidDropped => 17,
        Fault::RPayloadChanged => 18,
        Fault::RLastEarly => 19,
        Fault::RLastMissing => 20,
        Fault::RBeatUnexpected => 21,
        Fault::RIdMismatch => 22,
    }
}

/// Emit a self-contained Verilog testbench driving the transcript into
/// the hand-written monitor.
///
/// Signals are applied on the falling edge so that they are stable
/// across the rising edge the monitor samples on, and `rstn` is held
/// low for one sampled edge to match Tier A's `with_reset(1)`.
pub fn write_testbench(cycles: &[WriteCycle<A, DW, STRB, ID>]) -> String {
    let mut s = String::new();
    s.push_str(
        "`timescale 1ns/1ps\nmodule tb;\n\
         reg clk = 0, rstn = 0;\n\
         reg aw_valid, aw_ready; reg [3:0] aw_id; reg [31:0] aw_addr;\n\
         reg [7:0] aw_len; reg [2:0] aw_size; reg [1:0] aw_burst;\n\
         reg w_valid, w_ready; reg [31:0] w_data; reg [3:0] w_strb; reg w_last;\n\
         reg b_valid, b_ready; reg [3:0] b_id; reg [1:0] b_resp;\n\
         wire [4:0] fault; wire [15:0] faults; wire [15:0] unchecked;\n\
         axi4_write_monitor m(.clk(clk), .rstn(rstn),\n\
           .aw_valid(aw_valid), .aw_ready(aw_ready), .aw_id(aw_id),\n\
           .aw_addr(aw_addr), .aw_len(aw_len), .aw_size(aw_size),\n\
           .aw_burst(aw_burst),\n\
           .w_valid(w_valid), .w_ready(w_ready), .w_data(w_data),\n\
           .w_strb(w_strb), .w_last(w_last),\n\
           .b_valid(b_valid), .b_ready(b_ready), .b_id(b_id), .b_resp(b_resp),\n\
           .fault(fault), .faults(faults), .unchecked(unchecked));\n\
         always #5 clk = ~clk;\n\
         initial begin\n\
         aw_valid=0; aw_ready=0; aw_id=0; aw_addr=0; aw_len=0; aw_size=0; aw_burst=0;\n\
         w_valid=0; w_ready=0; w_data=0; w_strb=0; w_last=0;\n\
         b_valid=0; b_ready=0; b_id=0; b_resp=0;\n\
         rstn = 0;\n\
         @(negedge clk);\n\
         rstn = 1;\n",
    );
    for c in cycles {
        s.push_str(&format!(
            "aw_valid={}; aw_ready={}; aw_id={}; aw_addr={}; aw_len={}; aw_size={}; aw_burst={};\n\
             w_valid={}; w_ready={}; w_data={}; w_strb={}; w_last={};\n\
             b_valid={}; b_ready={}; b_id={}; b_resp={};\n\
             @(negedge clk);\n",
            c.aw_valid as u8,
            c.aw_ready as u8,
            c.aw.id.raw(),
            c.aw.addr.raw(),
            c.aw.len.raw(),
            c.aw.size.raw(),
            encode_burst(c.aw.burst).raw(),
            c.w_valid as u8,
            c.w_ready as u8,
            c.w.data.raw(),
            c.w.strb.raw(),
            c.w.last as u8,
            c.b_valid as u8,
            c.b_ready as u8,
            c.b.id.raw(),
            rhdl_fpga::axi4::types::encode_response(c.b.resp).raw(),
        ));
    }
    s.push_str(
        "@(posedge clk); #1;\n\
         $display(\"VERDICT %0d %0d %0d\", fault, faults, unchecked);\n\
         $finish;\n end\nendmodule\n",
    );
    s
}

/// Compile the hand-written Verilog monitor around a transcript and
/// return its verdict as `(fault code, tally, unchecked beats)`.
pub fn tier_b_write(
    cycles: &[WriteCycle<A, DW, STRB, ID>],
    dir: &std::path::Path,
    tag: &str,
) -> (u32, u128, u128) {
    use std::io::Write;
    std::fs::create_dir_all(dir).unwrap();
    let tb = dir.join(format!("tb_{tag}.v"));
    let exe = dir.join(format!("tb_{tag}.vvp"));
    let mut f = std::fs::File::create(&tb).unwrap();
    f.write_all(write_testbench(cycles).as_bytes()).unwrap();
    drop(f);

    let monitor = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("verilog")
        .join("axi4")
        .join("axi4_write_monitor.v");
    let out = std::process::Command::new("iverilog")
        .arg("-g2001")
        .arg("-o")
        .arg(&exe)
        .arg(&tb)
        .arg(&monitor)
        .output()
        .expect("iverilog should be runnable");
    assert!(
        out.status.success(),
        "iverilog failed for {tag}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let run = std::process::Command::new("vvp")
        .arg(&exe)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&run.stdout);
    let line = text
        .lines()
        .find(|l| l.starts_with("VERDICT"))
        .unwrap_or_else(|| panic!("no verdict from Tier B for {tag}; output was:\n{text}"));
    let n: Vec<u128> = line
        .split_whitespace()
        .skip(1)
        .map(|t| t.parse().unwrap())
        .collect();
    (n[0] as u32, n[1], n[2])
}
