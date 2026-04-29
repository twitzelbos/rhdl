//! SVA-property metadata for FSM-tagged kernels.
//!
//! Layer 4 of `fsm-architecture.md`.  The
//! `#[fsm_properties(...)]` attribute macro records the user's
//! invariants, liveness properties, coverage points, and
//! environment assumptions next to the kernel function as a
//! `&'static [FsmProperty]`.  The Verilog-emission helper in
//! [`render_property_sva`] turns them into the corresponding
//! `assert`/`cover`/`assume property` statements.
//!
//! ## Sublanguage
//!
//! Per `fsm-architecture.md` §7.1, the expression body of each
//! property is a strict subset of the kernel-accepted expression
//! language: equality, comparison, Boolean ops, field access,
//! `matches!`, no calls.  This is for two reasons:
//!
//! 1. Keeps the SVA emission a one-pass mechanical transform —
//!    no recursive lowering of arbitrary kernel expressions.
//! 2. Keeps the property formula tractable when Layer 5 (the
//!    in-house BMC) lands and starts symbolically executing the
//!    property over the kernel's transition function.
//!
//! For v1 we do *not* parse the expression ourselves — we pass
//! the user's string through verbatim into the SVA emission, and
//! let the SystemVerilog parser at the SymbiYosys layer reject
//! unsupported constructs.  v2 will add a small AST + grammar
//! check at compile time so the user gets RHDL-style error
//! messages instead of Yosys-style ones.

/// What kind of SVA property a single declaration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsmPropertyKind {
    /// `assert property` — must hold on every cycle.
    Invariant,
    /// `assert property eventually` — must hold at *some* point.
    /// The `bound` field of [`FsmProperty`] carries an optional
    /// cycle bound for bounded liveness.
    Liveness,
    /// `cover property` — does the design ever reach this state?
    Cover,
    /// `assume property` — environment assumption the proof relies on.
    Assume,
}

impl FsmPropertyKind {
    /// The SVA verb that introduces this property kind.
    pub fn sva_verb(self) -> &'static str {
        match self {
            FsmPropertyKind::Invariant => "assert",
            FsmPropertyKind::Liveness => "assert",
            FsmPropertyKind::Cover => "cover",
            FsmPropertyKind::Assume => "assume",
        }
    }
}

/// A single SVA-property declaration emitted by the
/// `#[fsm_properties(...)]` macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsmProperty {
    /// What kind of property this is.
    pub kind: FsmPropertyKind,
    /// The property's name — used as the SVA `property` label and
    /// in diagnostic output.  Defaults to a generated identifier
    /// like `inv_3`; the user can override via the `name = "..."`
    /// argument inside the property declaration.
    pub name: &'static str,
    /// The property's body — the Boolean expression in SVA-friendly
    /// form.  Passed through verbatim to the Verilog emitter for v1.
    pub expression: &'static str,
    /// Optional bound for liveness properties.  `Some(N)` lowers
    /// to `##[1:N]`; `None` lowers to the unbounded form.  Ignored
    /// for non-liveness kinds.
    pub bound: Option<u64>,
}

/// Trait implemented by the `#[fsm_properties(...)]` macro on
/// kernel functions, exposing the static property slice for the
/// Verilog emission helpers and any future formal-verification
/// driver.
///
/// The trait is generic over the kernel's name (encoded as a
/// zero-sized marker type) rather than the kernel function type
/// directly, because Rust functions can't be generic parameters
/// to traits without the unstable `fn_traits` feature.  The
/// macro emits both the marker type and the impl.
pub trait FsmKernelProperties {
    fn fsm_properties() -> &'static [FsmProperty];
}

/// Render a property slice as a Verilog `// SVA property` block.
///
/// Returns a single string suitable for splicing into the body
/// of a generated `module`.  Each property becomes one labelled
/// `assert/cover/assume property (@(posedge clock) ...)` line.
///
/// The output is wrapped in a `// pragma rhdl-fsm-property begin`
/// / `// pragma rhdl-fsm-property end` pair so downstream tools
/// can splice it out (or in) without re-parsing the SVA body.
pub fn render_property_sva(props: &[FsmProperty]) -> String {
    let mut s = String::new();
    s.push_str("// pragma rhdl-fsm-property begin\n");
    if props.is_empty() {
        s.push_str("// (no properties declared)\n");
    } else {
        for p in props {
            let bound_clause = match (p.kind, p.bound) {
                (FsmPropertyKind::Liveness, Some(n)) => format!("##[1:{n}] "),
                (FsmPropertyKind::Liveness, None) => "s_eventually ".to_string(),
                _ => String::new(),
            };
            s.push_str(&format!(
                "{verb} property ({label}_p) (@(posedge clk) {bound}{expr});\n",
                verb = p.kind.sva_verb(),
                label = p.name,
                bound = bound_clause,
                expr = p.expression,
            ));
        }
    }
    s.push_str("// pragma rhdl-fsm-property end\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invariant_renders_to_assert_property() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "no_error",
            expression: "state != State::Error",
            bound: None,
        }];
        let out = render_property_sva(props);
        assert!(out.contains("assert property (no_error_p)"));
        assert!(out.contains("@(posedge clk) state != State::Error"));
        assert!(out.starts_with("// pragma rhdl-fsm-property begin"));
        assert!(out.trim_end().ends_with("// pragma rhdl-fsm-property end"));
    }

    #[test]
    fn cover_renders_to_cover_property() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Cover,
            name: "reaches_done",
            expression: "state == State::Done",
            bound: None,
        }];
        let out = render_property_sva(props);
        assert!(out.contains("cover property (reaches_done_p)"));
    }

    #[test]
    fn bounded_liveness_emits_cycle_window() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Liveness,
            name: "eventually_done",
            expression: "state == State::Done",
            bound: Some(64),
        }];
        let out = render_property_sva(props);
        assert!(
            out.contains("##[1:64]"),
            "expected `##[1:64]` in: {out}"
        );
    }

    #[test]
    fn unbounded_liveness_emits_s_eventually() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Liveness,
            name: "always_eventually_done",
            expression: "state == State::Done",
            bound: None,
        }];
        let out = render_property_sva(props);
        assert!(
            out.contains("s_eventually"),
            "expected `s_eventually` in: {out}"
        );
    }

    #[test]
    fn assume_renders_to_assume_property() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Assume,
            name: "input_valid_holds",
            expression: "input.valid",
            bound: None,
        }];
        let out = render_property_sva(props);
        assert!(out.contains("assume property (input_valid_holds_p)"));
    }

    #[test]
    fn empty_property_list_emits_marker_only() {
        let out = render_property_sva(&[]);
        assert!(out.contains("(no properties declared)"));
    }

    #[test]
    fn multiple_properties_render_in_order() {
        let props = &[
            FsmProperty {
                kind: FsmPropertyKind::Invariant,
                name: "p1",
                expression: "state != State::Error",
                bound: None,
            },
            FsmProperty {
                kind: FsmPropertyKind::Cover,
                name: "p2",
                expression: "state == State::Done",
                bound: None,
            },
            FsmProperty {
                kind: FsmPropertyKind::Assume,
                name: "p3",
                expression: "input.valid",
                bound: None,
            },
        ];
        let out = render_property_sva(props);
        let assert_idx = out.find("assert property").unwrap();
        let cover_idx = out.find("cover property").unwrap();
        let assume_idx = out.find("assume property").unwrap();
        assert!(assert_idx < cover_idx);
        assert!(cover_idx < assume_idx);
    }
}
