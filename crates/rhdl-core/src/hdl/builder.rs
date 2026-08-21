use crate::{
    Kind, RHDLError, TypedBits,
    ast::SourceLocation,
    bitx::BitX,
    compiler::mir::error::{ICE, RHDLCompileError},
    error::rhdl_error,
    rtl::{
        self,
        object::LocatedOpCode,
        spec::{AluUnary, CaseArgument, CastKind, Operand},
    },
};
use quote::{format_ident, quote};
use rhdl_vlog::{self as vlog};
use rtl::spec as tl;
use syn::parse_quote;

type Result<T> = std::result::Result<T, RHDLError>;

struct TranslationContext<'a> {
    func: vlog::FunctionDef,
    body: Vec<vlog::Stmt>,
    rtl: &'a rtl::Object,
}

impl TryFrom<&TypedBits> for vlog::LitVerilog {
    type Error = RHDLError;

    /// Fails for a zero-width value.
    ///
    /// Verilog has no zero-width literal: the natural rendering is
    /// `0'b`, which carries no digits and is a syntax error. Emitting it
    /// anyway produced malformed modules from `DFF<()>` and
    /// `Constant<()>`.
    ///
    /// Rejecting rather than substituting is deliberate. The only legal
    /// alternative would be a one-bit literal, and that makes the
    /// literal's width disagree with the value's, which is the same lie
    /// the `saturating_sub` width coercions tell elsewhere.
    fn try_from(tb: &TypedBits) -> std::result::Result<Self, Self::Error> {
        if tb.is_empty() {
            return Err(RHDLError::ZeroWidthVerilogLiteral);
        }
        // Signed values emit the `s` base specifier, so the constant
        // carries its own signedness the way Verilog intends.
        //
        // Without it a `SignedBits<N>` literal becomes an unsigned
        // constant, and IEEE 1364 5.5.1 makes a relational expression
        // unsigned if *either* operand is unsigned -- so every negative
        // value compares greater than a positive bound while the Rust
        // simulator, which never sees Verilog, reports the right answer.
        //
        // The same rule bounds the blast radius of emitting it: mixing a
        // signed literal with an unsigned register still yields an
        // unsigned expression, so this can only promote signed-vs-signed
        // to a signed comparison.  It cannot change one that is unsigned
        // today and correct.
        let base = if tb.kind().is_signed() { "sb" } else { "b" };
        let bits = base
            .chars()
            .chain(tb.iter().rev().map(|b| match b {
                BitX::Zero => '0',
                BitX::One => '1',
                BitX::X => 'X',
            }))
            .collect::<String>();
        Ok(vlog::lit_verilog(tb.len() as u32, &bits))
    }
}

/// The Verilog literal to drive onto a signal carrying `tb`, where a
/// zero-width value becomes a one-bit zero.
///
/// # Why this exists
///
/// Verilog has no zero-width signal *or* literal, so RHDL's emitter
/// already declares a zero-bit type as a one-bit port (`[0:0]`, via the
/// `saturating_sub` in `Kind -> SignedWidth`). The literal has to agree
/// with that declaration, and until now it did not: the declaration
/// said one bit and [`TryFrom<&TypedBits>`] produced `0'b`, which has
/// no digits and is a syntax error. `DFF<()>` emitted `o = 0'b;`.
///
/// # Why a placeholder rather than an error
///
/// A zero-width sub-circuit is a **deliberate idiom** in this tree, not
/// a mistake to reject. `rcstream::util::split` and `combine` carry
/// their framing type through a `Constant<F>` field precisely because
/// `PhantomData` has no HDL and would fail at `descriptor()`. At
/// `F = ()` that is a `Constant<()>`, and it has to keep working — an
/// unframed `RCStream` is a documented, load-bearing case.
///
/// A zero-width type has exactly one inhabitant, so the placeholder
/// cannot lose information: there is only one value it could be. The
/// module is in any case dropped from the composed design, since a
/// port with no bits has nothing for a parent to connect.
///
/// Callers that must **not** paper over a zero-width value should use
/// the [`TryFrom`] conversion, which still rejects it.
pub fn signal_literal(tb: &TypedBits) -> vlog::LitVerilog {
    if tb.is_empty() {
        // One bit, matching the port width the emitter declares for a
        // zero-bit type.
        return vlog::lit_verilog(1, "b0");
    }
    // Cannot fail: the emptiness case is handled above.
    match vlog::LitVerilog::try_from(tb) {
        Ok(lit) => lit,
        Err(_) => vlog::lit_verilog(1, "b0"),
    }
}

