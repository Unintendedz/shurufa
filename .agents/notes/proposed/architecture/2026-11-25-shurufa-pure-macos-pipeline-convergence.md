# Agent Note: Shurufa 纯 macOS 单产品减法与三段单向候选管线收敛

Status: proposed

## Problem

在落地 [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](../../implemented/architecture/2026-11-24-shurufa-v2-probabilistic-neural-ime.md) 之后，底层的统一概率词网格、`Qwen3-0.6B-Base` 进程外 Metal 推理、BPE 边界校准与追加式个人学习日志已经具备坚实基础，但日常真实打字仍接连暴露出一系列跨层耦合缺陷：

1. **六道原地数组洗牌缺少唯一排序权威**：一次击键产生的候选依次穿过 `quanpin/dictionary.rs`（在召回阶段直接计算插入下标 `at` 塞入纠错行）、`session/candidates.rs`（个人上下文重排、混输合并、任意槽位固定 `fixed_position`）、10 个平行 `Vec` 组成的 `EngineSnapshot`、`input-runtime`（`runner_up_order` 次选让位、遗留 4.5MB `rerank`、遗留 25MB `settled_rerank`、`SlotAllocator` 内联 Emoji 投影及 `IndexRemap` 下标重映射）、JSON 序列化 C ABI，最后在异步神经回包后再次剥离 Emoji、运行 `neural_v2::fuse_neural_response`（用 `usize::MAX` 伪造引擎下标注入束搜索候选）并重新投影 Emoji。这导致「置顶固定位被挤走」需同时在 4 个模块打补丁，「漏关 4.5MB 旧模型」直接引发 `jiashe` 首帧被顶成「建设*」再跳回「假设」的排序闪烁。
2. **召回、打分与排座职责混杂**：同一条业务规则（如精确字面词典整词不该被推测性错拼篡夺首槽）被迫分散写在 `dictionary.rs`、`ordering.rs` 和 `neural_v2.rs` 三处，条件稍有偏差即产生跨阶段跳变；分段选词（`prefix_active` / `conversion`）与整句解码走不同分支，已选前缀无法自然推进神经模型上文 KV-Cache。
3. **异步时序与光标上下文分散在宿主 UI 层**：`22/35 ms` 延迟绘制定时器、`wasDeferred` 锁槽布尔位、快敲空格条件变量等待以及ObjC++ 侧第二套 `_adaptiveCandidates` 影子分页散落在 1.2 万行的 `InputController.mm` 与 `ShurufaNeuralClient.mm` 中，无法在纯 Rust 单测中做确定性验证；前文 `surrounding_context` 与后文 `following_context` 分两次可变注入，缺少原子绑定的光标现场快照。
4. **沉重的非 Shurufa 历史遗产拖累架构与构建**：尽管 [Shurufa 对齐系统拼音的输入边界](../../implemented/simplification/2026-10-29-shurufa-system-pinyin.md) 已将产品收敛为仅 macOS、仅全拼与双拼，仓库仍物理保留并编译检查 `windows / linux / android / ios / harmony` 五个平台、Tauri/React 外壳、脚本插件运行时、其余 5 个 edition、8 套非拼音输入方案、8 套全拼辅助码表、`Tab` 笔画筛选、包内 4.5MB/25MB 两套旧模型，以及为 Tauri 跨进程设计的 `NSUserDefaults + options.json` 双存储合并层（约 2500 行）和百余处 `MSIMEEditionIsShurufa()` 判断分支。

## Proposal

在不丢失本机任何现有用户数据（个人学习事件日志、个人词表、语音服务配置与加密存档、`Qwen3-0.6B` 与万象 `.gram` 模型）的前提下，分 7 个可独立装机验证的切片完成**纯 macOS 单产品物理减法 + 三段单向函数式候选管线重构**：

### 1. 仓库、资源与输入表面物理收敛

- **单平台单产品**：物理移除 `platforms/{windows,linux,android,ios,harmony}`、`apps/`、`packages/ui`、`plugins/`、`fuzz/` 及 `crates/{engine-wasm,host-windows,ios-native-ffi,tauri-mobile-platform,script-runtime,script-sdk,pack-tool,clipboard-relay,backend-engine,mcp-server}`，移除多余 edition 与全部 `MSIMEEditionIsShurufa()` 间接分支。
- **输入能力精确白名单**：仅保留**全拼 + 双拼、简繁转换、基本中文标点与全半角、中文组字下的英文单词自动补全（`msime-english.db`）、精选内联 Emoji 投影（`msime-others.db`，可开关）、小写 `v` 数字与算式计算、个人学习闭环（含 5 秒退格回滚、15 秒改错自动学词、个人词表）、本地/云端语音输入与润色**。
- **移除边缘筛选与旧模型包袱**：彻底移除五笔/日语/韩语/粤语/注音/越南语/藏文/笔画输入方案，移除 `Tab` 笔画筛选（`msime-stroke.db`）与 8 套全拼辅助码表（`resources/helpcodes/`），移除任意槽位固定 `fixed_position = 2..9`（仅保留「置顶到第 1 位」与「删除/屏蔽词条」），彻底移除 `sentence-model.safetensors`（4.5MB）、`sentence-model-desktop.safetensors`（25MB）及 `chinese_ime_lm` 依赖。

