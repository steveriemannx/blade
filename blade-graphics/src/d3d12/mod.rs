//! Direct3D 12 device layer (phase 1).
//!
//! Phase 0 proved that a feature level 12_0 `ID3D12Device` can be created. This
//! module grows that probe into the device layer a HAL backend needs before
//! command recording starts: adapter enumeration, the debug layer, a WARP
//! fallback for headless tests, a direct queue with fence-based synchronization,
//! committed buffers, and a minimal command recorder that performs buffer
//! copies.
//!
//! It does **not** implement the `blade-graphics` HAL traits yet and it is not
//! wired into backend selection. See
//! [`blade-graphics/etc/d3d12.md`](../../etc/d3d12.md) for the full design and
//! the phased plan.
//!
//! Enabled by the `d3d12` cargo feature on Windows only.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::ptr;

pub mod shader;
pub mod window;

pub use shader::ShaderCompiler;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::{
    D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_12_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_COMMAND_QUEUE_PRIORITY_NORMAL, D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_FENCE_FLAG_NONE,
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK,
    D3D12_HEAP_TYPE_UPLOAD, D3D12_MEMORY_POOL_UNKNOWN, D3D12_MESSAGE, D3D12_RANGE,
    D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_DEPTH_READ,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_GENERIC_READ,
    D3D12_RESOURCE_STATE_INDEX_BUFFER, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_RLDO_DETAIL, D3D12_RLDO_IGNORE_INTERNAL,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12CreateDevice, D3D12GetDebugInterface,
    ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue, ID3D12Debug, ID3D12DebugDevice,
    ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12InfoQueue, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_DEBUG,
    DXGI_CREATE_FACTORY_FLAGS, DXGI_DEBUG_ALL, DXGI_ERROR_NOT_FOUND, DXGI_GPU_PREFERENCE,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, DXGI_INFO_QUEUE_MESSAGE, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, DXGIGetDebugInterface1, IDXGIAdapter1, IDXGIDevice,
    IDXGIFactory6, IDXGIInfoQueue, IDXGIOutput, IDXGISwapChain1, IDXGISwapChain2, IDXGISwapChain4,
};
use windows::core::{Error as WinError, HRESULT, Interface, PCSTR, PCWSTR};

// Phase 2: textures, descriptors and samplers.
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1_0, D3D12_BLEND_DESC, D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD,
    D3D12_BLEND_ZERO, D3D12_BUFFER_SRV, D3D12_BUFFER_SRV_FLAGS, D3D12_BUFFER_UAV,
    D3D12_BUFFER_UAV_FLAGS, D3D12_CACHED_PIPELINE_STATE, D3D12_CLEAR_FLAG_DEPTH,
    D3D12_COLOR_WRITE_ENABLE_ALL, D3D12_COMPARISON_FUNC_ALWAYS, D3D12_COMPARISON_FUNC_LESS,
    D3D12_COMPARISON_FUNC_NEVER, D3D12_COMPUTE_PIPELINE_STATE_DESC,
    D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF, D3D12_CONSTANT_BUFFER_VIEW_DESC,
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CULL_MODE_NONE, D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
    D3D12_DEPTH_STENCIL_DESC, D3D12_DEPTH_STENCILOP_DESC, D3D12_DEPTH_WRITE_MASK_ALL,
    D3D12_DEPTH_WRITE_MASK_ZERO, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, D3D12_DESCRIPTOR_HEAP_TYPE,
    D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, D3D12_DESCRIPTOR_RANGE,
    D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND, D3D12_DESCRIPTOR_RANGE_TYPE,
    D3D12_DESCRIPTOR_RANGE_TYPE_CBV, D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
    D3D12_DESCRIPTOR_RANGE_TYPE_SRV, D3D12_DESCRIPTOR_RANGE_TYPE_UAV, D3D12_FILL_MODE_SOLID,
    D3D12_FILTER, D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_GPU_DESCRIPTOR_HANDLE,
    D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
    D3D12_INDEX_BUFFER_VIEW, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC,
    D3D12_INPUT_LAYOUT_DESC, D3D12_LOGIC_OP_NOOP, D3D12_PIPELINE_STATE_FLAG_NONE,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
    D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC, D3D12_RESOURCE_BARRIER_TYPE_UAV,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
    D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET,
    D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_UAV_BARRIER, D3D12_ROOT_CONSTANTS,
    D3D12_ROOT_DESCRIPTOR_TABLE, D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0,
    D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS, D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
    D3D12_ROOT_SIGNATURE_DESC, D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    D3D12_ROOT_SIGNATURE_FLAG_NONE, D3D12_ROOT_SIGNATURE_FLAGS, D3D12_SAMPLER_DESC,
    D3D12_SHADER_BYTECODE, D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
    D3D12_SHADER_VISIBILITY_ALL, D3D12_SRV_DIMENSION_BUFFER, D3D12_SRV_DIMENSION_TEXTURE2D,
    D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE, D3D12_STATIC_SAMPLER_DESC, D3D12_STREAM_OUTPUT_DESC,
    D3D12_TEX2D_SRV, D3D12_TEXTURE_ADDRESS_MODE, D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
    D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12_UAV_DIMENSION_BUFFER, D3D12_UNORDERED_ACCESS_VIEW_DESC,
    D3D12_UNORDERED_ACCESS_VIEW_DESC_0, D3D12_VERTEX_BUFFER_VIEW, D3D12_VIEWPORT,
    D3D12SerializeRootSignature, ID3D12DescriptorHeap, ID3D12PipelineState, ID3D12RootSignature,
};
pub use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_D32_FLOAT, DXGI_FORMAT_R8_UNORM,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R16_UINT,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_UINT,
    DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

/// Minimum feature level the backend targets. Blade's Vulkan backend already
/// requires 1.3-era features (dynamic rendering, descriptor indexing, ray
/// queries), which D3D12 only matches at 12_0 + recent drivers.
pub const MIN_FEATURE_LEVEL: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL_12_0;

/// Allocation granularity used for buffer sizes and copy offsets.
///
/// D3D12 only requires 4-byte alignment for buffer copies, but 256 bytes keep
/// every buffer eligible for `D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT`-style
/// layout code (and the eventual sub-allocation scheme) without extra work.
pub const BUFFER_ALIGNMENT: u64 = 256;

/// Properties of a DXGI adapter, reported for diagnostics.
#[derive(Clone, Debug)]
pub struct AdapterInfo {
    /// Adapter description, e.g. `"NVIDIA GeForce RTX 4070"`.
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_video_memory: u64,
    pub shared_system_memory: u64,
    /// True for the software rasterizer (WARP).
    pub is_software: bool,
}

impl fmt::Display for AdapterInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "\"{}\" (vendor 0x{:04X}, device 0x{:04X}), {} MiB dedicated, \
             {} MiB shared, software: {}",
            self.name,
            self.vendor_id,
            self.device_id,
            self.dedicated_video_memory / (1024 * 1024),
            self.shared_system_memory / (1024 * 1024),
            self.is_software,
        )
    }
}

/// Options for [`init_with_options`].
#[derive(Clone, Debug)]
pub struct InitOptions {
    /// Adapter preference passed to DXGI.
    pub preference: DXGI_GPU_PREFERENCE,
    /// Create the WARP software device instead of enumerating hardware
    /// adapters. Useful for headless CI, where no GPU may be present.
    pub force_warp: bool,
    /// Fall back to WARP when no hardware adapter can host a device.
    pub warp_fallback: bool,
    /// Enable the D3D12 debug layer before creating the device.
    /// [`InitOptions::default`] enables it when `BLADE_D3D12_DEBUG` is set
    /// to anything but `0`/`false`.
    pub enable_debug_layer: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        Self {
            preference: DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
            force_warp: false,
            warp_fallback: true,
            enable_debug_layer: debug_layer_requested(),
        }
    }
}

impl InitOptions {
    /// Headless options: WARP only, debug layer if `BLADE_D3D12_DEBUG` is set.
    pub fn headless() -> Self {
        Self {
            force_warp: true,
            ..Self::default()
        }
    }
}

fn debug_layer_requested() -> bool {
    match std::env::var("BLADE_D3D12_DEBUG") {
        Ok(value) => !value.is_empty() && value != "0" && value != "false",
        Err(_) => false,
    }
}

/// Why a Direct3D 12 operation failed.
#[derive(Debug)]
pub enum Error {
    /// No DXGI adapter could be enumerated.
    NoAdapter,
    /// Adapters exist, but none supports a feature level 12_0 D3D12 device.
    NoCompatibleDevice,
    /// The call was made in a state that does not allow it (e.g. recording on a
    /// closed command list, or writing to a non-upload buffer).
    InvalidState(&'static str),
    /// A shader failed to compile; the payload holds the DXC diagnostics.
    ShaderCompile(String),
    /// The DXC toolchain (`dxcompiler.dll`) is missing or unusable.
    ShaderToolchain(String),
    /// Root signature serialization failed; the payload holds the diagnostics.
    RootSignature(String),
    /// A DXGI/D3D12 call failed.
    Windows(WinError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter => f.write_str("no DXGI adapter found"),
            Self::NoCompatibleDevice => {
                f.write_str("no adapter supports a feature level 12_0 D3D12 device")
            }
            Self::InvalidState(reason) => write!(f, "invalid D3D12 state: {reason}"),
            Self::ShaderCompile(diagnostics) => {
                write!(f, "shader compilation failed:\n{diagnostics}")
            }
            Self::ShaderToolchain(reason) => write!(f, "DXC toolchain unavailable: {reason}"),
            Self::RootSignature(diagnostics) => {
                write!(f, "root signature serialization failed:\n{diagnostics}")
            }
            Self::Windows(err) => write!(f, "D3D12 call failed: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Windows(err) => Some(err),
            _ => None,
        }
    }
}

impl From<WinError> for Error {
    fn from(err: WinError) -> Self {
        Self::Windows(err)
    }
}

/// Compatibility alias: initialization returns [`Error`] too.
pub type InitError = Error;

/// A minimal description of the resource states the phase 1 layer can use.
///
/// D3D12 has hundreds of states; the device layer only needs the ones that a
/// copy/render/compute hand-off uses. `state` is tracked on [`Buffer`] so
/// [`CommandEncoder::transition`] can emit the correct `StateBefore`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceState {
    Common,
    CopyDest,
    CopySource,
    GenericRead,
    /// Sampled by shaders through an SRV (textures and buffers).
    ShaderRead,
    /// Written by shaders through a UAV.
    UnorderedAccess,
    /// Written by draw calls as a render target.
    RenderTarget,
    /// Back-buffer state expected by DXGI when presenting.
    Present,
    /// Read as a vertex buffer (or constant buffer) by the input assembler.
    VertexBuffer,
    /// Read as an index buffer by the input assembler.
    IndexBuffer,
    /// Bound as a depth-stencil target for depth testing and writes.
    DepthWrite,
    /// Sampled or read as depth, without depth writes.
    DepthRead,
}

impl ResourceState {
    fn to_d3d12(self) -> D3D12_RESOURCE_STATES {
        match self {
            Self::Common => D3D12_RESOURCE_STATE_COMMON,
            Self::CopyDest => D3D12_RESOURCE_STATE_COPY_DEST,
            Self::CopySource => D3D12_RESOURCE_STATE_COPY_SOURCE,
            Self::GenericRead => D3D12_RESOURCE_STATE_GENERIC_READ,
            Self::ShaderRead => D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE,
            Self::UnorderedAccess => D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            Self::RenderTarget => D3D12_RESOURCE_STATE_RENDER_TARGET,
            Self::Present => D3D12_RESOURCE_STATE_PRESENT,
            Self::VertexBuffer => D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
            Self::IndexBuffer => D3D12_RESOURCE_STATE_INDEX_BUFFER,
            Self::DepthWrite => D3D12_RESOURCE_STATE_DEPTH_WRITE,
            Self::DepthRead => D3D12_RESOURCE_STATE_DEPTH_READ,
        }
    }
}

/// Heap a buffer lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Memory {
    /// GPU-local, not CPU mappable; the default for render targets and buffers
    /// read by the GPU.
    Device,
    /// CPU writable, GPU read-only. Used for staging uploads.
    Upload,
    /// CPU readable, GPU write-only. Used for staging readbacks.
    Readback,
}

impl Memory {
    fn heap_type(self) -> windows::Win32::Graphics::Direct3D12::D3D12_HEAP_TYPE {
        match self {
            Self::Device => D3D12_HEAP_TYPE_DEFAULT,
            Self::Upload => D3D12_HEAP_TYPE_UPLOAD,
            Self::Readback => D3D12_HEAP_TYPE_READBACK,
        }
    }

    fn initial_state(self) -> ResourceState {
        match self {
            Self::Device => ResourceState::Common,
            Self::Upload => ResourceState::GenericRead,
            Self::Readback => ResourceState::CopyDest,
        }
    }

    fn is_cpu_accessible(self) -> bool {
        match self {
            Self::Device => false,
            Self::Upload | Self::Readback => true,
        }
    }
}

/// Description of a buffer to create.
#[derive(Clone, Copy, Debug)]
pub struct BufferDesc {
    pub size: u64,
    pub memory: Memory,
    /// Allow UAV (unordered access) views over this buffer. D3D12 requires the
    /// resource to be created with `D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS`;
    /// creating a UAV for a buffer without it removes the device.
    pub allow_unordered_access: bool,
}

impl BufferDesc {
    pub fn new(size: u64, memory: Memory) -> Self {
        Self {
            size,
            memory,
            allow_unordered_access: false,
        }
    }

    pub fn with_unordered_access(mut self) -> Self {
        self.allow_unordered_access = true;
        self
    }
}

/// A committed `ID3D12Resource` buffer.
pub struct Buffer {
    resource: ID3D12Resource,
    size: u64,
    memory: Memory,
    state: Cell<ResourceState>,
    allow_unordered_access: bool,
}

impl Buffer {
    /// Size in bytes (aligned up to [`BUFFER_ALIGNMENT`]).
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn memory(&self) -> Memory {
        self.memory
    }

    /// Whether the buffer was created with the UAV flag.
    pub fn allows_unordered_access(&self) -> bool {
        self.allow_unordered_access
    }

    pub fn state(&self) -> ResourceState {
        self.state.get()
    }

    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    /// GPU virtual address, for vertex/index/constant buffer views.
    pub fn gpu_address(&self) -> u64 {
        unsafe { self.resource.GetGPUVirtualAddress() }
    }

