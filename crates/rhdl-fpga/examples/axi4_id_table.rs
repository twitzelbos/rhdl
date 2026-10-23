use rhdl::prelude::*;
use rhdl_fpga::axi4::id_table::{Axi4IdTable, In};
use rhdl_fpga::doc::write_svg_as_markdown;

// Four-bit IDs into a four-slot table: IDs 0x1 and 0x5 both map to
// slot 1, which is what makes a collision reachable.
type Table = Axi4IdTable<4, 2, 4>;
const _: () = Table::check_widths();

fn main() -> Result<(), RHDLError> {
    let alloc = |id: u128| In::<4> {
        alloc: Some(bits(id)),
        retire: None,
    };
    let retire = |id: u128| In::<4> {
        alloc: None,
        retire: Some(bits(id)),
    };
    let input = vec![
        alloc(1),  // slot 1 free -> Ready
        alloc(1),  // same ID again -> Ready, count 2
        alloc(5),  // 5 maps to slot 1 too -> Collision, stalls
        retire(1), // count 1
        alloc(5),  // still occupied by ID 1 -> Collision
        retire(1), // slot 1 now free
        alloc(5),  // -> Ready
        In::<4>::default(),
    ];
    let input = input.into_iter().with_reset(1).clock_pos_edge(100);
    let uut = Table::default();
    let vcd = uut.run(input).collect::<SvgFile>();
    let options = SvgOptions::default()
        .with_filter("(^top.input.*)|(^top.output.*)")
        .with_label_width(20);
    write_svg_as_markdown(vcd, "axi4_id_table.md", options)?;
    Ok(())
}
