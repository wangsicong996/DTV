use reqwest::Proxy;
use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener as TokioTcpListener;
use tokio::net::TcpStream as TokioTcpStream;
use tokio_socks::tcp::Socks5Stream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::{
    client_async_tls_with_config, connect_async, Connector, MaybeTlsStream, WebSocketStream,
};

static PROXY: OnceLock<Option<ParsedProxy>> = OnceLock::new();
static HTTP_FORWARDER_PORT: OnceLock<u16> = OnceLock::new();

#[derive(Debug, Clone)]
struct ParsedProxy {
    raw: String,
    kind: ProxyKind,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyKind {
    Socks5,
    Http,
}

pub fn init() {
    let _ = PROXY.set(parse_proxy());
    if let Some(p) = current() {
        eprintln!("[net_proxy] using proxy {}", p.raw);
        // HTTP 与 SOCKS 都起本机 HTTP 转发器，给 WebKitGTK WebView 用。
        start_http_proxy_forwarder();
        apply_webview_env();
    }
}

/// 有代理时返回本机 HTTP 转发地址，例如 `http://127.0.0.1:43123`。
pub fn webview_proxy_url() -> Option<String> {
    HTTP_FORWARDER_PORT
        .get()
        .copied()
        .map(|port| format!("http://127.0.0.1:{port}"))
}

const WEBVIEW_PROXY_BYPASS: &str = "127.0.0.1,localhost,::1,<local>";

fn apply_webview_env() {
    let Some(url) = webview_proxy_url() else {
        return;
    };
    std::env::set_var("http_proxy", &url);
    std::env::set_var("https_proxy", &url);
    std::env::set_var("HTTP_PROXY", &url);
    std::env::set_var("HTTPS_PROXY", &url);
    std::env::set_var("all_proxy", &url);
    std::env::set_var("ALL_PROXY", &url);
    std::env::set_var("no_proxy", WEBVIEW_PROXY_BYPASS);
    std::env::set_var("NO_PROXY", WEBVIEW_PROXY_BYPASS);
    eprintln!("[net_proxy] webview proxy {url} bypass={WEBVIEW_PROXY_BYPASS}");
}

fn current() -> Option<&'static ParsedProxy> {
    PROXY.get().and_then(|v| v.as_ref())
}

#[allow(dead_code)]
pub fn raw_url() -> Option<String> {
    current().map(|p| p.raw.clone())
}

fn parse_proxy() -> Option<ParsedProxy> {
    let mut args = std::env::args().skip(1);
    let mut from_args = None;
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--proxy-server=") {
            from_args = Some(value.to_string());
            break;
        }
        if arg == "--proxy-server" {
            from_args = args.next();
            break;
        }
    }
    let raw = from_args
        .or_else(|| std::env::var("DTV_PROXY").ok())
        .unwrap_or_default();
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return None;
    }
    parse_proxy_url(&raw)
}

fn parse_proxy_url(raw: &str) -> Option<ParsedProxy> {
    let first = raw.split(';').next().unwrap_or(raw).trim();
    if first.is_empty() {
        return None;
    }
    let normalized = if first.contains("://") {
        first.to_string()
    } else {
        format!("http://{first}")
    };
    let url = url::Url::parse(&normalized).ok()?;
    let host = url.host_str()?.to_string();
    let scheme = url.scheme().to_ascii_lowercase();
    let kind = match scheme.as_str() {
        "socks" | "socks5" | "socks5h" => ProxyKind::Socks5,
        "http" | "https" => ProxyKind::Http,
        _ => {
            eprintln!("[net_proxy] unsupported proxy scheme: {scheme}");
            return None;
        }
    };
    let port = url.port().unwrap_or(match kind {
        ProxyKind::Socks5 => 1080,
        ProxyKind::Http => 8080,
    });
    let username = if url.username().is_empty() {
        None
    } else {
        Some(url.username().to_string())
    };
    let password = url.password().map(|s| s.to_string());
    Some(ParsedProxy {
        raw: first.to_string(),
        kind,
        host,
        port,
        username,
        password,
    })
}

