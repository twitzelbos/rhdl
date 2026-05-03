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
    let inputs: Vec<ChipIn> = (0..200).map(|_| ChipIn { wakeups: bits::<16>(0x0001) }).collect();
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

    // Dump every distinct (task, mpc) pair Disk Sector visits, decoded.
    use rhdl_alto::isa::Microinstruction;
    use std::collections::BTreeMap;
    let mut seen: BTreeMap<(u128, u128), u128> = BTreeMap::new();
    for t in trace.iter() {
        if t.current_task.raw() == 4 {
            seen.insert((t.current_task.raw(), t.mpc.raw()), t.instruction.raw());
        }
    }
    println!("\n{} unique (task=4, mpc) pairs in trace:", seen.len());
    for ((task, mpc), &instr) in seen.iter() {
        let mi = Microinstruction::unpack(instr as u32);
        println!("  task={task:2} mpc=0x{mpc:03x} instr=0x{instr:08x}");
        println!(
            "    rsel={:>2} aluf={:?} bs={:?} f1={:?} f2={:?} t={} l={} next={:#05x}",
            mi.rsel.raw(),
            mi.aluf,
            mi.bs,
            mi.f1,
            mi.f2,
            mi.t_load,
            mi.l_load,
            mi.next.raw()
        );
    }
}
