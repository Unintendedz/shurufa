# Agent Note: 本地语音输入全链路管线（音频前端、确定性后处理、上下文润色与本机托管模型）

Status: implemented

## Problem

Shurufa 的 macOS 本地语音输入此前虽然已经跑通 `TextStream` 流式识别与基础云端 HTTP 润色，但在高频真实口述与全离线场景下存在六类结构性短板：

1. **音频采集与长口述截断**：`VoiceInputService` 为存档与识别分别维护 `AVAudioConverter` 造成重复重采样，且缺乏高通滤波、轻声 AGC 与纯静音误触判定；`TextStream` 在 >30 秒连续口述时容易触发窗口漂移或截断，且空闲驻留与内存压力回收未按物理内存分级。
2. **关闭润色或润色失败时质量断层**：缺乏跨平台共享的无网络确定性后处理管线，关闭 LLM 或润色超时降级后，ASR 原始文本直接上屏，残留纯语气词（`嗯。`）、块边界单字结巴、短音频静音幻觉、无中英 Pangu 空格、技术词大小写错乱、汉字数字未 ITN 等问题。
3. **缺少上下文感知、选区口头改写与跨语种个人词表**：润色环节不知道当前前台应用（聊天软件 vs 终端/IDE vs 邮件）、窗口标题、前文或选区内容，无法区分「口述插入」与「口头指令改写选区」，也无法按字面/拼音近似度把用户个人词库与 `voice-vocabulary.json` 的 Top-K 术语注入润色。
4. **缺少全离线本地大模型润色闭环**：`POLISH_PROVIDERS` 与凭据探测仅覆盖云端必填 Token 服务，不支持 Shurufa 托管的本机 `llama-server`（`local`）与本机 `ollama` 免 Token 路径，也缺乏进程生命周期守护、7.5 秒慢响应提示与防幻觉清洗降级。
5. **macOS 双态流式预览与结构化存档不足**：浮层只能单色显示流式文本，无法区分已定稿与实时草稿；语音存档仅存单份最终文本，无法对照保存 `raw_text`、各阶段耗时、`app_category`、降级原因以及静默丢弃原因。
6. **跨层集成边界需要严守红线**：`VoiceInputPreferences` 扩展不得静默改变非 Shurufa 版本的出厂默认值；`shared/voice/VoiceProviders.{h,cpp}` 既有签名与 `http-voice-request-test` 链接图不得被破坏。

## Decision

采用「Rust 共享纯逻辑 + 独立 C ABI 契约 + macOS 原生薄编排」的分层架构，分 Wave 1（A–E 五个独立模块）与 Wave 2（总集成）落地完整本地语音管线：

### 跨层架构与 C ABI 契约

- **音频前端与流式识别（macOS + TextStream）**：
  - `VoiceAudioPreprocess.h` 在 16 kHz 单声道 float 域执行 80 Hz 二阶 Butterworth 高通、噪声门控温和 AGC（目标 -20 dBFS、底噪门 -48 dBFS、上限 +12 dB）、软限幅与 `activeSpeechMs` / `peakAbs` / `hasLikelySpeech` / `isLikelyAccidentalSilence` 统计；`VoiceInputService` 合并为单次重采样后同时供给识别与加密存档。
  - `textstream_worker.py` 升级为全程 Silero VAD 状态机，支持 256 ms preroll、句中长停顿跳过、>=12 s / 24 s / 26 s / 28 s 安全切段与单调累积 `finalized` / `draft` 双态预览；`LocalVoiceRequest` 监听系统内存压力并按物理内存分级配置空闲回收，公开 `partialStateHandler`、`lastFinalizedText`、`lastDraftText` 与 `lastFinalMetrics`，由 `InputController` 透传至 `VoiceWaveOverlay::setFinalizedTranscript:draft:`。
