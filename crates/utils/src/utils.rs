use url::Url;

lazy_static::lazy_static! {
    pub static ref VANILLA_MANIFEST_URL: Url = Url::parse("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json").unwrap();
}

pub fn is_connect_error(e: &anyhow::Error) -> bool {
    if let Some(e) = e.downcast_ref::<reqwest::Error>() {
        return e.is_connect() || e.status().is_some_and(|s| s.as_u16() == 523);
        // 523 = Cloudflare Origin is Unreachable
    }

    // Check for connection-related error messages that cannot be checked by reqwest
    let error_str = format!("{e:?}");
    error_str.contains("peer closed connection without sending TLS close_notify")
        || error_str.contains("connection closed")
        || error_str.contains("connection reset")
        || error_str.contains("connection aborted")
        || error_str.contains("broken pipe")
        || error_str.contains("SendRequest")
        || error_str.contains("connection error")
        || error_str.contains("Connection refused")
        || error_str.contains("Network is unreachable")
        || error_str.contains("Connection timed out")
}
