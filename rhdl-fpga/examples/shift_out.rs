use rhdl::prelude::*;
use rhdl_fpga::core::shift_out::ShiftOut;

fn main() -> Result<(), RHDLError> {
    // Create an 8-bit shift-out register
    let shift_out: ShiftOut<U8> = ShiftOut::default();

    // Test sequence: load a byte and shift it out bit by bit
    let test_data = vec![
        (false, true, bits(0xA5)),  // Load 0xA5 = 0b10100101
        (true, false, bits(0x00)),  // Enable shifting (data_in ignored)
        (true, false, bits(0x00)),  // Continue shifting
        (true, false, bits(0x00)),  // Continue shifting
        (true, false, bits(0x00)),  // Continue shifting
        (true, false, bits(0x00)),  // Continue shifting  
        (true, false, bits(0x00)),  // Continue shifting
        (true, false, bits(0x00)),  // Continue shifting
        (true, false, bits(0x00)),  // Continue shifting (last bit)
        (false, false, bits(0x00)), // Disabled
    ];

    // Run the simulation
    let input = test_data
        .with_reset(2)
        .clock_pos_edge(100);
        
    let output_stream = shift_out.run(input)?;
    
    println!("Shift-Out Register Example");
    println!("=========================");
    println!("Loading byte 0xA5 (0b10100101) and shifting out MSB first:");
    println!();
    
    for (cycle, sample) in output_stream.enumerate() {
        let (enable, load, data_in) = sample.value.1;
        let serial_out = sample.value.2;
        
        if cycle < 2 {
            println!("Cycle {}: RESET", cycle);
        } else if load {
            println!("Cycle {}: LOAD 0x{:02X} -> serial_out = {}", 
                     cycle, data_in.val, if serial_out { '1' } else { '0' });
        } else if enable {
            println!("Cycle {}: SHIFT -> serial_out = {} (bit {})", 
                     cycle, 
                     if serial_out { '1' } else { '0' },
                     cycle - 2); // Adjust for reset cycles
        } else {
            println!("Cycle {}: HOLD -> serial_out = {}", 
                     cycle, if serial_out { '1' } else { '0' });
        }
    }
    
    println!();
    println!("Expected bit sequence (MSB first): 1 0 1 0 0 1 0 1");
    
    Ok(())
}