- **Rust 共享管线与 6 个新 C ABI 入口（`crates/client-providers/src/voice/` + `crates/host-api/src/ffi/voice.rs` + `msime_client.h`）**：
  - `msime_client_voice_postprocess` → `voice::postprocess::postprocess`：无网络确定性后处理（保缩进空白规整、块边界结巴与句尾语气词清理、纯语气词/短静音幻觉/复读循环静默拦截、拆散缩写与口述符号合并含下划线/短横杠/斜杠/域名点、中文标点/弯引号、双边界保守 ITN、个人词表与内置技术词大小写、Pangu 中英空格、双正交句末句号策略）。
  - `msime_client_voice_vocabulary` → `voice::vocabulary::{load_from_preferences_dir, build_vocabulary, select_prompt_terms}`：只读聚合用户拼音/英文/快捷短语词库与 `<preferences_directory>/voice-vocabulary.json`（不经文件锁），并支持按 `raw_text` / `app_category` 返回 Top-K <= 60 的 `prompt_hints`。
  - `msime_client_voice_vocabulary_manage` → `voice::vocabulary::{upsert_term_in_preferences_dir, remove_term_in_preferences_dir, restore_term_in_preferences_dir, learn_corrections_in_preferences_dir}`：以同目录临时文件原子 `rename` 持久化 `<preferences_directory>/voice-vocabulary.json`（不经 `File::lock`），支持设置页增删查、事后编辑局部词级纠错提取（`extract_edit_corrections`，严格拦截清空发送、纯续写/纯删除、整句重写与单字汉字误收）及气泡一键精确撤销。
  - `msime_client_voice_polish_clean` → `voice::polish_clean::clean_polish_output`：剥离 `<think>`、解析 `<action>rewrite|dictate</action>` 路由标签、拦截未闭合思考/助手元回复/异常膨胀塌缩/繁体漂移，并给出稳定的 `rejection_reason` 与 `resolved_action`。
  - `msime_client_voice_polish_http_request` 与 `msime_client_voice_polish_parse_response` → `voice::polish::{build_polish_http_request, parse_polish_response}`：Sans-IO OpenAI 兼容请求描述器（自动补全 `/v1` 等 API 根路径为 `/chat/completions`）与非流式/SSE 解析器；支持注入受围栏转义保护的 `<preceding_context>`、`<following_context>` 与 `<visible_screen_context>`，内置固定字节前缀 + 4 条合成 Few-Shot 的 v3.1 提示词（命中本机 `llama-server` KV cache），对密码框实施分层隐私闸门（非本机回环的云端与局域网端点直接返回 `polish_secure_field_blocked`，本机回环剥离全部上下文、可见屏、选区与词表后放行），对 `local`/`ollama`/本机私网/Tailscale 端点免 Token 且设 `no_proxy = true`，对公网明文 `http://` 与非法控制字符直接拒绝。
- **Provider 目录、凭据探测与偏好扩展**：
  - `crates/client-core/src/preferences.rs` 与 `crates/client-providers/src/voice/provider_catalog.rs` 将 `local` 与 `ollama` 并入 `POLISH_PROVIDERS`（置于 `custom` 之前），保留 `LOCAL_POLISH_PROVIDERS` / `is_local_polish_provider` / `polish_provider_preset` / `all_polish_providers` 兼容；`credential/probe.rs` 接入 `polish_requires_token`，使 `voice.polish` 支持 `ollama`（强制剥离旧 Token）、`local` 及本机/私网端点免 Token 探测，同时保留本机自定义端点显式填写合法 Token 时的 `Authorization` 透传。
  - `VoiceInputPreferences` 新增 4 个带 `#[serde(default)]` 的字段：`polish_local_model_path`、`polish_translation_model_path`、`polish_enable_selection_rewrite`、`polish_context_aware`；两项模型路径参与 `valid_model_path` 校验并在 `restored_to_defaults_for` 时保留；存量 Shurufa 用户的旧 `preferences.json` 缺键时由 `read_locked_keeping_unknown` 按 `edition.id == "shurufa"` 自动回填 `true`（显式保存的 `false` 原样保留，非 Shurufa 版本不动），同步登记进 `scripts/test-preferences-field-parity.py` 的 `RUST_ONLY["VoiceInputPreferences"]` 与 macOS `SharedVoicePreferences.h` / `VoiceProviderOptions.h` / `VoiceRecognitionQuery.h`（未知字段保留契约见 [偏好文档容忍并保留本版本不认识的键](2026-10-15-preferences-keep-unknown-fields.md)）。