    /// Uploads `data` into an CPU-writable buffer, at `offset`.
    ///
    /// Only valid for [`Memory::Upload`] buffers; the write happens immediately
    /// on the CPU and is visible to any command list submitted afterwards.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), Error> {
        if !self.memory.is_cpu_accessible() || self.memory == Memory::Readback {
            return Err(Error::InvalidState("buffer is not CPU-writable"));
        }
        let end = offset + data.len() as u64;
        if end > self.size {
            return Err(Error::InvalidState("write past the end of the buffer"));
        }
        unsafe {
            let mut mapped: *mut core::ffi::c_void = ptr::null_mut();
            // A null read range states that the CPU does not intend to read.
            self.resource.Map(0, None, Some(&mut mapped))?;
            ptr::copy_nonoverlapping(
                data.as_ptr(),
                (mapped as *mut u8).add(offset as usize),
                data.len(),
            );
            self.resource.Unmap(0, None);
        }
        Ok(())
    }

    /// Reads the whole buffer back to the CPU.
    ///
    /// Only valid for [`Memory::Readback`] buffers, and only after the GPU work
    /// that wrote them has been waited on (see [`Device::wait`]).
    pub fn read(&self) -> Result<Vec<u8>, Error> {
        if self.memory != Memory::Readback {
            return Err(Error::InvalidState("buffer is not CPU-readable"));
        }
        let size = self.size as usize;
        let mut data = vec![0u8; size];
        let range = D3D12_RANGE {
            Begin: 0,
            End: size,
        };
        unsafe {
            let mut mapped: *mut core::ffi::c_void = ptr::null_mut();
            self.resource.Map(0, Some(&range), Some(&mut mapped))?;
            ptr::copy_nonoverlapping(mapped as *const u8, data.as_mut_ptr(), size);
            // A null written range states that the CPU did not modify the range.
            self.resource.Unmap(0, None);
        }
        Ok(data)
    }
}

/// A value the GPU queue signals once submitted work is complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncPoint {
    value: u64,
}

impl SyncPoint {
    pub fn value(self) -> u64 {
        self.value
    }
}

/// The D3D12 device, its direct queue and the fence used to synchronize with it.
pub struct Device {
    raw: ID3D12Device,
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    fence_event: HANDLE,
    next_fence_value: Cell<u64>,
    /// Command allocators still in use by submitted work, dropped once the
    /// fence reaches their submission value. A command allocator holds
    /// references to the resources recorded into its lists, so leaking it keeps
    /// buffers alive and makes `ResizeBuffers` fail with
    /// `DXGI_ERROR_INVALID_CALL`.
    pending_allocators: RefCell<Vec<(u64, ID3D12CommandAllocator)>>,
    node_count: u32,
    debug_layer: bool,
}

impl Device {
    fn new(raw: ID3D12Device, debug_layer: bool) -> Result<Self, Error> {
        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: D3D12_COMMAND_QUEUE_PRIORITY_NORMAL.0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        let queue: ID3D12CommandQueue = unsafe { raw.CreateCommandQueue(&queue_desc)? };
        let fence: ID3D12Fence = unsafe { raw.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        let fence_event = unsafe { CreateEventW(None, false, false, PCWSTR::null())? };
        let node_count = unsafe { raw.GetNodeCount() };

        Ok(Self {
            raw,
            queue,
            fence,
            fence_event,
            next_fence_value: Cell::new(0),
            pending_allocators: RefCell::new(Vec::new()),
            node_count,
            debug_layer,
        })
    }

    /// The underlying `ID3D12Device`.
    pub fn raw(&self) -> &ID3D12Device {
        &self.raw
    }

    /// The direct command queue. Phase 1 keeps a single queue; compute/copy
    /// queues are a later phase.
    pub fn queue(&self) -> &ID3D12CommandQueue {
        &self.queue
    }

    pub fn node_count(&self) -> u32 {
        self.node_count
    }

    pub fn has_debug_layer(&self) -> bool {
        self.debug_layer
    }

    /// Drains the debug layer's message queue (empty when the debug layer is
    /// off). Call this after an API call fails to see what D3D12 objected to.
    pub fn debug_messages(&self) -> Vec<String> {
        let Ok(queue) = self.raw.cast::<ID3D12InfoQueue>() else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        let count = unsafe { queue.GetNumStoredMessages() };
        for index in 0..count {
            let mut size = 0usize;
            unsafe {
                let _ = queue.GetMessage(index, None, &mut size);
            }
            if size == 0 {
                continue;
            }
            let mut buffer = vec![0u8; size];
            let message = buffer.as_mut_ptr() as *mut D3D12_MESSAGE;
            if unsafe { queue.GetMessage(index, Some(message), &mut size) }.is_ok() {
                let message = unsafe { &*message };
                if message.pDescription.is_null() || message.DescriptionByteLength == 0 {
                    continue;
                }
                let description = unsafe {
                    std::slice::from_raw_parts(message.pDescription, message.DescriptionByteLength)
                };
                messages.push(
                    String::from_utf8_lossy(description)
                        .trim_end_matches(char::from(0))
                        .to_string(),
                );
            }
        }
        unsafe {
            queue.ClearStoredMessages();
        }
        messages
    }

    /// Drains the DXGI info queue (needs a factory created with
    /// `DXGI_CREATE_FACTORY_DEBUG`, i.e. `BLADE_D3D12_DEBUG=1`).
    pub fn dxgi_debug_messages(&self) -> Vec<String> {
        let Ok(queue) = (unsafe { DXGIGetDebugInterface1::<IDXGIInfoQueue>(0) }) else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        let count = unsafe { queue.GetNumStoredMessages(DXGI_DEBUG_ALL) };
        for index in 0..count {
            let mut size = 0usize;
            unsafe {
                let _ = queue.GetMessage(DXGI_DEBUG_ALL, index, None, &mut size);
            }
            if size == 0 {
                continue;
            }
            let mut buffer = vec![0u8; size];
            let message = buffer.as_mut_ptr() as *mut DXGI_INFO_QUEUE_MESSAGE;
            if unsafe { queue.GetMessage(DXGI_DEBUG_ALL, index, Some(message), &mut size) }.is_ok()
            {
                let message = unsafe { &*message };
                if message.pDescription.is_null() || message.DescriptionByteLength == 0 {
                    continue;
                }
                let description = unsafe {
                    std::slice::from_raw_parts(message.pDescription, message.DescriptionByteLength)
                };
                messages.push(
                    String::from_utf8_lossy(description)
                        .trim_end_matches(char::from(0))
                        .to_string(),
                );
            }
        }
        unsafe {
            queue.ClearStoredMessages(DXGI_DEBUG_ALL);
        }
        messages
    }

    /// Asks the debug layer to report every live object and its reference count
    /// into the message queue (`Device::debug_messages` reads it back).
    pub fn report_live_objects(&self) -> Result<(), Error> {
        let debug_device: ID3D12DebugDevice = self.raw.cast()?;
        unsafe {
            debug_device.ReportLiveDeviceObjects(D3D12_RLDO_DETAIL | D3D12_RLDO_IGNORE_INTERNAL)?;
        }
        Ok(())
    }

    /// Fails if the device was removed (driver reset, invalid resource use,
    /// etc.). Useful at phase boundaries so a lost device is reported where it
    /// happened rather than at the next submission.
    pub fn check_device(&self) -> Result<(), Error> {
        unsafe { self.raw.GetDeviceRemovedReason()? };
        Ok(())
    }

    /// Creates a committed buffer and returns it in its memory type's initial
    /// state.
    pub fn create_buffer(&self, desc: BufferDesc) -> Result<Buffer, Error> {
        let size = align_up(desc.size, BUFFER_ALIGNMENT).max(BUFFER_ALIGNMENT);
        let heap = D3D12_HEAP_PROPERTIES {
            Type: desc.memory.heap_type(),
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let resource_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: size,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: if desc.allow_unordered_access {
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
            } else {
                D3D12_RESOURCE_FLAG_NONE
            },
        };
        let state = desc.memory.initial_state();
        let mut resource: Option<ID3D12Resource> = None;
        unsafe {
            self.raw.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &resource_desc,
                state.to_d3d12(),
                None,
                &mut resource,
            )?;
        }
        let resource = resource.ok_or(Error::InvalidState(
            "CreateCommittedResource returned no resource",
        ))?;
        Ok(Buffer {
            resource,
            size,
            memory: desc.memory,
            state: Cell::new(state),
            allow_unordered_access: desc.allow_unordered_access,
        })
    }

    /// Starts recording a command list together with a fresh allocator.
    ///
    /// A new allocator per encoder keeps the phase 1 model trivial; the
    /// allocator ring for transient frames arrives with the HAL wiring.
    pub fn record(&self) -> Result<CommandEncoder, Error> {
        let allocator: ID3D12CommandAllocator = unsafe {
            self.raw
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?
        };
        let list: ID3D12GraphicsCommandList = unsafe {
            self.raw
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?
        };
        Ok(CommandEncoder {
            allocator,
            list,
            closed: false,
        })
    }

    /// Closes the recorded list, submits it to the direct queue and returns the
    /// fence value to wait on.
    pub fn submit(&self, encoder: CommandEncoder) -> Result<SyncPoint, Error> {
        if encoder.closed {
            return Err(Error::InvalidState("command list is already closed"));
        }
        unsafe { encoder.list.Close()? };
        let command_list: ID3D12CommandList = encoder.list.cast()?;
        unsafe {
            self.queue.ExecuteCommandLists(&[Some(command_list)]);
            let value = self.next_fence_value.get() + 1;
            self.next_fence_value.set(value);
            self.queue.Signal(&self.fence, value)?;
            // The allocator must outlive the submitted work; `wait` releases it
            // once the fence passes this value.
            self.pending_allocators
                .borrow_mut()
                .push((value, encoder.allocator));
            Ok(SyncPoint { value })
        }
    }

    /// Blocks the CPU until the GPU reaches `sp`.
    pub fn wait(&self, sp: SyncPoint) -> Result<(), Error> {
        unsafe {
            self.fence
                .SetEventOnCompletion(sp.value, self.fence_event)?;
            let result = WaitForSingleObject(self.fence_event, INFINITE);
            if result != WAIT_OBJECT_0 {
                return Err(Error::InvalidState(
                    "fence wait did not return WAIT_OBJECT_0",
                ));
            }
        }
        self.pending_allocators
            .borrow_mut()
            .retain(|(value, _)| *value > sp.value);
        Ok(())
    }

    /// Waits until all previously submitted work has completed.
    pub fn wait_idle(&self) -> Result<(), Error> {
        unsafe {
            let value = self.next_fence_value.get() + 1;
            self.next_fence_value.set(value);
            self.queue.Signal(&self.fence, value)?;
            self.wait(SyncPoint { value })
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // Best effort: let the GPU finish before tearing the objects down.
        let _ = self.wait_idle();
        self.pending_allocators.borrow_mut().clear();
        unsafe {
            let _ = CloseHandle(self.fence_event);
        }
    }
}

/// Records copies and resource barriers into an `ID3D12GraphicsCommandList`.
pub struct CommandEncoder {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    closed: bool,
}

impl CommandEncoder {
    /// Records a transition of `buffer` to `target_state`.
    ///
    /// The buffer tracks its own state, so barriers are emitted with the right
    /// `StateBefore` without the caller having to remember it.
    pub fn transition_buffer(&mut self, buffer: &Buffer, target_state: ResourceState) {
        let before = buffer.state.get();
        if before == target_state {
            return;
        }
        self.transition_resource(&buffer.resource, before, target_state);
        buffer.state.set(target_state);
    }

    /// Records a transition of `texture` to `target_state`.
    pub fn transition_texture(&mut self, texture: &Texture, target_state: ResourceState) {
        let before = texture.state.get();
        if before == target_state {
            return;
        }
        self.transition_resource(&texture.resource, before, target_state);
        texture.state.set(target_state);
    }

    fn transition_resource(
        &mut self,
        resource: &ID3D12Resource,
        before: ResourceState,
        after: ResourceState,
    ) {
        let barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: borrow_interface(resource),
                    Subresource: 0,
                    StateBefore: before.to_d3d12(),
                    StateAfter: after.to_d3d12(),
                }),
            },
        };
        unsafe {
            self.list.ResourceBarrier(&[barrier]);
        }
    }

    /// Records a buffer-to-buffer copy.
    ///
    /// Both buffers must be in states that allow the copy: the source in
    /// [`ResourceState::CopySource`] or [`ResourceState::GenericRead`], the
    /// destination in [`ResourceState::CopyDest`]. This is not validated; the
    /// debug layer reports violations.
    pub fn copy_buffer(
        &mut self,
        dst: &Buffer,
        dst_offset: u64,
        src: &Buffer,
        src_offset: u64,
        size: u64,
    ) {
        unsafe {
            self.list
                .CopyBufferRegion(&dst.resource, dst_offset, &src.resource, src_offset, size);
        }
    }

    /// Records a copy from a placed footprint of `buffer` into mip 0 of
    /// `texture`.
    ///
    /// [`Device::upload_texture`] is the safe synchronous wrapper: it creates
    /// the staging buffer, records the barriers and waits.
    pub(crate) fn copy_buffer_to_texture(
        &mut self,
        texture: &Texture,
        buffer: &Buffer,
        footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    ) {
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: borrow_interface(&texture.resource),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: borrow_interface(&buffer.resource),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: *footprint,
            },
        };
        unsafe {
            self.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        }
    }

    /// Records a copy from mip 0 of `texture` into a placed footprint of
    /// `buffer`.
    ///
    /// [`Device::read_texture`] is the safe synchronous wrapper.
    pub(crate) fn copy_texture_to_buffer(
        &mut self,
        buffer: &Buffer,
        texture: &Texture,
        footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    ) {
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: borrow_interface(&buffer.resource),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: *footprint,
            },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: borrow_interface(&texture.resource),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        unsafe {
            self.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        }
    }
}

/// Description of a 2D texture.
#[derive(Clone, Copy, Debug)]
pub struct TextureDesc {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u16,
    pub format: DXGI_FORMAT,
    /// Allow render-target views. D3D12 requires the resource flag
    /// `D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET`; creating an RTV for a texture
    /// without it removes the device.
    pub render_target: bool,
    /// Allow depth-stencil views. D3D12 requires
    /// `D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL`; creating a DSV for a texture
    /// without it removes the device.
    pub depth_stencil: bool,
}

