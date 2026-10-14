# Fetch Lecture

东南大学研究生素质讲座预约桌面端，使用 Tauri 2、Rust、Vue 3 和 Tailwind CSS 2 构建。

## 功能

- 东南大学统一身份认证与短信二次验证
- 浏览讲座和已预约记录，支持手动、定时自动预约及结果确认
- Rust 下载验证码图片，并通过本机 ONNX Runtime 和模型进行 OCR
- 预约过程日志、名额满时等待和自动重试

密码仅在当前进程内用于认证，不会写入账号文件。随机设备标识保存在本机 WebView 存储中，用于复用学校的可信设备记录。

## 本地开发

需要 Node.js 22 和 Rust stable。Linux 构建依赖请参考 [Tauri 2 前置要求](https://v2.tauri.app/start/prerequisites/)。

```sh
npm ci
npm run tauri dev
```

构建当前平台安装包：

```sh
npm run tauri build
```

## 自动发布

向 GitHub 推送 `v*` 版本标签后，`.github/workflows/release.yml` 会构建并发布 Windows x64、macOS arm64 和 Linux x64 安装包。macOS 构建使用 ad-hoc 签名；首次打开时，系统可能要求在“隐私与安全性”中批准应用。

## OCR 资源与许可

OCR 模型和字符集来自 [ddddocr](https://github.com/sml2h3/ddddocr)，遵循 MIT 许可。ONNX Runtime 由 Microsoft 提供，遵循 MIT 许可。对应许可证、来源说明和第三方声明随应用资源保留。运行库覆盖 Windows x64、macOS arm64 和 Linux x64。

应用源码遵循仓库根目录的 MIT License。`images/` 保留原仓库的参考截图，不参与桌面应用构建。