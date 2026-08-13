import { invoke } from "@tauri-apps/api/core";

export function isHlsUrl(url: string | null | undefined): boolean {
  const lower = String(url || "").toLowerCase();
  if (!lower) return false;
  return (
    lower.includes(".m3u8") ||
    lower.includes("pull-hls") ||
    lower.includes("/hls?url=") ||
    lower.includes("/hls-seg?url=")
  );
}

export function isLocalHlsProxyUrl(url: string | null | undefined): boolean {
  const value = String(url || "");
  return value.includes("/hls?url=") || value.includes("/hls-seg?url=");
}

export async function wrapHlsPlayUrl(upstream: string): Promise<{
  playUrl: string;
  upstream: string;
  usedLocalProxy: boolean;
}> {
  const trimmed = (upstream || "").trim();
  if (!trimmed) {
    return { playUrl: trimmed, upstream: trimmed, usedLocalProxy: false };
  }
  if (trimmed.startsWith("http://127.0.0.1") || trimmed.startsWith("http://localhost")) {
    return { playUrl: trimmed, upstream: trimmed, usedLocalProxy: isLocalHlsProxyUrl(trimmed) };
  }
  try {
    const baseRaw = await invoke<string>("start_static_proxy_server");
    const base = (baseRaw || "").replace(/\/$/, "");
    if (!base) {
      return { playUrl: trimmed, upstream: trimmed, usedLocalProxy: false };
    }
    return {
      playUrl: `${base}/hls?url=${encodeURIComponent(trimmed)}`,
      upstream: trimmed,
      usedLocalProxy: true,
    };
  } catch (e) {
    console.warn("[hlsProxy] wrapHlsPlayUrl failed, fallback to upstream:", e);
    return { playUrl: trimmed, upstream: trimmed, usedLocalProxy: false };
  }
}

export function hlsPlayUrlForMode(upstream: string, localPlayUrl: string | null, mode: "local" | "direct"): string {
  if (mode === "direct") return upstream;
  return localPlayUrl || upstream;
}