### 2. 光标现场快照与 `Recall → UnifiedScorer → PurePresenter` 单向管线

- **一等不可变输入 `CursorSnapshot`**：会话在组字开始或光标移动时原子绑定 `CursorSnapshot { left_text, right_text, locked_prefix, app_bundle_id, is_secure_field }`。长句分段选词采用**段内暂存前缀（退格可反悔拆回拼音）**，模型与搭配查表看到的左上下文统一为 `left_text + locked_prefix`，使剩余未选拼音 100% 走同一条单向管线并自然推进 `Qwen3-0.6B` KV-Cache；端侧 `ShurufaNeuralEngine` 将左 Token Healing、右单字前缀边际化与句中右侧前瞻收口为统一的双端边界评分算子 $\log P(Y \mid C_{\text{left}}, C_{\text{right}})$。
- **Stage 1 — 纯召回池 `RecallPool<CandidateEntry>`**：彻底废除 10 平行 `Vec` 的 `EngineSnapshot`、`IndexRemap` 与 `usize::MAX` 假下标。引擎输入 `(RemainingKeys, EffectiveLeftContext)` 只产出带 64 位稳定 `CandidateId` 与结构化证据（`SpanKind`、`TypoEvidence`、词库先验、`.gram` 搭配分、个人学习先验、`is_pinned_top`）的候选集合，**召回层严禁计算展示座位下标**。
- **Stage 2 — 唯一同构打分器 `UnifiedScorer`**：全仓仅保留唯一三参数对数线性目标函数。当 `neural_scores == None`（同步 Tier 0 或无 GGUF 回退）时以万象 `.gram` 局部搭配分作为零延迟 LM 项；当 `neural_scores == Some`（异步 Tier 1 回包）时替换为 `Qwen3-0.6B` 双端条件对数概率。错拼信道代价、精确字面词保护与非成词拆字先验全仓只写一份。
- **Stage 3 — 唯一纯函数排座器 `PurePresenter`**：单一无状态纯函数输入 `&[ScoredCandidate]`，一次性完成：`pinned_top` 锁第 0 槽 → 等跨度候选前段连续排座与首页纠错限流（`Provisional` 未获神经分前精确字面词守第 0 槽）→ 精选内联 Emoji 锚定投影 → 前缀词 / 英文补全（首页上限 3 席）/ 单字。选词、置顶、屏蔽与学习回流一律按稳定 `CandidateId` 寻址。

### 3. 纯 Rust 托管神经 UDS 客户端、两阶段状态机与强类型 C ABI

- **神经通信与 `Provisional → Presented` 状态机全归 Rust**：由 Rust 会话层直接管理 `ShurufaNeuralService` 子进程、私有 UDS 二进制帧通信、50ms 空闲预热、`~28 ms` 延迟绘制微窗口定时器及暖机快敲空格有界等待，物理删除 ObjC++ `ShurufaNeuralClient.{h,mm}`。代次状态显式区分为 `Provisional`（微窗口内尚未绘出，允许更新槽 0）与 `Presented`（已通知宿主绘出，Rust 层硬锁槽 0 绝不跳动，迟到回包仅重排槽 `1..`），通过轻量单函数回调唤醒 AppKit 主队列重绘。
- **单一 `config.json` 与强类型零拷贝 `ShurufaViewSnapshot`**：废除历史 Tauri 双进程遗留的 `NSUserDefaults + options.json` 双写合并监听，收敛为单一 `~/Library/Application Support/Shurufa/config.json`（首次启动幂等导入既有设置）；按键与翻页热路径改为只读 C 结构体快照 `ShurufaViewSnapshot`（零 JSON 编解码），取消 `InputController.mm` 的 `_adaptiveCandidates` 影子分页状态，同时清退 `platforms/macos` 内的 Tauri 桥、账号、皮肤、悬浮栏、屏幕键盘与 Sparkle 死模块。

## Historical audit

检索 `.agents/notes/` 后，本提案与以下活跃决策存在演进或部分取代关系：

- [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](../../implemented/architecture/2026-11-24-shurufa-v2-probabilistic-neural-ime.md)：保留其统一概率词格、`Qwen3-0.6B-Base` 进程外 Metal 服务、BPE 边界概率模型与退格负反馈学习内核；将其嫁接在旧 `EngineSnapshot` / ObjC++ 定时器上的多段原地洗牌骨架升级为纯 Rust 单向三段管线与 Rust 原生 UDS 两阶段状态机。
- [Shurufa 候选词决策链五层重构与分阶段演进](2026-11-20-shurufa-candidate-pipeline-redesign.md) 及 [Stage 3 单遍 4.5MB 神经复用、停顿首选锁定与语境同音标定](../../implemented/architecture/2026-11-20-stage3-single-pass-scorer-and-settled-lock.md)：清退其中过渡期保留的 4.5MB / 25MB `chinese_ime_lm` 模型与双层重排，彻底收口至单一 `UnifiedScorer` 与 `PurePresenter`。
- [Shurufa 对齐系统拼音的输入边界](../../implemented/simplification/2026-10-29-shurufa-system-pinyin.md)：在其运行时收窄基础上，进一步物理删除非 macOS 平台、非拼音方案、`Tab` 笔画筛选、全拼辅助码表与 `fixed_position = 2..9`。
- [Stage 1 落地纯函数 SlotAllocator 与重排后精选内联 Emoji 投影](../../implemented/feature/2026-11-20-shurufa-slot-allocator-and-inline-emoji.md) 与 [Shurufa 中文输入下的英文单词自动补全](../../implemented/feature/2026-11-17-shurufa-english-word-autocomplete.md)：保留内联 Emoji 与英文自动补全产品行为，合并入 `PurePresenter` 并废除 `IndexRemap` 下标折算。