fn reqwest_proxy() -> Option<Proxy> {
    let parsed = current()?;
    let url = match parsed.kind {
        ProxyKind::Socks5 => {
            let mut url = parsed.raw.clone();
            if url.starts_with("socks5://") {
                url = url.replacen("socks5://", "socks5h://", 1);
            } else if url.starts_with("socks://") {
                url = url.replacen("socks://", "socks5h://", 1);
            }
            url
        }
        ProxyKind::Http => {
            let port = HTTP_FORWARDER_PORT.get().copied().unwrap_or(parsed.port);
            if HTTP_FORWARDER_PORT.get().is_some() {
                format!("http://127.0.0.1:{port}")
            } else {
                parsed.raw.clone()
            }
        }
    };
    let mut proxy = Proxy::all(&url).ok()?;
    if let Some(no_proxy) =
        reqwest::NoProxy::from_string("localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16")
    {
        proxy = proxy.no_proxy(Some(no_proxy));
    }
    Some(proxy)
}

pub fn apply(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let builder = builder
        .http1_only()
        .pool_max_idle_per_host(2)
        .pool_idle_timeout(Duration::from_secs(15));
    match reqwest_proxy() {
        Some(proxy) => builder.proxy(proxy),
        None => builder.no_proxy(),
    }
}

pub fn apply_blocking(
    builder: reqwest::blocking::ClientBuilder,
) -> reqwest::blocking::ClientBuilder {
    let builder = builder.http1_only();
    match reqwest_proxy() {
        Some(proxy) => builder.proxy(proxy),
        None => builder.no_proxy(),
    }
}

pub trait DtvProxyExt {
    fn dtv_proxy(self) -> Self;
}

impl DtvProxyExt for reqwest::ClientBuilder {
    fn dtv_proxy(self) -> Self {
        apply(self)
    }
}

impl DtvProxyExt for reqwest::blocking::ClientBuilder {
    fn dtv_proxy(self) -> Self {
        apply_blocking(self)
    }
}

fn ws_io_err(msg: impl std::fmt::Display) -> tokio_tungstenite::tungstenite::Error {
    tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        msg.to_string(),
    ))
}

pub async fn connect_ws<R>(
    request: R,
) -> Result<
    (
        WebSocketStream<MaybeTlsStream<TokioTcpStream>>,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    ),
    tokio_tungstenite::tungstenite::Error,
>
where
    R: IntoClientRequest + Unpin,
{
    let request = request.into_client_request()?;
    if current().is_none() {
        return connect_async(request).await;
    }
    connect_ws_via_proxy(request).await
}

async fn connect_ws_via_proxy(
    request: Request<()>,
) -> Result<
    (
        WebSocketStream<MaybeTlsStream<TokioTcpStream>>,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    let uri = request.uri().clone();
    let host = uri
        .host()
        .ok_or_else(|| ws_io_err("websocket url missing host"))?;
    let host = host.to_string();
    let scheme = uri.scheme_str().unwrap_or("ws");
    let tls = scheme.eq_ignore_ascii_case("wss") || scheme.eq_ignore_ascii_case("https");
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = connect_tcp_async(&host, port).await.map_err(ws_io_err)?;
    let connector = if tls {
        let tls_connector = native_tls::TlsConnector::new().map_err(ws_io_err)?;
        Some(Connector::NativeTls(tls_connector))
    } else {
        Some(Connector::Plain)
    };
    client_async_tls_with_config(request, tcp, None, connector).await
}

fn connect_authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn connect_status_ok(status_line: &str) -> bool {
    status_line
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code == "200")
}

fn build_connect_request(
    dest_host: &str,
    dest_port: u16,
    host_header: &str,
    proxy: &ParsedProxy,
) -> String {
    let authority = connect_authority(dest_host, dest_port);
    let mut header = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {host_header}\r\nProxy-Connection: Keep-Alive\r\nUser-Agent: Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36\r\n"
    );
    if let (Some(user), Some(pass)) = (&proxy.username, &proxy.password) {
        let token = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("{user}:{pass}").as_bytes(),
        );
        header.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    header.push_str("\r\n");
    header
}