impl TextureDesc {
    pub fn new(width: u32, height: u32, format: DXGI_FORMAT) -> Self {
        Self {
            width,
            height,
            mip_levels: 1,
            format,
            render_target: false,
            depth_stencil: false,
        }
    }

    pub fn with_mips(mut self, mip_levels: u16) -> Self {
        self.mip_levels = mip_levels;
        self
    }

    pub fn with_render_target(mut self) -> Self {
        self.render_target = true;
        self
    }

    pub fn with_depth_stencil(mut self) -> Self {
        self.depth_stencil = true;
        self
    }
}

/// A committed `ID3D12Resource` 2D texture.
pub struct Texture {
    resource: ID3D12Resource,
    desc: TextureDesc,
    state: Cell<ResourceState>,
}

impl Texture {
    pub fn desc(&self) -> TextureDesc {
        self.desc
    }

    pub fn format(&self) -> DXGI_FORMAT {
        self.desc.format
    }

    pub fn width(&self) -> u32 {
        self.desc.width
    }

    pub fn height(&self) -> u32 {
        self.desc.height
    }

    pub fn mip_levels(&self) -> u16 {
        self.desc.mip_levels
    }

    pub fn state(&self) -> ResourceState {
        self.state.get()
    }

    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }
}

/// Size in bytes of one element (or texel) of `format`, for the formats the
/// device layer knows about.
pub fn format_size(format: DXGI_FORMAT) -> Option<u32> {
    if format == DXGI_FORMAT_R8_UNORM {
        Some(1)
    } else if format == DXGI_FORMAT_R8G8B8A8_UNORM
        || format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        || format == DXGI_FORMAT_B8G8R8A8_UNORM
        || format == DXGI_FORMAT_R32_FLOAT
        || format == DXGI_FORMAT_R32_UINT
    {
        Some(4)
    } else if format == DXGI_FORMAT_R16G16B16A16_FLOAT {
        Some(8)
    } else if format == DXGI_FORMAT_R32G32B32A32_FLOAT {
        Some(16)
    } else {
        None
    }
}

impl Device {
    /// Creates an `ID3D12Resource` 2D texture in device-local memory.
    pub fn create_texture(&self, desc: TextureDesc) -> Result<Texture, Error> {
        if desc.width == 0 || desc.height == 0 || desc.mip_levels == 0 {
            return Err(Error::InvalidState("texture dimensions must be non-zero"));
        }
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let resource_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: desc.width as u64,
            Height: desc.height,
            DepthOrArraySize: 1,
            MipLevels: desc.mip_levels,
            Format: desc.format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: if desc.render_target {
                D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
            } else if desc.depth_stencil {
                D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL
            } else {
                D3D12_RESOURCE_FLAG_NONE
            },
        };
        let mut resource: Option<ID3D12Resource> = None;
        unsafe {
            self.raw.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &resource_desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )?;
        }
        let resource = resource.ok_or(Error::InvalidState(
            "CreateCommittedResource returned no texture",
        ))?;
        Ok(Texture {
            resource,
            desc,
            state: Cell::new(ResourceState::Common),
        })
    }

    /// Uploads tightly packed mip 0 data into `texture` and leaves the texture
    /// in [`ResourceState::ShaderRead`].
    ///
    /// The copy is synchronous: it creates a staging buffer, records the copy,
    /// submits and waits, then drops the staging buffer. That is the right
    /// trade-off for asset upload; frame-time uploads will use an upload ring.
    pub fn upload_texture(&self, texture: &Texture, data: &[u8]) -> Result<(), Error> {
        let (footprint, rows, row_size, total_bytes) = self.mip0_footprint(texture)?;
        let expected = row_size * rows as u64;
        if data.len() as u64 != expected {
            return Err(Error::InvalidState(
                "texture upload data size does not match the mip 0 footprint",
            ));
        }

        let staging = self.create_buffer(BufferDesc::new(total_bytes, Memory::Upload))?;
        let row_pitch = footprint.Footprint.RowPitch as usize;
        for row in 0..rows as usize {
            let start = row * row_size as usize;
            staging.write(
                footprint.Offset + (row * row_pitch) as u64,
                &data[start..start + row_size as usize],
            )?;
        }

        let mut encoder = self.record()?;
        encoder.transition_texture(texture, ResourceState::CopyDest);
        encoder.copy_buffer_to_texture(texture, &staging, &footprint);
        encoder.transition_texture(texture, ResourceState::ShaderRead);
        let sp = self.submit(encoder)?;
        self.wait(sp)
    }

    /// Reads mip 0 of `texture` back to the CPU as tightly packed rows.
    ///
    /// Synchronous, like [`Device::upload_texture`]. The texture keeps (or
    /// regains) its previous state.
    pub fn read_texture(&self, texture: &Texture) -> Result<Vec<u8>, Error> {
        let (footprint, rows, row_size, total_bytes) = self.mip0_footprint(texture)?;
        let staging = self.create_buffer(BufferDesc::new(total_bytes, Memory::Readback))?;
        let before = texture.state();

        let mut encoder = self.record()?;
        encoder.transition_texture(texture, ResourceState::CopySource);
        encoder.copy_texture_to_buffer(&staging, texture, &footprint);
        encoder.transition_texture(texture, before);
        let sp = self.submit(encoder)?;
        self.wait(sp)?;

        let padded = staging.read()?;
        let row_pitch = footprint.Footprint.RowPitch as usize;
        let mut data = Vec::with_capacity(row_size as usize * rows as usize);
        for row in 0..rows as usize {
            let start = footprint.Offset as usize + row * row_pitch;
            data.extend_from_slice(&padded[start..start + row_size as usize]);
        }
        Ok(data)
    }

    /// Queries the mip 0 copy footprint: placement, row count, row size in
    /// bytes and the total staging size D3D12 requires.
    fn mip0_footprint(
        &self,
        texture: &Texture,
    ) -> Result<(D3D12_PLACED_SUBRESOURCE_FOOTPRINT, u32, u64, u64), Error> {
        let resource_desc = unsafe { texture.resource.GetDesc() };
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut rows = 0u32;
        let mut row_size = 0u64;
        let mut total_bytes = 0u64;
        unsafe {
            self.raw.GetCopyableFootprints(
                &resource_desc,
                0,
                1,
                0,
                Some(&mut footprint),
                Some(&mut rows),
                Some(&mut row_size),
                Some(&mut total_bytes),
            );
        }
        if rows == 0 || row_size == 0 || total_bytes == 0 {
            return Err(Error::InvalidState("texture has no copyable footprint"));
        }
        Ok((footprint, rows, row_size, total_bytes))
    }
}

/// Kind of descriptor heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorHeapType {
    CbvSrvUav,
    Sampler,
    /// Render-target views; CPU-only, never shader visible.
    Rtv,
    /// Depth-stencil views; CPU-only, never shader visible.
    Dsv,
}

impl DescriptorHeapType {
    fn to_d3d12(self) -> D3D12_DESCRIPTOR_HEAP_TYPE {
        match self {
            Self::CbvSrvUav => D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            Self::Sampler => D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER,
            Self::Rtv => D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            Self::Dsv => D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
        }
    }

    fn shader_visible(self) -> bool {
        matches!(self, Self::CbvSrvUav | Self::Sampler)
    }
}

/// A shader-visible descriptor heap plus a bump allocator over it.
///
/// Phase 2 keeps a single bump pointer per heap. Blades frame model
/// (re-push bindings per draw) will replace it with a per-frame descriptor ring
/// once the pipeline/root-signature code lands; `reset` is the hook for that.
pub struct DescriptorHeap {
    heap: ID3D12DescriptorHeap,
    ty: DescriptorHeapType,
    increment: u32,
    capacity: u32,
    allocated: Cell<u32>,
}

impl DescriptorHeap {
    pub fn new(device: &Device, ty: DescriptorHeapType, capacity: u32) -> Result<Self, Error> {
        if capacity == 0 {
            return Err(Error::InvalidState(
                "descriptor heap capacity must be non-zero",
            ));
        }
        let desc = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: ty.to_d3d12(),
            NumDescriptors: capacity,
            Flags: if ty.shader_visible() {
                D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
            } else {
                D3D12_DESCRIPTOR_HEAP_FLAG_NONE
            },
            NodeMask: 0,
        };
        let heap: ID3D12DescriptorHeap = unsafe { device.raw.CreateDescriptorHeap(&desc)? };
        let increment = unsafe { device.raw.GetDescriptorHandleIncrementSize(ty.to_d3d12()) };
        Ok(Self {
            heap,
            ty,
            increment,
            capacity,
            allocated: Cell::new(0),
        })
    }

    pub fn ty(&self) -> DescriptorHeapType {
        self.ty
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn allocated(&self) -> u32 {
        self.allocated.get()
    }

    pub fn remaining(&self) -> u32 {
        self.capacity - self.allocated.get()
    }

    /// Bump-allocates `count` consecutive descriptors, or returns `None` when
    /// the heap is exhausted (no wrapping).
    pub fn allocate(&self, count: u32) -> Option<DescriptorRange> {
        let start = self.allocated.get();
        let end = start.checked_add(count)?;
        if end > self.capacity {
            return None;
        }
        self.allocated.set(end);
        Some(DescriptorRange { start, count })
    }

    /// Rewinds the bump allocator. Only safe once the GPU is done with the
    /// descriptors, e.g. after a fence wait.
    pub fn reset(&self) {
        self.allocated.set(0);
    }

    pub fn cpu_handle(&self, index: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        let base = unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: base.ptr + (index * self.increment) as usize,
        }
    }

    pub fn gpu_handle(&self, index: u32) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        let base = unsafe { self.heap.GetGPUDescriptorHandleForHeapStart() };
        D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: base.ptr + (index * self.increment) as u64,
        }
    }

    pub fn range_cpu_handle(&self, range: DescriptorRange) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        self.cpu_handle(range.start)
    }

    pub fn range_gpu_handle(&self, range: DescriptorRange) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        self.gpu_handle(range.start)
    }

    fn allocate_one(&self) -> Result<DescriptorRange, Error> {
        self.allocate(1)
            .ok_or(Error::InvalidState("descriptor heap is full"))
    }
}

/// A contiguous run of descriptors inside a [`DescriptorHeap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorRange {
    pub start: u32,
    pub count: u32,
}

/// A sampler baked into the root signature as a `D3D12_STATIC_SAMPLER_DESC`.
///
/// Static samplers are cheaper than sampler descriptor heaps and are what the
/// render passes use for the common linear/clamp case.
#[derive(Clone, Copy, Debug)]
pub struct StaticSampler {
    pub shader_register: u32,
    pub filter: D3D12_FILTER,
    pub address_mode: D3D12_TEXTURE_ADDRESS_MODE,
    pub max_anisotropy: u32,
    pub min_lod: f32,
    pub max_lod: f32,
}

impl Default for StaticSampler {
    fn default() -> Self {
        Self {
            shader_register: 0,
            filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            address_mode: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            max_anisotropy: 1,
            min_lod: 0.0,
            max_lod: f32::MAX,
        }
    }
}

/// Sampler state. Phase 2 only needs the linear/clamp default.
#[derive(Clone, Copy, Debug)]
pub struct SamplerDesc {
    pub filter: D3D12_FILTER,
    pub address_mode: D3D12_TEXTURE_ADDRESS_MODE,
    pub max_anisotropy: u32,
    pub min_lod: f32,
    pub max_lod: f32,
}

impl Default for SamplerDesc {
    fn default() -> Self {
        Self {
            filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            address_mode: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            max_anisotropy: 1,
            min_lod: 0.0,
            max_lod: f32::MAX,
        }
    }
}

impl Device {
    fn ensure_heap_type(
        &self,
        heap: &DescriptorHeap,
        expected: DescriptorHeapType,
    ) -> Result<(), Error> {
        if heap.ty() != expected {
            return Err(Error::InvalidState(
                "descriptor heap type does not match the view being created",
            ));
        }
        Ok(())
    }