impl TryFrom<TypedBits> for vlog::LitVerilog {
    type Error = RHDLError;

    fn try_from(tb: TypedBits) -> std::result::Result<Self, Self::Error> {
        (&tb).try_into()
    }
}

impl From<Kind> for vlog::SignedWidth {
    fn from(kind: Kind) -> Self {
        let len = kind.bits();
        let bit_range = vlog::BitRange {
            start: 0,
            end: len.saturating_sub(1) as u32,
        };
        let width_spec = vlog::WidthSpec { bit_range };
        if kind.is_signed() {
            vlog::SignedWidth::Signed(width_spec)
        } else {
            vlog::SignedWidth::Unsigned(width_spec)
        }
    }
}

impl TranslationContext<'_> {
    fn raise_ice(&self, cause: ICE, id: SourceLocation) -> RHDLError {
        rhdl_error(RHDLCompileError {
            cause,
            src: self.rtl.symbols.source(),
            err_span: self.rtl.symbols.span(id).into(),
        })
    }
    fn add_stmt(&mut self, stmt: vlog::Stmt) {
        self.body.push(stmt);
    }
    fn op_ident(&self, op: Operand) -> syn::Ident {
        format_ident!("{}", self.rtl.op_name(op))
    }
    /// Cast the argument ot the desired width, considering the result a signed value.
    /// The cast length must be less than or equal to the argument length, or an ICE is raised.
    fn translate_as_signed(&mut self, cast: &tl::Cast, id: SourceLocation) -> Result<()> {
        if cast.len > self.rtl.kind(cast.arg).bits() {
            return Err(self.raise_ice(
                ICE::InvalidSignedCast {
                    lhs: cast.lhs,
                    arg: cast.arg,
                    len: cast.len,
                },
                id,
            ));
        }
        let lhs = self.op_ident(cast.lhs);
        let arg = self.op_ident(cast.arg);
        let msb = syn::Index::from(cast.len - 1);
        self.add_stmt(parse_quote! { #lhs[#msb:0] = $signed(#arg) });
        Ok(())
    }
    /// Cast the argument to the desired width, with no error and no sign extension
    fn translate_resize_unsigned(&mut self, cast: &tl::Cast) {
        let arg_kind = self.rtl.kind(cast.arg);
        let lhs = self.op_ident(cast.lhs);
        let arg = self.op_ident(cast.arg);
        let msb = syn::Index::from(cast.len - 1);
        // Truncation case
        if cast.len <= arg_kind.bits() {
            self.add_stmt(parse_quote! { #lhs = #arg[#msb:0] });
        } else {
            // zero extend
            let num_z = syn::Index::from(cast.len - arg_kind.bits());
            let prefix: vlog::Expr = parse_quote!( { #num_z { 1'b0 } }  );
            self.add_stmt(parse_quote! { #lhs = { #prefix, #arg } });
        }
    }
    /// Cast the argument to the desired width, with sign extension if needed.
    fn translate_resize_signed(&mut self, cast: &tl::Cast) {
        let arg_kind = self.rtl.kind(cast.arg);
        let lhs = self.op_ident(cast.lhs);
        let arg = self.op_ident(cast.arg);
        // Truncation case
        if cast.len <= arg_kind.bits() {
            // lhs = $signed(arg[cast.len:0])
            let msb = syn::Index::from(cast.len - 1);
            self.add_stmt(parse_quote! { #lhs = $signed(#arg[#msb:0]) });
        } else {
            // sign extend
            let num_s = syn::Index::from(cast.len - arg_kind.bits());
            let sign_bit = syn::Index::from(arg_kind.bits().saturating_sub(1));
            // #lhs = $signed({ {#num_s{arg[#sign_bit]}}, #arg })
            self.add_stmt(parse_quote! { #lhs = $signed({ {#num_s{#arg[#sign_bit]}}, #arg }) });
        }
    }
    fn translate_resize(&mut self, cast: &tl::Cast, id: SourceLocation) -> Result<()> {
        if cast.len == 0 {
            return Err(self.raise_ice(
                ICE::InvalidResize {
                    lhs: cast.lhs,
                    arg: cast.arg,
                    len: cast.len,
                },
                id,
            ));
        }
        if self.rtl.kind(cast.arg).is_signed() {
            self.translate_resize_signed(cast);
        } else {
            self.translate_resize_unsigned(cast);
        }
        Ok(())
    }
    fn translate_cast(&mut self, cast: &tl::Cast, id: SourceLocation) -> Result<()> {
        match cast.kind {
            CastKind::Signed => self.translate_as_signed(cast, id),
            CastKind::Unsigned => {
                self.translate_resize_unsigned(cast);
                Ok(())
            }
            CastKind::Resize => self.translate_resize(cast, id),
        }
    }
    fn translate_assign(&mut self, assign: &tl::Assign) -> Result<()> {
        // #lhs = #rhs
        let lhs = self.op_ident(assign.lhs);
        let rhs = self.op_ident(assign.rhs);
        self.add_stmt(parse_quote! { #lhs = #rhs });
        Ok(())
    }
    fn translate_binary(&mut self, binary: &tl::Binary) -> Result<()> {
        let lhs = self.op_ident(binary.lhs);
        let arg1 = self.op_ident(binary.arg1);
        let arg2 = self.op_ident(binary.arg2);
        let op = match binary.op {
            tl::AluBinary::Add => vlog::kw_ops::BinaryOp::Plus,
            tl::AluBinary::Sub => vlog::kw_ops::BinaryOp::Minus,
            tl::AluBinary::Mul => vlog::kw_ops::BinaryOp::Mul,
            tl::AluBinary::BitXor => vlog::kw_ops::BinaryOp::Xor,
            tl::AluBinary::BitAnd => vlog::kw_ops::BinaryOp::And,
            tl::AluBinary::BitOr => vlog::kw_ops::BinaryOp::Or,
            tl::AluBinary::Shl => vlog::kw_ops::BinaryOp::Shl,
            tl::AluBinary::Shr => vlog::kw_ops::BinaryOp::SignedRightShift,
            tl::AluBinary::Eq => vlog::kw_ops::BinaryOp::Eq,
            tl::AluBinary::Lt => vlog::kw_ops::BinaryOp::Lt,
            tl::AluBinary::Le => vlog::kw_ops::BinaryOp::Le,
            tl::AluBinary::Ne => vlog::kw_ops::BinaryOp::Ne,
            tl::AluBinary::Ge => vlog::kw_ops::BinaryOp::Ge,
            tl::AluBinary::Gt => vlog::kw_ops::BinaryOp::Gt,
        };
        log::trace!("Translating binary op: {lhs} = {arg1} {op:?} {arg2}");
        self.add_stmt(parse_quote! { #lhs = #arg1 #op #arg2 });
        Ok(())
    }
    fn translate_case(&mut self, case: &tl::Case) -> Result<()> {
        let discriminant = self.op_ident(case.discriminant);
        let lhs = self.op_ident(case.lhs);
        // Collected into a `Result` rather than left lazy so a failed
        // literal conversion propagates out of `translate_case`.
        let table = case
            .table
            .iter()
            .map(|(arg, value)| -> Result<proc_macro2::TokenStream> {
                let value = self.op_ident(*value);
                Ok(match arg {
                    CaseArgument::Literal(lit) => {
                        let lit: vlog::LitVerilog = (&self.rtl.symtab[lit]).try_into()?;
                        quote! { #lit : #lhs = #value }
                    }
                    CaseArgument::Wild => quote! { default : #lhs = #value },
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.add_stmt(parse_quote! {
            case (#discriminant)
                #(#table;)*
            endcase
        });
        Ok(())
    }
    fn translate_concat(&mut self, concat: &tl::Concat) -> Result<()> {
        let args = concat.args.iter().rev().map(|arg| self.op_ident(*arg));
        let lhs = self.op_ident(concat.lhs);
        self.add_stmt(parse_quote! { #lhs = { #(#args),* } });
        Ok(())
    }
    fn translate_index(&mut self, index: &tl::Index) -> Result<()> {
        let lhs = self.op_ident(index.lhs);
        let arg = self.op_ident(index.arg);
        let range: vlog::BitRange = (&index.bit_range).into();
        self.add_stmt(parse_quote! { #lhs = #arg[#range] });
        Ok(())
    }
    fn translate_select(&mut self, select: &tl::Select) -> Result<()> {
        let lhs = self.op_ident(select.lhs);
        let cond = self.op_ident(select.cond);
        let true_value = self.op_ident(select.true_value);
        let false_value = self.op_ident(select.false_value);
        self.add_stmt(parse_quote! {
            #lhs = #cond ? #true_value : #false_value
        });
        Ok(())
    }
    fn translate_splice(&mut self, splice: &tl::Splice) -> Result<()> {
        let lhs = self.op_ident(splice.lhs);
        let orig = self.op_ident(splice.orig);
        let value = self.op_ident(splice.value);
        let range: vlog::BitRange = (&splice.bit_range).into();
        self.add_stmt(parse_quote! { #lhs = #orig });
        self.add_stmt(parse_quote! { #lhs[#range] = #value });
        Ok(())
    }
    fn translate_unary(&mut self, unary: &tl::Unary) -> Result<()> {
        let lhs = self.op_ident(unary.lhs);
        let arg1 = self.op_ident(unary.arg1);
        self.add_stmt(match unary.op {
            AluUnary::Neg => parse_quote! {#lhs = -#arg1},
            AluUnary::Not => parse_quote! {#lhs = ~#arg1},
            AluUnary::All => parse_quote! {#lhs = &#arg1},
            AluUnary::Any => parse_quote! {#lhs = |#arg1},
            AluUnary::Xor => parse_quote! {#lhs = ^#arg1},
            AluUnary::Signed => parse_quote! {#lhs = $signed(#arg1)},
            AluUnary::Unsigned => parse_quote! {#lhs = $unsigned(#arg1)},
            AluUnary::Val => parse_quote! {#lhs = #arg1},
        });
        Ok(())
    }
    fn translate_op(&mut self, lop: &LocatedOpCode) -> Result<()> {
        let op = &lop.op;
        match op {
            tl::OpCode::Noop => Ok(()),
            tl::OpCode::Assign(assign) => self.translate_assign(assign),
            tl::OpCode::Binary(binary) => self.translate_binary(binary),
            tl::OpCode::Case(case) => self.translate_case(case),
            tl::OpCode::Cast(cast) => self.translate_cast(cast, lop.loc),
            tl::OpCode::Concat(concat) => self.translate_concat(concat),
            tl::OpCode::Index(index) => self.translate_index(index),
            tl::OpCode::Select(select) => self.translate_select(select),
            tl::OpCode::Splice(splice) => self.translate_splice(splice),
            tl::OpCode::Unary(unary) => self.translate_unary(unary),
        }
    }
    fn translate_block(&mut self, block: &[LocatedOpCode]) -> Result<()> {
        block.iter().try_for_each(|lop| self.translate_op(lop))
    }
    fn translate_kernel_for_object(mut self) -> Result<vlog::FunctionDef> {
        // Declare the input ports for the function
        // These are of the form input reg <width> arg_<ndx>
        // width = self.rtl.symtab[&reg].into()
        self.func.args = self
            .rtl
            .arguments
            .iter()
            .enumerate()
            .flat_map(|(ndx, x)| x.map(|r| (ndx, r)))
            .map(|(ndx, reg)| {
                let width: vlog::SignedWidth = self.rtl.symtab[&reg].into();
                let name = format_ident!("arg_{ndx}");
                parse_quote! {input reg #width #name}
            })
            .collect();
        // Declare the registers for the function
        // These are of the form reg <width> <name>
        let reg_decls = self
            .rtl
            .symtab
            .iter_reg()
            .map(|(rid, (kind, _))| {
                let alias = self.rtl.op_alias(Operand::Register(rid));
                let name = self.op_ident(Operand::Register(rid));
                let width: vlog::SignedWidth = (*kind).into();
                let alias = if let Some(alias) = alias {
                    quote! { #[doc = #alias] }
                } else {
                    quote! {}
                };
                parse_quote! {
                    #alias
                    reg #width #name;
                }
            })
            .collect::<Vec<vlog::Item>>();
        // Declare the literals for the function
        // These are of the form localparam <name> = <value>
        let lit_decls = self
            .rtl
            .symtab
            .iter_lit()
            .map(|(lit, (tb, _))| -> Result<vlog::Item> {
                let name = self.op_ident(Operand::Literal(lit));
                let value: vlog::LitVerilog = tb.try_into()?;
                Ok(parse_quote! {
                    localparam #name = #value;
                })
            })
            .collect::<Result<Vec<vlog::Item>>>()?;
        // Add these to the preamble
        self.func.items.extend(reg_decls);
        self.func.items.extend(lit_decls);
        // Bind the argument registers
        for (ndx, reg) in self.rtl.arguments.iter().enumerate() {
            if let Some(reg) = reg {
                let reg_name = self.op_ident(Operand::Register(*reg));
                let arg_name = format_ident!("arg_{ndx}");
                self.add_stmt(parse_quote! { #reg_name = #arg_name });
            }
        }
        self.translate_block(&self.rtl.ops)?;
        let ret_reg = self.op_ident(self.rtl.return_register);
        let ret_name = format_ident!("{}", self.func.name);
        self.add_stmt(parse_quote! {
            #ret_name = #ret_reg
        });
        let block = vlog::block_stmt(self.body);
        self.func.items.push(parse_quote! { #block });
        Ok(self.func)
    }
}

fn translate(object: &crate::rtl::Object) -> Result<vlog::FunctionDef> {
    let context = TranslationContext {
        func: vlog::FunctionDef {
            name: format!("kernel_{}", object.name),
            signed_width: object.kind(object.return_register).into(),
            args: vec![],
            items: vec![],
        },
        body: vec![],
        rtl: object,
    };
    context.translate_kernel_for_object()
}

pub fn generate_verilog(object: &crate::rtl::Object) -> Result<vlog::FunctionDef> {
    translate(object)
}

#[cfg(test)]
mod literal_tests {
    use super::*;
    use crate::Digital;

    /// The conversion this module owns: a zero-width value has no
    /// Verilog literal, and asking for one is an error.
    ///
    /// It used to render `0'b` — a sized literal with no digits, and a
    /// syntax error. See
    /// `rhdl-fpga/tests/zero_width_verilog_literal.rs` for the
    /// widget-level consequences.
    #[test]
    fn a_zero_width_value_has_no_literal() {
        let tb = ().typed_bits();
        assert_eq!(tb.len(), 0);
        let r: std::result::Result<vlog::LitVerilog, _> = (&tb).try_into();
        assert!(
            matches!(r, Err(RHDLError::ZeroWidthVerilogLiteral)),
            "a zero-width value must not produce a literal"
        );
    }

    /// One bit is the smallest legal literal and must convert.
    #[test]
    fn one_bit_converts() {
        let tb = true.typed_bits();
        let lit: vlog::LitVerilog = (&tb).try_into().expect("one bit is legal");
        assert_eq!(quote::quote!(#lit).to_string(), "1 'b1");
    }

    /// Signedness survives, which is load-bearing: dropping the `s`
    /// base specifier silently flips relational comparisons in Verilog
    /// while the Rust simulator stays correct (see the comment on the
    /// conversion, and PR #73).
    #[test]
    fn signedness_survives() {
        let tb = rhdl_bits::signed::<8>(-3).typed_bits();
        let lit: vlog::LitVerilog = (&tb).try_into().expect("signed is legal");
        let s = quote::quote!(#lit).to_string();
        assert!(s.contains("'sb"), "expected an sb literal, got {s}");
    }

    /// The owning-value conversion agrees with the borrowing one.
    #[test]
    fn the_owned_conversion_agrees() {
        let r: std::result::Result<vlog::LitVerilog, _> = ().typed_bits().try_into();
        assert!(matches!(r, Err(RHDLError::ZeroWidthVerilogLiteral)));
        let r: std::result::Result<vlog::LitVerilog, _> = false.typed_bits().try_into();
        assert!(r.is_ok());
    }
}
