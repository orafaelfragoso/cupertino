//! Memory of this process from `/proc/self/smaps_rollup`, and the GPU's from the management library when it is there.

use std::ffi::{c_char, c_int, c_uint, c_void, CString};

/// What `/proc/self/smaps_rollup` says, in KiB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub rss: u64,
    pub pss: u64,
    pub private_clean: u64,
    pub private_dirty: u64,
}

impl Memory {
    pub fn now() -> Memory {
        let text = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
        let mut m = Memory::default();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
                continue;
            };
            let value: u64 = value.parse().unwrap_or(0);
            match key {
                "Rss:" => m.rss = value,
                "Pss:" => m.pss = value,
                "Private_Clean:" => m.private_clean = value,
                "Private_Dirty:" => m.private_dirty = value,
                _ => {}
            }
        }
        m
    }

    /// Private memory, clean and dirty, in KiB.
    pub fn private(&self) -> u64 {
        self.private_clean + self.private_dirty
    }
}

/// Resident memory in KiB from `/proc/self/statm`, cheap enough to read often.
pub fn resident_kib() -> u64 {
    let text = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    pages * (unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64) / 1024
}

#[repr(C)]
#[derive(Default)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NvmlProcess {
    pid: c_uint,
    used: u64,
    gpu_instance: c_uint,
    compute_instance: c_uint,
}

type Init = unsafe extern "C" fn() -> c_int;
type Handle = unsafe extern "C" fn(c_uint, *mut *mut c_void) -> c_int;
type MemoryInfo = unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> c_int;
type Processes = unsafe extern "C" fn(*mut c_void, *mut c_uint, *mut NvmlProcess) -> c_int;

/// The GPU's memory as its management library reports it (loaded at run time; absent on other drivers).
pub struct GpuMemory {
    device: *mut c_void,
    memory_info: MemoryInfo,
    processes: Option<Processes>,
}

unsafe fn sym<T: Copy>(lib: *mut c_void, name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let p = libc::dlsym(lib, c.as_ptr() as *const c_char);
    (!p.is_null()).then(|| std::mem::transmute_copy(&p))
}

impl GpuMemory {
    pub fn open() -> Option<GpuMemory> {
        unsafe {
            let name = CString::new("libnvidia-ml.so.1").unwrap();
            let lib = libc::dlopen(name.as_ptr(), libc::RTLD_NOW);
            if lib.is_null() {
                return None;
            }
            let init: Init = sym(lib, "nvmlInit_v2")?;
            if init() != 0 {
                return None;
            }
            let handle: Handle = sym(lib, "nvmlDeviceGetHandleByIndex_v2")?;
            let mut device = std::ptr::null_mut();
            if handle(0, &mut device) != 0 {
                return None;
            }
            Some(GpuMemory {
                device,
                memory_info: sym(lib, "nvmlDeviceGetMemoryInfo")?,
                processes: sym(lib, "nvmlDeviceGetGraphicsRunningProcesses_v3"),
            })
        }
    }

    /// Memory in use on the whole GPU, in KiB.
    pub fn used_kib(&self) -> u64 {
        let mut m = NvmlMemory::default();
        unsafe { (self.memory_info)(self.device, &mut m) };
        m.used / 1024
    }

    /// The GPU memory the driver counts against process `pid`, in KiB; `None` when it lists no such process.
    pub fn process_kib(&self, pid: u32) -> Option<u64> {
        let processes = self.processes?;
        let mut list = [NvmlProcess::default(); 128];
        let mut n: c_uint = list.len() as c_uint;
        if unsafe { processes(self.device, &mut n, list.as_mut_ptr()) } != 0 {
            return None;
        }
        list[..n as usize]
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.used / 1024)
    }
}