    /// Creates a constant buffer view over `size` bytes of `buffer` at `offset`
    /// (both must be 256-byte aligned).
    pub fn create_buffer_cbv(
        &self,
        heap: &DescriptorHeap,
        buffer: &Buffer,
        offset: u64,
        size: u32,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::CbvSrvUav)?;
        if offset % 256 != 0 || size % 256 != 0 {
            return Err(Error::InvalidState(
                "constant buffer views need 256-byte aligned offset and size",
            ));
        }
        if offset + size as u64 > buffer.size() {
            return Err(Error::InvalidState(
                "constant buffer view exceeds the buffer",
            ));
        }
        let desc = D3D12_CONSTANT_BUFFER_VIEW_DESC {
            BufferLocation: buffer.gpu_address() + offset,
            SizeInBytes: size,
        };
        let range = heap.allocate_one()?;
        unsafe {
            self.raw
                .CreateConstantBufferView(Some(&desc), heap.cpu_handle(range.start));
        }
        Ok(range)
    }

    /// Creates a typed buffer SRV over the whole buffer.
    pub fn create_buffer_srv_typed(
        &self,
        heap: &DescriptorHeap,
        buffer: &Buffer,
        format: DXGI_FORMAT,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::CbvSrvUav)?;
        let element_size = format_size(format).ok_or(Error::InvalidState(
            "unsupported format for a typed buffer view",
        ))?;
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_SRV {
                    FirstElement: 0,
                    NumElements: (buffer.size() / element_size as u64) as u32,
                    StructureByteStride: 0,
                    Flags: D3D12_BUFFER_SRV_FLAGS(0),
                },
            },
        };
        let range = heap.allocate_one()?;
        unsafe {
            self.raw.CreateShaderResourceView(
                &buffer.resource,
                Some(&desc),
                heap.cpu_handle(range.start),
            );
        }
        Ok(range)
    }

    /// Creates a structured buffer SRV over the whole buffer.
    pub fn create_buffer_srv_structured(
        &self,
        heap: &DescriptorHeap,
        buffer: &Buffer,
        structure_byte_stride: u32,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::CbvSrvUav)?;
        if structure_byte_stride == 0 {
            return Err(Error::InvalidState(
                "structured buffer stride must be non-zero",
            ));
        }
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_UNKNOWN,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_SRV {
                    FirstElement: 0,
                    NumElements: (buffer.size() / structure_byte_stride as u64) as u32,
                    StructureByteStride: structure_byte_stride,
                    Flags: D3D12_BUFFER_SRV_FLAGS(0),
                },
            },
        };
        let range = heap.allocate_one()?;
        unsafe {
            self.raw.CreateShaderResourceView(
                &buffer.resource,
                Some(&desc),
                heap.cpu_handle(range.start),
            );
        }
        Ok(range)
    }

    /// Creates a typed buffer UAV over the whole buffer.
    ///
    /// The buffer must have been created with
    /// [`BufferDesc::with_unordered_access`]; D3D12 removes the device when a
    /// UAV is created for a resource without the UAV flag.
    pub fn create_buffer_uav_typed(
        &self,
        heap: &DescriptorHeap,
        buffer: &Buffer,
        format: DXGI_FORMAT,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::CbvSrvUav)?;
        if !buffer.allows_unordered_access() {
            return Err(Error::InvalidState(
                "buffer needs BufferDesc::with_unordered_access() for UAV views",
            ));
        }
        let element_size = format_size(format).ok_or(Error::InvalidState(
            "unsupported format for a typed buffer view",
        ))?;
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: (buffer.size() / element_size as u64) as u32,
                    StructureByteStride: 0,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAGS(0),
                },
            },
        };
        let range = heap.allocate_one()?;
        unsafe {
            self.raw.CreateUnorderedAccessView(
                &buffer.resource,
                None,
                Some(&desc),
                heap.cpu_handle(range.start),
            );
        }
        Ok(range)
    }

    /// Creates a 2D SRV covering every mip of `texture`.
    pub fn create_texture_srv(
        &self,
        heap: &DescriptorHeap,
        texture: &Texture,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::CbvSrvUav)?;
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: texture.format(),
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: texture.mip_levels() as u32,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        let range = heap.allocate_one()?;
        unsafe {
            self.raw.CreateShaderResourceView(
                &texture.resource,
                Some(&desc),
                heap.cpu_handle(range.start),
            );
        }
        Ok(range)
    }

    /// Creates a sampler in a sampler descriptor heap.
    pub fn create_sampler(
        &self,
        heap: &DescriptorHeap,
        desc: &SamplerDesc,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::Sampler)?;
        let d3d12 = D3D12_SAMPLER_DESC {
            Filter: desc.filter,
            AddressU: desc.address_mode,
            AddressV: desc.address_mode,
            AddressW: desc.address_mode,
            MipLODBias: 0.0,
            MaxAnisotropy: desc.max_anisotropy,
            ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
            BorderColor: [0.0; 4],
            MinLOD: desc.min_lod,
            MaxLOD: desc.max_lod,
        };
        let range = heap.allocate_one()?;
        unsafe {
            self.raw.CreateSampler(&d3d12, heap.cpu_handle(range.start));
        }
        Ok(range)
    }
}

/// Kind of descriptor range in a root signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorRangeKind {
    Cbv,
    Srv,
    Uav,
    Sampler,
}

impl DescriptorRangeKind {
    fn to_d3d12(self) -> D3D12_DESCRIPTOR_RANGE_TYPE {
        match self {
            Self::Cbv => D3D12_DESCRIPTOR_RANGE_TYPE_CBV,
            Self::Srv => D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            Self::Uav => D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
            Self::Sampler => D3D12_DESCRIPTOR_RANGE_TYPE_SAMPLER,
        }
    }
}

/// One descriptor table in a [`RootSignature`].
#[derive(Clone, Copy, Debug)]
pub struct RootDescriptorTable {
    pub kind: DescriptorRangeKind,
    pub count: u32,
    pub base_shader_register: u32,
}

/// A root parameter in a graphics root signature.
#[derive(Clone, Copy, Debug)]
pub enum RootParameter {
    /// A descriptor table (one range).
    DescriptorTable(RootDescriptorTable),
    /// 32-bit root constants, e.g. `{ shader_register: 0, count: 4 }` for a
    /// `float4` at `b0`.
    Constants { shader_register: u32, count: u32 },
}

/// A graphics pipeline state and the root signature it was created with.
pub struct RenderPipeline {
    raw: ID3D12PipelineState,
    root_signature: RootSignature,
}

impl RenderPipeline {
    pub fn root_signature(&self) -> &RootSignature {
        &self.root_signature
    }
}

/// Description of a minimal render pipeline.
/// One per-vertex input attribute, bound to input slot 0.
#[derive(Clone, Copy, Debug)]
pub struct VertexAttribute {
    /// HLSL semantic name, e.g. `"POSITION"`.
    pub semantic: &'static str,
    pub format: DXGI_FORMAT,
    /// Byte offset inside the vertex struct.
    pub offset: u32,
}

/// Description of a render pipeline.
pub struct RenderPipelineDesc<'a> {
    pub vertex_dxil: &'a [u8],
    pub pixel_dxil: &'a [u8],
    /// Render-target formats, in `SV_Target` order.
    pub color_formats: &'a [DXGI_FORMAT],
    /// Per-vertex attributes; empty for a `SV_VertexID`-only shader.
    pub vertex_attributes: &'a [VertexAttribute],
    /// Depth-stencil format when the pass has a depth target. Enables depth
    /// testing (`LESS`) and writes.
    pub depth_format: Option<DXGI_FORMAT>,
}

/// A serialized `ID3D12RootSignature`.
pub struct RootSignature {
    raw: ID3D12RootSignature,
}

/// A compute pipeline state and the root signature it was created with.
pub struct ComputePipeline {
    raw: ID3D12PipelineState,
    root_signature: RootSignature,
}

impl ComputePipeline {
    pub fn root_signature(&self) -> &RootSignature {
        &self.root_signature
    }
}

impl Device {
    /// Creates a structured buffer UAV over the whole buffer.
    pub fn create_buffer_uav_structured(
        &self,
        heap: &DescriptorHeap,
        buffer: &Buffer,
        structure_byte_stride: u32,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::CbvSrvUav)?;
        if !buffer.allows_unordered_access() {
            return Err(Error::InvalidState(
                "buffer needs BufferDesc::with_unordered_access() for UAV views",
            ));
        }
        if structure_byte_stride == 0 {
            return Err(Error::InvalidState(
                "structured buffer stride must be non-zero",
            ));
        }
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_UNKNOWN,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: (buffer.size() / structure_byte_stride as u64) as u32,
                    StructureByteStride: structure_byte_stride,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAGS(0),
                },
            },
        };
        let range = heap.allocate_one()?;
        unsafe {
            self.raw.CreateUnorderedAccessView(
                &buffer.resource,
                None,
                Some(&desc),
                heap.cpu_handle(range.start),
            );
        }
        Ok(range)
    }

    /// Builds a compute root signature: one descriptor table per entry, no root
    /// constants or static samplers yet.
    pub fn create_compute_root_signature(
        &self,
        tables: &[RootDescriptorTable],
    ) -> Result<RootSignature, Error> {
        let parameters: Vec<RootParameter> = tables
            .iter()
            .map(|table| RootParameter::DescriptorTable(*table))
            .collect();
        self.build_root_signature(&parameters, &[], D3D12_ROOT_SIGNATURE_FLAG_NONE)
    }

    /// Builds a graphics root signature from descriptor tables and 32-bit root
    /// constants, without static samplers.
    pub fn create_graphics_root_signature(
        &self,
        parameters: &[RootParameter],
    ) -> Result<RootSignature, Error> {
        self.create_graphics_root_signature_with_samplers(parameters, &[])
    }

    /// Builds a graphics root signature with static samplers baked in.
    pub fn create_graphics_root_signature_with_samplers(
        &self,
        parameters: &[RootParameter],
        samplers: &[StaticSampler],
    ) -> Result<RootSignature, Error> {
        self.build_root_signature(
            parameters,
            samplers,
            D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        )
    }

    fn build_root_signature(
        &self,
        parameters: &[RootParameter],
        samplers: &[StaticSampler],
        flags: D3D12_ROOT_SIGNATURE_FLAGS,
    ) -> Result<RootSignature, Error> {
        let ranges: Vec<D3D12_DESCRIPTOR_RANGE> = parameters
            .iter()
            .filter_map(|parameter| match parameter {
                RootParameter::DescriptorTable(table) => Some(D3D12_DESCRIPTOR_RANGE {
                    RangeType: table.kind.to_d3d12(),
                    NumDescriptors: table.count,
                    BaseShaderRegister: table.base_shader_register,
                    RegisterSpace: 0,
                    OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
                }),
                RootParameter::Constants { .. } => None,
            })
            .collect();

        let mut range_index = 0;
        let root_parameters: Vec<D3D12_ROOT_PARAMETER> = parameters
            .iter()
            .map(|parameter| match parameter {
                RootParameter::DescriptorTable(_) => {
                    let range = &ranges[range_index];
                    range_index += 1;
                    D3D12_ROOT_PARAMETER {
                        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                        Anonymous: D3D12_ROOT_PARAMETER_0 {
                            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                                NumDescriptorRanges: 1,
                                pDescriptorRanges: range,
                            },
                        },
                        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                    }
                }
                RootParameter::Constants {
                    shader_register,
                    count,
                } => D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                    Anonymous: D3D12_ROOT_PARAMETER_0 {
                        Constants: D3D12_ROOT_CONSTANTS {
                            ShaderRegister: *shader_register,
                            RegisterSpace: 0,
                            Num32BitValues: *count,
                        },
                    },
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                },
            })
            .collect();
        let static_samplers: Vec<D3D12_STATIC_SAMPLER_DESC> = samplers
            .iter()
            .map(|sampler| D3D12_STATIC_SAMPLER_DESC {
                Filter: sampler.filter,
                AddressU: sampler.address_mode,
                AddressV: sampler.address_mode,
                AddressW: sampler.address_mode,
                MipLODBias: 0.0,
                MaxAnisotropy: sampler.max_anisotropy,
                ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
                MinLOD: sampler.min_lod,
                MaxLOD: sampler.max_lod,
                ShaderRegister: sampler.shader_register,
                RegisterSpace: 0,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            })
            .collect();
        let desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: root_parameters.len() as u32,
            pParameters: root_parameters.as_ptr(),
            NumStaticSamplers: static_samplers.len() as u32,
            pStaticSamplers: static_samplers.as_ptr(),
            Flags: flags,
        };

        let mut blob: Option<ID3DBlob> = None;
        let mut error_blob: Option<ID3DBlob> = None;
        unsafe {
            D3D12SerializeRootSignature(
                &desc,
                D3D_ROOT_SIGNATURE_VERSION_1_0,
                &mut blob,
                Some(&mut error_blob),
            )?;
        }
        let Some(blob) = blob else {
            let diagnostics = error_blob
                .map(|blob| blob_to_string(&blob))
                .unwrap_or_else(|| "no diagnostics".to_string());
            return Err(Error::RootSignature(diagnostics));
        };

        let data = unsafe { blob.GetBufferPointer() } as *const u8;
        let size = unsafe { blob.GetBufferSize() };
        let bytes = unsafe { std::slice::from_raw_parts(data, size) };
        let raw: ID3D12RootSignature = unsafe { self.raw.CreateRootSignature(0, bytes)? };
        Ok(RootSignature { raw })
    }

    /// Creates a render-target view for `texture` in an RTV heap.
    pub fn create_render_target_view(
        &self,
        heap: &DescriptorHeap,
        texture: &Texture,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::Rtv)?;
        if !texture.desc().render_target {
            return Err(Error::InvalidState(
                "texture needs TextureDesc::with_render_target() for render-target views",
            ));
        }
        let range = heap.allocate_one()?;
        unsafe {
            self.raw
                .CreateRenderTargetView(&texture.resource, None, heap.cpu_handle(range.start));
        }
        Ok(range)
    }

    /// Creates a depth-stencil view for `texture` in a DSV heap.
    pub fn create_depth_stencil_view(
        &self,
        heap: &DescriptorHeap,
        texture: &Texture,
    ) -> Result<DescriptorRange, Error> {
        self.ensure_heap_type(heap, DescriptorHeapType::Dsv)?;
        if !texture.desc().depth_stencil {
            return Err(Error::InvalidState(
                "texture needs TextureDesc::with_depth_stencil() for depth-stencil views",
            ));
        }
        let range = heap.allocate_one()?;
        unsafe {
            self.raw
                .CreateDepthStencilView(&texture.resource, None, heap.cpu_handle(range.start));
        }
        Ok(range)
    }

    /// Builds a render pipeline: vertex + pixel DXIL against one color target,
    /// optional per-vertex input layout and optional depth-stencil target.
    pub fn create_render_pipeline(
        &self,
        desc: &RenderPipelineDesc<'_>,
        root_signature: &RootSignature,
    ) -> Result<RenderPipeline, Error> {
        if desc.vertex_dxil.is_empty() || desc.pixel_dxil.is_empty() {
            return Err(Error::InvalidState(
                "render pipeline stages need non-empty bytecode",
            ));
        }
        let shader_bytecode = |data: &[u8]| D3D12_SHADER_BYTECODE {
            pShaderBytecode: data.as_ptr() as *const core::ffi::c_void,
            BytecodeLength: data.len(),
        };

        // D3D12 zeroed state is not "default state": disabled blend still needs a
        // valid op and a write mask, otherwise nothing reaches the target.
        let mut blend = D3D12_BLEND_DESC::default();
        blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
            BlendEnable: false.into(),
            LogicOpEnable: false.into(),
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_ZERO,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ONE,
            DestBlendAlpha: D3D12_BLEND_ZERO,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            LogicOp: D3D12_LOGIC_OP_NOOP,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let rasterizer = D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            FrontCounterClockwise: false.into(),
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            SlopeScaledDepthBias: 0.0,
            DepthClipEnable: true.into(),
            MultisampleEnable: false.into(),
            AntialiasedLineEnable: false.into(),
            ForcedSampleCount: 0,
            ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
        };
        let depth_enabled = desc.depth_format.is_some();
        let depth_stencil = D3D12_DEPTH_STENCIL_DESC {
            DepthEnable: depth_enabled.into(),
            DepthWriteMask: if depth_enabled {
                D3D12_DEPTH_WRITE_MASK_ALL
            } else {
                D3D12_DEPTH_WRITE_MASK_ZERO
            },
            DepthFunc: if depth_enabled {
                D3D12_COMPARISON_FUNC_LESS
            } else {
                D3D12_COMPARISON_FUNC_ALWAYS
            },
            StencilEnable: false.into(),
            StencilReadMask: 0,
            StencilWriteMask: 0,
            FrontFace: D3D12_DEPTH_STENCILOP_DESC::default(),
            BackFace: D3D12_DEPTH_STENCILOP_DESC::default(),
        };

        // D3D12 reads the semantic names only while creating the PSO, but the
        // `D3D12_INPUT_ELEMENT_DESC` array must stay alive for the call.
        let semantic_names: Vec<Vec<u8>> = desc
            .vertex_attributes
            .iter()
            .map(|attribute| {
                attribute
                    .semantic
                    .bytes()
                    .chain(std::iter::once(0))
                    .collect()
            })
            .collect();
        let input_elements: Vec<D3D12_INPUT_ELEMENT_DESC> = desc
            .vertex_attributes
            .iter()
            .zip(&semantic_names)
            .map(|(attribute, name)| D3D12_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(name.as_ptr()),
                SemanticIndex: 0,
                Format: attribute.format,
                InputSlot: 0,
                AlignedByteOffset: attribute.offset,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            })
            .collect();

        if desc.color_formats.is_empty() || desc.color_formats.len() > 8 {
            return Err(Error::InvalidState(
                "render pipelines need between 1 and 8 color targets",
            ));
        }
        let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
        for (destination, format) in rtv_formats.iter_mut().zip(desc.color_formats) {
            *destination = *format;
        }
        let pipeline_desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: borrow_interface(&root_signature.raw),
            VS: shader_bytecode(desc.vertex_dxil),
            PS: shader_bytecode(desc.pixel_dxil),
            DS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: ptr::null(),
                BytecodeLength: 0,
            },
            HS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: ptr::null(),
                BytecodeLength: 0,
            },
            GS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: ptr::null(),
                BytecodeLength: 0,
            },
            StreamOutput: D3D12_STREAM_OUTPUT_DESC {
                pSODeclaration: ptr::null(),
                NumEntries: 0,
                pBufferStrides: ptr::null(),
                NumStrides: 0,
                RasterizedStream: 0,
            },
            BlendState: blend,
            SampleMask: u32::MAX,
            RasterizerState: rasterizer,
            DepthStencilState: depth_stencil,
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: input_elements.as_ptr(),
                NumElements: input_elements.len() as u32,
            },
            IBStripCutValue: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: desc.color_formats.len() as u32,
            RTVFormats: rtv_formats,
            DSVFormat: desc.depth_format.unwrap_or(DXGI_FORMAT_UNKNOWN),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            NodeMask: 0,
            CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
            Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
        };
        let raw: ID3D12PipelineState =
            unsafe { self.raw.CreateGraphicsPipelineState(&pipeline_desc)? };
        Ok(RenderPipeline {
            raw,
            root_signature: RootSignature {
                raw: root_signature.raw.clone(),
            },
        })
    }

    /// Builds a compute pipeline state from a DXIL blob and its root signature.
    pub fn create_compute_pipeline(
        &self,
        dxil: &[u8],
        root_signature: &RootSignature,
    ) -> Result<ComputePipeline, Error> {
        if dxil.is_empty() {
            return Err(Error::InvalidState("compute shader bytecode is empty"));
        }
        let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: borrow_interface(&root_signature.raw),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: dxil.as_ptr() as *const core::ffi::c_void,
                BytecodeLength: dxil.len(),
            },
            NodeMask: 0,
            CachedPSO: D3D12_CACHED_PIPELINE_STATE::default(),
            Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
        };
        let raw: ID3D12PipelineState = unsafe { self.raw.CreateComputePipelineState(&desc)? };
        Ok(ComputePipeline {
            raw,
            root_signature: RootSignature {
                raw: root_signature.raw.clone(),
            },
        })
    }
}

