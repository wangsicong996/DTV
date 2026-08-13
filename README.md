<div align="center">
  <img src="images/icon.png" alt="DTV" width="128">
  <h1>DTV</h1>
  <p>基于 Tauri 2.0 的 Linux 斗鱼、虎牙、抖音、bilibili 直播桌面客户端</p>
</div>

## 说明

> 支持本项目？可以前往底部 [打赏](#打赏)  
> 安卓版本：[`dtv_mobile`](https://github.com/chen-zeong/dtv_mobile)

1. 本项目基于 Tauri 2.0 开发，体积小，占用率低，实测可以在双核、4GB内存的电脑上流畅运行
2. 当前只维护 Linux `.deb` 包（以及可直接运行的 `dtv` 二进制）
3. 平台接口可能有访问频率限制，过于频繁的请求会触发验证码校验，建议合理使用搜索功能
4. 本项目仅供学习编程目的使用，未进行任何逆向工程
5. 本项目所有的直播版权都归属各个平台

### 支持平台

| 平台       | 直播流 | 弹幕  | 搜索   |
| -------- | --- | --- | ---- |
| 斗鱼       | ✅   | ✅   | ✅    |
| 虎牙       | ✅   | ✅   | ✅    |
| bilibili | ✅   | ✅   | ✅    |
| 抖音       | ✅   | ✅   | 仅房间号 |

## 功能

- 📺 平台支持：支持斗鱼、虎牙、bilibili、抖音直播
- 💬 弹幕显示：实时显示直播间弹幕，只显示聊天弹幕，不显示礼物等其他类型弹幕
- ⭐ 主播收藏：支持收藏喜欢的主播，支持收藏列表手动拖拽排序
- 🔁 数据同步：支持局域网一键同步或者json文件手动同步，可以与桌面端或者移动端同步数据
- 📋 系统支持：Linux（deb）
- 🛡️ 代理：支持 HTTP / SOCKS5（`DTV_PROXY` 或 `--proxy-server`）
- 🌓 主题切换：支持明暗主题切换

## 软件截图



<div align="center">
  <p>日间模式</p>
  <img src="images/iShot_light.webp" alt="win-日间模式" style="width: 100%; max-width: 800px; display: block; margin-left: auto; margin-right: auto;">
</div>

<br>

<div align="center">
  <p>夜间模式</p>
  <img src="images/iShot_dark.webp" alt="mac-夜间模式" style="width: 100%; max-width: 800px; display: block; margin-left: auto; margin-right: auto;">
</div>

<br>

<div align="center">
  <p>日间模式 - 播放器页面</p>
  <img src="images/iShot_light2.webp" alt="日间模式播放器页面" style="width: 100%; max-width: 800px; display: block; margin-left: auto; margin-right: auto;">
</div>

<br>

## 安装方式

GitHub Actions 会编译 Linux `.deb` 和 `dtv` 二进制，产物挂在对应 workflow 的 Artifacts 下，不再发布 Release。

也可以通过源码编译安装。若已有 Flatpak 的 Tauri 运行环境，解压后直接运行 `dtv` 二进制即可。

## 代理

本机 `127.0.0.1` / `localhost` / 局域网地址不进代理。

```bash
# 开发
DTV_PROXY=socks5://127.0.0.1:1080 pnpm tauri dev
# 或
pnpm tauri dev -- --proxy-server=socks5://127.0.0.1:1080

# 也支持 HTTP 代理
pnpm tauri dev -- --proxy-server=http://127.0.0.1:8080

# 直接跑二进制
DTV_PROXY=socks5://127.0.0.1:1080 ./dtv
./dtv --proxy-server=http://127.0.0.1:8080
```

## 编译

```bash
# 安装 protobuf、WebKitGTK 等系统依赖后再编译

# 安装依赖
pnpm install

# 开发调试
pnpm tauri dev

# 打包 Linux deb
pnpm tauri build
```

## 参考

- 斗鱼直播流获取参考了 [@wbt5/real-url](https://github.com/wbt5/real-url)  
- 抖音弹幕参考了[@saermart/DouyinLiveWebFetcher](https://github.com/saermart/DouyinLiveWebFetcher)
- 虎牙参考了https://github.com/liuchuancong/pure_live https://github.com/ihmily/DouyinLiveRecorder
- b站弹幕参考了https://github.com/xfgryujk/blivedm

## 打赏

软件完全免费，如果这个项目对你有帮助，欢迎打赏支持：

<div align="center">
  <img src="images/wechat.jpg" alt="微信赞赏码" width="260">
</div>