- **macOS 原生主控编排与独立传输客户端**：
  - `VoiceContextCapture.mm` 在录音启动瞬间以 <=25 ms AX 预算同步抓取脱敏上下文快照（密码框硬屏蔽、微信/Sublime/Google Docs/腾讯文档防卡死黑名单、浏览器仅提取小写 host 域名、选区剥离 `U+FFFC` 等占位符防假选区、光标前后双向上下文读取，并为已知浏览器与 Chromium/Electron 应用安全唤醒 `AXEnhancedUserInterface`）；同时在后台串行队列 `voicePipelineQueue` 异步执行 <=220 ms 预算的焦点窗口/WebArea 可见屏幕文本抓取（`MSIMECaptureVisibleScreenTextForPID`）并按代次回填，零阻塞主线程。
  - `VoiceEditTracker.{h,mm}` 与 `VoiceLearnedToast.{h,mm}` 实现语音上屏后 15 秒事后编辑自动学词闭环：后台延迟采样初始输入框全文，监听编辑键防抖采样并在按回车发送、输入框清空、切焦点或超时时调用 `msime_client_voice_vocabulary_manage(action="learn")`，学到新词后在屏幕右上角弹出不抢焦点的毛玻璃气泡（附带一键「撤销」精确恢复旧词条与悬停暂停自动消失），同时在 `VoiceProviderSettings.mm` 原生语音设置页提供「个人语音词表」增删查看面板。
  - `LocalPolishServer.mm` 托管仅绑定 `127.0.0.1`、内存态随机 256-bit `--api-key` 的 `llama-server` 子进程，录音开始时异步 `prewarmWithModelPath:`，支持按物理内存分级的 `--ctx-size` / 空闲回收、内存压力即时释放与崩溃限频重启。
  - `VoicePolishClient.mm` 作为独立于 `HTTPVoiceRequest` 的润色 HTTP/SSE 客户端，通过 `-DMSIME_VOICE_POLISH_USE_RUST_FFI=1` 静态绑定 Rust C ABI，强制 `ephemeralSessionConfiguration`、禁止重定向、`no_proxy` 绕过系统代理、7.5 秒慢响应回调 `VoiceWaveOverlay::setProcessingHint:`，任何错误/超时/清洗拒收均无损降级到确定性后处理文本。
  - 新增 `VoicePipelineHelper.{h,mm}` 承载后处理、词表加载、润色资格判定、请求装配与存档元数据构造，保持 `InputController.mm` 改动最小且完整兼容既有测试夹具（`HTTPVoiceControllerTest`、`DoubaoVoiceControllerTest`、`LiveVoiceControllerTest`）；`applyVoiceResult` 扩展支持 `replace_selection` 选区替换上屏与 `MSIMEVoiceArchive` 的 `noteText:metadata:forRecording:` / `noteDismissedReason:forRecording:`。

### 默认值差异表（冻结非 Shurufa 出厂默认值）

按仓库「能力可达而非静默改写存量用户手感」原则，新增 4 个偏好字段在所有非 Shurufa 版本保持保守零值，仅在 `edition.id == "shurufa"` 分支开启上下文感知与选区改写：