impl CommandEncoder {
    /// Binds a shader-visible descriptor heap for the root tables that follow.
    pub fn set_descriptor_heap(&mut self, heap: &DescriptorHeap) {
        unsafe {
            self.list.SetDescriptorHeaps(&[Some(heap.heap.clone())]);
        }
    }

    /// Binds a compute pipeline; its root signature is set along with it.
    pub fn set_compute_pipeline(&mut self, pipeline: &ComputePipeline) {
        unsafe {
            self.list
                .SetComputeRootSignature(&pipeline.root_signature.raw);
            self.list.SetPipelineState(&pipeline.raw);
        }
    }

    /// Binds a descriptor table of `heap` to root parameter `root_index`.
    pub fn set_compute_descriptor_table(
        &mut self,
        root_index: u32,
        heap: &DescriptorHeap,
        range: DescriptorRange,
    ) {
        unsafe {
            self.list
                .SetComputeRootDescriptorTable(root_index, heap.range_gpu_handle(range));
        }
    }

    /// Records a compute dispatch of `groups` thread groups.
    pub fn dispatch(&mut self, groups: [u32; 3]) {
        unsafe {
            self.list.Dispatch(groups[0], groups[1], groups[2]);
        }
    }

    /// Records a UAV barrier so UAV writes are visible to later reads/copies.
    pub fn uav_barrier(&mut self, buffer: &Buffer) {
        let barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: borrow_interface(&buffer.resource),
                }),
            },
        };
        unsafe {
            self.list.ResourceBarrier(&[barrier]);
        }
    }
}

impl CommandEncoder {
    /// Binds color render targets (in `SV_Target` order) and no depth buffer.
    pub fn set_render_targets(&mut self, heap: &DescriptorHeap, targets: &[DescriptorRange]) {
        let handles: Vec<D3D12_CPU_DESCRIPTOR_HANDLE> = targets
            .iter()
            .map(|target| heap.cpu_handle(target.start))
            .collect();
        unsafe {
            self.list
                .OMSetRenderTargets(handles.len() as u32, Some(handles.as_ptr()), false, None);
        }
    }

    /// Binds color render targets (in `SV_Target` order) plus a depth-stencil
    /// target.
    pub fn set_render_targets_with_depth(
        &mut self,
        color_heap: &DescriptorHeap,
        colors: &[DescriptorRange],
        depth_heap: &DescriptorHeap,
        depth: DescriptorRange,
    ) {
        let color_handles: Vec<D3D12_CPU_DESCRIPTOR_HANDLE> = colors
            .iter()
            .map(|color| color_heap.cpu_handle(color.start))
            .collect();
        let depth_handle = depth_heap.cpu_handle(depth.start);
        unsafe {
            self.list.OMSetRenderTargets(
                color_handles.len() as u32,
                Some(color_handles.as_ptr()),
                false,
                Some(&depth_handle),
            );
        }
    }

    /// Clears a depth-stencil target to `depth` (stencil is left untouched).
    pub fn clear_depth_stencil(
        &mut self,
        heap: &DescriptorHeap,
        depth: DescriptorRange,
        depth_value: f32,
    ) {
        unsafe {
            self.list.ClearDepthStencilView(
                heap.cpu_handle(depth.start),
                D3D12_CLEAR_FLAG_DEPTH,
                depth_value,
                0,
                None,
            );
        }
    }

    /// Binds a vertex buffer to input slot 0.
    pub fn set_vertex_buffer(&mut self, buffer: &Buffer, stride: u32) {
        let view = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: buffer.gpu_address(),
            SizeInBytes: buffer.size() as u32,
            StrideInBytes: stride,
        };
        unsafe {
            self.list.IASetVertexBuffers(0, Some(&[view]));
        }
    }

    /// Binds an index buffer. `format` is usually
    /// [`DXGI_FORMAT_R16_UINT`](crate::d3d12::DXGI_FORMAT_R16_UINT) or
    /// `DXGI_FORMAT_R32_UINT`.
    pub fn set_index_buffer(&mut self, buffer: &Buffer, format: DXGI_FORMAT) {
        let view = D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: buffer.gpu_address(),
            SizeInBytes: buffer.size() as u32,
            Format: format,
        };
        unsafe {
            self.list.IASetIndexBuffer(Some(&view));
        }
    }

    /// Draws `index_count` indices, one instance.
    pub fn draw_indexed(&mut self, index_count: u32) {
        unsafe {
            self.list.DrawIndexedInstanced(index_count, 1, 0, 0, 0);
        }
    }

    /// Clears a render target to `color`.
    pub fn clear_render_target(
        &mut self,
        heap: &DescriptorHeap,
        rtv: DescriptorRange,
        color: [f32; 4],
    ) {
        unsafe {
            self.list
                .ClearRenderTargetView(heap.cpu_handle(rtv.start), &color, None);
        }
    }

    /// Sets a full-surface viewport starting at the origin.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        let viewport = D3D12_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: width,
            Height: height,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            self.list.RSSetViewports(&[viewport]);
        }
    }

    /// Sets a full-surface scissor rectangle starting at the origin.
    pub fn set_scissor(&mut self, width: i32, height: i32) {
        let rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        unsafe {
            self.list.RSSetScissorRects(&[rect]);
        }
    }

    /// Binds a render pipeline: root signature, PSO and triangle topology.
    pub fn set_render_pipeline(&mut self, pipeline: &RenderPipeline) {
        unsafe {
            self.list
                .SetGraphicsRootSignature(&pipeline.root_signature.raw);
            self.list.SetPipelineState(&pipeline.raw);
            self.list
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        }
    }

    /// Binds a descriptor table of `heap` to graphics root parameter
    /// `root_index`.
    pub fn set_graphics_descriptor_table(
        &mut self,
        root_index: u32,
        heap: &DescriptorHeap,
        range: DescriptorRange,
    ) {
        unsafe {
            self.list
                .SetGraphicsRootDescriptorTable(root_index, heap.range_gpu_handle(range));
        }
    }

    /// Uploads 32-bit root constants to graphics root parameter `root_index`.
    pub fn set_graphics_root_constants(&mut self, root_index: u32, values: &[u32]) {
        unsafe {
            self.list.SetGraphicsRoot32BitConstants(
                root_index,
                values.len() as u32,
                values.as_ptr() as *const core::ffi::c_void,
                0,
            );
        }
    }

    /// Draws `vertex_count` vertices per instance for `instance_count`
    /// instances.
    pub fn draw_instanced(&mut self, vertex_count: u32, instance_count: u32) {
        unsafe {
            self.list.DrawInstanced(vertex_count, instance_count, 0, 0);
        }
    }
}

fn blob_to_string(blob: &ID3DBlob) -> String {
    let data = unsafe { blob.GetBufferPointer() } as *const u8;
    let size = unsafe { blob.GetBufferSize() };
    if data.is_null() || size == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, size) };
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

/// Result of [`Swapchain::present`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentStatus {
    /// The frame was queued for display.
    Presented,
    /// `DXGI_STATUS_OCCLUDED`: the window is not visible; skip rendering until
    /// it is.
    Occluded,
}

/// `DXGI_STATUS_OCCLUDED` is a success code and is not exposed by the `windows`
/// crate, so spell it out.
const DXGI_STATUS_OCCLUDED: HRESULT = HRESULT(0x087A0001);

/// A flip-model DXGI swapchain plus its back-buffer render targets.
///
/// The back buffers are exposed as [`Texture`]s so the rest of the device layer
/// treats them like any other render target. Expected frame loop:
///
/// 1. `acquire()` waits on the frame-latency handle and returns the frame index.
/// 2. Transition the back buffer to [`ResourceState::RenderTarget`] and render.
/// 3. Transition it back to [`ResourceState::Present`].
/// 4. `present()`.
pub struct Swapchain {
    raw: IDXGISwapChain4,
    format: DXGI_FORMAT,
    size: (u32, u32),
    waitable: HANDLE,
    rtv_heap: DescriptorHeap,
    rtvs: Vec<DescriptorRange>,
    buffers: Vec<Texture>,
}

/// A frame handed out by [`Swapchain::acquire`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    index: u32,
}

impl Frame {
    pub fn index(self) -> u32 {
        self.index
    }
}

impl Device {
    /// Creates a flip-model swapchain for `window` and render-target views for
    /// its back buffers.
    ///
    /// Frames are paced by `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT`
    /// with a maximum latency of 2, and `Present` uses sync interval 1.
    pub fn create_swapchain(
        &self,
        factory: &IDXGIFactory6,
        window: HWND,
        size: (u32, u32),
        format: DXGI_FORMAT,
    ) -> Result<Swapchain, Error> {
        if size.0 == 0 || size.1 == 0 {
            return Err(Error::InvalidState("swapchain size must be non-zero"));
        }
        let desc = swapchain_desc(size, format);
        let chain: IDXGISwapChain1 = unsafe {
            factory.CreateSwapChainForHwnd(
                &self.queue,
                window,
                &desc,
                None,
                None::<&IDXGIOutput>,
            )?
        };
        self.finish_swapchain(chain, size, format)
    }

    /// Creates a composition swapchain (`CreateSwapChainForComposition`).
    ///
    /// Unlike an HWND swapchain, this one does not need a desktop session, so
    /// the swapchain code path can be tested headlessly. Bind the swapchain to
    /// a DirectComposition visual to actually display it.
    pub fn create_swapchain_for_composition(
        &self,
        factory: &IDXGIFactory6,
        size: (u32, u32),
        format: DXGI_FORMAT,
    ) -> Result<Swapchain, Error> {
        if size.0 == 0 || size.1 == 0 {
            return Err(Error::InvalidState("swapchain size must be non-zero"));
        }
        let desc = swapchain_desc(size, format);
        let chain: IDXGISwapChain1 = unsafe {
            factory.CreateSwapChainForComposition(&self.queue, &desc, None::<&IDXGIOutput>)?
        };
        self.finish_swapchain(chain, size, format)
    }

    fn finish_swapchain(
        &self,
        chain: IDXGISwapChain1,
        size: (u32, u32),
        format: DXGI_FORMAT,
    ) -> Result<Swapchain, Error> {
        let chain2: IDXGISwapChain2 = chain.cast()?;
        unsafe { chain2.SetMaximumFrameLatency(2)? };
        let waitable = unsafe { chain2.GetFrameLatencyWaitableObject() };

        let mut swapchain = Swapchain {
            raw: chain.cast()?,
            format,
            size,
            waitable,
            rtv_heap: DescriptorHeap::new(self, DescriptorHeapType::Rtv, 8)?,
            rtvs: Vec::new(),
            buffers: Vec::new(),
        };
        swapchain.build_back_buffers(self)?;
        Ok(swapchain)
    }
}

/// Describes a 2-buffer flip-discard swapchain with frame-latency pacing.
fn swapchain_desc(size: (u32, u32), format: DXGI_FORMAT) -> DXGI_SWAP_CHAIN_DESC1 {
    DXGI_SWAP_CHAIN_DESC1 {
        Width: size.0,
        Height: size.1,
        Format: format,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
    }
}