## Alternatives considered

### 1. 保留跨平台目录与多 edition 契约，仅在现有目录内局部重构候选管线

- **最强理由**：无需改动构建脚本、资源锁与历史契约检查，首期文件删除面更小。
- **放弃原因**：Shurufa 已明确为独立演进的 macOS 个人输入法，永不跟随上游合并；继续背负 5 个无关平台、Tauri 壳、10 套语种方案和上百处 `MSIMEEditionIsShurufa()` 判断分支，会让每次重构都被跨平台 C ABI 兼容、平行 `EngineSnapshot` 和几分钟的容器门禁束缚，无法真正简化数据流与宿主壳。

### 2. 按键帧内同步阻塞等待 Metal Stage A（`<= 15 ms`），彻底取消异步两阶段状态机

- **最强理由**：若每次击键直接在主线程同步等待 `ShurufaNeuralService` 返回 Stage A 分数，可省去 `Provisional → Presented` 状态机与延迟绘制微窗口。
- **放弃原因**：IMKit 事件回调运行在主线程，一旦遇到长上文冷预热（`22–35 ms`）、GPU 瞬时繁忙或 Stage B 受限束搜索（`35–65 ms`），同步阻塞主线程会直接造成行内拼音上屏肉眼卡顿；采用纯 Rust 托管的「行内预编辑 `< 2 ms` 秒回 + 候选面板 `~28 ms` 异步微窗口 + 一绘硬锁槽 0」既保住极致跟手度，又从状态机层面保证面板绘出后 `0%` 首槽跳变。

### 3. 继续保留 `NSUserDefaults + options.json` 双存储及 JSON C ABI 字典协议

- **最强理由**：少改 `AppearancePreferences.mm` 和 `InputController.mm` 里的字典读取代码。
- **放弃原因**：双存储合并层本是为上游「独立 Tauri 设置进程 + 输入法进程」跨进程同步设计的，Shurufa 的原生设置窗就在输入法进程内部，保留双存储徒增 2500 行易错合并状态；而每键 JSON 编解码与 ObjC++ `_adaptiveCandidates` 影子分页正是视图状态分裂与无谓主线程开销的温床，一步收敛到单一 `config.json` 与只读 `ShurufaViewSnapshot` 结构体才能让宿主壳真正变薄。

## Acceptance criteria

- 仓库仅保留 macOS `Shurufa` 所需的源码、词库与测试门禁，移除全部非 macOS 平台、Tauri/React、脚本插件、非拼音方案、辅助码表、`Tab` 笔画筛选以及 4.5MB/25MB 旧模型。
- 候选生成与重排完全经由 `CursorSnapshot → RecallPool<CandidateEntry> → UnifiedScorer → PurePresenter` 单向管线与稳定 `CandidateId` 运行，`EngineSnapshot` 平行数组、`IndexRemap` 与 `usize::MAX` 假下标清零。
- `ShurufaNeuralService` 的 UDS 通信、空闲预热、`Provisional → Presented` 微窗口与槽 0 硬锁、暖机快敲空格有界等待全部由纯 Rust 会话层托管并通过模拟套接字单测覆盖，删除 `ShurufaNeuralClient.{h,mm}`。
- 升级安装后本机既有个人学习库、词表、语音配置/存档、模型包与用户偏好零丢失生效；`diagnostic_dev`、`blind_holdout`、`rolling_fresh_v2` 评测指标不退化，面板已绘出后的首槽跳动率为 `0%`。

## Risks

- **物理裁剪初期误删 macOS 原生宿主仍在引用的共享辅助文件**（例如 `crates/host-macos/native/uninstaller.mm`、`shared/apple/`、`platforms/common/`）：在 Slice 1 动手前先用 CMake 依赖图核清 `platforms/macos` 的全部源文件输入，先内迁被引用文件再删外围目录，每个切片均执行真实 `build-shurufa.sh` + `install-shurufa.sh` 验证。
- **偏好单真源迁移遗漏个别旧键**：在 Slice 5 为首次导入编写覆盖全部保留设置项（双拼方案、简繁、标点/全角、内联 Emoji、本地神经排序、万象搭配、语音快捷键与服务配置）的单测并保留原 plist 只读不毁，确保随时可回溯。
