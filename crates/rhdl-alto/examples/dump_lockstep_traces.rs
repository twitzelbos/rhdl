//! Dump the first ~50 cycles of OUR chip's trace alongside ContrAlto's,
//! showing task, MPC, T, L, IR, BUS, and ALU result.  Used to nail down
//! where divergences come from (e.g. predecessor MPC + BUS values).
//!
//! Run with:
//!   cargo run --example dump_lockstep_traces --package rhdl-alto

use rhdl::prelude::*;
use rhdl_alto::alto_chip::{AltoChip, ChipIn};
use rhdl_alto::{disk_image_loader, microcode_loader};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let disk = manifest_dir.join("assets/disk/nonprog.dsk");
    let rom_dir = manifest_dir.join("assets/rom");
    if !disk.exists() {
        eprintln!("[skip] no disk image at {disk:?}");
        return;
    }
    if !rom_dir.join("U55").exists() {
        eprintln!("[skip] no PROM assets at {rom_dir:?}");
        return;
    }

    const CYCLES: usize = 50;

    // ---- our chip ----
    let microcode =
        microcode_loader::load_alto_ii_microcode_from_dir(&rom_dir).unwrap();
    let constants =
        microcode_loader::load_alto_ii_constant_rom_from_dir(&rom_dir).unwrap();
    let disk_image =
        disk_image_loader::load_disk_image_from_file(&disk).unwrap();
    let boot_sector = disk_image.sector(0, 0, 0);
    let uut = AltoChip::with_microcode_constants_and_boot(
        &microcode, &constants, &boot_sector.data, &boot_sector.label,
    );
    let inputs: Vec<ChipIn> = (0..CYCLES)
        .map(|_| ChipIn { wakeups: bits::<16>(0x0001) })
        .collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let trace: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    println!("=== OURS ({} cycles) ===", trace.len());
    println!("idx  task  mpc    next   T     L     IR    BUS   ALU");
    for (i, t) in trace.iter().enumerate().take(30) {
        println!(
            "{i:3}   {:>2}   0x{:03x}  0x{:03x}  {:04x}  {:04x}  {:04x}  {:04x}  {:04x}",
            t.current_task.raw(), t.mpc.raw(), t.next_mpc.raw(),
            t.t.raw(), t.l.raw(), t.ir.raw(),
            t.bus.raw(), t.alu_result.raw(),
        );
    }

    // ---- ContrAlto ----
    let exe = workspace_root.join(
        "crates/rhdl-alto/tools/contralto-trace/bin/Release/net8.0/contralto-trace",
    );
    if exe.exists() {
        let output = Command::new(&exe)
            .args([
                "--disk", disk.to_str().unwrap(),
                "--rom-dir", rom_dir.to_str().unwrap(),
                "--cycles", &CYCLES.to_string(),
            ])
            .current_dir(workspace_root)
            .output()
            .expect("spawn contralto-trace");
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("\n=== ContrAlto first 30 cycles (raw TSV) ===");
        for (i, line) in stdout.lines().take(31).enumerate() {
            // Slice only the first ~7 fields (task, mpc, t, l, ir, aluc0)
            // for readability.
            let parts: Vec<&str> = line.split('\t').collect();
            if i == 0 {
                println!("idx  {}",
                    parts.iter().take(7).cloned().collect::<Vec<_>>().join("\t"));
            } else {
                println!("{:3}  {}", i - 1,
                    parts.iter().take(7).cloned().collect::<Vec<_>>().join("\t"));
            }
        }
    } else {
        println!("\n[contralto-trace not built — skipping CTR side]");
    }
}