impl Swapchain {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn format(&self) -> DXGI_FORMAT {
        self.format
    }

    /// The heap holding the back-buffer render-target views.
    pub fn rtv_heap(&self) -> &DescriptorHeap {
        &self.rtv_heap
    }

    /// The frame-latency waitable handle, for callers that wait themselves.
    pub fn frame_latency_waitable(&self) -> HANDLE {
        self.waitable
    }

    /// Waits for the frame-latency handle and returns the back buffer to render
    /// into.
    pub fn acquire(&mut self) -> Result<Frame, Error> {
        self.wait_for_frame_latency()?;
        let index = unsafe { self.raw.GetCurrentBackBufferIndex() };
        Ok(Frame { index })
    }

    /// Waits for the frame-latency handle with a bounded timeout: a hidden or
    /// stalled presentation engine must not hang the caller forever.
    fn wait_for_frame_latency(&self) -> Result<(), Error> {
        const TIMEOUT_MS: u32 = 5_000;
        let result = unsafe { WaitForSingleObject(self.waitable, TIMEOUT_MS) };
        if result != WAIT_OBJECT_0 {
            return Err(Error::InvalidState(
                "timed out waiting for the frame-latency handle",
            ));
        }
        Ok(())
    }

    /// The texture of frame `frame`.
    pub fn texture(&self, frame: Frame) -> &Texture {
        &self.buffers[frame.index as usize]
    }

    /// The render-target view of frame `frame`.
    pub fn render_target(&self, frame: Frame) -> DescriptorRange {
        self.rtvs[frame.index as usize]
    }

    /// Presents the frame in `frame`.
    ///
    /// The back buffer must already be transitioned back to
    /// [`ResourceState::Present`].
    pub fn present(&mut self) -> Result<PresentStatus, Error> {
        let result = unsafe { self.raw.Present(1, DXGI_PRESENT(0)) };
        if result == DXGI_STATUS_OCCLUDED {
            return Ok(PresentStatus::Occluded);
        }
        result.ok()?;
        Ok(PresentStatus::Presented)
    }

    /// Resizes the swapchain's back buffers and recreates their views.
    ///
    /// DXGI refuses to resize while the GPU or the presentation engine still
    /// holds references to the back buffers, so this drains the queue and waits
    /// on the frame-latency handle before calling `ResizeBuffers`.
    pub fn resize(&mut self, device: &Device, size: (u32, u32)) -> Result<(), Error> {
        if size.0 == 0 || size.1 == 0 {
            return Err(Error::InvalidState("swapchain size must be non-zero"));
        }
        device.wait_idle()?;
        // Wait for the last presented frame to be consumed by the presentation
        // engine; otherwise ResizeBuffers reports DXGI_ERROR_INVALID_CALL.
        // A hidden window never consumes frames, so surface a timeout instead
        // of blocking forever.
        self.wait_for_frame_latency()?;
        // Every view and reference has to go before DXGI can resize.
        self.rtvs.clear();
        self.buffers.clear();
        self.rtv_heap.reset();
        let resize_result = unsafe {
            self.raw.ResizeBuffers(
                0,
                size.0,
                size.1,
                self.format,
                DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
            )
        };
        if let Err(err) = &resize_result {
            // Surface why DXGI refused: usually an outstanding back-buffer
            // reference, which is much easier to find with the debug layer on.
            let _ = device.report_live_objects();
            for message in device.debug_messages() {
                log::error!("D3D12: {message}");
            }
            for message in device.dxgi_debug_messages() {
                log::error!("DXGI: {message}");
            }
            log::error!("D3D12: ResizeBuffers to {size:?} failed: {err}");
        }
        resize_result?;
        self.size = size;
        self.build_back_buffers(device)
    }

    fn build_back_buffers(&mut self, device: &Device) -> Result<(), Error> {
        let count = unsafe { self.raw.GetDesc1()? }.BufferCount;
        for index in 0..count {
            let resource: ID3D12Resource = unsafe { self.raw.GetBuffer(index)? };
            let texture = Texture {
                resource,
                desc: TextureDesc {
                    width: self.size.0,
                    height: self.size.1,
                    mip_levels: 1,
                    format: self.format,
                    render_target: true,
                    depth_stencil: false,
                },
                state: Cell::new(ResourceState::Present),
            };
            let rtv = device.create_render_target_view(&self.rtv_heap, &texture)?;
            self.buffers.push(texture);
            self.rtvs.push(rtv);
        }
        Ok(())
    }
}

/// Result of a successful [`init`] call.
pub struct Init {
    pub factory: IDXGIFactory6,
    /// `None` for a WARP device created without enumerating adapters.
    pub adapter: Option<IDXGIAdapter1>,
    pub device: Device,
    pub adapter_info: AdapterInfo,
}

/// Enumerates the hardware DXGI adapters, in GPU-preference order, and reports
/// which ones can host a feature level 12_0 D3D12 device.
pub fn enumerate_adapters() -> Result<Vec<AdapterInfo>, Error> {
    let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
    let mut adapters = Vec::new();
    let mut index = 0;
    loop {
        let adapter: IDXGIAdapter1 = match unsafe {
            factory.EnumAdapterByGpuPreference(index, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
        } {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(err) => return Err(err.into()),
        };
        index += 1;
        adapters.push(describe_adapter(&adapter)?);
    }
    if adapters.is_empty() {
        return Err(Error::NoAdapter);
    }
    Ok(adapters)
}

/// Creates a DXGI factory, a D3D12 device on the most preferred adapter, and
/// one direct command queue.
pub fn init() -> Result<Init, Error> {
    init_with_options(&InitOptions::default())
}

/// Like [`init`], but with explicit [`InitOptions`].
pub fn init_with_options(options: &InitOptions) -> Result<Init, Error> {
    // Enable the debug layer once, before any device is created, and remember
    // whether it actually came up: `Device::has_debug_layer` must not lie.
    let debug_layer = options.enable_debug_layer && enable_debug_layer();

    // A debug factory feeds the DXGI info queue; fall back to a normal factory
    // when the debug interface is not installed.
    let factory: IDXGIFactory6 = if debug_layer {
        match unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_DEBUG) } {
            Ok(factory) => factory,
            Err(err) => {
                log::warn!("D3D12: debug DXGI factory unavailable ({err})");
                unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? }
            }
        }
    } else {
        unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? }
    };

    if options.force_warp {
        return init_warp(factory, debug_layer);
    }

    let mut saw_any_adapter = false;
    let mut index = 0;
    loop {
        let adapter: IDXGIAdapter1 =
            match unsafe { factory.EnumAdapterByGpuPreference(index, options.preference) } {
                Ok(adapter) => adapter,
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(err) => return Err(err.into()),
            };
        index += 1;
        saw_any_adapter = true;

        // Skip adapters that cannot host a D3D12 device of the minimum level.
        let raw = match create_d3d12_device(Some(&adapter)) {
            Ok(raw) => raw,
            Err(err) => {
                log::debug!("D3D12: adapter {index} rejected: {err}");
                continue;
            }
        };

        let adapter_info = describe_adapter(&adapter)?;
        let device = Device::new(raw, debug_layer)?;
        log_init(&adapter_info, device.node_count());
        return Ok(Init {
            factory,
            adapter: Some(adapter),
            device,
            adapter_info,
        });
    }

    if options.warp_fallback {
        log::warn!("D3D12: no hardware adapter accepted, falling back to WARP");
        return init_warp(factory, debug_layer);
    }

    if saw_any_adapter {
        Err(Error::NoCompatibleDevice)
    } else {
        Err(Error::NoAdapter)
    }
}

/// Enables the D3D12 debug layer. Returns whether it is actually active.
fn enable_debug_layer() -> bool {
    let mut debug: Option<ID3D12Debug> = None;
    match unsafe { D3D12GetDebugInterface(&mut debug) } {
        Ok(()) => {
            let Some(debug) = debug else {
                log::warn!("D3D12: D3D12GetDebugInterface returned no debug interface");
                return false;
            };
            unsafe { debug.EnableDebugLayer() };
            log::info!("D3D12: debug layer enabled");
            true
        }
        Err(err) => {
            log::warn!(
                "D3D12: debug layer requested but unavailable ({err}); \
                 install the Windows \"Graphics Tools\" optional feature"
            );
            false
        }
    }
}

/// Creates the WARP (software) device for headless testing.
///
/// WARP is normally created by passing a null adapter to `D3D12CreateDevice`.
/// Creating it from the enumerated WARP adapter instead keeps `Init::adapter`
/// populated and reports the adapter's real description, which matters for the
/// same diagnostics and capability queries the hardware path uses.
fn init_warp(factory: IDXGIFactory6, debug_layer: bool) -> Result<Init, Error> {
    let adapter = find_software_adapter(&factory)?;
    let (raw, adapter_info) = match &adapter {
        Some(adapter) => {
            let raw = create_d3d12_device(Some(adapter)).or_else(|_| create_d3d12_device(None))?;
            (raw, describe_adapter(adapter)?)
        }
        None => {
            let raw = create_d3d12_device(None)?;
            let adapter_info =
                describe_device_adapter(&raw).unwrap_or_else(|_| warp_fallback_info());
            (raw, adapter_info)
        }
    };

    let device = Device::new(raw, debug_layer)?;
    log_init(&adapter_info, device.node_count());
    Ok(Init {
        factory,
        adapter,
        device,
        adapter_info,
    })
}

fn create_d3d12_device(adapter: Option<&IDXGIAdapter1>) -> Result<ID3D12Device, Error> {
    let mut device: Option<ID3D12Device> = None;
    unsafe {
        match adapter {
            Some(adapter) => D3D12CreateDevice(adapter, MIN_FEATURE_LEVEL, &mut device)?,
            None => D3D12CreateDevice(
                None::<&windows::core::IUnknown>,
                MIN_FEATURE_LEVEL,
                &mut device,
            )?,
        }
    }
    device.ok_or(Error::NoCompatibleDevice)
}

/// Finds the WARP adapter in the factory's adapter list.
fn find_software_adapter(factory: &IDXGIFactory6) -> Result<Option<IDXGIAdapter1>, Error> {
    let mut index = 0;
    loop {
        let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        index += 1;
        if adapter_is_software(&adapter) {
            return Ok(Some(adapter));
        }
    }
}

fn adapter_is_software(adapter: &IDXGIAdapter1) -> bool {
    match unsafe { adapter.GetDesc1() } {
        Ok(desc) => desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
        Err(_) => false,
    }
}

fn warp_fallback_info() -> AdapterInfo {
    AdapterInfo {
        name: "WARP (Microsoft Basic Render Driver)".to_string(),
        vendor_id: 0x1414,
        device_id: 0x008C,
        dedicated_video_memory: 0,
        shared_system_memory: 0,
        is_software: true,
    }
}

fn log_init(adapter_info: &AdapterInfo, node_count: u32) {
    log::info!("D3D12 device initialized: {adapter_info}, nodes: {node_count}");
}

fn describe_adapter(adapter: &IDXGIAdapter1) -> Result<AdapterInfo, Error> {
    let desc = unsafe { adapter.GetDesc1() }?;
    let name_len = desc
        .Description
        .iter()
        .position(|&ch| ch == 0)
        .unwrap_or(desc.Description.len());
    Ok(AdapterInfo {
        name: String::from_utf16_lossy(&desc.Description[..name_len]),
        vendor_id: desc.VendorId,
        device_id: desc.DeviceId,
        dedicated_video_memory: desc.DedicatedVideoMemory as u64,
        shared_system_memory: desc.SharedSystemMemory as u64,
        is_software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
    })
}

