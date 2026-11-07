//! WGSL -> HLSL translation through naga's `hlsl-out` backend.
//!
//! This is the first half of the D3D12 shader pipeline: Blade's shaders are
//! authored in WGSL, and D3D12 consumes DXIL. `hlsl-out` produces HLSL that
//! [`super::ShaderCompiler`] hands to `dxcompiler.dll` for the second half.
//!
//! Resource bindings: WGSL `@group(0) @binding(N)` maps to HLSL register `N`
//! (space 0) in the register class implied by the resource type, which matches
//! the descriptor tables this backend builds. Other groups are rejected until
//! register spaces are wired into the root signature.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use super::Error;

/// A WGSL shader translated to HLSL for one entry point.
pub struct HlslTranslation {
    /// Generated HLSL source.
    pub source: String,
    /// Entry point to compile.
    pub entry_point: String,
    /// Stage the entry point belongs to.
    pub stage: naga::ShaderStage,
}

/// Maps a stage name to a naga stage.
pub fn shader_stage(name: &str) -> Option<naga::ShaderStage> {
    match name {
        "vertex" | "vs" => Some(naga::ShaderStage::Vertex),
        "fragment" | "fs" | "pixel" | "ps" => Some(naga::ShaderStage::Fragment),
        "compute" | "cs" => Some(naga::ShaderStage::Compute),
        _ => None,
    }
}

/// The DXC target profile for a stage (shader model 6.0).
pub fn stage_target(stage: naga::ShaderStage) -> &'static str {
    match stage {
        naga::ShaderStage::Vertex => "vs_6_0",
        naga::ShaderStage::Fragment => "ps_6_0",
        naga::ShaderStage::Compute => "cs_6_0",
        _ => "cs_6_0",
    }
}

/// Translates `wgsl` to HLSL, emitting only the `entry` entry point of `stage`.
pub fn wgsl_to_hlsl(
    wgsl: &str,
    entry: &str,
    stage: naga::ShaderStage,
) -> Result<HlslTranslation, Error> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|err| {
        Error::ShaderCompile(format!("WGSL parse error:\n{}", err.emit_to_string(wgsl)))
    })?;

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    let info = validator
        .validate(&module)
        .map_err(|err| Error::ShaderCompile(format!("WGSL validation error: {err:?}")))?;

    let options = naga::back::hlsl::Options {
        shader_model: naga::back::hlsl::ShaderModel::V6_0,
        binding_map: binding_map(&module)?,
        fake_missing_bindings: false,
        special_constants_binding: None,
        immediates_target: None,
        ..Default::default()
    };
    let pipeline_options = naga::back::hlsl::PipelineOptions {
        entry_point: Some((stage, entry.to_owned())),
    };

    // `Writer` consumes its output sink and has no getter, so hand it a sink
    // that keeps the string reachable.
    let source = Rc::new(RefCell::new(String::new()));
    let mut writer =
        naga::back::hlsl::Writer::new(StringSink(source.clone()), &options, &pipeline_options);
    writer
        .write(&module, &info, None)
        .map_err(|err| Error::ShaderCompile(format!("HLSL generation error: {err:?}")))?;
    drop(writer);

    let source = source.borrow().clone();
    if source.is_empty() {
        return Err(Error::ShaderCompile(
            "HLSL generation produced an empty source".to_string(),
        ));
    }
    Ok(HlslTranslation {
        source,
        entry_point: entry.to_owned(),
        stage,
    })
}

/// Maps WGSL bindings from group 0 onto HLSL registers in space 0.
fn binding_map(module: &naga::Module) -> Result<naga::back::hlsl::BindingMap, Error> {
    use naga::back::hlsl::{BindTarget, BindingMap};
    let mut bindings = BindingMap::new();
    for (_, global) in module.global_variables.iter() {
        let Some(binding) = global.binding else {
            continue;
        };
        if binding.group != 0 {
            return Err(Error::ShaderCompile(format!(
                "WGSL binding group {} is not supported yet (only group 0 maps onto registers)",
                binding.group
            )));
        }
        bindings.insert(
            binding,
            BindTarget {
                space: 0,
                register: binding.binding,
                binding_array_size: None,
                dynamic_storage_buffer_offsets_index: None,
                restrict_indexing: false,
            },
        );
    }
    Ok(bindings)
}

struct StringSink(Rc<RefCell<String>>);

impl fmt::Write for StringSink {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0.borrow_mut().push_str(text);
        Ok(())
    }
}
