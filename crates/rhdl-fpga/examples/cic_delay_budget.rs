// Regenerate the delay-budget table the book chapter is built from.
//
// The numbers are group delays, in samples, of six CIC configurations:
// two ways of reaching the same receive rate change, each implemented in
// fabric and in software, and two transmit chains. What the table is for
// is visible in one column -- `comb pipe` is the cost of the pipelining
// that makes both cascades one adder deep, and in the split
// configuration it is the largest single contribution to the loop's
// delay.
//
// Deterministic: hand-specified shapes, closed-form maths, no search and
// no RNG, so the committed table regenerates byte-identically.

use rhdl_fpga::doc::delay_budget::{PATH, budget_markdown};

fn main() -> std::io::Result<()> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(PATH);
    let text = budget_markdown();
    print!("{text}");
    std::fs::write(&root, &text)?;
    println!("\nwrote {}", root.display());
    Ok(())
}
