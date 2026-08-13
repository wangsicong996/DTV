use actix_web::{dev::ServerHandle, web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use futures_util::TryStreamExt;
use reqwest::Client;
// awc removed for now due to API differences; using reqwest streaming
use crate::StreamUrlStore;
use crate::net_proxy::DtvProxyExt;
use serde::Deserialize;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tauri::{AppHandle, State};

// Define a struct to hold the server handle in a Tauri managed state
#[derive(Default)]
pub struct ProxyServerHandle(pub StdMutex<Option<ServerHandle>>);

// Align with pure_live-master's Huya playback UA
const HUYA_HYSDK_UA: &str =
    "HYSDK(Windows,30000002)_APP(pc_exe&7080000&official)_SDK(trans&2.34.0.5795)";

async fn find_free_port() -> u16 {
    // Using a fixed port as requested by the user for easier debugging
    34719
}

#[derive(Deserialize)]
struct ImageQuery {
    url: String,
}

async fn image_proxy_handler(
    query: web::Query<ImageQuery>,
    client: web::Data<Client>,
) -> impl Responder {
    let url = query.url.clone();
    if url.is_empty() {
        return HttpResponse::BadRequest().body("Missing url query parameter");
    }

    let mut req = client
        .get(&url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "image/avif,image/webp,image/apng,image/*;q=0.8,*/*;q=0.5",
        );

    // Set a Referer to bypass hotlink protections
    if url.contains("hdslb.com") || url.contains("bilibili.com") {
        req = req
            .header("Referer", "https://live.bilibili.com/")
            .header("Origin", "https://live.bilibili.com");
    } else if url.contains("huya.com") {
        req = req
            .header("Referer", "https://www.huya.com/")
            .header("Origin", "https://www.huya.com");
    } else if url.contains("douyin") || url.contains("douyinpic.com") {
        req = req.header("Referer", "https://www.douyin.com/");
    }

    match req.send().await {
        Ok(upstream_response) => {
            let content_type = upstream_response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();

            // 为避免 Windows 下 chunked 传输的 Early-EOF，改为一次性读取 bytes 并返回
            if upstream_response.status().is_success() {
                match upstream_response.bytes().await {
                    Ok(bytes) => HttpResponse::Ok()
                        .content_type(content_type)
                        .insert_header(("Content-Length", bytes.len().to_string()))
                        // Allow the WebView to cache proxied images to avoid re-downloading covers/avatars
                        // when switching routes or scrolling lists.
                        .insert_header(("Cache-Control", "public, max-age=86400, immutable"))
                        .body(bytes),
                    Err(e) => {
                        eprintln!("[Rust/proxy.rs image] Failed to read bytes: {}", e);
                        HttpResponse::InternalServerError()
                            .body(format!("Failed to read image bytes: {}", e))
                    }
                }
            } else {
                let status_from_reqwest = upstream_response.status();
                let error_text = upstream_response
                    .text()
                    .await
                    .unwrap_or_else(|e| format!("Failed to read error body from upstream: {}", e));
                eprintln!(
                    "[Rust/proxy.rs image] Upstream request to {} failed with status: {}. Body: {}",
                    url, status_from_reqwest, error_text
                );
                let actix_status_code =
                    actix_web::http::StatusCode::from_u16(status_from_reqwest.as_u16())
                        .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR);

                HttpResponse::build(actix_status_code).body(format!(
                    "Error fetching IMAGE from upstream (reqwest): {}. Status: {}. Details: {}",
                    url, status_from_reqwest, error_text
                ))
            }
        }
        Err(e) => {
            eprintln!(
                "[Rust/proxy.rs image] Failed to send request to upstream {}: {}",
                url, e
            );
            HttpResponse::InternalServerError()
                .body(format!("Error connecting to upstream IMAGE {}: {}", url, e))
        }
    }
}

fn hls_referer_headers(url: &str) -> (&'static str, &'static str) {
    if url.contains("huya.com") || url.contains("hy-cdn.com") || url.contains("huyaimg.com") {
        ("https://www.huya.com/", "https://www.huya.com")
    } else if url.contains("douyu.com") || url.contains("douyucdn") {
        ("https://www.douyu.com/", "https://www.douyu.com")
    } else if url.contains("douyin") || url.contains("byteimg") {
        ("https://www.douyin.com/", "https://www.douyin.com")
    } else {
        ("https://live.bilibili.com/", "https://live.bilibili.com")
    }
}

fn resolve_hls_uri(playlist_url: &url::Url, uri: &str) -> String {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    if let Ok(abs) = url::Url::parse(trimmed) {
        return abs.to_string();
    }
    playlist_url
        .join(trimmed)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| trimmed.to_string())
}

