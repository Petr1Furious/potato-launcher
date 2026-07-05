use std::fmt::Display;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmGc {
    G1GC,
    ZGC,
}

impl Display for JvmGc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JvmGc::G1GC => write!(f, "G1GC"),
            JvmGc::ZGC => write!(f, "ZGC"),
        }
    }
}

pub const ZGC_MIN_JVM_VERSION: u64 = 21;

pub fn get_default_gc(java_major: u64, xmx_mb: u64) -> JvmGc {
    if java_major >= ZGC_MIN_JVM_VERSION && xmx_mb >= 8192 {
        JvmGc::ZGC
    } else {
        JvmGc::G1GC
    }
}
