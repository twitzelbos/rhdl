use rhdl::prelude::*;
use rhdl_fpga::{core::shift_reg::ShiftRegister, doc::write_svg_as_markdown};

fn main() -> Result<(), RHDLError> {
    // Shift in the bit pattern 1,0,1,1,0,1,0,0 to demonstrate serial-in operation
    let test_pattern = vec![
        (true, true),   // Shift in 1
        (true, false),  // Shift in 0  
        (true, true),   // Shift in 1
        (true, true),   // Shift in 1
        (true, false),  // Shift in 0
        (true, true),   // Shift in 1
        (true, false),  // Shift in 0
        (true, false),  // Shift in 0
        (true, false),  // Continue shifting 0s
        (true, false),  
        (false, true),  // Disable shifting (should hold current value)
        (false, true),  // Still disabled
        (true, true),   // Re-enable and shift in 1
        (true, false),  // Shift in 0
    ];
    
    let input = test_pattern
        .with_reset(2)  // Hold reset for 2 cycles
        .clock_pos_edge(100); // 100 time unit clock period
        
    // Create an 8-bit shift register
    let uut = ShiftRegister::<U8>::default();
    
    let vcd = uut
        .run(input)?
        .take_while(|t| t.time < 1800) // Run for 1800 time units
        .collect::<Vcd>();
        
    let options = SvgOptions::default();
    write_svg_as_markdown(vcd, "shift_register.md", options)?;
    Ok(())
}