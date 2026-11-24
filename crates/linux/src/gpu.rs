//! Zero-copy NVIDIA DMA-BUF -> EGL/OpenGL -> CUDA bridge (loaded dynamically via dlopen).

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::os::fd::{AsRawFd, OwnedFd};
use std::ptr;
use std::sync::OnceLock;

pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258; // 'X','R','2','4' -> PipeWire BGRx
pub const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241; // 'A','R','2','4' -> PipeWire BGRA
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

pub struct DmaBufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

pub struct DmaBufFrame {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<DmaBufPlane>,
}

const EGL_FALSE: c_uint = 0;
const EGL_NONE: c_int = 0x3038;
const EGL_VENDOR: c_int = 0x3053;
const EGL_SURFACE_TYPE: c_int = 0x3033;
const EGL_PBUFFER_BIT: c_int = 0x0001;
const EGL_RENDERABLE_TYPE: c_int = 0x3040;
const EGL_OPENGL_BIT: c_int = 0x0008;
const EGL_OPENGL_API: c_uint = 0x30A2;
const EGL_CONTEXT_MAJOR_VERSION: c_int = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: c_int = 0x30FB;
const EGL_PLATFORM_DEVICE_EXT: c_uint = 0x313F;
const EGL_LINUX_DMA_BUF_EXT: c_uint = 0x3270;
const EGL_WIDTH: c_int = 0x3057;
const EGL_HEIGHT: c_int = 0x3056;
const EGL_LINUX_DRM_FOURCC_EXT: c_int = 0x3271;

const PLANE_ATTRS: [[c_int; 5]; 4] = [
    // [FD, OFFSET, PITCH, MODIFIER_LO, MODIFIER_HI]
    [0x3272, 0x3273, 0x3274, 0x3443, 0x3444],
    [0x3275, 0x3276, 0x3277, 0x3445, 0x3446],
    [0x3278, 0x3279, 0x327A, 0x3447, 0x3448],
    [0x3440, 0x3441, 0x3442, 0x3449, 0x344A],
];

const GL_TEXTURE_2D: c_uint = 0x0DE1;
const GL_RGBA8: c_int = 0x8058;
const GL_RGBA: c_uint = 0x1908;
const GL_UNSIGNED_BYTE: c_uint = 0x1401;
const GL_TEXTURE_MAG_FILTER: c_uint = 0x2800;
const GL_TEXTURE_MIN_FILTER: c_uint = 0x2801;
const GL_LINEAR: c_int = 0x2601;
const GL_TEXTURE_WRAP_S: c_uint = 0x2802;
const GL_TEXTURE_WRAP_T: c_uint = 0x2803;
const GL_CLAMP_TO_EDGE: c_int = 0x812F;
const GL_FRAMEBUFFER: c_uint = 0x8D40;
const GL_READ_FRAMEBUFFER: c_uint = 0x8CA8;
const GL_DRAW_FRAMEBUFFER: c_uint = 0x8CA9;
const GL_COLOR_ATTACHMENT0: c_uint = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: c_uint = 0x8CD5;
const GL_COLOR_BUFFER_BIT: c_uint = 0x0000_4000;

const CU_GRAPHICS_REGISTER_FLAGS_READ_ONLY: c_uint = 1;
const CU_MEMORYTYPE_DEVICE: u32 = 2;
const CU_MEMORYTYPE_ARRAY: u32 = 3;

#[repr(C)]
struct CudaMemcpy2D {
    src_x_in_bytes: usize,
    src_y: usize,
    src_memory_type: u32,
    _pad0: u32,
    src_host: *const c_void,
    src_device: u64,
    src_array: *mut c_void,
    src_pitch: usize,
    dst_x_in_bytes: usize,
    dst_y: usize,
    dst_memory_type: u32,
    _pad1: u32,
    dst_host: *mut c_void,
    dst_device: u64,
    dst_array: *mut c_void,
    dst_pitch: usize,
    width_in_bytes: usize,
    height: usize,
}

type EglGetProcAddressFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type EglInitializeFn = unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_uint;
type EglTerminateFn = unsafe extern "C" fn(*mut c_void) -> c_uint;
type EglQueryStringFn = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;
type EglBindApiFn = unsafe extern "C" fn(c_uint) -> c_uint;
type EglChooseConfigFn =
    unsafe extern "C" fn(*mut c_void, *const c_int, *mut *mut c_void, c_int, *mut c_int) -> c_uint;
type EglCreateContextFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const c_int) -> *mut c_void;
type EglDestroyContextFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_uint;
type EglMakeCurrentFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> c_uint;
type EglGetErrorFn = unsafe extern "C" fn() -> c_int;
type EglQueryDevicesExtFn = unsafe extern "C" fn(c_int, *mut *mut c_void, *mut c_int) -> c_uint;
type EglGetPlatformDisplayExtFn =
    unsafe extern "C" fn(c_uint, *mut c_void, *const c_int) -> *mut c_void;
type EglQueryDmaBufModifiersExtFn = unsafe extern "C" fn(
    *mut c_void,
    c_int,
    c_int,
    *mut u64,
    *mut c_uint,
    *mut c_int,
) -> c_uint;
type EglCreateImageKhrFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, c_uint, *mut c_void, *const c_int) -> *mut c_void;
type EglDestroyImageKhrFn = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_uint;

type GlGenTexturesFn = unsafe extern "C" fn(c_int, *mut c_uint);
type GlDeleteTexturesFn = unsafe extern "C" fn(c_int, *const c_uint);
type GlBindTextureFn = unsafe extern "C" fn(c_uint, c_uint);
type GlTexParameteriFn = unsafe extern "C" fn(c_uint, c_uint, c_int);
type GlTexImage2DFn = unsafe extern "C" fn(
    c_uint,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_uint,
    c_uint,
    *const c_void,
);
type GlGenFramebuffersFn = unsafe extern "C" fn(c_int, *mut c_uint);
type GlDeleteFramebuffersFn = unsafe extern "C" fn(c_int, *const c_uint);
type GlBindFramebufferFn = unsafe extern "C" fn(c_uint, c_uint);
type GlFramebufferTexture2DFn = unsafe extern "C" fn(c_uint, c_uint, c_uint, c_uint, c_int);
type GlCheckFramebufferStatusFn = unsafe extern "C" fn(c_uint) -> c_uint;
type GlBlitFramebufferFn = unsafe extern "C" fn(
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_uint,
    c_uint,
);
type GlFlushFn = unsafe extern "C" fn();
type GlGetErrorFn = unsafe extern "C" fn() -> c_uint;
type GlEglImageTargetTexture2DOesFn = unsafe extern "C" fn(c_uint, *mut c_void);

type CuInitFn = unsafe extern "C" fn(c_uint) -> c_int;
type CuCtxPushCurrentFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type CuCtxPopCurrentFn = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
type CuGraphicsGlRegisterImageFn =
    unsafe extern "C" fn(*mut *mut c_void, c_uint, c_uint, c_uint) -> c_int;
type CuGraphicsUnregisterResourceFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type CuGraphicsMapResourcesFn = unsafe extern "C" fn(c_uint, *mut *mut c_void, *mut c_void) -> c_int;
type CuGraphicsUnmapResourcesFn =
    unsafe extern "C" fn(c_uint, *mut *mut c_void, *mut c_void) -> c_int;
type CuGraphicsSubResourceGetMappedArrayFn =
    unsafe extern "C" fn(*mut *mut c_void, *mut c_void, c_uint, c_uint) -> c_int;
type CuMemcpy2DAsyncFn = unsafe extern "C" fn(*const CudaMemcpy2D, *mut c_void) -> c_int;

struct GpuApi {
    _egl_lib: *mut c_void,
    _gl_lib: *mut c_void,
    _cuda_lib: *mut c_void,

    egl_initialize: EglInitializeFn,
    egl_terminate: EglTerminateFn,
    egl_query_string: EglQueryStringFn,
    egl_bind_api: EglBindApiFn,
    egl_choose_config: EglChooseConfigFn,
    egl_create_context: EglCreateContextFn,
    egl_destroy_context: EglDestroyContextFn,
    egl_make_current: EglMakeCurrentFn,
    egl_get_error: EglGetErrorFn,
    egl_query_devices_ext: EglQueryDevicesExtFn,
    egl_get_platform_display_ext: EglGetPlatformDisplayExtFn,
    egl_query_dmabuf_modifiers_ext: EglQueryDmaBufModifiersExtFn,
    egl_create_image_khr: EglCreateImageKhrFn,
    egl_destroy_image_khr: EglDestroyImageKhrFn,

