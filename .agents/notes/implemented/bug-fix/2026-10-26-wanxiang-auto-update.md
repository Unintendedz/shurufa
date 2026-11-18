# Agent Note: 万象 LTS 原地替换不再阻断资源更新

Status: implemented

## Problem

万象 LTS 把同一个 release 地址当作可持续更新的渠道，会上游原地替换 `wanxiang-lts-zh-hans.gram`。此前资源包把锁文件中的 SHA-256 和长度同时当作下载身份与完整性证明；上游替换后，旧安装会被判过期，而新文件永远无法通过固定摘要校验。之前的临时锁刷新记录在[更新万象 LTS 语法模型锁](2026-10-26-wanxiang-lock-refresh.md)，它解决了当次资产变化，但不能覆盖下一次原地替换。

## Decision

万象 LTS 使用与 `msime-windows` 相同方向的远端可变资源处理：下载地址来自语法目录/锁文件，在线目录地址为本仓库 `GRAMMAR_CATALOG_URL`（`Unintendedz/shurufa`），远端不可达或未同步时回退到编译期内嵌目录 `EMBEDDED_GRAMMAR_CATALOG`（`resources/grammar-models.catalog.json`）。`download_dynamic_file` 从 HTTPS 响应取得完整长度，下载到独立临时文件，不复用旧摘要的断点文件，随后校验 `.gram` 头部、偏移和双数组边界（`valid_gram_file`）。`install_for_channel` 通过 `catalog_digest_check_required(pack)` 跳过 `ResourcePack::GrammarWanxiang` 对目录静态 `sha256`/`size` 的拦截，将本次实际 SHA-256 和长度写入本地清单，再沿用现有暂存目录和原子发布流程；已安装的万象只按清单形态和 `.gram` 格式判断有效，损坏文件会显示为过期并可重新安装；导入和收编也记录实际摘要。其他语法模型与固定资源包继续按目录/锁文件固定 SHA-256 和长度校验。

参考来源是 `msime-windows` 远端默认分支 `develop` 的提交 `94aed00012b895dd2fe734b087c5f7c404d9b658`，文件为 `server/src/settings/collocation_model.cpp`。原有[语法模型读取端](../feature/2026-10-21-octagram-grammar-model.md)的引擎契约与其他模型锁仍保留；Shurufa 宿主的下载触发、原生设置开关与默认启用迁移见[Shurufa 原生接入万象 LTS 语法搭配模型](../feature/2026-11-18-shurufa-wanxiang-grammar-model.md)。

## Alternatives considered

- **每次上游替换后继续刷新锁文件。** 最强理由是改动小且能保留所有固定校验；但它只能追赶一次替换，无法满足 LTS 地址持续原地更新的发布方式，因此保留为历史临时方案。
- **取消万象的完整性校验。** 最强理由是不会再因摘要变化阻断更新；但会失去长度和格式之外的边界保护，拒绝。当前仍要求 HTTPS、响应长度和可读 `.gram` 格式。
- **把模型文件复制进仓库或改托管不可变副本。** 最强理由是可以恢复固定摘要语义；但模型体积大且需要新的发布与镜像维护流程，本次不引入。

## Consequences

万象可以在同一 URL 原地替换后继续更新，旧缓存会在下一次安装时被整份新文件原子替换；磁盘和网络开销是重新下载完整模型，不能使用旧摘要断点续传。清单中的摘要表示实际安装字节，不再表示上游 LTS 标签的永久身份。万象状态检查增加格式读取，格式损坏时会触发重新安装。固定资源的安全与校验行为保持不变。

## Verification

- `cargo test -p msime-client-providers --lib`（271 项通过，2 项联网验收按原有标记忽略）；包含新版本导入、损坏响应保留旧安装、坏镜像回退、截断及缺失响应长度、意外 206、取消与临时文件回收。
- `cargo check -p msime-client-providers`
- `cargo check -p msime-host-api`
- `cargo clippy -p msime-client-providers -p msime-host-api --lib --tests -- -D warnings`
- 真实上游以 HTTP Range 读取 44 字节头部，响应为 206；总长为 `418078764` 字节、数组为 `104519680` 单元，魔数与数组边界符合本次规则。未完整下载实际大模型。
- 手动触发的远端工作流 `40279309594` 在提交 `e52e4f415513da4233fd17a3a104c5ae85229587` 的 API 28 和 35 模拟器上跑完 client-base、client-core、client-providers、client-services 整套单测，两个测试步骤均成功；APK 构建也成功，装包与输入冒烟尚在运行。后续桌面提交只补充 Tauri 错误映射及其测试，未改设备验证涉及的共享 Rust 源码；该桌面提交的 Linux 设置应用检查成功。单测通过不代表整包冒烟完成。
- 由于本机磁盘余量低于仓库规定的 50GB 门槛，未运行会创建其他平台构建树的 `scripts/verify-local.sh --quick`。全仓格式检查存在基线 `client-core/src/preferences.rs` 的差异，本次改动文件单独通过格式检查。
- `install_resource_pack "$HOME/Library/Application Support/Shurufa" grammar-model-wanxiang` 从上游实际完成 390,786,092 字节下载并校验 `.gram` 写入本地 `msime-model.json`；同步更新 `resources/grammar-models.catalog.json` 与 `resources/grammar-models.lock.json` 后，`cargo clippy -p msime-client-providers --all-targets -- -D warnings` 与 `cargo test -p msime-client-providers --lib -- resource_packs`（36 项通过，含 `wanxiang_catalog_fallback_and_rolling_digest_policy`）通过，`cargo test -p msime-engine --lib -- octagram`（10 passed, 1 ignored）通过。
