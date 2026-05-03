//! The Alto's 16-task wakeup arbiter as a [`rhdl_rule`] kernel,
//! using the **ContrAlto / real-Alto-II task numbering convention**.
//!
//! ## The Alto's task system, per the *Alto Hardware Manual* §4
//!
//! The Alto microengine is shared by **16 hardware tasks** taking
//! turns one cycle at a time.  Each task has:
//!
//! - its own **MPC** (microprogram counter, 10 bits)
//! - a **wakeup signal** (driven by hardware: disk, display, mouse,
//!   etc.)
//! - a fixed **priority** (Task 14 highest, Task 0 lowest)
//!
//! Each cycle the hardware looks at the 16 wakeup signals, picks
//! the highest-priority woken task, and runs one microinstruction
//! from that task's MPC.  All 16 tasks share the universal
//! microengine state — T, L, R-registers, M-channel — but each
//! has its own MPC.
//!
//! ## ContrAlto task numbering (used here)
//!
//! Real Alto II only uses **10 of the 16 task slots**.  The other 6
//! exist as wakeup vector bits but no microcode targets them.  Per
//! ContrAlto's `Tasks/` directory:
//!
//! | Task | Name              | Wakeup bit | Priority (high → low) |
//! |-----:|-------------------|-----------:|----------------------:|
//! |  14  | Disk Word         | 0x4000     |  0 (highest)          |
//! |  13  | Parity            | 0x2000     |  1                    |
//! |  12  | Display Vertical  | 0x1000     |  2                    |
//! |  11  | Display Horizontal| 0x0800     |  3                    |
//! |  10  | Cursor            | 0x0400     |  4                    |
//! |   9  | Display Word      | 0x0200     |  5                    |
//! |   8  | Memory Refresh /<br/>Reference (MRT) | 0x0100 | 6     |
//! |   7  | Ethernet          | 0x0080     |  7                    |
//! |   4  | Disk Sector       | 0x0010     |  8                    |
//! |   0  | Emulator          | 0x0001     |  9 (lowest)           |
//!
//! Tasks 1, 2, 3, 5, 6, 15 have no microcode and no rule.
//!
//! ## Why this is the canonical [`rhdl_rule`] use case
//!
//! In Bluespec System Verilog, each Alto task is naturally written
//! as a `rule` with a `when` clause.  The 10 rules are mutually
//! exclusive by construction (they share write-state); the BSV
//! scheduler picks at most one per cycle, the highest-priority
//! one whose guard is true.
//!
//! [`rhdl_rule`] expresses this directly via `#[rule_kernel_attr]`
//! on the impl block plus `#[rule(priority = N)]` annotations —
//! the same shape a BSV programmer would write.

use rhdl::prelude::*;
use rhdl_fpga::core::dff;
use rhdl_rule::rule_kernel_attr;

/// Inputs to the task system.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct AltoIn {
    /// 16-bit wakeup vector.  Bit i = 1 means task i wants to run.
    pub wakeups: Bits<16>,
    /// Per-task next-MPC values, computed by the parent (typically
    /// from the microengine's compute applied to each task's
    /// microinstruction).  The winning task's slot is committed; the
    /// others are ignored.  Slots for unused tasks (1, 2, 3, 5, 6, 15)
    /// are present in the array but never read.
    pub next_mpc_per_task: [Bits<10>; 16],
}

/// Outputs from the task system.
#[derive(PartialEq, Debug, Digital, Clone, Copy, Default)]
pub struct AltoOut {
    /// The 16 per-task MPCs (visible for debug / lockstep).
    pub task_mpc: [Bits<10>; 16],
    /// Index of the task that ran *last* cycle (committed at edge).
    pub last_task: Bits<4>,
    /// True iff at least one wakeup bit fired and a rule committed.
    pub any_running: bool,
    /// Number of times Task 4 (Disk Sector) has fired since reset.
    pub disk_sector_count: Bits<16>,
    /// Number of times Task 14 (Disk Word) has fired since reset.
    pub disk_word_count: Bits<16>,
}

/// The Alto task system: 10 priority-ordered hardware tasks
/// sharing one microengine.
///
/// Each `#[rule]` corresponds to one Alto hardware task at the
/// ContrAlto-numbered slot.  Rules are listed in **descending priority
/// order** — highest task # = highest priority.
#[derive(Clone, Debug, Default, Synchronous, SynchronousDQ)]
pub struct AltoTaskSystem {
    /// Per-task MPC (16 × 10 bits).  Defaults to all zeros at the
    /// widget level; per-task reset MPCs (= task number per Alto
    /// Hardware Manual §2 "Initialization") are applied at the
    /// `AltoChip` composition layer via the chip kernel reading
    /// `current_task` and rewriting the MPC of any task that hasn't
    /// run yet to its task number.  This keeps the widget's Verilog
    /// reset behavior aligned with the Rust simulator (DFF resets to
    /// `[0; 16]` in both, same as the synthesizable default).
    pub task_mpc: dff::DFF<[Bits<10>; 16]>,
    /// Last task that fired.
    pub last_task: dff::DFF<Bits<4>>,
    /// True iff a task fired (sticky once any task has fired —
    /// rhdl-rule auto-hold semantics).
    pub any_running: dff::DFF<bool>,
    /// Disk Sector (Task 4) fire counter.
    pub disk_sector_count: dff::DFF<Bits<16>>,
    /// Disk Word (Task 14) fire counter.
    pub disk_word_count: dff::DFF<Bits<16>>,
}