fn split_http_headers(buf: &[u8]) -> Option<(usize, String)> {
    let pos = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let end = pos + 4;
    let text = String::from_utf8_lossy(&buf[..end]).to_string();
    Some((end, text))
}

fn host_header_candidates(dest_host: &str, dest_port: u16) -> Vec<String> {
    // Some HTTP proxies (Privoxy / 简易 HTTP inbound) 会把 Host 整段拿去解析 DNS。
    // `Host: example.com:443` 会变成查 `example.com:443`，返回 404 No such domain。
    vec![
        dest_host.to_string(),
        connect_authority(dest_host, dest_port),
    ]
}

async fn read_http_head(stream: &mut TokioTcpStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= 8192 {
            break;
        }
    }
    Ok(buf)
}

async fn connect_tcp_async(host: &str, port: u16) -> Result<TokioTcpStream, String> {
    let Some(proxy) = current() else {
        return TokioTcpStream::connect((host, port))
            .await
            .map_err(|e| e.to_string());
    };
    match proxy.kind {
        ProxyKind::Socks5 => {
            let proxy_addr = (proxy.host.as_str(), proxy.port);
            let dest = (host, port);
            let stream = if let (Some(user), Some(pass)) = (&proxy.username, &proxy.password) {
                Socks5Stream::connect_with_password(proxy_addr, dest, user, pass).await
            } else {
                Socks5Stream::connect(proxy_addr, dest).await
            }
            .map_err(|e| format!("socks5 connect failed: {e}"))?;
            Ok(stream.into_inner())
        }
        ProxyKind::Http => http_connect_async(host, port, proxy).await,
    }
}

async fn http_connect_async(
    dest_host: &str,
    dest_port: u16,
    proxy: &ParsedProxy,
) -> Result<TokioTcpStream, String> {
    let mut last_err = "http proxy CONNECT failed".to_string();
    for host_header in host_header_candidates(dest_host, dest_port) {
        match try_http_connect_async(dest_host, dest_port, &host_header, proxy).await {
            Ok(stream) => return Ok(stream),
            Err(err) => {
                eprintln!(
                    "[net_proxy] CONNECT {dest_host}:{dest_port} Host={host_header} failed: {err}"
                );
                last_err = err;
            }
        }
    }
    Err(last_err)
}

async fn try_http_connect_async(
    dest_host: &str,
    dest_port: u16,
    host_header: &str,
    proxy: &ParsedProxy,
) -> Result<TokioTcpStream, String> {
    let mut stream = TokioTcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|e| format!("http proxy connect failed: {e}"))?;
    let header = build_connect_request(dest_host, dest_port, host_header, proxy);
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let buf = read_http_head(&mut stream).await?;
    let (_, text) = split_http_headers(&buf).ok_or_else(|| {
        format!(
            "http proxy CONNECT failed: {}",
            String::from_utf8_lossy(&buf)
        )
    })?;
    let status = text.lines().next().unwrap_or("");
    if !connect_status_ok(status) {
        return Err(format!("http proxy CONNECT failed: {status}"));
    }
    Ok(stream)
}

fn start_http_proxy_forwarder() {
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("[net_proxy] http forwarder bind failed: {err}");
            return;
        }
    };
    if let Err(err) = listener.set_nonblocking(true) {
        eprintln!("[net_proxy] http forwarder set_nonblocking failed: {err}");
        return;
    }
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(err) => {
            eprintln!("[net_proxy] http forwarder addr failed: {err}");
            return;
        }
    };
    let _ = HTTP_FORWARDER_PORT.set(port);
    eprintln!("[net_proxy] http CONNECT forwarder 127.0.0.1:{port}");
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(serve_http_proxy_forwarder(listener));
        }
        Err(_) => {
            if let Err(err) = std::thread::Builder::new()
                .name("dtv-http-forwarder".into())
                .spawn(move || match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .build()
                {
                    Ok(rt) => rt.block_on(serve_http_proxy_forwarder(listener)),
                    Err(err) => eprintln!("[net_proxy] http forwarder runtime failed: {err}"),
                })
            {
                eprintln!("[net_proxy] http forwarder thread spawn failed: {err}");
            }
        }
    }
}