| 偏好字段（`voice_input.*`） | 非 Shurufa 版本出厂值（冻结） | Shurufa 版本出厂值 | 说明 |
| --- | --- | --- | --- |
| `polish_local_model_path` | `""` | `""` | 空串表示由 `LocalPolishServer` 在默认本机目录自动发现 GGUF，不硬编码私人路径 |
| `polish_translation_model_path` | `""` | `""` | 空串表示未单独指定专用翻译 GGUF，由通用润色模型承担翻译 |
| `polish_context_aware` | `false` | `true` | 非 Shurufa 版本保持原有无上下文行为；Shurufa 启用受密码框与黑名单保护的上下文感知 |
| `polish_enable_selection_rewrite` | `false` | `true` | 非 Shurufa 版本默认不触发选区改写；Shurufa 启用两层意图守卫的选区口头改写 |

## Alternatives considered

- **在 `HTTPVoiceRequest` 内部直接叠加上下文、本机 `llama-server` 与 SSE 流式润色。** 最强的理由是减少一个新类、沿用既有 `InputController` 注入点。但 `HTTPVoiceRequest` 承担着旧版 multipart/chat_audio ASR 与简单云端润色，并由 `http-voice-request-test` 锁定了不链接 `apple-client` 的精简链接图；把 Rust FFI、本地进程托管与 7.5 秒慢响应编排塞进去会破坏测试隔离并耦合多条生命周期。因此选择新增独立 `VoicePolishClient`，同时在 `InputController` 保留原有 `makeDoubaoPolishRequest:` / `makeLiveVoicePolishRequest:` 测试覆写钩子。
- **在 macOS Objective-C++ 侧单独实现确定性后处理与 Top-K 词表筛选。** 最强的理由是省去 5 个 C ABI 入口与 JSON 编解码开销。但文本清洗、保守 ITN、Pangu 空格、拼音近似度词表筛选与防幻觉规则属于跨平台核心逻辑，放在 Objective-C++ 会让未来 Windows/Linux/移动端重复实现且难以用纯 Rust 单测回归。统一收口在 `crates/client-providers/src/voice/` 并经 C ABI 暴露后，所有宿主共享唯一真源。
- **直接为所有版本开启 `polish_context_aware` 与 `polish_enable_selection_rewrite`。** 最强的理由是全平台行为一致、减少版本分支。但这会静默改变存量非 Shurufa 用户的语音润色提示词结构与选区替换行为，违反仓库默认出厂值冻结原则；因此将其严格限定在 `edition.id == "shurufa"` 分支与 macOS 对应默认值兜底中。

## Consequences

- **收益**：
  - 关闭 LLM 或 LLM 故障/超时/拒收时，语音输出仍经过完整确定性后处理并无损上屏，不再出现纯语气词误上屏、静音幻觉、中英粘连或漏字丢句。
  - Shurufa 可在完全离线环境下以 `TextStream` + 本机托管 `llama-server`（或 `Ollama`）闭环完成听写精修、选区口头改写与翻译，且密码框、AX 黑名单与公网明文 HTTP 均具备硬门控。
  - 语音存档同时记录 `raw_text`、最终文本、各阶段耗时、场景分类、服务/模型/提示词与降级/静默丢弃原因，为后续离线评测与复盘提供完整观测面。
- **代价与约束**：
  - `MSIMEClientInputMethod`、控制器测试目标及 `voice-polish-client-test` 新增编译 `VoiceContextCapture.mm`、`LocalPolishServer.mm`、`VoicePolishClient.mm`、`VoicePipelineHelper.mm` 并链接 `Security` 框架；`VoicePolishClient.mm` 统一通过源文件属性定义 `MSIME_VOICE_POLISH_USE_RUST_FFI=1`，避免遗漏目标导致静默退回 `dlsym`。
  - 4 个新增偏好字段暂登记在 `scripts/test-preferences-field-parity.py` 的 `RUST_ONLY["VoiceInputPreferences"]`，未来若要在共享 React 设置页暴露控件，须同步从 `RUST_ONLY` 移出并补齐 TypeScript schema。
  - 本机 `llama-server` 依赖外部或随包二进制与本地 GGUF 模型；缺失时自动降级到确定性后处理并记录 `local_server_unavailable`，不会阻塞或丢弃用户输入。
