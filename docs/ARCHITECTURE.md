# Shurufa 架构与工程约束

这份文档说明纯 macOS 单产品重构后的 Shurufa 分层架构、各层职责边界，以及不可随意打破的不变量。改动代码前请先读这里；日常工作流、本机安装与提交约定见 [AGENTS.md](../AGENTS.md)。

## 分层架构

仓库严格按「谁拥有什么状态」分层，自上而下为：

```text
platforms/macos (Shurufa.app + 原生 AppKit 设置窗)   IMKit 事件捕获、候选面板绘制、AX 上下文与音频采集
        │   零拷贝只读 ShurufaViewSnapshot + C ABI (msime_client.h)
        ▼
crates/host-api                                       C ABI 边界、单一 config.json 与词库/语音管理入口
        │
        ▼
crates/input-runtime                                  会话编排、UDS 托管 ShurufaNeuralService、两阶段状态机
        │
        ├──▶ platforms/macos/src/neural               ShurufaNeuralService（进程外 Apple Metal Qwen3-0.6B 推理）
        ▼
crates/engine                                         纯 Rust 输入内核（CursorSnapshot → RecallPool → UnifiedScorer → PurePresenter）
```

各层具体职责如下：

- **`platforms/macos`**：macOS IMKit 原生宿主（`Shurufa.app`）与进程内原生 AppKit 设置窗口（`platforms/macos/src/settings/`）。宿主只负责接收 `IMKInputController` 键盘事件、读取系统辅助功能上下文、采集麦克风音频，并将 `ShurufaViewSnapshot` 渲染为候选面板；宿主不持有候选分页影子状态，不直接操作神经网络套接字或调度重排定时器。
- **`crates/host-api`**：统一 C ABI 边界（`crates/host-api/include/msime_client.h`），为按键、翻页与选词热路径暴露只读零拷贝结构体 `ShurufaViewSnapshot`（`shurufa_session_view`、`shurufa_session_poll_neural`、`shurufa_session_flush_provisional_neural`）以及异步重绘唤醒回调注册，同时承接配置读写、个人词库管理与语音/AI 辅助接口。
- **`crates/input-runtime`**：纯 Rust 会话编排层。负责托管 `ShurufaNeuralService` 子进程生命周期与私有 UDS 二进制帧通信（`neural_client.rs`）、`50 ms` 空闲上文 prefill 防抖、独立定时器线程驱动的 `PresentationPhase::Provisional → PresentationPhase::Presented` 状态机（`~28 ms` 微窗口延迟绘制与面板绘出后槽 0 硬锁）、热态快敲空格有界等待，以及按稳定 `CandidateId` 驱动候选分页和退格负反馈学习闭环。
- **`crates/engine`**：纯 Rust 输入算法内核，围绕不可变 `CursorSnapshot` 执行 `RecallPool → UnifiedScorer → PurePresenter` 三段单向函数式管线（`crates/engine/src/pipeline.rs`）：
  - **支持能力**：仅覆盖全拼与双拼、OpenCC 简繁转换、基本中文标点与全半角、中文组字下的英文单词自动补全（`msime-english.db`）、精选内联 Emoji 投影（`slot_allocator.rs` / `emoji_mru.rs`）、小写 `v` 数字与算式计算、万象 `.gram` 语法搭配与个人学习闭环。
  - **三段单向流**：`RecallPool` 只做多源召回与证据收集，严禁计算显示座次；`UnifiedScorer` 持有唯一三参数对数线性目标函数（无神经回包时用 `.gram` 局部搭配分，有回包时替换为 `Qwen3-0.6B` 双边界概率）；`PurePresenter` 作为无状态纯函数一次性完成槽 0 置顶保护、首页纠错限流、内联 Emoji 锚定、英文补全上限（最多 3 席）与单字排座，所有动作通过 64 位稳定 `CandidateId` 寻址。
- **平行支撑 crate**：
  - `crates/client-base`：文件锁、原子写入、OpenCC 简繁转换等离线基础能力；
  - `crates/client-core`：以单一 `~/Library/Application Support/Shurufa/config.json` 作为唯一配置真源（兼容首次从旧偏好幂等导入），以及固定资源清单校验；
  - `crates/client-services`：词库维护与导入导出服务；
  - `crates/client-providers`：本地/云端语音识别、AI 润色与按需模型包管理；
  - `crates/path-trust`：受限路径与符号链接安全校验。
  - 依赖方向固定为 `client-base → client-core → client-services` 与 `client-core → client-providers`（`services` 与 `providers` 互不引用），`input-runtime` 只依赖 `client-core` 与 `engine`。