async fn serve_http_proxy_forwarder(listener: std::net::TcpListener) {
    match TokioTcpListener::from_std(listener) {
        Ok(listener) => run_http_proxy_forwarder(listener).await,
        Err(err) => eprintln!("[net_proxy] http forwarder tokio listen failed: {err}"),
    }
}

async fn run_http_proxy_forwarder(listener: TokioTcpListener) {
    loop {
        match listener.accept().await {
            Ok((inbound, _)) => {
                tokio::spawn(async move {
                    if let Err(err) = handle_forwarded_conn(inbound).await {
                        eprintln!("[net_proxy] http forwarder: {err}");
                    }
                });
            }
            Err(err) => {
                eprintln!("[net_proxy] http forwarder accept: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn handle_forwarded_conn(mut inbound: TokioTcpStream) -> Result<(), String> {
    let Some(proxy) = current() else {
        return Err("proxy missing".into());
    };
    let buf = read_http_head(&mut inbound).await?;
    let (header_end, text) = split_http_headers(&buf)
        .ok_or_else(|| "incomplete http request from reqwest".to_string())?;
    let leftover = buf[header_end..].to_vec();
    let first = text.lines().next().unwrap_or("");
    if first.to_ascii_uppercase().starts_with("CONNECT ") {
        let authority = first.split_whitespace().nth(1).unwrap_or("");
        let (dest_host, dest_port) = parse_connect_authority(authority)?;
        // SOCKS / HTTP 上游都走 connect_tcp_async
        let mut outbound = connect_tcp_async(&dest_host, dest_port).await?;
        inbound
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .map_err(|e| e.to_string())?;
        if !leftover.is_empty() {
            outbound
                .write_all(&leftover)
                .await
                .map_err(|e| e.to_string())?;
        }
        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
        return Ok(());
    }

    if proxy.kind == ProxyKind::Http {
        let mut outbound = TokioTcpStream::connect((proxy.host.as_str(), proxy.port))
            .await
            .map_err(|e| format!("http proxy connect failed: {e}"))?;
        outbound.write_all(&buf).await.map_err(|e| e.to_string())?;
        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
        return Ok(());
    }

    let (dest_host, dest_port, origin_req) = rewrite_absolute_http_request(&text)?;
    let mut outbound = connect_tcp_async(&dest_host, dest_port).await?;
    outbound
        .write_all(origin_req.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    if !leftover.is_empty() {
        outbound
            .write_all(&leftover)
            .await
            .map_err(|e| e.to_string())?;
    }
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
    Ok(())
}

fn rewrite_absolute_http_request(header_text: &str) -> Result<(String, u16, String), String> {
    let first = header_text.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let target = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("HTTP/1.1");

    let mut host_header: Option<String> = None;
    for line in header_text.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("host") {
                host_header = Some(value.trim().to_string());
                break;
            }
        }
    }

    let (host, port, path) = if let Ok(parsed) = url::Url::parse(target) {
        let host = parsed
            .host_str()
            .ok_or_else(|| format!("plain HTTP request missing host: {target}"))?
            .to_string();
        let port = parsed.port_or_known_default().unwrap_or(80);
        let mut path = parsed.path().to_string();
        if path.is_empty() {
            path = "/".to_string();
        }
        if let Some(query) = parsed.query() {
            path.push('?');
            path.push_str(query);
        }
        (host, port, path)
    } else {
        let host_port = host_header
            .as_deref()
            .ok_or_else(|| "plain HTTP request missing Host".to_string())?;
        let (host, port) = match parse_connect_authority(host_port) {
            Ok(v) => v,
            Err(_) => (host_port.trim().to_string(), 80),
        };
        (host, port, target.to_string())
    };

    let rest = header_text
        .split_once("\r\n")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    Ok((host, port, format!("{method} {path} {version}\r\n{rest}")))
}

fn parse_connect_authority(authority: &str) -> Result<(String, u16), String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, port_part) = rest
            .split_once("]:")
            .ok_or_else(|| format!("bad CONNECT authority: {authority}"))?;
        let port = port_part
            .parse::<u16>()
            .map_err(|_| format!("bad CONNECT port: {authority}"))?;
        return Ok((host.to_string(), port));
    }
    let (host, port_part) = authority
        .rsplit_once(':')
        .ok_or_else(|| format!("bad CONNECT authority: {authority}"))?;
    let port = port_part
        .parse::<u16>()
        .map_err(|_| format!("bad CONNECT port: {authority}"))?;
    Ok((host.to_string(), port))
}

