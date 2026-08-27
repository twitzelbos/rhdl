//! The primitive library, and what `resolve` refuses.
//!
//! The library itself is validated by `build.rs`, which rejects a zero
//! width port, a path naming a port that does not exist, and a path
//! running the wrong way — those are build failures, so there is nothing
//! to assert here about them. What remains is the boundary between a
//! library entry and the widget wrapping it, which is `resolve`.

use rhdl::core::circuit::blackbox_decl::{ConnectivityDecl, PortDir, PortRole};
use rhdl::prelude::*;
use rhdl_bsp::primitives::xilinx;

/// A mapping naming a port the module does not have is refused.
///
/// Almost always a typo, and the cost of ignoring it silently is a lost
/// edge — which is the failure this whole mechanism exists to prevent.
#[test]
fn a_mapping_for_an_unknown_port_is_refused() {
    let err = xilinx::MUXF7
        .resolve(&[
            ("I0", Path::default().field("i0")),
            ("I1", Path::default().field("i1")),
            ("S", Path::default().field("s")),
            ("O", Path::default()),
            ("NOPE", Path::default()),
        ])
        .expect_err("a port the module does not have must be refused");
    let text = format!("{err}");
    assert!(
        text.contains("NOPE"),
        "the error should name the port: {text}"
    );
}

/// A data port with no mapping is refused.
#[test]
fn an_unmapped_data_port_is_refused() {
    let err = xilinx::MUXF7
        .resolve(&[("I0", Path::default().field("i0")), ("O", Path::default())])
        .expect_err("every data port needs a mapping");
    let text = format!("{err}");
    assert!(
        text.contains("I1") || text.contains("S"),
        "the error should name a missing port: {text}"
    );
}

/// Clock and reset ports need no mapping.
///
/// They are excluded from the analysis, so a path for them would be
/// ignored and requiring one would be ceremony. `FDRE` has both.
#[test]
fn clock_and_reset_ports_need_no_mapping() {
    let connectivity = xilinx::FDRE
        .resolve(&[
            ("CE", Path::default().field("enable")),
            ("D", Path::default().field("data")),
            ("Q", Path::default()),
        ])
        .expect("a flop's clock and reset are not mapped");
    assert_eq!(
        connectivity,
        rhdl::core::circuit::reachability::BlackBoxConnectivity::None,
        "FDRE registers what it carries"
    );
}

/// An `Opaque` entry resolves to `Opaque`, with no mapping needed for it
/// beyond its data ports.
#[test]
fn an_opaque_entry_resolves_to_opaque() {
    let connectivity = xilinx::MMCME2_ADV
        .resolve(&[
            ("CLKOUT0", Path::default().field("clk_out")),
            ("LOCKED", Path::default().field("locked")),
        ])
        .expect("resolves");
    assert_eq!(
        connectivity,
        rhdl::core::circuit::reachability::BlackBoxConnectivity::Opaque,
        "a module nobody analysed must concede everything"
    );
}

/// The library carries its provenance and its notes through to the
/// generated code.
///
/// The note is the part a machine cannot check and a reader needs: when
/// the loop detector reports a path through a module, the note about
/// which configuration the entry describes is exactly what they will not
/// look up themselves.
#[test]
fn the_library_carries_its_notes() {
    assert!(
        xilinx::MMCME2_ADV
            .note
            .is_some_and(|n| n.contains("Not analysed")),
        "an unanalysed entry should say so where a reader will see it"
    );
    assert!(
        xilinx::MUXF7.note.is_none(),
        "an entry that needs no caveat should not invent one"
    );
}

/// Every entry's ports are well formed, and the roles are used.
///
/// A weak assertion on its own; its value is that it fails if the build
/// script's emission drifts from the runtime types in a way that still
/// compiles — a role silently defaulting to `Data`, say.
#[test]
fn the_generated_entries_are_well_formed() {
    assert!(!xilinx::ALL.is_empty(), "the library is not empty");
    for decl in xilinx::ALL {
        assert!(!decl.module.is_empty());
        assert!(!decl.ports.is_empty(), "{} has no ports", decl.module);
        assert!(
            decl.ports.iter().any(|p| p.dir == PortDir::Output),
            "{} has no output",
            decl.module
        );
        for p in decl.ports {
            assert!(p.width > 0, "{}.{} has zero width", decl.module, p.name);
        }
        // Paths must run input to output, which the build script also
        // checks; asserting it here catches an emission that bypassed it.
        if let ConnectivityDecl::Paths(pairs) = decl.connectivity {
            for (from, to) in pairs {
                let src = decl.ports.iter().find(|p| p.name == *from).expect("source");
                let dst = decl.ports.iter().find(|p| p.name == *to).expect("sink");
                assert_eq!(src.dir, PortDir::Input, "{}.{from}", decl.module);
                assert_eq!(dst.dir, PortDir::Output, "{}.{to}", decl.module);
            }
        }
    }
    // `FDRE` is the entry with non-data roles; if the build script stopped
    // emitting them this is what notices.
    assert!(
        xilinx::FDRE.ports.iter().any(|p| p.role == PortRole::Clock),
        "FDRE's clock should be declared as a clock"
    );
    assert!(
        xilinx::FDRE.ports.iter().any(|p| p.role == PortRole::Reset),
        "FDRE's reset should be declared as a reset"
    );
}

/// A declaration's ports can be checked against real Verilog.
///
/// `rhdl-vlog` parses Verilog text, so a module header can be compared
/// against a declaration mechanically — which is worth having because
/// ports are what silently change between tool versions, while paths are
/// a judgement that cannot be checked this way at all.
///
/// This runs against a hand-written header rather than a vendor model,
/// and that limitation is the honest part: whether `rhdl-vlog`'s parser
/// accepts real `unisims` sources is untested and probably needs work,
/// since it implements a subset. What is demonstrated here is that the
/// comparison itself is right, so pointing it at real sources later is a
/// matter of the parser rather than of this logic.
#[test]
fn a_declaration_can_be_checked_against_parsed_verilog() {
    let source = r#"
        module MUXF7(output wire [0:0] O, input wire [0:0] I0, input wire [0:0] I1, input wire [0:0] S);
        endmodule
    "#;
    let parsed: vlog::ModuleDef = syn::parse_str(source).expect("the header parses");
    assert_eq!(parsed.name, xilinx::MUXF7.module);

    for port in xilinx::MUXF7.ports {
        let found = parsed
            .args
            .iter()
            .find(|a| a.decl.name == port.name)
            .unwrap_or_else(|| panic!("{} is declared but not in the Verilog", port.name));
        let expected = match port.dir {
            PortDir::Input => vlog::Direction::Input,
            PortDir::Output => vlog::Direction::Output,
            PortDir::Inout => vlog::Direction::Inout,
        };
        assert_eq!(found.direction, expected, "{} direction", port.name);
        assert_eq!(found.width(), port.width, "{} width", port.name);
    }
    assert_eq!(
        parsed.args.len(),
        xilinx::MUXF7.ports.len(),
        "the Verilog has ports the declaration does not"
    );
}
