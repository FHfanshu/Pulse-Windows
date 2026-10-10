# 发布 Pulse for Windows

## 流程

CI 在 Windows Server 2022 runner 上安装 Node 22 和 Rust stable，使用锁文件构建，运行 TypeScript 检查、前端构建和 Rust workspace 测试，再生成 Windows x64 NSIS 安装包、免安装 ZIP 与 SHA-256 校验表。`release-artifacts/` 中的文件上传为 `pulse-windows-x64` artifact，保留 14 天。

Release 工作流复用同一份 CI。任何检查或打包失败都会阻止 Release 上传。成功后创建或更新草稿，维护者审阅、安装验证后在 GitHub 点击 **Publish release**。已发布的同版本 Release 不允许被流水线覆盖。

Release 构建还会用更新签名密钥给安装包签名（见下文“应用内更新”）；普通 CI 与 Pull Request 不签名，也不需要密钥。除此之外不需要配置服务商密钥或个人访问令牌。GitHub 提供的 `GITHUB_TOKEN` 仅在上传草稿的任务中获得 `contents: write`；仓库设置必须允许 GitHub Actions 运行及该任务写入仓库。

## 准备版本

1. 同步 `package.json`、`package-lock.json`（含根 package）、`Cargo.toml` 中的 workspace 版本、`Cargo.lock` 中的 `pulse` / `pulse-core` 版本，以及 `src-tauri/tauri.conf.json`。设置页的版本来自应用本身，无需另行修改。
2. 新建 `docs/releases/v<版本>.md`。它是 Release 正文的来源，应说明新增功能、修复、下载文件与已知限制。
3. 运行 `node scripts/check-release-version.mjs`。版本不一致时流水线会停止。
4. 依赖有变动（`Cargo.lock` 或 `package-lock.json`）时，安装 `cargo install --locked cargo-about --features cli`，在 `npm ci` 后运行 `node scripts/third-party-licenses.mjs`，提交更新后的 `THIRD_PARTY_LICENSES.md`。
5. 将改动提交并推送到 `main`。

已有版本的正文保存在 [releases/](releases/)，例如 [releases/v0.1.2.md](releases/v0.1.2.md)。

## 触发发行

两种方式均生成草稿：

- **手动运行**：在 Actions → Release → Run workflow，选择 `main`，填写 `v0.1.1` 等匹配版本的 tag。草稿指向本次实际构建的提交；新 tag 随 Release 发布建立。
- **推送标签**：在已通过检查的目标提交上建立并推送 `v*` 标签。标签必须与构建版本严格一致。

```powershell
git tag v0.1.1 <待发布的提交 SHA>
git push origin v0.1.1
```

不要把旧版本标签移到新代码上。修复已发布版本应递增版本号、补充说明并重新发行。

重复运行相同版本只更新仍为草稿的 Release 正文、目标提交和附件。如果标签已经存在，它必须指向本次构建的提交；公开版本会使上传任务失败。先查看运行日志，再选择原标签或使用新的版本号。

## 发布前验证

- CI 与 Release 的构建任务成功，草稿所指提交与预期一致。
- 附件包含 `Pulse_<版本>_x64-setup.exe`、`Pulse_<版本>_x64-setup.exe.sig`、`latest.json`、`Pulse_<版本>_windows-x64.zip` 和 `SHA256SUMS.txt`。
- 打开 `latest.json`：`version` 与发行版本一致，`platforms.windows-x86_64.url` 指向本 Release 的安装包，`signature` 非空。
- 在 Windows 10 / 11 测试安装、启动、托盘、设置和卸载；CI 只验证构建与自动化测试，不代替桌面实机验证。
- 核对 SHA-256，检查版本说明，不将仅有解析测试的服务描述为已验证真实账号。
- 安装包包含 LICENSE、NOTICE、THIRD_PARTY_NOTICES.md 和 THIRD_PARTY_LICENSES.md，免安装包也保留这些文件。

安装包当前未配置代码签名，可能出现 SmartScreen 提示。以后接入代码签名时，应单独配置证书，不能仅修改文档来宣称已支持。

## 应用内更新

设置 → 关于 会读取 `https://github.com/FHfanshu/Pulse-Windows/releases/latest/download/latest.json`（最新的**已发布** Release 的附件；草稿不会被读到，所以维护者点击 Publish release 之后用户才会看到更新）。用户点击“更新到 X”后，应用下载安装包，用 `tauri.conf.json` 里的公钥验证 `.sig`，以 NSIS 的 passive 模式（只有进度条，无向导页，当前用户安装无需 UAC）安装，安装完成后由安装程序重新启动 Pulse。

- 更新签名密钥是 Tauri 的 minisign 密钥对，**与 Windows 代码签名无关**。公钥写在 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`；私钥在仓库密钥 `TAURI_SIGNING_PRIVATE_KEY`（密码为空，`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`）。丢失或更换私钥后，已安装的版本将无法验证新的更新，只能手动重装。
- Release 工作流向 CI 传入 `sign-updates: true`，构建时加上 `src-tauri/tauri.release.conf.json`（`createUpdaterArtifacts`）生成 `.sig`，再由 `scripts/make-latest-json.mjs` 按 `docs/releases/v<版本>.md` 与安装包签名写出 `latest.json`。Pull Request 的 CI 不签名。
- 本地试签：`$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw <密钥文件>`，然后 `npm run tauri build -- --config src-tauri/tauri.release.conf.json`。不要打印或提交私钥。
- 开发版本（debug）不检查更新；设置 `PULSE_UPDATE_FEED=<地址>` 可让开发版读取指定的 `latest.json`，仅用于试验设置页。
- 0.1.0 没有内置更新，需要手动安装 0.1.1 一次，之后即可应用内更新。

## 官方参考

- [Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/)
- [Tauri GitHub pipelines](https://v2.tauri.app/distribute/pipelines/github/)
- [GitHub reusable workflows](https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows)
