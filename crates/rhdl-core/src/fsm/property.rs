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

    // ===========================================================
    // Adversarial SVA-emission conformance tests
    // -----------------------------------------------------------
    // Per IEEE 1800-2017 §16.5 (Property declarations) and §5.6
    // (Identifiers), each emitted line must satisfy:
    //
    //   <verb> property (<label>_p) (@(posedge clk) [<bound>] <expr>);
    //
    // where:
    //   - <verb> ∈ {assert, cover, assume}
    //   - <label>_p starts with a letter or '_' and contains only
    //     alphanumeric / '_' / '$' (or is a backslash-escaped
    //     identifier delimited by leading `\` + trailing whitespace,
    //     which v1 does not produce)
    //   - <bound> is empty, "##[1:N] ", or "s_eventually "
    //   - <expr> is the user-supplied body, passed through verbatim
    //
    // The tests below verify the *structural shape* of every line
    // the renderer produces, not just substring presence — that's
    // the only check that catches a broken renderer that happens
    // to include the right keywords in a syntactically wrong place.
    // ===========================================================

    /// Parse a single non-pragma, non-empty rendered line into its
    /// (verb, label_with_p, bound_clause, expr_body) parts.  Returns
    /// `None` if the line doesn't match the expected SVA shape.
    /// Used by the structural-conformance tests below.
    fn parse_property_line(line: &str) -> Option<(String, String, String, String)> {
        // Form: `<verb> property (<label>_p) (@(posedge clk) <bound><expr>);`
        let line = line.trim_end_matches(';').trim();
        let (verb, rest) = line.split_once(' ')?;
        let rest = rest.strip_prefix("property ")?;
        let rest = rest.strip_prefix('(')?;
        let close_label = rest.find(')')?;
        let label_with_p = &rest[..close_label];
        let after_label = rest[close_label + 1..].trim_start();
        let after_label = after_label.strip_prefix('(')?;
        let inside = after_label.strip_suffix(')')?;
        let after_clk = inside.strip_prefix("@(posedge clk) ")?;
        // The bound clause is empty, "##[1:N] ", or "s_eventually ".
        let (bound, expr) = if let Some(rest) = after_clk.strip_prefix("s_eventually ") {
            ("s_eventually".to_string(), rest.to_string())
        } else if after_clk.starts_with("##[") {
            let close = after_clk.find("] ")?;
            (after_clk[..close + 1].to_string(), after_clk[close + 2..].to_string())
        } else {
            (String::new(), after_clk.to_string())
        };
        Some((verb.to_string(), label_with_p.to_string(), bound, expr))
    }

    /// Identifier validity per IEEE 1800-2017 §5.6 (simple form,
    /// not escaped): start with letter or '_', then alphanumeric,
    /// '_', or '$'.
    fn is_valid_sv_simple_identifier(s: &str) -> bool {
        let mut chars = s.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !first.is_ascii_alphabetic() && first != '_' {
            return false;
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    }

    /// Pragma-stripped, non-empty lines from the renderer's output —
    /// the lines that should match the property shape.
    fn property_lines(out: &str) -> Vec<String> {
        out.lines()
            .filter(|l| !l.starts_with("// pragma") && !l.is_empty() && !l.starts_with("//"))
            .map(|l| l.to_string())
            .collect()
    }

    #[test]
    fn every_emitted_line_parses_to_valid_property_form() {
        let props = &[
            FsmProperty {
                kind: FsmPropertyKind::Invariant,
                name: "no_error",
                expression: "state != State::Error",
                bound: None,
            },
            FsmProperty {
                kind: FsmPropertyKind::Cover,
                name: "reaches_done",
                expression: "state == State::Done",
                bound: None,
            },
            FsmProperty {
                kind: FsmPropertyKind::Liveness,
                name: "eventually_done",
                expression: "state == State::Done",
                bound: Some(64),
            },
            FsmProperty {
                kind: FsmPropertyKind::Liveness,
                name: "always_eventually_done",
                expression: "state == State::Done",
                bound: None,
            },
            FsmProperty {
                kind: FsmPropertyKind::Assume,
                name: "input_valid",
                expression: "input.valid",
                bound: None,
            },
        ];
        let out = render_property_sva(props);
        let lines = property_lines(&out);
        assert_eq!(lines.len(), props.len(), "line count mismatch:\n{out}");
        for (line, prop) in lines.iter().zip(props.iter()) {
            let parsed = parse_property_line(line).unwrap_or_else(|| {
                panic!("line failed to parse as SVA property:\n  line: {line}\n  prop: {prop:?}")
            });
            let (verb, label_with_p, bound, expr) = parsed;
            // §16.5 verb is exactly one of these three.
            assert!(
                matches!(verb.as_str(), "assert" | "cover" | "assume"),
                "non-SVA verb `{verb}` in line: {line}"
            );
            assert_eq!(verb, prop.kind.sva_verb(), "verb mismatch in: {line}");
            // §5.6 identifier rule + the `_p` suffix the renderer adds.
            assert!(
                label_with_p.ends_with("_p"),
                "label `{label_with_p}` must end with `_p`",
            );
            assert!(
                is_valid_sv_simple_identifier(&label_with_p),
                "label `{label_with_p}` is not a valid SystemVerilog identifier"
            );
            // Bound clause must match the kind+bound combination.
            match (prop.kind, prop.bound) {
                (FsmPropertyKind::Liveness, Some(n)) => {
                    assert_eq!(
                        bound,
                        format!("##[1:{n}]"),
                        "bounded liveness should emit `##[1:N]`, got `{bound}`"
                    );
                }
                (FsmPropertyKind::Liveness, None) => {
                    assert_eq!(
                        bound, "s_eventually",
                        "unbounded liveness should emit `s_eventually`, got `{bound}`"
                    );
                }
                _ => {
                    assert!(
                        bound.is_empty(),
                        "non-liveness property must not have a bound clause, got `{bound}`"
                    );
                }
            }
            // Expression passes through verbatim.
            assert_eq!(expr, prop.expression, "expression body altered: got `{expr}`");
        }
    }

    #[test]
    fn pragma_markers_bracket_the_property_block() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "x",
            expression: "1",
            bound: None,
        }];
        let out = render_property_sva(props);
        // §1.2 (informal) — pragmas must be on their own line; we
        // emit them as comments so any Verilog parser ignores them
        // but downstream tooling can still grep.
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.first(), Some(&"// pragma rhdl-fsm-property begin"));
        assert_eq!(lines.last(), Some(&"// pragma rhdl-fsm-property end"));
    }

    #[test]
    fn empty_property_list_emits_marker_only_with_pragmas() {
        let out = render_property_sva(&[]);
        let lines: Vec<&str> = out.lines().collect();
        // Three lines: begin pragma, marker comment, end pragma.
        assert_eq!(lines.len(), 3, "got: {out:?}");
        assert_eq!(lines[0], "// pragma rhdl-fsm-property begin");
        assert!(lines[1].starts_with("// "), "marker is a comment line");
        assert!(
            lines[1].contains("no properties declared"),
            "marker text changed: {}",
            lines[1]
        );
        assert_eq!(lines[2], "// pragma rhdl-fsm-property end");
    }

    #[test]
    fn bounded_liveness_with_bound_zero_still_parses() {
        // Edge case: bound = 0.  Per IEEE 1800-2017 §16.9.2 the
        // cycle-delay range `##[1:0]` is technically degenerate
        // (lower > upper is invalid SVA, but `##[1:0]` is parsed as
        // a zero-cycle window).  Whether this is meaningful is a
        // user question — we just verify the renderer emits the
        // `##[1:0]` form without crashing or producing garbage.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Liveness,
            name: "p",
            expression: "x",
            bound: Some(0),
        }];
        let out = render_property_sva(props);
        let lines = property_lines(&out);
        assert_eq!(lines.len(), 1);
        let (_, _, bound, _) = parse_property_line(&lines[0]).unwrap();
        assert_eq!(bound, "##[1:0]");
    }

    #[test]
    fn bounded_liveness_with_u64_max_does_not_overflow() {
        // Edge case: bound = u64::MAX.  Renderer must handle the
        // full range without panic / overflow / scientific notation.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Liveness,
            name: "p",
            expression: "x",
            bound: Some(u64::MAX),
        }];
        let out = render_property_sva(props);
        let lines = property_lines(&out);
        assert_eq!(lines.len(), 1);
        let (_, _, bound, _) = parse_property_line(&lines[0]).unwrap();
        assert_eq!(bound, format!("##[1:{}]", u64::MAX));
    }

    #[test]
    fn invariant_with_bound_ignores_bound_per_spec() {
        // The `bound` field is documented as ignored for non-liveness
        // kinds (`fsm-architecture.md` §7.1).  Verify the emitter
        // honours that — an Invariant with `bound = Some(N)` must
        // NOT emit a `##[1:N]` clause.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "p",
            expression: "x",
            bound: Some(99),
        }];
        let out = render_property_sva(props);
        let lines = property_lines(&out);
        assert_eq!(lines.len(), 1);
        let (_, _, bound, _) = parse_property_line(&lines[0]).unwrap();
        assert!(
            bound.is_empty(),
            "Invariant must drop bound clause; got `{bound}`"
        );
        assert!(
            !lines[0].contains("##["),
            "no cycle-delay range in invariant: {}",
            lines[0]
        );
    }

    #[test]
    fn cover_with_bound_ignores_bound_per_spec() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Cover,
            name: "p",
            expression: "x",
            bound: Some(99),
        }];
        let out = render_property_sva(props);
        assert!(!out.contains("##["));
        assert!(!out.contains("s_eventually"));
    }

    #[test]
    fn assume_with_bound_ignores_bound_per_spec() {
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Assume,
            name: "p",
            expression: "x",
            bound: Some(99),
        }];
        let out = render_property_sva(props);
        assert!(!out.contains("##["));
        assert!(!out.contains("s_eventually"));
    }

    #[test]
    fn property_label_with_underscore_prefix_is_valid_identifier() {
        // §5.6 allows leading underscore.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "_internal_check",
            expression: "x",
            bound: None,
        }];
        let out = render_property_sva(props);
        let (_, label, _, _) = parse_property_line(&property_lines(&out)[0]).unwrap();
        assert_eq!(label, "_internal_check_p");
        assert!(is_valid_sv_simple_identifier(&label));
    }

    #[test]
    fn property_label_with_dollar_sign_is_valid_identifier() {
        // §5.6 allows '$' as a non-leading character.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "tag$1",
            expression: "x",
            bound: None,
        }];
        let out = render_property_sva(props);
        let (_, label, _, _) = parse_property_line(&property_lines(&out)[0]).unwrap();
        assert_eq!(label, "tag$1_p");
        assert!(is_valid_sv_simple_identifier(&label));
    }

    #[test]
    fn property_label_with_digit_prefix_is_invalid_per_sv_spec() {
        // §5.6 forbids leading digits.  This test documents the
        // current renderer's lack of validation: it passes the bad
        // label through, producing invalid SystemVerilog.  The test
        // is inverted: it asserts the renderer DOES produce invalid
        // output for invalid input, so a future tightening (rejecting
        // bad labels at the macro layer) will trip this test and
        // invite the author to update both layers in lockstep.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "1bad_label",
            expression: "x",
            bound: None,
        }];
        let out = render_property_sva(props);
        let (_, label, _, _) = parse_property_line(&property_lines(&out)[0]).unwrap();
        assert_eq!(label, "1bad_label_p");
        assert!(
            !is_valid_sv_simple_identifier(&label),
            "v1 renderer does NOT validate identifiers — see fsm-architecture.md §7.1; v2 should reject this at the macro layer"
        );
    }

    #[test]
    fn empty_expression_passes_through_verbatim() {
        // Edge case the v1 renderer doesn't defend against (the
        // SystemVerilog parser will reject downstream).  Test that
        // we don't synthesize a placeholder or crash.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "p",
            expression: "",
            bound: None,
        }];
        let out = render_property_sva(props);
        let line = &property_lines(&out)[0];
        // The parser still accepts the structural shape because the
        // expression is just empty between the parens.
        let (_, _, _, expr) = parse_property_line(line).unwrap();
        assert_eq!(expr, "");
    }

    #[test]
    fn expression_with_systemverilog_keywords_passes_through() {
        // SV reserved words (`property`, `assert`, `clk`, etc.) in
        // user expressions do NOT cause the renderer to escape or
        // validate.  Documented behaviour per fsm-architecture.md
        // §7.1: v1 passes user expressions verbatim; the SV parser
        // at SymbiYosys time is the catcher for bad syntax.
        let props = &[FsmProperty {
            kind: FsmPropertyKind::Invariant,
            name: "p",
            expression: "assert == property",
            bound: None,
        }];
        let out = render_property_sva(props);
        let (_, _, _, expr) = parse_property_line(&property_lines(&out)[0]).unwrap();
        assert_eq!(expr, "assert == property");
    }

    #[test]
    fn line_count_matches_property_count_exactly() {
        // Adversarial: 100 properties.  Renderer must emit one
        // property line per input + 2 pragma lines, no extra blank
        // lines, no truncation.
        let mut props_vec = Vec::new();
        for i in 0..100 {
            // Static names are required by the FsmProperty struct;
            // synthesise leak-style strings via Box::leak.
            let name: &'static str = Box::leak(format!("p_{i}").into_boxed_str());
            let expr: &'static str = Box::leak(format!("a == {i}").into_boxed_str());
            props_vec.push(FsmProperty {
                kind: FsmPropertyKind::Invariant,
                name,
                expression: expr,
                bound: None,
            });
        }
        let out = render_property_sva(&props_vec);
        let total_lines = out.lines().count();
        assert_eq!(
            total_lines, 102,
            "expected 100 props + 2 pragmas = 102 lines, got {total_lines}"
        );
    }

    #[test]
    fn liveness_distinguishes_bound_zero_from_bound_none() {
        // Adversarial: bound=Some(0) and bound=None must produce
        // *different* SVA — the renderer must not collapse zero to
        // the unbounded form.
        let props = &[
            FsmProperty {
                kind: FsmPropertyKind::Liveness,
                name: "p_zero",
                expression: "x",
                bound: Some(0),
            },
            FsmProperty {
                kind: FsmPropertyKind::Liveness,
                name: "p_none",
                expression: "x",
                bound: None,
            },
        ];
        let out = render_property_sva(props);
        let lines = property_lines(&out);
        let (_, _, b0, _) = parse_property_line(&lines[0]).unwrap();
        let (_, _, b1, _) = parse_property_line(&lines[1]).unwrap();
        assert_ne!(b0, b1, "bound=0 and bound=None must be distinguishable");
        assert_eq!(b0, "##[1:0]");
        assert_eq!(b1, "s_eventually");
    }

    #[test]
    fn renderer_uses_the_canonical_clock_label() {
        // §16.5 requires a clocking event for each property
        // (`@(posedge clk)` is the convention; `clk` is the canonical
        // RHDL clock signal name from `circuit::synchronous`).  All
        // emitted lines must use the same clock label so SymbiYosys
        // doesn't fragment them across multiple clock domains.
        let props = &[
            FsmProperty {
                kind: FsmPropertyKind::Invariant,
                name: "a",
                expression: "x",
                bound: None,
            },
            FsmProperty {
                kind: FsmPropertyKind::Cover,
                name: "b",
                expression: "y",
                bound: None,
            },
        ];
        let out = render_property_sva(props);
        for line in property_lines(&out) {
            assert!(
                line.contains("@(posedge clk)"),
                "missing `@(posedge clk)` in: {line}"
            );
        }
    }
}