    gl_gen_textures: GlGenTexturesFn,
    gl_delete_textures: GlDeleteTexturesFn,
    gl_bind_texture: GlBindTextureFn,
    gl_tex_parameteri: GlTexParameteriFn,
    gl_tex_image_2d: GlTexImage2DFn,
    gl_gen_framebuffers: GlGenFramebuffersFn,
    gl_delete_framebuffers: GlDeleteFramebuffersFn,
    gl_bind_framebuffer: GlBindFramebufferFn,
    gl_framebuffer_texture_2d: GlFramebufferTexture2DFn,
    gl_check_framebuffer_status: GlCheckFramebufferStatusFn,
    gl_blit_framebuffer: GlBlitFramebufferFn,
    gl_flush: GlFlushFn,
    gl_get_error: GlGetErrorFn,
    gl_egl_image_target_texture_2d_oes: GlEglImageTargetTexture2DOesFn,

    cu_init: CuInitFn,
    cu_ctx_push_current: CuCtxPushCurrentFn,
    cu_ctx_pop_current: CuCtxPopCurrentFn,
    cu_graphics_gl_register_image: CuGraphicsGlRegisterImageFn,
    cu_graphics_unregister_resource: CuGraphicsUnregisterResourceFn,
    cu_graphics_map_resources: CuGraphicsMapResourcesFn,
    cu_graphics_unmap_resources: CuGraphicsUnmapResourcesFn,
    cu_graphics_sub_resource_get_mapped_array: CuGraphicsSubResourceGetMappedArrayFn,
    cu_memcpy_2d_async: CuMemcpy2DAsyncFn,
}

unsafe impl Send for GpuApi {}
unsafe impl Sync for GpuApi {}

unsafe fn dl_sym<T>(lib: *mut c_void, name: &[u8]) -> Result<T, String> {
    let p = libc::dlsym(lib, name.as_ptr() as *const c_char);
    if p.is_null() {
        return Err(format!("missing symbol {}", String::from_utf8_lossy(name)));
    }
    Ok(std::mem::transmute_copy(&p))
}

unsafe fn egl_proc<T>(get_proc: EglGetProcAddressFn, name: &[u8]) -> Result<T, String> {
    let p = get_proc(name.as_ptr() as *const c_char);
    if p.is_null() {
        return Err(format!("missing EGL/GL proc {}", String::from_utf8_lossy(name)));
    }
    Ok(std::mem::transmute_copy(&p))
}