fn describe_device_adapter(device: &ID3D12Device) -> Result<AdapterInfo, Error> {
    let dxgi_device: IDXGIDevice = device.cast()?;
    let adapter = unsafe { dxgi_device.GetAdapter()? };
    let adapter: IDXGIAdapter1 = adapter.cast()?;
    describe_adapter(&adapter)
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

/// A non-owning copy of a COM interface for `ManuallyDrop` struct fields.
///
/// The D3D12 structs hold resources and interfaces in `ManuallyDrop` because the
/// runtime only borrows them for the duration of the call. Cloning them would
/// `AddRef`, and because the field is never dropped (by design) that reference
/// would leak. Leaking a reference to a swapchain back buffer is exactly what
/// makes `ResizeBuffers` fail with `DXGI_ERROR_INVALID_CALL`, so pass a borrowed
/// copy and let the caller's own reference keep the object alive.
fn borrow_interface<T: Interface>(value: &T) -> std::mem::ManuallyDrop<Option<T>> {
    // Bitwise copy of the interface wrapper without `AddRef`; `ManuallyDrop`
    // keeps the copy from calling `Release`, so the count stays unchanged.
    std::mem::ManuallyDrop::new(Some(unsafe { std::ptr::read(value) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_headless() -> Init {
        init_with_options(&InitOptions::headless()).expect("WARP device should initialize")
    }

    #[test]
    fn enumerate_adapters_reports_at_least_one() {
        // Adapter enumeration must work even when device creation does not.
        match enumerate_adapters() {
            Ok(adapters) => assert!(!adapters.is_empty()),
            Err(Error::NoAdapter) => {} // acceptable in a GPU-less environment
            Err(err) => panic!("adapter enumeration failed: {err}"),
        }
    }

    #[test]
    fn warp_device_initializes() {
        let init = init_headless();
        assert!(
            init.adapter_info.is_software,
            "WARP should report a software adapter"
        );
        assert_eq!(init.device.node_count(), 1);
    }

    #[test]
    fn copy_roundtrip_through_device_memory() {
        let init = init_headless();
        let device = &init.device;

        let payload: Vec<u8> = (0..4096).map(|i| ((i * 7 + 3) % 251) as u8).collect();
        let upload = device
            .create_buffer(BufferDesc::new(payload.len() as u64, Memory::Upload))
            .expect("upload buffer");
        upload.write(0, &payload).expect("write staging data");

        let gpu = device
            .create_buffer(BufferDesc::new(payload.len() as u64, Memory::Device))
            .expect("device buffer");
        let readback = device
            .create_buffer(BufferDesc::new(payload.len() as u64, Memory::Readback))
            .expect("readback buffer");

        let mut encoder = device.record().expect("command encoder");
        // staging -> device memory -> staging back to the CPU.
        encoder.transition_buffer(&gpu, ResourceState::CopyDest);
        encoder.copy_buffer(&gpu, 0, &upload, 0, payload.len() as u64);
        encoder.transition_buffer(&gpu, ResourceState::CopySource);
        encoder.copy_buffer(&readback, 0, &gpu, 0, payload.len() as u64);
        encoder.transition_buffer(&gpu, ResourceState::Common);

        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("fence wait");

        let result = readback.read().expect("read back");
        assert_eq!(&result[..payload.len()], &payload[..]);
    }

    #[test]
    fn texture_upload_and_readback() {
        let init = init_headless();
        let device = &init.device;
        let (width, height) = (64u32, 32u32);
        let texture = device
            .create_texture(TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM))
            .expect("texture");
        let payload: Vec<u8> = (0..width * height)
            .flat_map(|i| [(i % 251) as u8, 0x10, 0x20, 0xFF])
            .collect();
        device
            .upload_texture(&texture, &payload)
            .expect("texture upload");
        assert_eq!(texture.state(), ResourceState::ShaderRead);
        let readback = device.read_texture(&texture).expect("texture readback");
        assert_eq!(readback, payload);
    }

    #[test]
    fn descriptor_heaps_and_views() {
        let init = init_headless();
        let device = &init.device;
        let view_heap =
            DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 8).expect("view heap");
        let sampler_heap =
            DescriptorHeap::new(device, DescriptorHeapType::Sampler, 4).expect("sampler heap");

        let buffer = device
            .create_buffer(BufferDesc::new(1024, Memory::Device).with_unordered_access())
            .expect("buffer");
        let srv = device
            .create_buffer_srv_typed(&view_heap, &buffer, DXGI_FORMAT_R32_FLOAT)
            .expect("buffer srv");
        let uav = device
            .create_buffer_uav_typed(&view_heap, &buffer, DXGI_FORMAT_R32_FLOAT)
            .expect("buffer uav");
        let cbv = device
            .create_buffer_cbv(&view_heap, &buffer, 0, 1024)
            .expect("buffer cbv");
        let texture = device
            .create_texture(TextureDesc::new(16, 16, DXGI_FORMAT_R8G8B8A8_UNORM))
            .expect("texture");
        let texture_srv = device
            .create_texture_srv(&view_heap, &texture)
            .expect("texture srv");
        let sampler = device
            .create_sampler(&sampler_heap, &SamplerDesc::default())
            .expect("sampler");

        // Every view got its own descriptor slot.
        let mut starts: Vec<u32> = [srv, uav, cbv, texture_srv]
            .iter()
            .map(|range| range.start)
            .collect();
        starts.sort_unstable();
        starts.dedup();
        assert_eq!(starts.len(), 4);
        for range in [srv, uav, cbv, texture_srv] {
            assert_ne!(view_heap.cpu_handle(range.start).ptr, 0);
            assert_ne!(view_heap.gpu_handle(range.start).ptr, 0);
        }
        assert_eq!(view_heap.allocated(), 4);
        assert_eq!(sampler_heap.allocated(), 1);
        assert_eq!(sampler_heap.cpu_handle(sampler.start).ptr != 0, true);
        assert_eq!(view_heap.remaining(), 4);

        // Exhausting the heap fails instead of wrapping.
        assert!(view_heap.allocate(4).is_some());
        assert!(view_heap.allocate(1).is_none());
        // A UAV view for a buffer without the UAV flag is rejected up front.
        let plain = device
            .create_buffer(BufferDesc::new(256, Memory::Device))
            .expect("plain buffer");
        assert!(
            device
                .create_buffer_uav_typed(&view_heap, &plain, DXGI_FORMAT_R32_FLOAT)
                .is_err()
        );
        device.check_device().expect("device still alive");
        // A sampler cannot be created in a CBV/SRV/UAV heap.
        assert!(
            device
                .create_sampler(&view_heap, &SamplerDesc::default())
                .is_err()
        );
    }

    #[test]
    fn dxc_compiles_compute_shader() {
        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                // DXC ships out of band; tests skip when it is not installed.
                eprintln!("skipping DXC test: {err}");
                return;
            }
        };

        let dxil = compiler
            .compile_hlsl(shader::TRIVIAL_COMPUTE_HLSL, "main", "cs_6_0")
            .expect("compile the trivial compute shader");
        assert!(dxil.len() > 4, "DXIL blob is suspiciously small");
        assert_eq!(&dxil[..4], b"DXBC", "DXIL container magic missing");

        // Broken HLSL reports the diagnostics instead of an opaque error.
        let err = compiler
            .compile_hlsl("this is not hlsl", "main", "cs_6_0")
            .expect_err("invalid HLSL must not compile");
        match err {
            Error::ShaderCompile(diagnostics) => {
                assert!(!diagnostics.is_empty(), "compile error had no diagnostics")
            }
            other => panic!("expected Error::ShaderCompile, got {other:?}"),
        }
    }

    #[test]
    fn compute_shader_writes_uav_buffer() {
        let init = init_headless();
        let device = &init.device;
        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                eprintln!("skipping compute test: {err}");
                return;
            }
        };

        let dxil = compiler
            .compile_hlsl(shader::TRIVIAL_COMPUTE_HLSL, "main", "cs_6_0")
            .expect("compile compute shader");
        let root = device
            .create_compute_root_signature(&[RootDescriptorTable {
                kind: DescriptorRangeKind::Uav,
                count: 1,
                base_shader_register: 0,
            }])
            .expect("root signature");
        let pipeline = device
            .create_compute_pipeline(&dxil, &root)
            .expect("compute pipeline");

        let count = 64u32;
        let byte_size = count as u64 * 4;
        let gpu = device
            .create_buffer(BufferDesc::new(byte_size, Memory::Device).with_unordered_access())
            .expect("uav buffer");
        let readback = device
            .create_buffer(BufferDesc::new(byte_size, Memory::Readback))
            .expect("readback buffer");
        let heap = DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 4).expect("heap");
        let uav = device
            .create_buffer_uav_structured(&heap, &gpu, 4)
            .expect("uav view");

        let mut encoder = device.record().expect("encoder");
        encoder.transition_buffer(&gpu, ResourceState::UnorderedAccess);
        encoder.set_descriptor_heap(&heap);
        encoder.set_compute_pipeline(&pipeline);
        encoder.set_compute_descriptor_table(0, &heap, uav);
        encoder.dispatch([1, 1, 1]);
        encoder.uav_barrier(&gpu);
        encoder.transition_buffer(&gpu, ResourceState::CopySource);
        encoder.copy_buffer(&readback, 0, &gpu, 0, byte_size);
        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("wait");
        device.check_device().expect("device alive after dispatch");

        let bytes = readback.read().expect("readback");
        let values: Vec<u32> = bytes[..byte_size as usize]
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        assert_eq!(values, (0..count).collect::<Vec<_>>());
    }

    #[test]
    fn render_pipeline_draws_fullscreen_triangle() {
        let init = init_headless();
        let device = &init.device;
        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                eprintln!("skipping render test: {err}");
                return;
            }
        };

        let vs = compiler
            .compile_hlsl(shader::FULLSCREEN_HLSL, "vs_main", "vs_6_0")
            .expect("compile vertex shader");
        let ps = compiler
            .compile_hlsl(shader::FULLSCREEN_HLSL, "ps_main", "ps_6_0")
            .expect("compile pixel shader");
        let root = device
            .create_graphics_root_signature(&[RootParameter::Constants {
                shader_register: 0,
                count: 4,
            }])
            .expect("graphics root signature");
        let pipeline = device
            .create_render_pipeline(
                &RenderPipelineDesc {
                    vertex_dxil: &vs,
                    pixel_dxil: &ps,
                    color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
                    vertex_attributes: &[],
                    depth_format: None,
                },
                &root,
            )
            .expect("render pipeline");

        let (width, height) = (64u32, 32u32);
        let texture = device
            .create_texture(
                TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
            )
            .expect("render target texture");
        let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4).expect("rtv heap");
        let rtv = device
            .create_render_target_view(&rtv_heap, &texture)
            .expect("rtv");

        let green = [0.0f32, 1.0, 0.0, 1.0];
        let green_bits: Vec<u32> = green.iter().map(|channel| channel.to_bits()).collect();

        let mut encoder = device.record().expect("encoder");
        encoder.transition_texture(&texture, ResourceState::RenderTarget);
        encoder.set_render_targets(&rtv_heap, &[rtv]);
        encoder.clear_render_target(&rtv_heap, rtv, [0.0, 0.0, 1.0, 1.0]);
        encoder.set_render_pipeline(&pipeline);
        encoder.set_viewport(width as f32, height as f32);
        encoder.set_scissor(width as i32, height as i32);
        encoder.set_graphics_root_constants(0, &green_bits);
        encoder.draw_instanced(3, 1);
        encoder.transition_texture(&texture, ResourceState::CopySource);
        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("wait");
        device.check_device().expect("device alive after draw");

        let pixels = device.read_texture(&texture).expect("readback");
        let wrong = pixels
            .chunks_exact(4)
            .filter(|pixel| *pixel != [0u8, 255, 0, 255])
            .count();
        assert_eq!(wrong, 0, "{wrong} pixels were not the drawn green");
    }

    fn exercise_swapchain(device: &Device, swapchain: &mut Swapchain) {
        let frame = swapchain.acquire().expect("acquire");
        let texture = swapchain.texture(frame);
        let rtv = swapchain.render_target(frame);
        let mut encoder = device.record().expect("encoder");
        encoder.transition_texture(texture, ResourceState::RenderTarget);
        encoder.clear_render_target(swapchain.rtv_heap(), rtv, [1.0, 0.0, 0.0, 1.0]);
        encoder.transition_texture(texture, ResourceState::CopySource);
        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("wait");

        let pixels = device.read_texture(texture).expect("back-buffer readback");
        assert_eq!(&pixels[..4], &[255, 0, 0, 255], "back buffer clear color");

        let mut encoder = device.record().expect("encoder");
        encoder.transition_texture(texture, ResourceState::Present);
        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("wait");
        let status = swapchain.present().expect("present");
        assert!(matches!(
            status,
            PresentStatus::Presented | PresentStatus::Occluded
        ));
        device.check_device().expect("device alive after present");
    }

    /// `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE`: HWND swapchains need an
    /// interactive desktop session, which SSH sessions do not provide.
    fn is_dxgi_not_currently_available(err: &Error) -> bool {
        matches!(err, Error::Windows(inner) if inner.code().0 == 0x887A0022u32 as i32)
    }

    #[test]
    fn swapchain_renders_presents_and_resizes() {
        let init = init_headless();
        let device = &init.device;

        // Prefer an HWND swapchain; SSH sessions cannot create one, so fall back
        // to a composition swapchain, which also exercises the flip/present path.
        let window = window::HiddenWindow::new("blade-d3d12-swapchain-test", 128, 64)
            .expect("hidden window");
        let mut swapchain = match device.create_swapchain(
            &init.factory,
            window.hwnd(),
            (128, 64),
            DXGI_FORMAT_R8G8B8A8_UNORM,
        ) {
            Ok(swapchain) => {
                // A hidden window never consumes frames, and DXGI then refuses
                // to resize; show it so the presentation engine runs.
                window.show();
                swapchain
            }
            Err(err) if is_dxgi_not_currently_available(&err) => {
                eprintln!("HWND swapchain unavailable ({err}); using composition swapchain");
                match device.create_swapchain_for_composition(
                    &init.factory,
                    (128, 64),
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                ) {
                    Ok(swapchain) => swapchain,
                    Err(err) => {
                        eprintln!("skipping swapchain test: {err}");
                        return;
                    }
                }
            }
            Err(err) => panic!("swapchain creation failed: {err}"),
        };
        assert_eq!(swapchain.size(), (128, 64));

        exercise_swapchain(device, &mut swapchain);

        // Resize recreates the back buffers and views; render and present again.
        swapchain.resize(device, (64, 32)).expect("resize");
        assert_eq!(swapchain.size(), (64, 32));
        exercise_swapchain(device, &mut swapchain);
        device
            .check_device()
            .expect("device alive after resize/present");
    }

    /// Renders with a vertex-buffer pipeline and returns the tightly packed
    /// pixels of a `width` x `height` RGBA8 target.
    fn render_mesh(
        device: &Device,
        pipeline: &RenderPipeline,
        depth: Option<(&Texture, DescriptorRange, &DescriptorHeap)>,
        rtv_heap: &DescriptorHeap,
        rtv: DescriptorRange,
        texture: &Texture,
        vertex_buffers: &[(&Buffer, u32)],
        clear: [f32; 4],
    ) -> Vec<u8> {
        let (width, height) = (texture.width(), texture.height());
        let mut encoder = device.record().expect("encoder");
        encoder.transition_texture(texture, ResourceState::RenderTarget);
        if let Some((depth_texture, depth_range, depth_heap)) = depth {
            encoder.transition_texture(depth_texture, ResourceState::DepthWrite);
            encoder.set_render_targets_with_depth(rtv_heap, &[rtv], depth_heap, depth_range);
            encoder.clear_depth_stencil(depth_heap, depth_range, 1.0);
        } else {
            encoder.set_render_targets(rtv_heap, &[rtv]);
        }
        encoder.clear_render_target(rtv_heap, rtv, clear);
        encoder.set_render_pipeline(pipeline);
        encoder.set_viewport(width as f32, height as f32);
        encoder.set_scissor(width as i32, height as i32);
        for (buffer, stride) in vertex_buffers {
            encoder.set_vertex_buffer(buffer, *stride);
            encoder.draw_instanced(3, 1);
        }
        encoder.transition_texture(texture, ResourceState::CopySource);
        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("wait");
        device.read_texture(texture).expect("readback")
    }

    #[test]
    fn vertex_buffer_input_layout_draws_triangle() {
        let init = init_headless();
        let device = &init.device;
        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                eprintln!("skipping mesh test: {err}");
                return;
            }
        };
        let vs = compiler
            .compile_hlsl(shader::MESH_HLSL, "vs_main", "vs_6_0")
            .expect("vertex shader");
        let ps = compiler
            .compile_hlsl(shader::MESH_HLSL, "ps_main", "ps_6_0")
            .expect("pixel shader");
        let root = device
            .create_graphics_root_signature(&[])
            .expect("root signature");
        let pipeline = device
            .create_render_pipeline(
                &RenderPipelineDesc {
                    vertex_dxil: &vs,
                    pixel_dxil: &ps,
                    color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
                    vertex_attributes: &[
                        VertexAttribute {
                            semantic: "POSITION",
                            format: DXGI_FORMAT_R32G32B32_FLOAT,
                            offset: 0,
                        },
                        VertexAttribute {
                            semantic: "COLOR",
                            format: DXGI_FORMAT_R32G32B32_FLOAT,
                            offset: 12,
                        },
                    ],
                    depth_format: None,
                },
                &root,
            )
            .expect("mesh pipeline");

        // Clip-space triangle: red bottom-left, green bottom-right, blue top.
        let vertices: [f32; 18] = [
            -0.5, -0.5, 0.0, 1.0, 0.0, 0.0, //
            0.5, -0.5, 0.0, 0.0, 1.0, 0.0, //
            0.0, 0.5, 0.0, 0.0, 0.0, 1.0,
        ];
        let vertex_bytes: Vec<u8> = vertices.iter().flat_map(|v| v.to_le_bytes()).collect();
        let vertex_buffer = device
            .create_buffer(BufferDesc::new(vertex_bytes.len() as u64, Memory::Upload))
            .expect("vertex buffer");
        vertex_buffer
            .write(0, &vertex_bytes)
            .expect("write vertices");

        let (width, height) = (64u32, 64u32);
        let texture = device
            .create_texture(
                TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
            )
            .expect("color target");
        let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4).expect("rtv heap");
        let rtv = device
            .create_render_target_view(&rtv_heap, &texture)
            .expect("rtv");

        let pixels = render_mesh(
            device,
            &pipeline,
            None,
            &rtv_heap,
            rtv,
            &texture,
            &[(&vertex_buffer, 24)],
            [0.0, 0.0, 0.0, 1.0],
        );

        // At the triangle centroid the three vertex colors average to a grey:
        // the triangle is (-0.5,-0.5), (0.5,-0.5), (0,0.5) in NDC, so its
        // centroid lands at (width/2, 7 * height / 12) in pixels.
        let centroid = ((7 * height / 12) * width + width / 2) as usize * 4;
        for (channel, value) in pixels[centroid..centroid + 3].iter().enumerate() {
            assert!(
                (i32::from(*value) - 85).abs() <= 8,
                "centroid channel {channel} = {value}, expected ~85"
            );
        }
        // A corner is outside the triangle: the clear color survives.
        assert_eq!(
            &pixels[..4],
            &[0, 0, 0, 255],
            "top-left pixel should be clear"
        );
        device.check_device().expect("device alive after mesh draw");
    }

    #[test]
    fn depth_test_keeps_near_triangle() {
        let init = init_headless();
        let device = &init.device;
        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                eprintln!("skipping depth test: {err}");
                return;
            }
        };
        let vs = compiler
            .compile_hlsl(shader::MESH_HLSL, "vs_main", "vs_6_0")
            .expect("vertex shader");
        let ps = compiler
            .compile_hlsl(shader::MESH_HLSL, "ps_main", "ps_6_0")
            .expect("pixel shader");
        let root = device
            .create_graphics_root_signature(&[])
            .expect("root signature");
        let attributes = [
            VertexAttribute {
                semantic: "POSITION",
                format: DXGI_FORMAT_R32G32B32_FLOAT,
                offset: 0,
            },
            VertexAttribute {
                semantic: "COLOR",
                format: DXGI_FORMAT_R32G32B32_FLOAT,
                offset: 12,
            },
        ];
        let with_depth = device
            .create_render_pipeline(
                &RenderPipelineDesc {
                    vertex_dxil: &vs,
                    pixel_dxil: &ps,
                    color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
                    vertex_attributes: &attributes,
                    depth_format: Some(DXGI_FORMAT_D32_FLOAT),
                },
                &root,
            )
            .expect("depth pipeline");
        let without_depth = device
            .create_render_pipeline(
                &RenderPipelineDesc {
                    vertex_dxil: &vs,
                    pixel_dxil: &ps,
                    color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
                    vertex_attributes: &attributes,
                    depth_format: None,
                },
                &root,
            )
            .expect("no-depth pipeline");

        // Two fullscreen triangles; the near one is red at z=0.25, the far one
        // green at z=0.75. Both use a vertex buffer each.
        let triangle = |z: f32, color: [f32; 3]| -> Vec<u8> {
            let positions = [(-1.0f32, 1.0f32), (3.0, 1.0), (-1.0, -3.0)];
            positions
                .iter()
                .flat_map(|(x, y)| [*x, *y, z, color[0], color[1], color[2]])
                .flat_map(|value| value.to_le_bytes())
                .collect()
        };
        let make_vertex_buffer = |data: Vec<u8>| {
            let buffer = device
                .create_buffer(BufferDesc::new(data.len() as u64, Memory::Upload))
                .expect("vertex buffer");
            buffer.write(0, &data).expect("write vertices");
            buffer
        };
        let near_vertices = make_vertex_buffer(triangle(0.25, [1.0, 0.0, 0.0]));
        let far_vertices = make_vertex_buffer(triangle(0.75, [0.0, 1.0, 0.0]));

        let (width, height) = (32u32, 32u32);
        let color_target = || {
            device
                .create_texture(
                    TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM)
                        .with_render_target(),
                )
                .expect("color target")
        };
        let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 8).expect("rtv heap");

        // With depth testing, drawing the near triangle first keeps it red even
        // after the far triangle is drawn.
        let texture = color_target();
        let rtv = device
            .create_render_target_view(&rtv_heap, &texture)
            .expect("rtv");
        let depth_texture = device
            .create_texture(
                TextureDesc::new(width, height, DXGI_FORMAT_D32_FLOAT).with_depth_stencil(),
            )
            .expect("depth target");
        let dsv_heap = DescriptorHeap::new(device, DescriptorHeapType::Dsv, 4).expect("dsv heap");
        let dsv = device
            .create_depth_stencil_view(&dsv_heap, &depth_texture)
            .expect("dsv");
        let pixels = render_mesh(
            device,
            &with_depth,
            Some((&depth_texture, dsv, &dsv_heap)),
            &rtv_heap,
            rtv,
            &texture,
            &[(&near_vertices, 24), (&far_vertices, 24)],
            [0.0, 0.0, 0.0, 1.0],
        );
        assert_eq!(
            &pixels[..4],
            &[255, 0, 0, 255],
            "near triangle wins with depth test"
        );

        // Without depth testing the far triangle is drawn last and overwrites.
        let texture = color_target();
        let rtv = device
            .create_render_target_view(&rtv_heap, &texture)
            .expect("rtv");
        let pixels = render_mesh(
            device,
            &without_depth,
            None,
            &rtv_heap,
            rtv,
            &texture,
            &[(&near_vertices, 24), (&far_vertices, 24)],
            [0.0, 0.0, 0.0, 1.0],
        );
        assert_eq!(
            &pixels[..4],
            &[0, 255, 0, 255],
            "far triangle overwrites without depth test"
        );
        device
            .check_device()
            .expect("device alive after depth test");
    }

    #[test]
    fn textured_quad_samples_texture_srv() {
        let init = init_headless();
        let device = &init.device;
        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                eprintln!("skipping textured quad test: {err}");
                return;
            }
        };
        let vs = compiler
            .compile_hlsl(shader::TEXTURED_HLSL, "vs_main", "vs_6_0")
            .expect("vertex shader");
        let ps = compiler
            .compile_hlsl(shader::TEXTURED_HLSL, "ps_main", "ps_6_0")
            .expect("pixel shader");

        // Root signature: one SRV descriptor table at t0, static clamp sampler.
        let root = device
            .create_graphics_root_signature_with_samplers(
                &[RootParameter::DescriptorTable(RootDescriptorTable {
                    kind: DescriptorRangeKind::Srv,
                    count: 1,
                    base_shader_register: 0,
                })],
                &[StaticSampler::default()],
            )
            .expect("root signature with static sampler");
        let pipeline = device
            .create_render_pipeline(
                &RenderPipelineDesc {
                    vertex_dxil: &vs,
                    pixel_dxil: &ps,
                    color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM],
                    vertex_attributes: &[],
                    depth_format: None,
                },
                &root,
            )
            .expect("textured pipeline");

        // A 16x16 texture with a unique color per texel.
        let (size, texels) = (16u32, {
            let mut texels = Vec::with_capacity(16 * 16 * 4);
            for y in 0..16u32 {
                for x in 0..16u32 {
                    texels.extend_from_slice(&[
                        (x * 16) as u8,
                        (y * 16) as u8,
                        ((x + y) * 8) as u8,
                        255,
                    ]);
                }
            }
            texels
        });
        let texture = device
            .create_texture(TextureDesc::new(size, size, DXGI_FORMAT_R8G8B8A8_UNORM))
            .expect("source texture");
        device
            .upload_texture(&texture, &texels)
            .expect("upload source texture");

        let target = device
            .create_texture(
                TextureDesc::new(size, size, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
            )
            .expect("color target");
        let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4).expect("rtv heap");
        let rtv = device
            .create_render_target_view(&rtv_heap, &target)
            .expect("rtv");
        let srv_heap =
            DescriptorHeap::new(device, DescriptorHeapType::CbvSrvUav, 4).expect("srv heap");
        let srv = device
            .create_texture_srv(&srv_heap, &texture)
            .expect("texture srv");

        let mut encoder = device.record().expect("encoder");
        encoder.transition_texture(&texture, ResourceState::ShaderRead);
        encoder.transition_texture(&target, ResourceState::RenderTarget);
        encoder.set_render_targets(&rtv_heap, &[rtv]);
        encoder.clear_render_target(&rtv_heap, rtv, [0.0, 0.0, 0.0, 1.0]);
        encoder.set_render_pipeline(&pipeline);
        encoder.set_descriptor_heap(&srv_heap);
        encoder.set_graphics_descriptor_table(0, &srv_heap, srv);
        encoder.set_viewport(size as f32, size as f32);
        encoder.set_scissor(size as i32, size as i32);
        encoder.draw_instanced(3, 1);
        encoder.transition_texture(&target, ResourceState::CopySource);
        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("wait");
        device.check_device().expect("device alive after sampling");

        let pixels = device.read_texture(&target).expect("readback");
        let mut worst = 0i32;
        for (index, (sampled, source)) in pixels
            .chunks_exact(4)
            .zip(texels.chunks_exact(4))
            .enumerate()
        {
            for channel in 0..3 {
                let delta = i32::from(sampled[channel]) - i32::from(source[channel]);
                assert!(
                    delta.abs() <= 2,
                    "texel {index} channel {channel}: sampled {}, expected {}",
                    sampled[channel],
                    source[channel]
                );
                worst = worst.max(delta.abs());
            }
        }
        assert_eq!(worst <= 2, true);
    }

    #[test]
    fn multiple_render_targets_write_both_targets() {
        let init = init_headless();
        let device = &init.device;
        let compiler = match ShaderCompiler::load() {
            Ok(compiler) => compiler,
            Err(err) => {
                eprintln!("skipping MRT test: {err}");
                return;
            }
        };
        let vs = compiler
            .compile_hlsl(shader::MRT_HLSL, "vs_main", "vs_6_0")
            .expect("vertex shader");
        let ps = compiler
            .compile_hlsl(shader::MRT_HLSL, "ps_main", "ps_6_0")
            .expect("pixel shader");
        let root = device
            .create_graphics_root_signature(&[])
            .expect("root signature");
        let pipeline = device
            .create_render_pipeline(
                &RenderPipelineDesc {
                    vertex_dxil: &vs,
                    pixel_dxil: &ps,
                    color_formats: &[DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM],
                    vertex_attributes: &[],
                    depth_format: None,
                },
                &root,
            )
            .expect("MRT pipeline");

        let (width, height) = (32u32, 32u32);
        let target0 = device
            .create_texture(
                TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
            )
            .expect("target 0");
        let target1 = device
            .create_texture(
                TextureDesc::new(width, height, DXGI_FORMAT_R8G8B8A8_UNORM).with_render_target(),
            )
            .expect("target 1");
        let rtv_heap = DescriptorHeap::new(device, DescriptorHeapType::Rtv, 4).expect("rtv heap");
        let rtv0 = device
            .create_render_target_view(&rtv_heap, &target0)
            .expect("rtv 0");
        let rtv1 = device
            .create_render_target_view(&rtv_heap, &target1)
            .expect("rtv 1");

        let mut encoder = device.record().expect("encoder");
        encoder.transition_texture(&target0, ResourceState::RenderTarget);
        encoder.transition_texture(&target1, ResourceState::RenderTarget);
        encoder.set_render_targets(&rtv_heap, &[rtv0, rtv1]);
        encoder.clear_render_target(&rtv_heap, rtv0, [0.0, 0.0, 0.0, 1.0]);
        encoder.clear_render_target(&rtv_heap, rtv1, [0.0, 0.0, 0.0, 1.0]);
        encoder.set_render_pipeline(&pipeline);
        encoder.set_viewport(width as f32, height as f32);
        encoder.set_scissor(width as i32, height as i32);
        encoder.draw_instanced(3, 1);
        encoder.transition_texture(&target0, ResourceState::CopySource);
        encoder.transition_texture(&target1, ResourceState::CopySource);
        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("wait");
        device.check_device().expect("device alive after MRT draw");

        let pixels0 = device.read_texture(&target0).expect("readback target 0");
        let pixels1 = device.read_texture(&target1).expect("readback target 1");
        let wrong0 = pixels0
            .chunks_exact(4)
            .filter(|p| *p != [255, 0, 0, 255])
            .count();
        let wrong1 = pixels1
            .chunks_exact(4)
            .filter(|p| *p != [0, 255, 0, 255])
            .count();
        assert_eq!(wrong0, 0, "target 0 should be red (SV_Target0)");
        assert_eq!(wrong1, 0, "target 1 should be green (SV_Target1)");
    }

    #[test]
    fn repeated_submit_and_wait() {
        let init = init_headless();
        let device = &init.device;
        for iteration in 0..4u8 {
            let size = 1024u64;
            let payload = vec![iteration; size as usize];
            let upload = device
                .create_buffer(BufferDesc::new(size, Memory::Upload))
                .unwrap();
            upload.write(0, &payload).unwrap();
            let readback = device
                .create_buffer(BufferDesc::new(size, Memory::Readback))
                .unwrap();
            let mut encoder = device.record().unwrap();
            encoder.copy_buffer(&readback, 0, &upload, 0, size);
            let sp = device.submit(encoder).unwrap();
            device.wait(sp).unwrap();
            let result = readback.read().unwrap();
            assert_eq!(&result[..size as usize], &payload[..]);
        }
    }
}
