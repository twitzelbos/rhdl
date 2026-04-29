//! Example: materialise the FSM diagram for the `#[fsm_doc]`-tagged
//! [`AutoDocMachine`] demo widget.
//!
//! Run after editing the demo kernel to refresh the on-disk markdown
//! the `#[fsm_doc]` attribute's `include_str!` reads from.

use rhdl::prelude::*;
use rhdl_fpga::doc::{demo::AutoDocMachine, write_fsm_diagram};

fn main() -> Result<(), RHDLError> {
    write_fsm_diagram::<AutoDocMachine>("AutoDocMachine_fsm.md")
        .map_err(|e| anyhow::anyhow!("write_fsm_diagram failed: {e}"))?;
    eprintln!("Refreshed crates/rhdl-fpga/doc/AutoDocMachine_fsm.md");
    Ok(())
}
