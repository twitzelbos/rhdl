use rhdl::prelude::*;
use rhdl_fpga::{doc::write_svg_as_markdown, fifo::asynchronous::AsyncFIFO};

/// Deterministic stand-in for `rand::random`.
///
/// This example writes a committed artifact (`doc/async_fifo.md`, which
/// the widget's rustdoc includes), so it must regenerate byte-identically
/// on every machine and every run.  A seeded xorshift gives the same
/// irregular feed/drain pattern the trace is meant to illustrate without
/// the irreproducibility.
fn xorshift(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn main() -> Result<(), RHDLError> {
    let uut = AsyncFIFO::<Bits<16>, Red, Blue, 3>::default();

    let mut data_rng = 0x1234_5678_u32;
    let test_seq = (0..100)
        .map(|_| b16((xorshift(&mut data_rng) & 0xFFFF) as u128))
        .collect::<Vec<_>>();

    let mut input_seq = test_seq.iter().copied();
    let mut output_seq = test_seq.iter().copied();
    let mut write_rng = 0x9E37_79B9_u32;
    let mut read_rng = 0x85EB_CA6B_u32;

    let vcd = run_async_red_blue(
        &uut,
        |output, input| {
            // By default, we do not insert data.
            input.data = signal(None);
            if !output.full.val() && xorshift(&mut write_rng) & 1 == 1 {
                input.data = signal(input_seq.next());
            }
        },
        |output, input| {
            input.next = signal(false);
            if output.data.val().is_some() && xorshift(&mut read_rng) & 1 == 1 {
                input.next = signal(true);
                assert_eq!(output_seq.next(), output.data.val())
            }
        },
        50,
        78,
        |red, blue, input| {
            input.cr_r = blue;
            input.cr_w = red;
        },
    )
    .take_while(|t| t.time < 1500)
    .collect::<SvgFile>();
    let options = SvgOptions::default().with_io_filter();
    write_svg_as_markdown(vcd, "async_fifo.md", options)?;
    Ok(())
}
