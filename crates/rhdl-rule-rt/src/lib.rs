//! Runtime support types for [`rhdl-rule`](../rhdl_rule/index.html).
//!
//! Phase 2 of the rule-based RHDL plan introduces two ergonomic
//! type aliases / marker types that user code refers to inside
//! `rule_kernel!` invocations:
//!
//! - [`Reg<T>`] — alias for `rhdl_fpga::core::dff::DFF<T>`, the
//!   D-flip-flop sub-circuit that backs every rule register.  Rules
//!   declare their register fields as `Reg<T>` so the source reads
//!   "this is a typed register" instead of leaking the
//!   sub-circuit name.  Today it is a thin alias; in a later phase
//!   the runtime crate may grow type-state distinctions (e.g.
//!   `RegSticky<T>`, `RegEnable<T>`) without breaking user code.
//!
//! - [`RuleCtx<W>`] — phantom-typed marker that appears in the
//!   first-parameter position of every `#[rule]` method
//!   (`ctx: &mut RuleCtx<Self>`).  At the source level it carries
//!   the static knowledge that this method *is* a rule; the macro
//!   strips it during expansion so the type never reaches the
//!   silicon path.  The phantom parameter `W` is the widget's own
//!   struct type, which lets future phases attach widget-specific
//!   capabilities (e.g. lookup tables of register references) when
//!   we want them — without forcing every rule body to thread
//!   them by hand.
//!
//! Neither type carries any runtime state.  They exist purely to
//! shape what the user reads and writes inside `rule_kernel!`
//! invocations.  The macro recognises them syntactically and
//! lowers them to the appropriate IR; nothing reaches the
//! synthesiser.
//!
//! Re-exported from [`rhdl_rule::prelude`](../rhdl_rule/prelude/index.html)
//! so users normally never reach for this crate by name.

#![deny(missing_docs)]

use std::marker::PhantomData;

/// Type alias for a D flip-flop sub-circuit, used as the backing
/// store for a rule register.
///
/// Inside a `rule_kernel!` invocation, every register field is
/// declared as `Reg<T>` (or the underlying `dff::DFF<T>` directly,
/// for backwards compatibility).  The two are interchangeable today
/// — `Reg<T>` is simply the user-facing spelling.
///
/// # Example
///
/// ```ignore
/// use rhdl_rule::rule_kernel;
/// use rhdl_rule_rt::Reg;
///
/// rule_kernel! {
///     pub struct Counter {
///         count: Reg<Bits<8>>,
///     }
///     // ...
/// }
/// ```
pub type Reg<T> = rhdl_fpga::core::dff::DFF<T>;

/// Phantom-typed marker for the rule-context parameter.
///
/// Every `#[rule]` method declares its first parameter as
/// `ctx: &mut RuleCtx<Self>`.  The macro strips this parameter
/// during expansion — the kernel function it emits has the standard
/// `(cr: ClockReset, i: I, q: Q) -> (O, D)` signature.  Inside rule
/// bodies, accesses spelt `*ctx.<field>` are rewritten to `q.<field>`
/// (read), and `set!(ctx.<field>, value)` is rewritten to a
/// scheduled write.
///
/// The `W` type parameter is the widget's own struct (typically
/// `Self`).  It is unused at the silicon level but is kept in the
/// type so future phases can attach widget-specific capability
/// methods (e.g. register-reference lookups) without changing the
/// surface syntax.
pub struct RuleCtx<W> {
    _phantom: PhantomData<W>,
}

impl<W> Default for RuleCtx<W> {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<W> RuleCtx<W> {
    /// Construct a fresh rule-context marker.
    ///
    /// Users should never need to call this directly — the macro
    /// strips `ctx` from the rule signature during expansion.  The
    /// constructor exists only so that hand-written test scaffolding
    /// can stand up a `RuleCtx<W>` value where it is convenient.
    pub fn new() -> Self {
        Self::default()
    }
}
