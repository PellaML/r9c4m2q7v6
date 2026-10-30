use crate::json::Json;
#[cfg(any(target_os = "macos", probe_bridge_check))]
use crate::{
    checks::Sample,
    plan::{Case, COLORS},
};
#[cfg(any(target_os = "macos", probe_bridge_check))]
use std::{
    ffi::{c_char, c_void, CString},
    path::Path,
    ptr,
};

#[repr(C)]
struct Config {
    count: u32,
    linear: u32,
    zero_axes: u32,
    pixel_coords: u32,
    use_constexpr: u32,
    token: u32,
    rgba: [u8; 4],
    bias: f32,
}

#[repr(C)]
#[derive(Default)]
pub struct RunInfo {
    wait_called: u32,
    command_status: u32,
    texture_verified: u32,
    upload_encoded: u32,
    render_encoded: u32,
    readback_copied: u32,
}
impl RunInfo {
    pub fn completed(&self) -> bool {
        self.wait_called == 1
            && self.command_status == 4
            && self.texture_verified == 1
            && self.upload_encoded == 1
            && self.render_encoded == 1
            && self.readback_copied == 1
    }
    pub fn json(&self) -> Json {
        Json::object([
            (
                "wait_until_completed_called",
                Json::from(self.wait_called == 1),
            ),
            ("command_status_value", Json::from(self.command_status)),
            (
                "command_status_is_completed",
                Json::from(self.command_status == 4),
            ),
            (
                "actual_texture_verified",
                Json::from(self.texture_verified == 1),
            ),
            ("upload_encoded", Json::from(self.upload_encoded == 1)),
            ("render_encoded", Json::from(self.render_encoded == 1)),
            (
                "shared_buffer_copied_after_successful_wait",
                Json::from(self.readback_copied == 1),
            ),
        ])
    }
}

#[cfg(any(target_os = "macos", probe_bridge_check))]
unsafe extern "C" {
    fn probe_open(context: *mut *mut c_void, json: *mut c_char, capacity: usize) -> i32;
    fn probe_sample(
        context: *mut c_void,
        library: *const c_char,
        function: *const c_char,
        coordinates: *const u32,
        config: *const Config,
        samples: *mut Sample,
        info: *mut RunInfo,
        json: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn probe_close(context: *mut c_void);
}

#[cfg(any(target_os = "macos", probe_bridge_check))]
fn sdk_json(bytes: &[u8]) -> Json {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    match std::str::from_utf8(&bytes[..end]) {
        Ok(s) if s.starts_with('{') && s.ends_with('}') => Json::Raw(s.to_string()),
        _ => Json::object([("phase", Json::from("invalid_bridge_metadata"))]),
    }
}

#[cfg(any(target_os = "macos", probe_bridge_check))]
pub struct Native {
    context: *mut c_void,
}
#[cfg(any(target_os = "macos", probe_bridge_check))]
impl Native {
    pub fn open() -> (Option<Self>, i32, Json) {
        let mut context = ptr::null_mut();
        let mut bytes = [0_u8; 8192];
        // The bridge owns the retained Objective-C context until Drop.
        let code = unsafe { probe_open(&mut context, bytes.as_mut_ptr().cast(), bytes.len()) };
        let native = if code == 0 && !context.is_null() {
            Some(Self { context })
        } else {
            None
        };
        (native, code, sdk_json(&bytes))
    }

    pub fn sample(
        &mut self,
        library: &Path,
        case: &Case,
        coordinates: &[[u32; 3]],
        samples: &mut [Sample],
    ) -> (i32, RunInfo, Json) {
        let mut info = RunInfo::default();
        if coordinates.len() < case.count || samples.len() != case.count {
            return (
                1,
                info,
                Json::object([("phase", Json::from("invalid_host_buffer_lengths"))]),
            );
        }
        let Some(path) = library.to_str().and_then(|p| CString::new(p).ok()) else {
            return (
                1,
                info,
                Json::object([("phase", Json::from("invalid_library_path"))]),
            );
        };
        let entry = CString::new(case.module_name()).expect("static entry point names");
        let config = Config {
            count: case.count as u32,
            linear: case.linear as u32,
            zero_axes: case.zero_axes,
            pixel_coords: case.pixel as u32,
            use_constexpr: case.constexpr as u32,
            token: case.token(),
            rgba: COLORS[case.color],
            bias: f32::from_bits(0x3f40_0000),
        };
        let mut bytes = [0_u8; 4096];
        let code = unsafe {
            probe_sample(
                self.context,
                path.as_ptr(),
                entry.as_ptr(),
                coordinates.as_ptr().cast(),
                &config,
                samples.as_mut_ptr(),
                &mut info,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        (code, info, sdk_json(&bytes))
    }
}
#[cfg(any(target_os = "macos", probe_bridge_check))]
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { probe_close(self.context) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffi_layout_is_explicit() {
        assert_eq!(std::mem::size_of::<Config>(), 32);
        assert_eq!(std::mem::offset_of!(Config, rgba), 24);
        assert_eq!(std::mem::offset_of!(Config, bias), 28);
        assert_eq!(std::mem::size_of::<RunInfo>(), 24);
    }
    #[test]
    fn wait_alone_does_not_mean_success() {
        let mut info = RunInfo {
            wait_called: 1,
            command_status: 5,
            texture_verified: 1,
            upload_encoded: 1,
            render_encoded: 1,
            readback_copied: 0,
        };
        assert!(!info.completed());
        info.command_status = 4;
        assert!(!info.completed());
        info.readback_copied = 1;
        assert!(info.completed());
    }
}
