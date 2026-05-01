use std::any::type_name;

use crate::{DigitalFn, RHDLError, kernel::KernelFnKind};

use super::stage1::CompilationMode;
use anyhow::anyhow;

pub fn compile_design_stage1<K: DigitalFn>(
    mode: CompilationMode,
) -> Result<crate::rhif::Object, RHDLError> {
    let Some(KernelFnKind::AstKernel(kernel)) = K::kernel_fn() else {
        return Err(anyhow!("Missing kernel function provided for {}", type_name::<K>()).into());
    };
    super::stage1::compile(&kernel, mode)
}

/// Like [`compile_design_stage1`] but invokes `hook` after every
/// pass.  See [`super::stage1::compile_with_checkpoints`] for the
/// observation contract.
///
/// Used by the Phase 2 RHIF property tests to verify well-formedness
/// and semantic-preservation invariants between every adjacent pair
/// of passes — not just at the end of the pipeline.
pub fn compile_design_stage1_with_checkpoints<K: DigitalFn>(
    mode: CompilationMode,
    hook: &mut super::CheckpointFn<'_>,
) -> Result<crate::rhif::Object, RHDLError> {
    let Some(KernelFnKind::AstKernel(kernel)) = K::kernel_fn() else {
        return Err(anyhow!("Missing kernel function provided for {}", type_name::<K>()).into());
    };
    super::stage1::compile_with_checkpoints(&kernel, mode, hook)
}

pub fn compile_design_stage2(
    object: &crate::rhif::Object,
) -> Result<crate::rtl::Object, RHDLError> {
    super::stage2::compile(object)
}

pub fn compile_design<K: DigitalFn>(
    mode: CompilationMode,
) -> Result<crate::rtl::Object, RHDLError> {
    let rhif = compile_design_stage1::<K>(mode)?;
    compile_design_stage2(&rhif)
}
