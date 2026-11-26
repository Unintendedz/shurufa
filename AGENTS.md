# AGENTS.md

给在本仓库工作的编码代理的操作约定。架构边界、模块职责与本地验证体系见 [ARCHITECTURE.md](docs/ARCHITECTURE.md)——改动前请先阅读，本文档聚焦日常工作流与本机工程纪律。

## 项目定位与主干开发

- **独立演进项目**：Shurufa（`Unintendedz/shurufa`）是从水杉输入法（`metasequoiaime/msime`）分叉出的纯 macOS 个人输入法（分叉点 `fork-point/msime-2026-10-27`），**不跟随上游**。上游之后的提交一律不合并、不 cherry-pick、不 rebase，也不主动对比或建议同步上游；`upstream` 远端仅作历史查阅。
- **单主干本地验证**：主干为 `main`，没有 `develop` 分支、没有 `release/*` 分支，也不使用 GitHub Actions 工作流；所有代码质量与功能正确性完全依赖本地门禁与本机装机验证。
- **本机构建、签名与强制部署**：
  - 构建使用 `platforms/macos/build-shurufa.sh`，安装使用 `platforms/macos/scripts/install-shurufa.sh`（原子部署至 `~/Library/Input Methods/Shurufa.app`）。
  - 构建默认使用登录钥匙串中的本机自签名证书「`Shurufa Local Signing`」签名，保持授权身份稳定，重编重装后无需重新授予麦克风、辅助功能或输入监控权限；若新环境缺少该证书，`build-shurufa.sh` 会自动调用 `platforms/macos/scripts/create-shurufa-signing-identity.sh` 创建。语音存档加密密钥在首次录音时自动写入登录钥匙串（`dev.shurufa.voice-archive`）。
  - **每次合入 `main` 后必须直接部署到本机并 `git push origin main`，不用先问**：影响产物的改动跑完 `build-shurufa.sh` + `install-shurufa.sh` 并核验签名与包内容（仅改文档、笔记或测试时可跳过重编重装，仍须 push）；未部署或未 push 均不算完成。

## 改动范围与契约检查

- 只暂存明确列出的路径，**禁止使用 `git add -A`**。
- 不复制相邻仓库的未提交内容；不擅自修改会改变用户手感的出厂默认值，若确需调整须获得维护者明确授权，并记录在决策笔记中。
- 日志、测试与提交记录严禁包含真实用户输入、凭据或私人隐私数据，测试用例一律使用合成数据。
- Rust 改动须通过相关单测、`cargo fmt --all -- --check` 与 `cargo clippy --workspace --lib --no-deps -- -D warnings -A clippy::chunks_exact_to_as_chunks`；macOS 原生宿主改动须通过 `target/macos-isolated` 对应 CTest；提交前运行 `bash scripts/verify-local.sh --quick` 与 `git diff --check`。
- 新增文本契约检查时只需编写 `scripts/test-<name>.py`：`scripts/run-checks.sh` 会自动发现并执行（缺工具链或离线输入时打印 `skipped: <原因>` 并以 0 退出）；确实不能无参运行的检查须按 `名字=执行脚本或说明文档` 写入 `special_checks` 接受校验。

## 工具链纪律

- **`python3` 须指向 3.11 以上版本**：系统 CommandLineTools 自带 `python3` 为 3.9，契约脚本需要 3.11+ 特性。执行验证与构建前优先将 Homebrew `python@3.14` 置于 `PATH` 最前（`export PATH="$(brew --prefix python@3.14)/bin:$PATH"`）；`scripts/verify-local.sh` 与 `build-shurufa.sh` 已内置相应优先探测。
- **严禁在自动化命令中调用 `lldb` 或 `sample`**：在 macOS 上附加调试器会触发阻塞等待指纹解锁的「Developer Tools Access」系统弹窗。崩溃或断言定位一律直接运行测试二进制并查看标准输出 / 日志文件。
- **模拟器界面变化**：Xcode 新版本的模拟器图形界面已迁至 `/Applications/Xcode.app/Contents/Applications/DeviceHub.app`，但 CLI 控制与状态仍以 `xcrun simctl` 为准。

## 语言、提交与决策笔记

- **语言**：本仓库新增与修改的所有代码注释、文档注释（Rust / C++ / Objective-C++ / Swift / Python / Shell）、`.agents/notes/` 笔记正文以及评审说明**一律使用简体中文**；代码标识符、命令、路径、错误码与配置键保持原样并用反引号括起。不要为了改语言批量重写未改动代码。
- **提交**：提交信息遵循 Conventional Commits 格式 `type(scope): 摘要`，每个可独立验证的切片单独提交；**严禁**添加任何自动生成标记、AI 署名或 `Co-Authored-By` 水印。
- **决策笔记**：非平凡改动（架构边界、跨层契约、排序/解码/时序状态机、配置或存储格式）须依据 `.agents/skills/write-notes-like-deepseek/SKILL.md` 维护 `.agents/notes/{proposed,implemented,rejected}/<class>/yyyy-mm-dd-topic.md`：
  - 动手前先检索 `.agents/notes/` 避免重复造篇，有归属就地更新并维护互链；
  - 新方案先落 `proposed/`（含 `## Problem`、`## Proposal`、`## Alternatives considered`、`## Acceptance criteria`、`## Risks`），随实现落地转为 `implemented/`（使用 `## Decision` 与 `## Consequences`，移除提案态标题）；
  - 被放弃的方案先写其最强理由，再写放弃原因；确实无真实备选时在 `## Alternatives considered` 下写「无」；
  - 提交前须通过 `pnpm run verify-notes`。

## Worktree 与磁盘纪律

- 若启用独立 worktree，一律创建在仓库外部的 `~/worktrees/<feature>/` 目录，**严禁创建在 `/tmp`、`/var/tmp` 或仓库内部**，并在合并或废弃后及时执行 `git worktree remove`、`git branch -D` 与 `git worktree prune`。
- **严禁另起构建产物目录**：Cargo 构建产物目录固定为 `target/macos-cargo`（通过 `CARGO_TARGET_DIR="$PWD/target/macos-cargo"` 指定），CMake 构建目录固定为 `target/macos-isolated`（`MSIME_MACOS_BUILD` 默认值），防止重复编译依赖树耗尽磁盘空间。
- **构建前检查磁盘余量**：开始大规模编译前先运行 `df -h /System/Volumes/Data`（或 `df -h ~`）确认剩余空间充足；若余量低于 5 GiB 须立即停下清理旧产物后再继续。清理并行 worktree 产物前，须用 `lsof +D` 与 `pgrep` 确认整个工作目录无活跃进程后再删除。