fn load_api() -> Result<&'static GpuApi, String> {
    static API: OnceLock<Result<GpuApi, String>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let egl_lib = libc::dlopen(b"libEGL.so.1\0".as_ptr() as *const c_char, libc::RTLD_NOW | libc::RTLD_LOCAL);
        if egl_lib.is_null() {
            return Err("dlopen libEGL.so.1 failed".into());
        }
        let gl_lib = libc::dlopen(b"libGL.so.1\0".as_ptr() as *const c_char, libc::RTLD_NOW | libc::RTLD_LOCAL);
        if gl_lib.is_null() {
            return Err("dlopen libGL.so.1 failed".into());
        }
        let cuda_lib = libc::dlopen(b"libcuda.so.1\0".as_ptr() as *const c_char, libc::RTLD_NOW | libc::RTLD_LOCAL);
        if cuda_lib.is_null() {
            return Err("dlopen libcuda.so.1 failed".into());
        }

        let get_proc: EglGetProcAddressFn = dl_sym(egl_lib, b"eglGetProcAddress\0")?;

        let api = GpuApi {
            _egl_lib: egl_lib,
            _gl_lib: gl_lib,
            _cuda_lib: cuda_lib,

            egl_initialize: dl_sym(egl_lib, b"eglInitialize\0")?,
            egl_terminate: dl_sym(egl_lib, b"eglTerminate\0")?,
            egl_query_string: dl_sym(egl_lib, b"eglQueryString\0")?,
            egl_bind_api: dl_sym(egl_lib, b"eglBindAPI\0")?,
            egl_choose_config: dl_sym(egl_lib, b"eglChooseConfig\0")?,
            egl_create_context: dl_sym(egl_lib, b"eglCreateContext\0")?,
            egl_destroy_context: dl_sym(egl_lib, b"eglDestroyContext\0")?,
            egl_make_current: dl_sym(egl_lib, b"eglMakeCurrent\0")?,
            egl_get_error: dl_sym(egl_lib, b"eglGetError\0")?,
            egl_query_devices_ext: egl_proc(get_proc, b"eglQueryDevicesEXT\0")?,
            egl_get_platform_display_ext: egl_proc(get_proc, b"eglGetPlatformDisplayEXT\0")?,
            egl_query_dmabuf_modifiers_ext: egl_proc(get_proc, b"eglQueryDmaBufModifiersEXT\0")?,
            egl_create_image_khr: egl_proc(get_proc, b"eglCreateImageKHR\0")?,
            egl_destroy_image_khr: egl_proc(get_proc, b"eglDestroyImageKHR\0")?,

            gl_gen_textures: dl_sym(gl_lib, b"glGenTextures\0")?,
            gl_delete_textures: dl_sym(gl_lib, b"glDeleteTextures\0")?,
            gl_bind_texture: dl_sym(gl_lib, b"glBindTexture\0")?,
            gl_tex_parameteri: dl_sym(gl_lib, b"glTexParameteri\0")?,
            gl_tex_image_2d: dl_sym(gl_lib, b"glTexImage2D\0")?,
            gl_gen_framebuffers: egl_proc(get_proc, b"glGenFramebuffers\0")?,
            gl_delete_framebuffers: egl_proc(get_proc, b"glDeleteFramebuffers\0")?,
            gl_bind_framebuffer: egl_proc(get_proc, b"glBindFramebuffer\0")?,
            gl_framebuffer_texture_2d: egl_proc(get_proc, b"glFramebufferTexture2D\0")?,
            gl_check_framebuffer_status: egl_proc(get_proc, b"glCheckFramebufferStatus\0")?,
            gl_blit_framebuffer: egl_proc(get_proc, b"glBlitFramebuffer\0")?,
            gl_flush: dl_sym(gl_lib, b"glFlush\0")?,
            gl_get_error: dl_sym(gl_lib, b"glGetError\0")?,
            gl_egl_image_target_texture_2d_oes: egl_proc(get_proc, b"glEGLImageTargetTexture2DOES\0")?,

            cu_init: dl_sym(cuda_lib, b"cuInit\0")?,
            cu_ctx_push_current: dl_sym(cuda_lib, b"cuCtxPushCurrent_v2\0")?,
            cu_ctx_pop_current: dl_sym(cuda_lib, b"cuCtxPopCurrent_v2\0")?,
            cu_graphics_gl_register_image: dl_sym(cuda_lib, b"cuGraphicsGLRegisterImage\0")?,
            cu_graphics_unregister_resource: dl_sym(cuda_lib, b"cuGraphicsUnregisterResource\0")?,
            cu_graphics_map_resources: dl_sym(cuda_lib, b"cuGraphicsMapResources\0")?,
            cu_graphics_unmap_resources: dl_sym(cuda_lib, b"cuGraphicsUnmapResources\0")?,
            cu_graphics_sub_resource_get_mapped_array: dl_sym(
                cuda_lib,
                b"cuGraphicsSubResourceGetMappedArray\0",
            )?,
            cu_memcpy_2d_async: dl_sym(cuda_lib, b"cuMemcpy2DAsync_v2\0")?,
        };
        if (api.cu_init)(0) != 0 {
            return Err("cuInit(0) failed".into());
        }
        Ok(api)
    })
    .as_ref()
    .map_err(|e| e.clone())
}

