//! Dump the first ~50 cycles of the boot trace, showing current_task,
//! mpc, instr, task_yield, sector_mark per cycle.  Helps debug the
//! per-cycle dispatch and pipeline timing.

use rhdl::prelude::*;
use rhdl_alto::alto_chip::{AltoChip, ChipIn};
use rhdl_alto::microcode_loader;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("crates/rhdl-alto/assets/rom");
    if !dir.join("U55").exists() {
        eprintln!("[skip] PROM assets missing under {dir:?}");
        return;
    }
    let microcode = microcode_loader::load_alto_ii_microcode_from_dir(&dir).unwrap();
    let constants = microcode_loader::load_alto_ii_constant_rom_from_dir(&dir).unwrap();
    let uut = AltoChip::with_microcode_and_constants(&microcode, &constants);
    let inputs: Vec<ChipIn> = (0..2000).map(|_| ChipIn { wakeups: bits::<16>(0x0001) }).collect();
    let stream = inputs.into_iter().with_reset(2).clock_pos_edge(100);
    let trace: Vec<_> = uut
        .run(stream)
        .synchronous_sample()
        .filter(|s| !s.input.0.reset.any())
        .map(|s| s.output)
        .collect();

    // Count cycles per current_task value.
    let mut counts = [0u32; 16];
    for t in &trace {
        let c = t.current_task.raw() as usize;
        if c < 16 {
            counts[c] += 1;
        }
    }
    let sm_count = trace.iter().filter(|t| t.disk_sector_mark).count();
    println!("Total cycles: {}, sector_marks: {sm_count}", trace.len());
    for k in 0..16 {
        if counts[k] > 0 {
            println!("  task {k:2}: {} cycles", counts[k]);
        }
    }

    // Dump first 12 cycles where current_task = 4 (Disk Sector).
    let mut shown = 0;
    println!("\nFirst Disk Sector (task=4) cycles:");
    for (i, t) in trace.iter().enumerate() {
        if t.current_task.raw() == 4 {
            let lo = i.saturating_sub(1);
            let hi = (i + 4).min(trace.len());
            for j in lo..hi {
                let s = &trace[j];
                let marker = if j == i { " *" } else { "  " };
                println!(
                    "{marker} cycle={j:4} task={} mpc=0x{:03x} instr=0x{:08x} sm={} ws={} wake=0x{:04x} ir=0x{:04x}",
                    s.current_task.raw(),
                    s.mpc.raw(),
                    s.instruction.raw(),
                    s.disk_sector_mark as u8,
                    s.disk_word_strobe as u8,
                    s.wakeups.raw(),
                    s.ir.raw(),
                );
            }
            println!("---");
            shown += 1;
            if shown >= 5 {
                break;
            }
        }
    }
}
