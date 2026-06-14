#[cfg(target_os = "windows")]
pub fn win_get_long_path_name(path: &str) -> std::io::Result<String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use winapi::um::fileapi::GetLongPathNameW;

    let path_wide: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
    let mut buf = vec![0u16; 32_768];
    let res = unsafe { GetLongPathNameW(path_wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if res == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(String::from_utf16_lossy(&buf[..res as usize]))
}

#[cfg(target_os = "linux")]
fn dlerror() -> String {
    use std::ffi::CStr;

    let error = unsafe { CStr::from_ptr(libc::dlerror()) };
    error.to_string_lossy().to_string()
}

#[cfg(target_os = "linux")]
pub fn linux_find_native_glfw() -> Result<String, NativeGlfwError> {
    // reference: https://github.com/unmojang/FjordLauncher/blob/6d0109357551bc29079da18543b7db61223c7f38/launcher/MangoHud.cpp#L141
    use std::ffi::{CStr, CString};

    let name = "libglfw.so";
    let name_cstr =
        CString::new(name).map_err(|_| NativeGlfwError::InvalidLibraryName(name.to_string()))?;
    let lib = unsafe { libc::dlopen(name_cstr.as_ptr(), libc::RTLD_NOW) };
    if lib.is_null() {
        return Err(NativeGlfwError::OpenFailed(dlerror()));
    }
    let mut path = [0u8; libc::PATH_MAX as usize + 1];
    if unsafe { libc::dlinfo(lib, libc::RTLD_DI_ORIGIN, path.as_mut_ptr() as _) } != 0 {
        return Err(NativeGlfwError::PathLookupFailed(dlerror()));
    }
    let origin = CStr::from_bytes_until_nul(&path)
        .map_err(|_| NativeGlfwError::PathLookupFailed("invalid origin path".to_string()))?
        .to_string_lossy()
        .to_string();
    Ok(format!("{origin}/{name}"))
}

#[derive(thiserror::Error, Debug)]
pub enum NativeGlfwError {
    #[error("invalid native GLFW library name: {0}")]
    InvalidLibraryName(String),
    #[error("failed to open native GLFW library: {0}")]
    OpenFailed(String),
    #[error("failed to resolve native GLFW library path: {0}")]
    PathLookupFailed(String),
}