pub fn connect_tcp_sync(host: &str, port: u16) -> std::io::Result<StdTcpStream> {
    let Some(proxy) = current() else {
        let stream = StdTcpStream::connect((host, port))?;
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        stream.set_write_timeout(Some(Duration::from_secs(20)))?;
        return Ok(stream);
    };
    match proxy.kind {
        ProxyKind::Socks5 => socks5_connect_sync(proxy, host, port),
        ProxyKind::Http => http_connect_sync(proxy, host, port),
    }
}

fn socks5_connect_sync(
    proxy: &ParsedProxy,
    dest_host: &str,
    dest_port: u16,
) -> std::io::Result<StdTcpStream> {
    let mut stream = StdTcpStream::connect((proxy.host.as_str(), proxy.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(20)))?;
    stream.set_write_timeout(Some(Duration::from_secs(20)))?;

    if proxy.username.is_some() {
        stream.write_all(&[0x05, 0x01, 0x02])?;
    } else {
        stream.write_all(&[0x05, 0x01, 0x00])?;
    }
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp)?;
    if resp[0] != 0x05 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "invalid socks5 version",
        ));
    }
    if resp[1] == 0x02 {
        let user = proxy.username.clone().unwrap_or_default();
        let pass = proxy.password.clone().unwrap_or_default();
        let mut auth = Vec::from([0x01, user.len() as u8]);
        auth.extend_from_slice(user.as_bytes());
        auth.push(pass.len() as u8);
        auth.extend_from_slice(pass.as_bytes());
        stream.write_all(&auth)?;
        let mut auth_resp = [0u8; 2];
        stream.read_exact(&mut auth_resp)?;
        if auth_resp[1] != 0x00 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "socks5 auth failed",
            ));
        }
    } else if resp[1] != 0x00 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "socks5 method not accepted",
        ));
    }

    let host_bytes = dest_host.as_bytes();
    if host_bytes.len() > 255 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "hostname too long",
        ));
    }
    let mut req = Vec::from([0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8]);
    req.extend_from_slice(host_bytes);
    req.extend_from_slice(&dest_port.to_be_bytes());
    stream.write_all(&req)?;

    let mut head = [0u8; 4];
    stream.read_exact(&mut head)?;
    if head[1] != 0x00 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("socks5 connect rejected: {}", head[1]),
        ));
    }
    match head[3] {
        0x01 => {
            let mut rest = [0u8; 6];
            stream.read_exact(&mut rest)?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len)?;
            let mut rest = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut rest)?;
        }
        0x04 => {
            let mut rest = [0u8; 18];
            stream.read_exact(&mut rest)?;
        }
        _ => {}
    }
    Ok(stream)
}

fn http_connect_sync(
    proxy: &ParsedProxy,
    dest_host: &str,
    dest_port: u16,
) -> std::io::Result<StdTcpStream> {
    let mut last_err = std::io::Error::new(std::io::ErrorKind::Other, "http proxy CONNECT failed");
    for host_header in host_header_candidates(dest_host, dest_port) {
        match try_http_connect_sync(proxy, dest_host, dest_port, &host_header) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_err = err,
        }
    }
    Err(last_err)
}

fn try_http_connect_sync(
    proxy: &ParsedProxy,
    dest_host: &str,
    dest_port: u16,
    host_header: &str,
) -> std::io::Result<StdTcpStream> {
    let mut stream = StdTcpStream::connect((proxy.host.as_str(), proxy.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(20)))?;
    stream.set_write_timeout(Some(Duration::from_secs(20)))?;
    let header = build_connect_request(dest_host, dest_port, host_header, proxy);
    stream.write_all(header.as_bytes())?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= 8192 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let status = text.lines().next().unwrap_or("");
    if !connect_status_ok(status) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("http proxy CONNECT failed: {status}"),
        ));
    }
    Ok(stream)
}
