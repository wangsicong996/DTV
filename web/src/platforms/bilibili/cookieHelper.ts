import { invoke } from '@tauri-apps/api/core';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

export interface BilibiliCookieResult {
  cookie: string | null;
  hasSessdata: boolean;
  hasBiliJct: boolean;
}

export const BILIBILI_LOGIN_WINDOW_LABEL = 'bilibili-login';
export const BILIBILI_LOGIN_URL = 'https://passport.bilibili.com/login';

const normalizeCookieResult = (result: any): BilibiliCookieResult => ({
  cookie: result?.cookie ?? null,
  hasSessdata: Boolean(result?.hasSessdata),
  hasBiliJct: Boolean(result?.hasBiliJct),
});

export const getBilibiliCookies = async (labels?: string[]): Promise<BilibiliCookieResult> => {
  const result = await invoke<BilibiliCookieResult>('get_bilibili_cookie', { labels });
  return normalizeCookieResult(result);
};

export const bootstrapBilibiliCookies = async (): Promise<BilibiliCookieResult> => {
  const result = await invoke<BilibiliCookieResult>('bootstrap_bilibili_cookie');
  return normalizeCookieResult(result);
};

let bootstrapAttempted = false;
let bootstrapPromise: Promise<BilibiliCookieResult> | null = null;
let lastBootstrapResult: BilibiliCookieResult | null = null;

export const ensureBilibiliCookieBootstrap = async (): Promise<BilibiliCookieResult | null> => {
  if (bootstrapAttempted) {
    return lastBootstrapResult;
  }

  if (!bootstrapPromise) {
    bootstrapPromise = bootstrapBilibiliCookies()
      .then((result) => {
        lastBootstrapResult = result;
        bootstrapAttempted = true;
        return result;
      })
      .catch((err) => {
        bootstrapAttempted = true;
        lastBootstrapResult = null;
        throw err;
      })
      .finally(() => {
        bootstrapPromise = null;
      });
  }

  try {
    return await bootstrapPromise;
  } catch (err) {
    console.warn('[BilibiliCookie] Silent bootstrap failed:', err);
    return null;
  }
};

export const ensureBilibiliLoginWindow = async (): Promise<WebviewWindow> => {
  await invoke<string>('open_bilibili_login_window');

  const existing = await WebviewWindow.getByLabel(BILIBILI_LOGIN_WINDOW_LABEL);
  if (existing) {
    try {
      await existing.show();
      await existing.setFocus();
    } catch (e) {
      console.warn('[BilibiliCookie] Failed to focus login window:', e);
    }
    return existing;
  }

  throw new Error('创建登录窗口失败');
};

export const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export const extractRequiredFlags = (raw: string | null | undefined) => {
  if (!raw) {
    return { hasSessdata: false, hasBiliJct: false };
  }
  const normalized = raw
    .split(';')
    .map((segment) => segment.trim().toLowerCase())
    .filter(Boolean);

  const hasSessdata = normalized.some((segment) => segment.startsWith('sessdata='));
  const hasBiliJct = normalized.some((segment) => segment.startsWith('bili_jct='));

  return { hasSessdata, hasBiliJct };
};

export const hasRequiredCookies = (result: BilibiliCookieResult | null | undefined) => {
  if (!result) return false;
  return Boolean(result.cookie) && result.hasSessdata && result.hasBiliJct;
};
