//! 12.1.2: the Flatpak bundles NVIDIA's NVRTC (libnvrtc.so.12) so GStreamer's CUDA filters can register on NVIDIA
//! machines. On every OTHER machine it must change nothing: this test runs GStreamer the way the Flatpak app does with
//! the library present (GST_CUDA_NVRTC_LIBNAME + a fresh registry, set before gst::init — its own process, as an
//! integration test) and checks that without an NVIDIA GPU no CUDA filter registers and the CPU share path still runs.
//! Needs TINHAT_TEST_NVRTC=/path/to/libnvrtc.so.12 (the archive the manifest pins); skips without it. Linux only.
#![cfg(target_os = "linux")]

use gstreamer as gst;
use gstreamer::prelude::*;

extern "C" {
    fn dlopen(filename: *const std::ffi::c_char, flag: i32) -> *mut std::ffi::c_void;
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const std::ffi::c_char) -> *mut std::ffi::c_void;
}

#[test]
fn bundled_nvrtc_is_inert_without_nvidia() {
    let Some(nvrtc) = std::env::var_os("TINHAT_TEST_NVRTC") else {
        eprintln!("skipping: TINHAT_TEST_NVRTC not set");
        return;
    };
    let nvidia = std::path::Path::new("/dev/nvidia0").exists() || std::path::Path::new("/proc/driver/nvidia").exists();
    if nvidia {
        eprintln!("skipping: an NVIDIA GPU is present (this proves the no-NVIDIA case)");
        return;
    }

    // The library is a real, loadable NVRTC (so "inert" is not just "failed to load").
    let path = std::ffi::CString::new(nvrtc.to_string_lossy().as_bytes()).unwrap();
    let h = unsafe { dlopen(path.as_ptr(), 2 /* RTLD_NOW */) };
    assert!(!h.is_null(), "dlopen {nvrtc:?} failed");
    let ver = unsafe { dlsym(h, c"nvrtcVersion".as_ptr()) };
    assert!(!ver.is_null(), "no nvrtcVersion in {nvrtc:?}");
    let (mut major, mut minor) = (0i32, 0i32);
    let f: extern "C" fn(*mut i32, *mut i32) -> i32 = unsafe { std::mem::transmute(ver) };
    assert_eq!(f(&mut major, &mut minor), 0);
    // ...and it compiles (which loads libnvrtc-builtins.so.12.9 by its soname; no GPU needed to compile).
    type Create = extern "C" fn(*mut *mut std::ffi::c_void, *const std::ffi::c_char, *const std::ffi::c_char, i32, *const *const std::ffi::c_char, *const *const std::ffi::c_char) -> i32;
    type Compile = extern "C" fn(*mut std::ffi::c_void, i32, *const *const std::ffi::c_char) -> i32;
    type Destroy = extern "C" fn(*mut *mut std::ffi::c_void) -> i32;
    let sym = |n: &std::ffi::CStr| {
        let p = unsafe { dlsym(h, n.as_ptr()) };
        assert!(!p.is_null(), "no {n:?}");
        p
    };
    let create: Create = unsafe { std::mem::transmute(sym(c"nvrtcCreateProgram")) };
    let compile: Compile = unsafe { std::mem::transmute(sym(c"nvrtcCompileProgram")) };
    let destroy: Destroy = unsafe { std::mem::transmute(sym(c"nvrtcDestroyProgram")) };
    let mut prog = std::ptr::null_mut();
    let src = c"extern \"C\" __global__ void k(float* a) { a[threadIdx.x] *= 2.0f; }";
    assert_eq!(create(&mut prog, src.as_ptr(), c"k.cu".as_ptr(), 0, std::ptr::null(), std::ptr::null()), 0);
    assert_eq!(compile(prog, 0, std::ptr::null()), 0, "NVRTC compile failed (builtins library missing?)");
    destroy(&mut prog);
    eprintln!("NVRTC {major}.{minor} loaded and compiled from {nvrtc:?}; NVIDIA GPU present: {nvidia}");

    // What early_startup does in the Flatpak, then a FRESH plugin scan.
    let reg = std::env::temp_dir().join(format!("tinhat-nvrtc-inert-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&reg);
    std::env::set_var("GST_CUDA_NVRTC_LIBNAME", &nvrtc);
    std::env::set_var("GST_REGISTRY", &reg);
    std::env::remove_var("GST_REGISTRY_1_0"); // it would win over GST_REGISTRY
    gst::init().unwrap();
    // the plugin that would register the CUDA filters WAS scanned (zero filters must not mean "no nvcodec here")
    assert!(gst::Registry::get().find_plugin("nvcodec").is_some(), "no nvcodec plugin on this machine: the test proves nothing");

    let cuda_filters = ["cudaupload", "cudaconvertscale", "cudadownload"]
        .iter()
        .filter(|n| gst::ElementFactory::find(n).is_some())
        .count();
    assert!(!nvidia, "this test is for machines WITHOUT an NVIDIA GPU");
    assert_eq!(cuda_filters, 0, "CUDA elements registered on a machine without NVIDIA");

    // The CPU share path (the launch line's convert + scale) still produces frames.
    let pipeline = gst::parse::launch(
        "videotestsrc num-buffers=30 ! video/x-raw,format=BGRx,width=3840,height=2160,framerate=60/1 ! \
         videoconvertscale method=lanczos add-borders=false n-threads=4 ! video/x-raw,format=I420,width=1920,height=1080 ! \
         appsink name=sink sync=false",
    )
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    let sink = pipeline.by_name("sink").unwrap().downcast::<gstreamer_app::AppSink>().unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let mut n = 0;
    while sink.try_pull_sample(gst::ClockTime::from_seconds(5)).is_some() {
        n += 1;
    }
    let _ = pipeline.set_state(gst::State::Null);
    let _ = std::fs::remove_file(&reg);
    eprintln!("CUDA elements registered: {cuda_filters}; CPU path frames: {n}/30");
    assert_eq!(n, 30);
}
