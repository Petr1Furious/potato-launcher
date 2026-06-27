use std::env;

use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyEndpoint {
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    socks: bool,
}

/// Additional JVM args for Java to support HTTP and SOCKS proxies.
pub fn jvm_args_from_env() -> Vec<String> {
    match proxy_config_from_env() {
        Ok(Some(config)) => config.jvm_args(),
        Ok(None) => Vec::new(),
        Err(err) => {
            log::warn!("Ignoring invalid HTTP proxy environment variables: {err:#}");
            Vec::new()
        }
    }
}

struct ProxyConfig {
    http: Option<ProxyEndpoint>,
    https: Option<ProxyEndpoint>,
    no_proxy: Option<String>,
}

impl ProxyConfig {
    fn jvm_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(endpoint) = self.preferred_jvm_endpoint() {
            if endpoint.socks {
                args.push(format!("-DsocksProxyHost={}", endpoint.host));
                args.push(format!("-DsocksProxyPort={}", endpoint.port));
            } else {
                let http = self.http.as_ref().unwrap_or(endpoint);
                let https = self.https.as_ref().unwrap_or(http);
                args.push(format!("-Dhttp.proxyHost={}", http.host));
                args.push(format!("-Dhttp.proxyPort={}", http.port));
                args.push(format!("-Dhttps.proxyHost={}", https.host));
                args.push(format!("-Dhttps.proxyPort={}", https.port));
                push_proxy_auth_jvm_args(&mut args, "http", http);
                if https.host != http.host
                    || https.port != http.port
                    || https.username != http.username
                    || https.password != http.password
                {
                    push_proxy_auth_jvm_args(&mut args, "https", https);
                }
            }
        }

        if let Some(no_proxy) = &self.no_proxy {
            let converted = no_proxy_to_java_hosts(no_proxy);
            if !converted.is_empty() {
                args.push(format!("-Dhttp.nonProxyHosts={converted}"));
            }
        }

        args
    }

    fn preferred_jvm_endpoint(&self) -> Option<&ProxyEndpoint> {
        self.https
            .as_ref()
            .or(self.http.as_ref())
            .filter(|endpoint| !endpoint.socks)
            .or(self.http.as_ref())
            .or(self.https.as_ref())
    }
}

fn proxy_config_from_env() -> Result<Option<ProxyConfig>, ProxyParseError> {
    let http = read_proxy_endpoint(&["HTTP_PROXY", "http_proxy"])?;
    let https = read_proxy_endpoint(&["HTTPS_PROXY", "https_proxy"])?;
    let all = read_proxy_endpoint(&["ALL_PROXY", "all_proxy"])?;
    let no_proxy = read_env_any(&["NO_PROXY", "no_proxy"]);

    let http = http.or(all.clone());
    let https = https.or(all);

    if http.is_none() && https.is_none() {
        return Ok(None);
    }

    Ok(Some(ProxyConfig {
        http,
        https,
        no_proxy,
    }))
}

#[derive(Debug, thiserror::Error)]
#[error("invalid proxy URL {value:?}: {reason}")]
struct ProxyParseError {
    value: String,
    reason: String,
}

fn read_env_any(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_proxy_endpoint(keys: &[&str]) -> Result<Option<ProxyEndpoint>, ProxyParseError> {
    let Some(raw) = read_env_any(keys) else {
        return Ok(None);
    };
    parse_proxy_endpoint(&raw).map(Some)
}

fn parse_proxy_endpoint(raw: &str) -> Result<ProxyEndpoint, ProxyParseError> {
    let url = if raw.contains("://") {
        Url::parse(raw)
    } else {
        Url::parse(&format!("http://{raw}"))
    }
    .map_err(|err| ProxyParseError {
        value: raw.to_string(),
        reason: err.to_string(),
    })?;

    let host = url
        .host_str()
        .ok_or_else(|| ProxyParseError {
            value: raw.to_string(),
            reason: "missing host".to_string(),
        })?
        .to_string();
    let port = url.port_or_known_default().ok_or_else(|| ProxyParseError {
        value: raw.to_string(),
        reason: "missing port".to_string(),
    })?;
    let socks = matches!(url.scheme(), "socks5" | "socks5h");
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(ProxyParseError {
            value: raw.to_string(),
            reason: format!("unsupported proxy scheme: {}", url.scheme()),
        });
    }

    Ok(ProxyEndpoint {
        host,
        port,
        username: non_empty(url.username()),
        password: url.password().map(ToString::to_string),
        socks,
    })
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn push_proxy_auth_jvm_args(args: &mut Vec<String>, prefix: &str, endpoint: &ProxyEndpoint) {
    if let Some(username) = &endpoint.username {
        args.push(format!("-D{prefix}.proxyUser={username}"));
    }
    if let Some(password) = &endpoint.password {
        args.push(format!("-D{prefix}.proxyPassword={password}"));
    }
}

fn no_proxy_to_java_hosts(no_proxy: &str) -> String {
    no_proxy
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            if entry.starts_with('.') {
                format!("*{entry}")
            } else {
                entry.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("|")
}
