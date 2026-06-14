pub fn get_os_name() -> String {
    if cfg!(windows) {
        "windows".to_string()
    } else if cfg!(target_os = "macos") {
        "osx".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else {
        unimplemented!("Unsupported OS");
    }
}

pub fn get_system_arch() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "arm" => "arm32",
        arch => arch,
    }
    .to_string()
}