unsafe fn open_nvidia_egl_display(api: &GpuApi) -> Result<*mut c_void, String> {
    let mut devs = [ptr::null_mut::<c_void>(); 16];
    let mut ndev: c_int = 0;
    if (api.egl_query_devices_ext)(devs.len() as c_int, devs.as_mut_ptr(), &mut ndev) == EGL_FALSE
        || ndev <= 0
    {
        return Err("eglQueryDevicesEXT returned no devices".into());
    }
    for &dev in &devs[..ndev as usize] {
        let dpy = (api.egl_get_platform_display_ext)(EGL_PLATFORM_DEVICE_EXT, dev, ptr::null());
        if dpy.is_null() {
            continue;
        }
        let (mut maj, mut min) = (0, 0);
        if (api.egl_initialize)(dpy, &mut maj, &mut min) == EGL_FALSE {
            continue;
        }
        let vendor_ptr = (api.egl_query_string)(dpy, EGL_VENDOR);
        if !vendor_ptr.is_null() {
            let vendor = CStr::from_ptr(vendor_ptr).to_string_lossy();
            if vendor.contains("NVIDIA") {
                return Ok(dpy);
            }
        }
    }
    Err("no NVIDIA EGL device found".into())
}

/// Query DRM format modifiers supported for zero-copy `GL_TEXTURE_2D` DMA-BUF import on NVIDIA.
/// Returns `None` if `CLIPSD_NO_DMABUF=1` is set or no NVIDIA EGL/CUDA stack is available.
pub fn query_dmabuf_modifiers() -> Option<Vec<u64>> {
    if std::env::var("CLIPSD_NO_DMABUF").ok().as_deref() == Some("1") {
        return None;
    }
    static MODS: OnceLock<Option<Vec<u64>>> = OnceLock::new();
    MODS.get_or_init(|| {
        let api = match load_api() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("clipsd-linux: DMA-BUF zero-copy unavailable ({e}); using MemFd");
                return None;
            }
        };
        unsafe {
            let dpy = match open_nvidia_egl_display(api) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("clipsd-linux: DMA-BUF zero-copy unavailable ({e}); using MemFd");
                    return None;
                }
            };
            let mut count: c_int = 0;
            if (api.egl_query_dmabuf_modifiers_ext)(
                dpy,
                DRM_FORMAT_XRGB8888 as c_int,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut count,
            ) == EGL_FALSE
                || count <= 0
            {
                return None;
            }
            let mut mods = vec![0u64; count as usize];
            let mut ext_only = vec![0u32; count as usize];
            if (api.egl_query_dmabuf_modifiers_ext)(
                dpy,
                DRM_FORMAT_XRGB8888 as c_int,
                count,
                mods.as_mut_ptr(),
                ext_only.as_mut_ptr(),
                &mut count,
            ) == EGL_FALSE
            {
                return None;
            }
            // Only offer non-external-only modifiers so we can bind directly to GL_TEXTURE_2D
            // and GPU-scale with glBlitFramebuffer.
            let usable: Vec<u64> = mods
                .into_iter()
                .zip(ext_only.into_iter())
                .take(count as usize)
                .filter(|&(_, ext)| ext == EGL_FALSE)
                .map(|(m, _)| m)
                .collect();
            if usable.is_empty() {
                None
            } else {
                eprintln!(
                    "clipsd-linux: NVIDIA EGL DMA-BUF ready ({} tiled modifiers)",
                    usable.len()
                );
                Some(usable)
            }
        }
    })
    .clone()
}

/// Thread-affine EGL/OpenGL → CUDA bridge. Must be created, used, and dropped on the same thread
/// (the video encode thread).
pub struct GpuCudaBridge {
    api: &'static GpuApi,
    egl_dpy: *mut c_void,
    egl_ctx: *mut c_void,
    cuda_ctx: *mut c_void,
    cuda_stream: *mut c_void,
    dst_width: u32,
    dst_height: u32,
    src_tex: c_uint,
    src_fbo: c_uint,
    dst_tex: c_uint,
    dst_fbo: c_uint,
    cuda_res: *mut c_void,
    has_frame: bool,
}