- **`platforms/macos/src/neural`（`ShurufaNeuralService`）**：随包分发的进程外 Apple Metal 守护进程，承载 `Qwen3-0.6B-Base` 推理与 `ShurufaNeuralEngine` 双边界条件对数概率算子（左 Token Healing + 右单字前缀边际化 + 句中右侧前瞻）。

## 不能打破的架构不变量

1. **算法与排座唯一权威在 `crates/engine`**：组词、错拼惩罚、搭配打分与展示排座一律留在 `crates/engine`，禁止在 `crates/input-runtime` 或 Objective-C++ 宿主层复制组词规则、重建第二套候选列表或按显示数组下标回写状态。
2. **异步时序与槽 0 硬锁唯一权威在 `crates/input-runtime`**：`~28 ms` `Provisional` 防闪窗、`Presented` 后硬锁槽 0（迟到神经回包只重排槽 `1..`）、暖机快敲空格有界等待全部由 Rust 运行时状态机控制，宿主 UI 只能轮询 `ShurufaViewSnapshot` 并按回调重绘。
3. **配置唯一真源为 `config.json`**：设置面板与输入法在同一 `Shurufa.app` 进程内运行，通过 `client-core` 读写单一 `config.json`，不再维护双存储合并监听或跨进程 IPC；`client-*` 支撑 crate 不反向依赖 `engine` 或 AppKit。
4. **原生宿主与引擎之间的静态边界**：所有新增或修改的 C ABI 结构（`msime_client.h`）、云候选超时预算（`client-core::cloud::candidates`）及偏好键值对变更，均须通过 `scripts/test-*.py` 文本契约检查验证。

## 固定资源与许可证

- **版本与核心资源锁**：`shared/contracts/editions.json` 收敛为 `["full", "shurufa"]`，随包桌面资源由 `resources/editions/shurufa.lock.json` 固定校验并在构建时通过 `platforms/macos/stage-resources.sh` 装配。
- **OpenCC 简繁转换表**：由 `resources/opencc.lock.json` 固定 7 份表及许可证校验，`client-base/build.rs` 通过 `scripts/fetch_opencc.py` 在 `target/opencc/<commit>/` 缓存准备并内嵌，支持 `MSIME_OPENCC_OFFLINE=1` 离线构建。
- **符号与第三方声明**：`crates/engine/src/local/symbol_groups.tsv` 随仓库提交并受 `scripts/test-symbol-groups.py` 校验；所有内嵌与按需下载组件（OpenCC、SCOWL、symphonia、sherpa-onnx、TextStream/MLX、`Qwen3-0.6B`、万象 `.gram` 等）的来源、固定提交与许可证全文须同步登记在 `platforms/macos/resources/Licenses/THIRD_PARTY_NOTICES.txt` 与 [third-party.md](third-party.md)，网络与隐私边界见 [cloud-candidates.md](cloud-candidates.md)。

## 本地验证体系

仓库通过 `.githooks/`（执行一次 `git config core.hooksPath .githooks` 启用）与两级本地门禁脚本保障质量：

```sh
bash scripts/verify-local.sh --quick   # 快速门禁：决策笔记校验 + run-checks.sh + workspace 编译 + macOS 主目标编译
bash scripts/verify-local.sh           # 完整门禁：快速门禁 + workspace 单元测试 + clippy + 全量 macOS CTest
```

- **文本契约检查（`scripts/run-checks.sh`）**：自动发现并并发执行全部 `scripts/test-*.py`（包括格式、冲突标记、符号链接、文件锁、Edition 锁、OpenCC 校验、符号表、云候选预算、macOS 孤立源文件、设置窗口生命周期与 `NSUserDefaults` 测试 suite 清理）。
- **macOS 原生测试套件（`target/macos-isolated`）**：通过 CMake/Ninja 配置 `MSIME_EDITION=shurufa`，由 `ctest --test-dir target/macos-isolated -E '^(textstream-real|textstream-protocol)$' --parallel 4 --output-on-failure` 运行全套原生单元与 UI 回归测试。
- **拼音转换评测门禁（`convert_eval`）**：使用 `crates/input-runtime/examples/convert_eval` 对照锁定资源与万象 `.gram` 模型评估 `resources/eval/shurufa-spoken-v1.tsv` 与 `resources/eval/sentences-v2.tsv` 的 Top-1 准确率及分阶段归因。
