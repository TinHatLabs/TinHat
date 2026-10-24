//! FFI to the C++ clipboard shim (`clipboard_bridge.cpp`): what a Ctrl+V in the composer should
//! ATTACH (9.3.2). An image on the clipboard is saved as a PNG under the media cache; files
//! copied in a file manager come back as their paths. Text-only clipboards yield nothing and
//! the TextArea pastes the text itself. GUI thread only (QClipboard).

use std::ffi::CString;
use std::os::raw::{c_char, c_int};

extern "C" {
    fn tinhat_clipboard_attachments(dir: *const c_char, out: *mut c_char, out_len: c_int) -> c_int;
    fn tinhat_image_size(path: *const c_char, w: *mut c_int, h: *mut c_int) -> c_int;
}

/// Pixel size of an image file from its header (QImageReader, no full decode); `None` when
/// Qt cannot read it. Goes out as m.image `info.w/h` so receivers reserve the row height.
pub fn image_size(path: &std::path::Path) -> Option<(u32, u32)> {
    let cpath = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let (mut w, mut h): (c_int, c_int) = (0, 0);
    let ok = unsafe { tinhat_image_size(cpath.as_ptr(), &mut w, &mut h) };
    (ok == 1 && w > 0 && h > 0).then(|| (w as u32, h as u32))
}

/// `file://` URLs for the clipboard's image (saved under `dir`) or its local files; empty
/// when there is nothing to attach.
pub fn attachments(dir: &std::path::Path) -> Vec<String> {
    let Ok(cdir) = CString::new(dir.to_string_lossy().as_bytes()) else {
        return Vec::new();
    };
    let mut buf = vec![0u8; 64 * 1024];
    let n = unsafe {
        tinhat_clipboard_attachments(cdir.as_ptr(), buf.as_mut_ptr() as *mut c_char, buf.len() as c_int)
    };
    if n <= 0 {
        return Vec::new();
    }
    String::from_utf8_lossy(&buf[..n as usize])
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| crate::platform::file_url(std::path::Path::new(l)))
        .collect()
}
