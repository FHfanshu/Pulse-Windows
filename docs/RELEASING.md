# 发布 Pulse for Windows

## 流程

CI 在 Windows Server 2022 runner 上安装 Node 22 和 Rust stable，使用锁文件构建，运行 TypeScript 检查、前端构建和 Rust workspace 测试，再生成 Windows x64 NSIS 安装包、免安装 ZIP 与 SHA-256 校验表。`release-artifacts/` 中的文件上传为 `pulse-windows-x64` artifact，保留 14 天。

Release 工作流复用同一份 CI。任何检查或打包失败都会阻止 Release 上传。成功后创建或更新草稿，维护者审阅、安装验证后在 GitHub 点击 **Publish release**。已发布的同版本 Release 不允许被流水线覆盖。

不需要配置服务商密钥或个人访问令牌。GitHub 提供的 `GITHUB_TOKEN` 仅在上传草稿的任务中获得 `contents: write`；仓库设置必须允许 GitHub Actions 运行及该任务写入仓库。

## 准备版本

1. 同步 `package.json`、`package-lock.json`（含根 package）、`Cargo.toml` 中的 workspace 版本、`Cargo.lock` 中的 `pulse` / `pulse-core` 版本，以及 `src-tauri/tauri.conf.json`。设置页的版本展示当前仍是 `ui/src/settings/panes/AboutPane.tsx` 中的文本，也需同步。
2. 新建 `docs/releases/v<版本>.md`。它是 Release 正文的来源，应说明新增功能、修复、下载文件与已知限制。
3. 运行 `node scripts/check-release-version.mjs`。版本不一致时流水线会停止。
4. 依赖有变动（`Cargo.lock` 或 `package-lock.json`）时，安装 `cargo install --locked cargo-about --features cli`，在 `npm ci` 后运行 `node scripts/third-party-licenses.mjs`，提交更新后的 `THIRD_PARTY_LICENSES.md`。
5. 将改动提交并推送到 `main`。

当前 `0.1.0` 的正文已保存在 [releases/v0.1.0.md](releases/v0.1.0.md)。

## 触发发行

两种方式均生成草稿：

- **手动运行**：在 Actions → Release → Run workflow，选择 `main`，填写 `v0.1.0` 等匹配版本的 tag。草稿指向本次实际构建的提交；新 tag 随 Release 发布建立。
- **推送标签**：在已通过检查的目标提交上建立并推送 `v*` 标签。标签必须与构建版本严格一致。

```powershell
git tag v0.1.0 <待发布的提交 SHA>
git push origin v0.1.0
```

不要把旧版本标签移到新代码上。修复已发布版本应递增版本号、补充说明并重新发行。

重复运行相同版本只更新仍为草稿的 Release 正文、目标提交和附件。如果标签已经存在，它必须指向本次构建的提交；公开版本会使上传任务失败。先查看运行日志，再选择原标签或使用新的版本号。

## 发布前验证

- CI 与 Release 的构建任务成功，草稿所指提交与预期一致。
- 附件包含 `Pulse_<版本>_x64-setup.exe`、`Pulse_<版本>_windows-x64.zip` 和 `SHA256SUMS.txt`。
- 在 Windows 10 / 11 测试安装、启动、托盘、设置和卸载；CI 只验证构建与自动化测试，不代替桌面实机验证。
- 核对 SHA-256，检查版本说明，不将仅有解析测试的服务描述为已验证真实账号。
- 安装包包含 LICENSE、NOTICE、THIRD_PARTY_NOTICES.md 和 THIRD_PARTY_LICENSES.md，免安装包也保留这些文件。

安装包当前未配置代码签名，可能出现 SmartScreen 提示。应用内 updater 尚未实现；用户通过 Releases 手动升级。以后接入代码签名或自动更新时，应单独配置证书或签名密钥，不能仅修改文档来宣称已支持。

## 官方参考

- [Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/)
- [Tauri GitHub pipelines](https://v2.tauri.app/distribute/pipelines/github/)
- [GitHub reusable workflows](https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows)