fn wrap_hls_uri(proxy_base: &str, playlist: bool, abs: &str) -> String {
    let path = if playlist { "hls" } else { "hls-seg" };
    format!(
        "{proxy_base}/{path}?url={}",
        urlencoding::encode(abs)
    )
}

fn rewrite_tag_uris(line: &str, playlist_url: &url::Url, proxy_base: &str, as_playlist: bool) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(idx) = rest.find("URI=\"") {
        out.push_str(&rest[..idx]);
        out.push_str("URI=\"");
        rest = &rest[idx + 5..];
        if let Some(end) = rest.find('"') {
            let uri = &rest[..end];
            let abs = resolve_hls_uri(playlist_url, uri);
            let looks_playlist = as_playlist || abs.contains(".m3u8");
            out.push_str(&wrap_hls_uri(proxy_base, looks_playlist, &abs));
            out.push('"');
            rest = &rest[end + 1..];
        } else {
            out.push_str(rest);
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn rewrite_hls_playlist(body: &str, playlist_url: &str, proxy_base: &str) -> String {
    let Ok(base) = url::Url::parse(playlist_url) else {
        return body.to_string();
    };
    let mut out = String::with_capacity(body.len() + 64);
    let mut next_is_playlist = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if trimmed.starts_with('#') {
            next_is_playlist = trimmed.starts_with("#EXT-X-STREAM-INF");
            if trimmed.contains("URI=\"") {
                let as_playlist = trimmed.starts_with("#EXT-X-MEDIA")
                    || trimmed.starts_with("#EXT-X-I-FRAME-STREAM-INF")
                    || trimmed.starts_with("#EXT-X-SESSION-DATA");
                out.push_str(&rewrite_tag_uris(trimmed, &base, proxy_base, as_playlist));
            } else {
                out.push_str(line);
            }
            out.push('\n');
            continue;
        }
        let abs = resolve_hls_uri(&base, trimmed);
        let playlist = next_is_playlist || abs.contains(".m3u8");
        next_is_playlist = false;
        out.push_str(&wrap_hls_uri(proxy_base, playlist, &abs));
        out.push('\n');
    }
    out
}

async fn hls_proxy_handler(
    req: HttpRequest,
    query: web::Query<ImageQuery>,
    client: web::Data<Client>,
) -> impl Responder {
    let url = query.url.clone();
    if url.is_empty() {
        return HttpResponse::BadRequest().body("Missing url query parameter");
    }
    let (referer, origin) = hls_referer_headers(&url);
    match client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Accept", "application/vnd.apple.mpegurl,application/x-mpegURL,*/*")
        .header("Referer", referer)
        .header("Origin", origin)
        .send()
        .await
    {
        Ok(upstream) => {
            if !upstream.status().is_success() {
                let status = upstream.status();
                let body = upstream.text().await.unwrap_or_default();
                let actix_status = actix_web::http::StatusCode::from_u16(status.as_u16())
                    .unwrap_or(actix_web::http::StatusCode::BAD_GATEWAY);
                return HttpResponse::build(actix_status).body(format!(
                    "Error fetching HLS playlist {}: {}. {}",
                    url, status, body
                ));
            }
            match upstream.text().await {
                Ok(body) => {
                    let conn = req.connection_info();
                    let proxy_base = format!("http://{}", conn.host());
                    let rewritten = rewrite_hls_playlist(&body, &url, &proxy_base);
                    HttpResponse::Ok()
                        .content_type("application/vnd.apple.mpegurl")
                        .insert_header(("Cache-Control", "no-store"))
                        .body(rewritten)
                }
                Err(e) => HttpResponse::InternalServerError()
                    .body(format!("Failed to read HLS playlist: {e}")),
            }
        }
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Error connecting to upstream HLS playlist {url}: {e}")),
    }
}

async fn hls_seg_proxy_handler(
    query: web::Query<ImageQuery>,
    client: web::Data<Client>,
) -> impl Responder {
    let url = query.url.clone();
    if url.is_empty() {
        return HttpResponse::BadRequest().body("Missing url query parameter");
    }
    let (referer, origin) = hls_referer_headers(&url);
    match client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Accept", "*/*")
        .header("Referer", referer)
        .header("Origin", origin)
        .send()
        .await
    {
        Ok(upstream) => {
            if !upstream.status().is_success() {
                let status = upstream.status();
                let body = upstream.text().await.unwrap_or_default();
                let actix_status = actix_web::http::StatusCode::from_u16(status.as_u16())
                    .unwrap_or(actix_web::http::StatusCode::BAD_GATEWAY);
                return HttpResponse::build(actix_status)
                    .body(format!("Error fetching HLS segment {url}: {status}. {body}"));
            }
            let content_type = upstream
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let mut response_builder = HttpResponse::Ok();
            response_builder
                .content_type(content_type)
                .insert_header(("Cache-Control", "no-store"));
            let byte_stream = upstream.bytes_stream().map_err(|e| {
                eprintln!("[Rust/proxy.rs hls-seg] upstream stream error: {e}");
                actix_web::error::ErrorInternalServerError(format!("Upstream stream error: {e}"))
            });
            response_builder.streaming(byte_stream)
        }
        Err(e) => HttpResponse::InternalServerError()
            .body(format!("Error connecting to upstream HLS segment {url}: {e}")),
    }
}

fn build_image_proxy_client() -> Result<Client, String> {
    Client::builder()
        .dtv_proxy()
        .http1_only()
        .gzip(false)
        .brotli(false)
        .no_deflate()
        .pool_idle_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(2)
        .tcp_keepalive(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build image proxy client: {e}"))
}

fn build_flv_proxy_client() -> Result<Client, String> {
    Client::builder()
        .dtv_proxy()
        .http1_only()
        .gzip(false)
        .brotli(false)
        .no_deflate()
        .pool_idle_timeout(Duration::from_secs(15))
        .pool_max_idle_per_host(1)
        .tcp_keepalive(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(7200))
        .build()
        .map_err(|e| format!("failed to build flv proxy client: {e}"))
}

// Your actual proxy logic - this is a simplified placeholder
async fn flv_proxy_handler(
    _req: HttpRequest,
    stream_url_store: web::Data<StreamUrlStore>,
    client: web::Data<Client>,
) -> impl Responder {
    let url = stream_url_store.url.lock().unwrap().clone();
    if url.is_empty() {
        return HttpResponse::NotFound().body("Stream URL is not set or empty.");
    }

    println!(
        "[Rust/proxy.rs handler] Incoming FLV proxy request -> {}",
        url
    );

    let mut req = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Accept", "video/x-flv,application/octet-stream,*/*")
        .header("Range", "bytes=0-")
        .header("Connection", "keep-alive");

    // 如果是虎牙域名，添加必要的 Referer/Origin 头
    if url.contains("huya.com") || url.contains("hy-cdn.com") || url.contains("huyaimg.com") {
        req = req
            .header("User-Agent", HUYA_HYSDK_UA)
            .header("Referer", "https://www.huya.com/")
            .header("Origin", "https://www.huya.com");
    }
    // 如果是B站域名，添加必要的 Referer 头
    if url.contains("bilivideo") || url.contains("bilibili.com") || url.contains("hdslb.com") {
        req = req.header("Referer", "https://live.bilibili.com/");
    }

    match req.send().await {
        Ok(upstream_response) => {
            if upstream_response.status().is_success() {
                let mut response_builder = HttpResponse::Ok();
                response_builder
                    .content_type("video/x-flv")
                    .insert_header(("Connection", "keep-alive"))
                    .insert_header(("Cache-Control", "no-store"))
                    .insert_header(("Accept-Ranges", "bytes"));

                let byte_stream = upstream_response.bytes_stream().map_err(|e| {
                    eprintln!(
                        "[Rust/proxy.rs handler] Error reading bytes from upstream: {}",
                        e
                    );
                    actix_web::error::ErrorInternalServerError(format!(
                        "Upstream stream error: {}",
                        e
                    ))
                });

                response_builder.streaming(byte_stream)
            } else {
                let status_from_reqwest = upstream_response.status(); // Renamed for clarity
                let error_text = upstream_response
                    .text()
                    .await
                    .unwrap_or_else(|e| format!("Failed to read error body from upstream: {}", e));
                eprintln!(
                    "[Rust/proxy.rs handler] Upstream request to {} failed with status: {}. Body: {}",
                    url, status_from_reqwest, error_text
                );
                // Convert reqwest::StatusCode to actix_web::http::StatusCode
                let actix_status_code =
                    actix_web::http::StatusCode::from_u16(status_from_reqwest.as_u16())
                        .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR);

                HttpResponse::build(actix_status_code).body(format!(
                    "Error fetching FLV stream from upstream (reqwest): {}. Status: {}. Details: {}",
                    url, status_from_reqwest, error_text
                ))
            }
        }
        Err(e) => {
            eprintln!(
                "[Rust/proxy.rs handler] Failed to send request to upstream {} with reqwest: {}",
                url, e
            );
            HttpResponse::InternalServerError().body(format!(
                "Error connecting to upstream FLV stream {} with reqwest: {}",
                url, e
            ))
        }
    }
}

#[tauri::command]
pub async fn start_proxy(
    _app_handle: AppHandle,
    server_handle_state: State<'_, ProxyServerHandle>,
    stream_url_store: State<'_, StreamUrlStore>,
) -> Result<String, String> {
    let port = find_free_port().await;
    let current_stream_url = stream_url_store.url.lock().unwrap().clone();

    if current_stream_url.is_empty() {
        return Err("Stream URL is not set in store. Cannot start proxy.".to_string());
    }

    // stream_url_data_for_actix can be created once and cloned, as StreamUrlStore is Arc based and Send + Sync
    let stream_url_data_for_actix = web::Data::new(stream_url_store.inner().clone());
    let app_data_reqwest_client = web::Data::new(build_flv_proxy_client()?);

    // Ensure MutexGuard is dropped before .await
    let existing_handle_to_stop = { server_handle_state.0.lock().unwrap().take() };
    if let Some(existing_handle) = existing_handle_to_stop {
        existing_handle.stop(false).await;
    }

    let server = match HttpServer::new(move || {
        App::new()
            .app_data(stream_url_data_for_actix.clone())
            .app_data(app_data_reqwest_client.clone())
            .wrap(actix_cors::Cors::permissive())
            .route("/live.flv", web::get().to(flv_proxy_handler))
            .route("/image", web::get().to(image_proxy_handler))
            .route("/hls", web::get().to(hls_proxy_handler))
            .route("/hls-seg", web::get().to(hls_seg_proxy_handler))
    })
    .keep_alive(Duration::from_secs(120))
    .bind(("127.0.0.1", port))
    {
        Ok(srv) => srv,
        Err(e) => {
            let err_msg = format!(
                "[Rust/proxy.rs] Failed to bind server to port {}: {}",
                port, e
            );
            eprintln!("{}", err_msg);
            return Err(err_msg);
        }
    }
    .run();

    let server_handle_for_state = server.handle();
    *server_handle_state.0.lock().unwrap() = Some(server_handle_for_state);

    // Use tauri::async_runtime::spawn directly
    tauri::async_runtime::spawn(async move {
        if let Err(e) = server.await {
            eprintln!("[Rust/proxy.rs] Proxy server run error: {}", e);
        } else {
            println!("[Rust/proxy.rs] Proxy server on port {} shut down.", port);
        }
    });

    let proxy_url = format!("http://127.0.0.1:{}/live.flv", port);
    Ok(proxy_url)
}

#[tauri::command]
pub async fn start_static_proxy_server(
    _app_handle: AppHandle,
    stream_url_store: State<'_, StreamUrlStore>,
) -> Result<String, String> {
    // Use a dedicated port for static image proxy to avoid interfering with FLV stream proxy
    let port: u16 = 34721;

    // If the server is already running, just return the base URL (idempotent behavior)
    if TcpStream::connect(("127.0.0.1", port)).is_ok() {
        return Ok(format!("http://127.0.0.1:{}", port));
    }

    let stream_url_data_for_actix = web::Data::new(stream_url_store.inner().clone());
    let app_data_reqwest_client = web::Data::new(build_image_proxy_client()?);

    let server = match HttpServer::new(move || {
        App::new()
            .app_data(stream_url_data_for_actix.clone())
            .app_data(app_data_reqwest_client.clone())
            .wrap(actix_cors::Cors::permissive())
            .route("/live.flv", web::get().to(flv_proxy_handler))
            .route("/image", web::get().to(image_proxy_handler))
            .route("/hls", web::get().to(hls_proxy_handler))
            .route("/hls-seg", web::get().to(hls_seg_proxy_handler))
    })
    .keep_alive(Duration::from_secs(120))
    .bind(("127.0.0.1", port))
    {
        Ok(srv) => srv,
        Err(e) => {
            // If address already in use, assume server is running and return OK base URL
            if e.kind() == ErrorKind::AddrInUse {
                eprintln!(
                    "[Rust/proxy.rs] Port {} already in use; assuming static proxy running.",
                    port
                );
                return Ok(format!("http://127.0.0.1:{}", port));
            }
            let err_msg = format!(
                "[Rust/proxy.rs] Failed to bind server to port {}: {}",
                port, e
            );
            eprintln!("{}", err_msg);
            return Err(err_msg);
        }
    }
    .run();

    // Do NOT overwrite the main proxy server handle; run static proxy independently

    tauri::async_runtime::spawn(async move {
        if let Err(e) = server.await {
            eprintln!("[Rust/proxy.rs] Proxy server run error: {}", e);
        } else {
            println!("[Rust/proxy.rs] Proxy server on port {} shut down.", port);
        }
    });

    Ok(format!("http://127.0.0.1:{}", port))
}

#[tauri::command]
pub async fn stop_proxy(server_handle_state: State<'_, ProxyServerHandle>) -> Result<(), String> {
    // Ensure MutexGuard is dropped before .await
    let handle_to_stop = { server_handle_state.0.lock().unwrap().take() };

    if let Some(handle) = handle_to_stop {
        handle.stop(false).await; // Changed to non-graceful shutdown
        println!("[Rust/proxy.rs] stop_proxy: Initiated non-graceful shutdown.");
    } else {
        println!("[Rust/proxy.rs] stop_proxy command: No proxy server was running or handle already taken.");
    }
    Ok(())
}
