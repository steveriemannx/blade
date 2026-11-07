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

use std::cell::Cell;
use std::fmt;
use std::ptr;

use windows::core::{Error as WinError, Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_12_0};
use windows::Win32::Graphics::Direct3D12::{
    D3D12CreateDevice, D3D12GetDebugInterface, D3D12_COMMAND_LIST_TYPE_DIRECT,
    D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_COMMAND_QUEUE_PRIORITY_NORMAL,
    D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK,
    D3D12_HEAP_TYPE_UPLOAD, D3D12_MEMORY_POOL_UNKNOWN, D3D12_RANGE, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12CommandAllocator,
    ID3D12CommandList, ID3D12CommandQueue, ID3D12Debug, ID3D12Device, ID3D12Fence,
    ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_NOT_FOUND,
    DXGI_GPU_PREFERENCE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, IDXGIAdapter1, IDXGIDevice,
    IDXGIFactory6,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
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
}

impl ResourceState {
    fn to_d3d12(self) -> D3D12_RESOURCE_STATES {
        match self {
            Self::Common => D3D12_RESOURCE_STATE_COMMON,
            Self::CopyDest => D3D12_RESOURCE_STATE_COPY_DEST,
            Self::CopySource => D3D12_RESOURCE_STATE_COPY_SOURCE,
            Self::GenericRead => D3D12_RESOURCE_STATE_GENERIC_READ,
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
}

impl BufferDesc {
    pub fn new(size: u64, memory: Memory) -> Self {
        Self { size, memory }
    }
}

/// A committed `ID3D12Resource` buffer.
pub struct Buffer {
    resource: ID3D12Resource,
    size: u64,
    memory: Memory,
    state: Cell<ResourceState>,
}

impl Buffer {
    /// Size in bytes (aligned up to [`BUFFER_ALIGNMENT`]).
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn memory(&self) -> Memory {
        self.memory
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
        let range = D3D12_RANGE { Begin: 0, End: size };
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
            Flags: D3D12_RESOURCE_FLAG_NONE,
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
        let resource = resource.ok_or(Error::InvalidState("CreateCommittedResource returned no resource"))?;
        Ok(Buffer {
            resource,
            size,
            memory: desc.memory,
            state: Cell::new(state),
        })
    }

    /// Starts recording a command list together with a fresh allocator.
    ///
    /// A new allocator per encoder keeps the phase 1 model trivial; the
    /// allocator ring for transient frames arrives with the HAL wiring.
    pub fn record(&self) -> Result<CommandEncoder, Error> {
        let allocator: ID3D12CommandAllocator =
            unsafe { self.raw.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)? };
        let list: ID3D12GraphicsCommandList = unsafe {
            self.raw.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?
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
            self.queue
                .ExecuteCommandLists(&[Some(command_list)]);
            let value = self.next_fence_value.get() + 1;
            self.next_fence_value.set(value);
            self.queue.Signal(&self.fence, value)?;
            // Keep the allocator alive until the GPU is done with it: dropping
            // it now would be UB. Phase 1 leaks it into the queue's fence
            // tracking; the allocator ring in phase 3 will own it properly.
            std::mem::forget(encoder.allocator);
            Ok(SyncPoint { value })
        }
    }

    /// Blocks the CPU until the GPU reaches `sp`.
    pub fn wait(&self, sp: SyncPoint) -> Result<(), Error> {
        unsafe {
            self.fence.SetEventOnCompletion(sp.value, self.fence_event)?;
            let result = WaitForSingleObject(self.fence_event, INFINITE);
            if result != WAIT_OBJECT_0 {
                return Err(Error::InvalidState("fence wait did not return WAIT_OBJECT_0"));
            }
        }
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
    pub fn transition(&mut self, buffer: &Buffer, target_state: ResourceState) {
        let before = buffer.state.get();
        if before == target_state {
            return;
        }
        let barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: std::mem::ManuallyDrop::new(Some(buffer.resource.clone())),
                    Subresource: 0,
                    StateBefore: before.to_d3d12(),
                    StateAfter: target_state.to_d3d12(),
                }),
            },
        };
        unsafe {
            self.list.ResourceBarrier(&[barrier]);
        }
        buffer.state.set(target_state);
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
            self.list.CopyBufferRegion(
                &dst.resource,
                dst_offset,
                &src.resource,
                src_offset,
                size,
            );
        }
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
        let adapter: IDXGIAdapter1 =
            match unsafe { factory.EnumAdapterByGpuPreference(index, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) } {
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

    let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };

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
            let adapter_info = describe_device_adapter(&raw).unwrap_or_else(|_| warp_fallback_info());
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
            None => {
                D3D12CreateDevice(None::<&windows::core::IUnknown>, MIN_FEATURE_LEVEL, &mut device)?
            }
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
        assert!(init.adapter_info.is_software, "WARP should report a software adapter");
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
        encoder.transition(&gpu, ResourceState::CopyDest);
        encoder.copy_buffer(&gpu, 0, &upload, 0, payload.len() as u64);
        encoder.transition(&gpu, ResourceState::CopySource);
        encoder.copy_buffer(&readback, 0, &gpu, 0, payload.len() as u64);
        encoder.transition(&gpu, ResourceState::Common);

        let sp = device.submit(encoder).expect("submit");
        device.wait(sp).expect("fence wait");

        let result = readback.read().expect("read back");
        assert_eq!(&result[..payload.len()], &payload[..]);
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