#[rule_kernel_attr]
impl AltoTaskSystem {
    // ---- Task 14 — Disk Word (highest priority) -----------------------
    //
    // Once per word transferred between disk and memory.  Highest priority
    // because per-word DMA can't be missed without losing data.  Body
    // bumps disk_word_count for observability.
    #[rule(priority = 0)]
    fn task_14_disk_word(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x4000)) != bits::<16>(0));
        let task = bits::<4>(14);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
        ctx.disk_word_count = *ctx.disk_word_count + bits::<16>(1);
    }

    // ---- Task 13 — Parity ---------------------------------------------
    #[rule(priority = 1)]
    fn task_13_parity(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x2000)) != bits::<16>(0));
        let task = bits::<4>(13);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    // ---- Task 12 — Display Vertical -----------------------------------
    #[rule(priority = 2)]
    fn task_12_display_vertical(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x1000)) != bits::<16>(0));
        let task = bits::<4>(12);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    // ---- Task 11 — Display Horizontal ---------------------------------
    #[rule(priority = 3)]
    fn task_11_display_horizontal(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x0800)) != bits::<16>(0));
        let task = bits::<4>(11);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    // ---- Task 10 — Cursor ---------------------------------------------
    #[rule(priority = 4)]
    fn task_10_cursor(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x0400)) != bits::<16>(0));
        let task = bits::<4>(10);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    // ---- Task 9 — Display Word ----------------------------------------
    #[rule(priority = 5)]
    fn task_9_display_word(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x0200)) != bits::<16>(0));
        let task = bits::<4>(9);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    // ---- Task 8 — Memory Refresh / Reference (MRT) --------------------
    //
    // The Memory Reference Task: handles both DRAM refresh and (per the
    // Alto Hardware Manual §3) memory bus operations issued by other
    // tasks via MAR← / MD→ / MD←.  In Phase 3.5 we don't yet model
    // the per-task gating of these codes, so MAR← / MD← are universal
    // (and thus accidentally available to any task that asserts them).
    #[rule(priority = 6)]
    fn task_8_mrt(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x0100)) != bits::<16>(0));
        let task = bits::<4>(8);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    // ---- Task 7 — Ethernet --------------------------------------------
    #[rule(priority = 7)]
    fn task_7_ethernet(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x0080)) != bits::<16>(0));
        let task = bits::<4>(7);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    // ---- Task 4 — Disk Sector -----------------------------------------
    //
    // Once per sector boundary (rotational tick).  Body bumps
    // disk_sector_count for observability.  Real microcode programs
    // KCOM/KADR/KCWA registers in this task body to set up the per-word
    // DMA that Task 14 then handles.
    #[rule(priority = 8)]
    fn task_4_disk_sector(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x0010)) != bits::<16>(0));
        let task = bits::<4>(4);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
        ctx.disk_sector_count = *ctx.disk_sector_count + bits::<16>(1);
    }

    // ---- Task 0 — Emulator (lowest priority — runs by default) --------
    //
    // Emulator runs whenever no higher-priority task is woken.  In real
    // Alto hardware Task 0's wakeup is hardwired-true; here we model it
    // as a regular wakeup bit that the platform leaves asserted.
    #[rule(priority = 9)]
    fn task_0_emulator(ctx: &mut RuleCtx<Self>, i: AltoIn) {
        guard!((i.wakeups & bits::<16>(0x0001)) != bits::<16>(0));
        let task = bits::<4>(0);
        let mut new_mpcs = *ctx.task_mpc;
        new_mpcs[task] = i.next_mpc_per_task[task];
        ctx.task_mpc = new_mpcs;
        ctx.last_task = task;
        ctx.any_running = true;
    }

    /// Output: visible task-system state for trace + tests.
    #[output]
    fn output(self_q: &Self, _i: AltoIn) -> AltoOut {
        AltoOut {
            task_mpc: *self_q.task_mpc,
            last_task: *self_q.last_task,
            any_running: *self_q.any_running,
            disk_sector_count: *self_q.disk_sector_count,
            disk_word_count: *self_q.disk_word_count,
        }
    }
}