impl GpuCudaBridge {
    pub fn new(
        dst_width: u32,
        dst_height: u32,
        cuda_ctx: *mut c_void,
        cuda_stream: *mut c_void,
    ) -> Result<Self, String> {
        let api = load_api()?;
        unsafe {
            let egl_dpy = open_nvidia_egl_display(api)?;
            if (api.egl_bind_api)(EGL_OPENGL_API) == EGL_FALSE {
                return Err("eglBindAPI(OPENGL) failed".into());
            }
            let cfg_attrs = [
                EGL_RENDERABLE_TYPE,
                EGL_OPENGL_BIT,
                EGL_SURFACE_TYPE,
                EGL_PBUFFER_BIT,
                EGL_NONE,
            ];
            let mut cfg = ptr::null_mut();
            let mut ncfg = 0;
            if (api.egl_choose_config)(egl_dpy, cfg_attrs.as_ptr(), &mut cfg, 1, &mut ncfg) == EGL_FALSE
                || ncfg == 0
            {
                return Err("eglChooseConfig failed".into());
            }
            let ctx_attrs = [
                EGL_CONTEXT_MAJOR_VERSION,
                3,
                EGL_CONTEXT_MINOR_VERSION,
                3,
                EGL_NONE,
            ];
            let egl_ctx = (api.egl_create_context)(egl_dpy, cfg, ptr::null_mut(), ctx_attrs.as_ptr());
            if egl_ctx.is_null() {
                return Err(format!("eglCreateContext failed (err=0x{:x})", (api.egl_get_error)()));
            }
            if (api.egl_make_current)(egl_dpy, ptr::null_mut(), ptr::null_mut(), egl_ctx) == EGL_FALSE {
                (api.egl_destroy_context)(egl_dpy, egl_ctx);
                return Err("eglMakeCurrent failed".into());
            }

            let (mut src_tex, mut dst_tex) = (0u32, 0u32);
            (api.gl_gen_textures)(1, &mut src_tex);
            (api.gl_bind_texture)(GL_TEXTURE_2D, src_tex);
            (api.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            (api.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            (api.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            (api.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);

            (api.gl_gen_textures)(1, &mut dst_tex);
            (api.gl_bind_texture)(GL_TEXTURE_2D, dst_tex);
            (api.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            (api.gl_tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            (api.gl_tex_image_2d)(
                GL_TEXTURE_2D,
                0,
                GL_RGBA8,
                dst_width as c_int,
                dst_height as c_int,
                0,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                ptr::null(),
            );
            (api.gl_bind_texture)(GL_TEXTURE_2D, 0);

            let (mut src_fbo, mut dst_fbo) = (0u32, 0u32);
            (api.gl_gen_framebuffers)(1, &mut src_fbo);
            (api.gl_gen_framebuffers)(1, &mut dst_fbo);
            (api.gl_bind_framebuffer)(GL_FRAMEBUFFER, dst_fbo);
            (api.gl_framebuffer_texture_2d)(
                GL_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                dst_tex,
                0,
            );
            let st = (api.gl_check_framebuffer_status)(GL_FRAMEBUFFER);
            (api.gl_bind_framebuffer)(GL_FRAMEBUFFER, 0);
            if st != GL_FRAMEBUFFER_COMPLETE {
                return Err(format!("dst FBO incomplete (0x{st:x})"));
            }

            // Register `dst_tex` into FFmpeg's CUDA context once for the whole session.
            let rc = (api.cu_ctx_push_current)(cuda_ctx);
            if rc != 0 {
                return Err(format!("cuCtxPushCurrent failed ({rc})"));
            }
            let mut cuda_res = ptr::null_mut();
            let reg_rc = (api.cu_graphics_gl_register_image)(
                &mut cuda_res,
                dst_tex,
                GL_TEXTURE_2D,
                CU_GRAPHICS_REGISTER_FLAGS_READ_ONLY,
            );
            let mut popped = ptr::null_mut();
            let _ = (api.cu_ctx_pop_current)(&mut popped);
            if reg_rc != 0 {
                return Err(format!("cuGraphicsGLRegisterImage failed ({reg_rc})"));
            }

            Ok(Self {
                api,
                egl_dpy,
                egl_ctx,
                cuda_ctx,
                cuda_stream,
                dst_width,
                dst_height,
                src_tex,
                src_fbo,
                dst_tex,
                dst_fbo,
                cuda_res,
                has_frame: false,
            })
        }
    }

    /// Import `dmabuf` into EGL/GL, GPU-scale it into `dst_tex`, and VRAM-copy to `dst_dev_ptr`.
    pub fn import_to_cuda_frame(
        &mut self,
        dmabuf: &DmaBufFrame,
        dst_dev_ptr: u64,
        dst_pitch: usize,
    ) -> Result<(), String> {
        if dmabuf.planes.is_empty() || dmabuf.planes.len() > 4 {
            return Err(format!("unsupported DMA-BUF plane count {}", dmabuf.planes.len()));
        }
        unsafe {
            let mut attrs = Vec::with_capacity(6 + dmabuf.planes.len() * 10 + 1);
            attrs.extend_from_slice(&[
                EGL_WIDTH,
                dmabuf.width as c_int,
                EGL_HEIGHT,
                dmabuf.height as c_int,
                EGL_LINUX_DRM_FOURCC_EXT,
                dmabuf.fourcc as c_int,
            ]);
            for (i, plane) in dmabuf.planes.iter().enumerate() {
                let pa = PLANE_ATTRS[i];
                attrs.extend_from_slice(&[
                    pa[0],
                    plane.fd.as_raw_fd(),
                    pa[1],
                    plane.offset as c_int,
                    pa[2],
                    plane.stride as c_int,
                ]);
                if dmabuf.modifier != DRM_FORMAT_MOD_INVALID {
                    attrs.extend_from_slice(&[
                        pa[3],
                        (dmabuf.modifier & 0xffff_ffff) as u32 as c_int,
                        pa[4],
                        (dmabuf.modifier >> 32) as u32 as c_int,
                    ]);
                }
            }
            attrs.push(EGL_NONE);

            let img = (self.api.egl_create_image_khr)(
                self.egl_dpy,
                ptr::null_mut(),
                EGL_LINUX_DMA_BUF_EXT,
                ptr::null_mut(),
                attrs.as_ptr(),
            );
            if img.is_null() {
                return Err(format!(
                    "eglCreateImageKHR(DMA_BUF) failed (err=0x{:x}, {}x{}, fourcc=0x{:x}, mod=0x{:x})",
                    (self.api.egl_get_error)(),
                    dmabuf.width,
                    dmabuf.height,
                    dmabuf.fourcc,
                    dmabuf.modifier
                ));
            }

            (self.api.gl_bind_texture)(GL_TEXTURE_2D, self.src_tex);
            (self.api.gl_egl_image_target_texture_2d_oes)(GL_TEXTURE_2D, img);

            (self.api.gl_bind_framebuffer)(GL_READ_FRAMEBUFFER, self.src_fbo);
            (self.api.gl_framebuffer_texture_2d)(
                GL_READ_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                self.src_tex,
                0,
            );
            (self.api.gl_bind_framebuffer)(GL_DRAW_FRAMEBUFFER, self.dst_fbo);
            (self.api.gl_blit_framebuffer)(
                0,
                0,
                dmabuf.width as c_int,
                dmabuf.height as c_int,
                0,
                0,
                self.dst_width as c_int,
                self.dst_height as c_int,
                GL_COLOR_BUFFER_BIT,
                GL_LINEAR as c_uint,
            );
            // Flush GL commands; cuGraphicsMapResources synchronizes GL->CUDA on the GPU without
            // stalling the CPU, and the kernel/driver refcount keeps the DMA-BUF alive until done.
            (self.api.gl_flush)();

            (self.api.gl_framebuffer_texture_2d)(
                GL_READ_FRAMEBUFFER,
                GL_COLOR_ATTACHMENT0,
                GL_TEXTURE_2D,
                0,
                0,
            );
            (self.api.gl_bind_framebuffer)(GL_READ_FRAMEBUFFER, 0);
            (self.api.gl_bind_framebuffer)(GL_DRAW_FRAMEBUFFER, 0);
            (self.api.gl_bind_texture)(GL_TEXTURE_2D, 0);
            (self.api.egl_destroy_image_khr)(self.egl_dpy, img);

            let gl_err = (self.api.gl_get_error)();
            if gl_err != 0 {
                return Err(format!("GL blit error 0x{gl_err:x}"));
            }
            self.has_frame = true;
        }
        self.copy_last_to_cuda_frame(dst_dev_ptr, dst_pitch)
    }

    /// Copy the already-scaled `dst_tex` VRAM surface into an FFmpeg CUDA `AVFrame` (also used
    /// for zero-cost static-screen frame repeats).
    pub fn copy_last_to_cuda_frame(
        &self,
        dst_dev_ptr: u64,
        dst_pitch: usize,
    ) -> Result<(), String> {
        if !self.has_frame {
            return Err("no GPU frame rendered yet".into());
        }
        unsafe {
            let rc = (self.api.cu_ctx_push_current)(self.cuda_ctx);
            if rc != 0 {
                return Err(format!("cuCtxPushCurrent failed ({rc})"));
            }
            let res = self.copy_locked(dst_dev_ptr, dst_pitch);
            let mut popped = ptr::null_mut();
            let _ = (self.api.cu_ctx_pop_current)(&mut popped);
            res
        }
    }

    unsafe fn copy_locked(&self, dst_dev_ptr: u64, dst_pitch: usize) -> Result<(), String> {
        let mut res_handle = self.cuda_res;
        let rc = (self.api.cu_graphics_map_resources)(1, &mut res_handle, self.cuda_stream);
        if rc != 0 {
            return Err(format!("cuGraphicsMapResources failed ({rc})"));
        }
        let mut arr = ptr::null_mut();
        let arr_rc = (self.api.cu_graphics_sub_resource_get_mapped_array)(&mut arr, res_handle, 0, 0);
        let copy_rc = if arr_rc == 0 {
            let cp = CudaMemcpy2D {
                src_x_in_bytes: 0,
                src_y: 0,
                src_memory_type: CU_MEMORYTYPE_ARRAY,
                _pad0: 0,
                src_host: ptr::null(),
                src_device: 0,
                src_array: arr,
                src_pitch: 0,
                dst_x_in_bytes: 0,
                dst_y: 0,
                dst_memory_type: CU_MEMORYTYPE_DEVICE,
                _pad1: 0,
                dst_host: ptr::null_mut(),
                dst_device: dst_dev_ptr,
                dst_array: ptr::null_mut(),
                dst_pitch,
                width_in_bytes: self.dst_width as usize * 4,
                height: self.dst_height as usize,
            };
            (self.api.cu_memcpy_2d_async)(&cp, self.cuda_stream)
        } else {
            arr_rc
        };
        let unmap_rc = (self.api.cu_graphics_unmap_resources)(1, &mut res_handle, self.cuda_stream);
        if arr_rc != 0 {
            return Err(format!("cuGraphicsSubResourceGetMappedArray failed ({arr_rc})"));
        }
        if copy_rc != 0 {
            return Err(format!("cuMemcpy2DAsync_v2 failed ({copy_rc})"));
        }
        if unmap_rc != 0 {
            return Err(format!("cuGraphicsUnmapResources failed ({unmap_rc})"));
        }
        Ok(())
    }
}

impl Drop for GpuCudaBridge {
    fn drop(&mut self) {
        unsafe {
            if !self.cuda_res.is_null() && (self.api.cu_ctx_push_current)(self.cuda_ctx) == 0 {
                let _ = (self.api.cu_graphics_unregister_resource)(self.cuda_res);
                let mut popped = ptr::null_mut();
                let _ = (self.api.cu_ctx_pop_current)(&mut popped);
            }
            if !self.egl_ctx.is_null() {
                let _ = (self.api.egl_make_current)(
                    self.egl_dpy,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    self.egl_ctx,
                );
                if self.src_fbo != 0 {
                    (self.api.gl_delete_framebuffers)(1, &self.src_fbo);
                }
                if self.dst_fbo != 0 {
                    (self.api.gl_delete_framebuffers)(1, &self.dst_fbo);
                }
                if self.src_tex != 0 {
                    (self.api.gl_delete_textures)(1, &self.src_tex);
                }
                if self.dst_tex != 0 {
                    (self.api.gl_delete_textures)(1, &self.dst_tex);
                }
                let _ = (self.api.egl_make_current)(
                    self.egl_dpy,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                let _ = (self.api.egl_destroy_context)(self.egl_dpy, self.egl_ctx);
            }
            let _ = self.api.egl_terminate; // Keep display shared/valid across restarts on NVIDIA
        }
    }
}
