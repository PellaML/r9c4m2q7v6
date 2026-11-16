use std::mem::size_of;

pub const SIDE: usize = 256;
pub const ROW: usize = 2048;
pub const GUARD: usize = 256;
pub const SLAB: usize = GUARD + ROW * SIDE + GUARD;
pub const MAX_READBACK: usize = 6 * SLAB;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Input {
    pub id: u32,
    pub draw_count: u32,
    pub vertex_start: u32,
    pub index_first: u32,
    pub indexed: u32,
    pub instances: u32,
    pub primitive: u32,
    pub keep: u32,
    pub clear_stencil: u32,
    pub no_draw: u32,
    pub position_mode: u32,
    pub vertices: [[f32; 2]; 8],
    pub indices: [u16; 10],
    pub offsets: [[f32; 2]; 3],
    pub viewport: [f32; 4],
    pub scissor: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Receipt {
    pub submitted: u32,
    pub completion_seen: u32,
    pub timed_out: u32,
    pub status: u32,
    pub error_present: u32,
    pub bytes: u32,
    pub mode: u32,
    pub reserved: u32,
    pub error_code: i64,
    pub gpu_start: f64,
    pub gpu_end: f64,
    pub reason: [u8; 96],
    pub error_domain: [u8; 96],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Metadata {
    pub registry_id: u64,
    pub paravirtual_name_match: u32,
    pub unified_memory: u32,
    pub low_power: u32,
    pub removable: u32,
    pub available_modes: u32,
    pub device_count: u32,
    pub selected_by: u32,
    pub sdk_max: u32,
    pub device_name: [u8; 192],
    pub os_version: [u8; 96],
    pub os_build: [u8; 64],
    pub architecture: [u8; 32],
    pub clang_version: [u8; 192],
    pub mode_errors: [Receipt; 2],
}

impl Receipt {
    pub fn zero() -> Self {
        Self {
            submitted: 0,
            completion_seen: 0,
            timed_out: 0,
            status: 0,
            error_present: 0,
            bytes: 0,
            mode: 0,
            reserved: 0,
            error_code: 0,
            gpu_start: 0.0,
            gpu_end: 0.0,
            reason: [0; 96],
            error_domain: [0; 96],
        }
    }
    pub fn completed(&self, mode: u32) -> bool {
        mode <= 1
            && self.submitted == 1
            && self.completion_seen == 1
            && self.timed_out == 0
            && self.status == 4
            && self.error_present == 0
            && self.error_code == 0
            && self.bytes as usize == (if mode == 0 { 6 } else { 2 }) * SLAB
            && self.mode == mode
    }
}
impl Metadata {
    pub fn zero() -> Self {
        Self {
            registry_id: 0,
            paravirtual_name_match: 0,
            unified_memory: 0,
            low_power: 0,
            removable: 0,
            available_modes: 0,
            device_count: 0,
            selected_by: 0,
            sdk_max: 0,
            device_name: [0; 192],
            os_version: [0; 96],
            os_build: [0; 64],
            architecture: [0; 32],
            clang_version: [0; 192],
            mode_errors: [Receipt::zero(); 2],
        }
    }
}
pub fn cstr(s: &[u8]) -> String {
    String::from_utf8_lossy(&s[..s.iter().position(|x| *x == 0).unwrap_or(s.len())]).into_owned()
}

pub struct Native {
    #[cfg(target_os = "macos")]
    handle: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
extern "C" {
    fn gp_check_abi(abi: u32, a: usize, b: usize, c: usize) -> i32;
    fn gp_open(
        registry: u64,
        handle: *mut *mut std::ffi::c_void,
        m: *mut Metadata,
        r: *mut Receipt,
    ) -> i32;
    fn gp_run(
        handle: *mut std::ffi::c_void,
        input: *const Input,
        mode: u32,
        out: *mut u8,
        capacity: usize,
        r: *mut Receipt,
    ) -> i32;
    fn gp_close(handle: *mut std::ffi::c_void);
}

impl Native {
    pub fn open(registry: u64) -> (Option<Self>, Metadata, Receipt) {
        let mut m = Metadata::zero();
        let mut r = Receipt::zero();
        #[cfg(target_os = "macos")]
        unsafe {
            if gp_check_abi(
                3,
                size_of::<Input>(),
                size_of::<Receipt>(),
                size_of::<Metadata>(),
            ) != 1
            {
                r.reason[..12].copy_from_slice(b"abi_mismatch");
                return (None, m, r);
            }
            let mut handle = std::ptr::null_mut();
            if gp_open(registry, &mut handle, &mut m, &mut r) == 0 && !handle.is_null() {
                return (Some(Self { handle }), m, r);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (registry, &mut m);
            let reason = b"unsupported_host_no_native_metal";
            r.reason[..reason.len()].copy_from_slice(reason);
        }
        (None, m, r)
    }

    pub fn run(&self, input: &Input, mode: u32) -> (Receipt, Option<Vec<u8>>) {
        #[cfg(target_os = "macos")]
        unsafe {
            let mut r = Receipt::zero();
            // Distinct from the GPU readback's 0xa5 poison. Not a synthetic result.
            let mut bytes = vec![0xde; MAX_READBACK];
            if gp_run(
                self.handle,
                input,
                mode,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut r,
            ) == 0
                && r.completed(mode)
                && (r.bytes as usize) <= bytes.len()
            {
                bytes.truncate(r.bytes as usize);
                return (r, Some(bytes));
            }
            (r, None)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (input, mode, MAX_READBACK);
            (Receipt::zero(), None)
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { gp_close(self.handle) }
    }
}

pub fn abi_sizes() -> [usize; 3] {
    [
        size_of::<Input>(),
        size_of::<Receipt>(),
        size_of::<Metadata>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffi_layout_is_explicit() {
        assert_eq!(abi_sizes(), [184, 248, 1112]);
    }
    #[test]
    fn zero_receipt_never_passes() {
        assert!(!Receipt::zero().completed(0));
    }
    #[test]
    fn incomplete_or_error_receipts_fail() {
        let mut r = Receipt::zero();
        r.mode = 1;
        r.submitted = 1;
        r.completion_seen = 1;
        r.status = 4;
        r.bytes = (2 * SLAB) as u32;
        assert!(r.completed(1));
        for which in 0..9 {
            let mut bad = r;
            match which {
                0 => bad.error_present = 1,
                1 => bad.timed_out = 1,
                2 => bad.status = 5,
                3 => bad.bytes -= 1,
                4 => bad.completion_seen = 0,
                5 => bad.submitted = 0,
                6 => bad.error_code = 1,
                7 => bad.mode = 0,
                _ => bad.bytes += 1,
            }
            assert!(!bad.completed(1));
        }
        r.mode = 2;
        assert!(!r.completed(2));
    }
    #[test]
    fn native_header_and_rust_dimensions_agree() {
        let h = include_str!("../native/metal_probe.h");
        assert!(h.contains("GP_SIDE = 256, GP_ROW = 2048, GP_GUARD = 256"));
        assert!(h.contains("GP_ABI = 3"));
        assert_eq!(MAX_READBACK, 3_148_800);
        assert_eq!(SLAB % 256, 0);
    }
}
