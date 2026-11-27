# 变更记录

Shurufa 从水杉输入法 tag `fork-point/msime-2026-10-27` 分出来之后的改动，按 `git log` 整理。没有版本号，也不发版：`main` 上的最新提交就是我在用的那一版。更早的历史是水杉的，见上游仓库。

## 2026-11-27

- **未测频占位整词词网格组合升格、单字幻影异读护栏与非汉字左边界 BPE Token Healing（`types.rs`、`lattice/merge.rs`、`lattice/octagram.rs`、`ordering.rs`、`session/candidates.rs`、`neural_v2.rs`、`input-runtime/src/runtime.rs`、`ShurufaNeuralEngine.cpp`）**：
  - **带上文 3–4 字未测频占位整词升格与 Tier 1 尾槽保底**：修复主词库 `238,926` 条 `weight == 100` 未测频占位整词（如「`怎么个事`」「`遇得到`」「`答案就是`」「`大爱无疆`」「`擦边主播`」「`有毛病吧`」「`按理来说`」「`干得漂亮`」「`恶狠狠地`」「`阿鼻地狱`」）在有中文上文与万象 `.gram` 时因后置护栏叠词网格去重而反被挤出首页与 `Qwen3-0.6B` 16 候选池的回归；`WordItem` 新增 `lattice_log_prob_milli_nats: Option<i64>` 保留 `CandidateSource::Database` 身份与可编辑性，`merge.rs` 仅在多词组合验证（`path.words.len() >= 2`）时升格入主块（继续后置未验证的「`先哲之石`」），`ordering.rs` 纳入主读法并修正同首 `leader_head_cap`，`candidates.rs` / `neural_v2.rs` 新增 `stat_from_lattice` 独立分桶归一，`runtime.rs` 在 3+ 音节满池时向 `candidates[4..]` 尾槽抢救至多 `2` 条后置精确词典整词（「`我看看` + `zenmegeshi`」Tier 0/1 双 `#1 怎么个事`，`yudedao` 稳居首页 `#2–#3 遇得到`）。
  - **单音节 `weight <= 1` 幻影异读护栏**：修复 `msime-pinyin.db` `tbl_1_*` 中 `552` 条高频字零权/极低权古音异读（如 `me -> 没(0)/末(0)`、`ti -> 是(0)`、`wei -> 有(0)`、`ne -> 那(0)/哪(1)`、`shui -> 说(1)`）被 `.gram` 与 `Qwen3-0.6B` 凭上下文反超现代常用字的问题（如「`青爷在说喊你开店` + `me`」原出 `1 没 / 2 么 / 3 末`）；当同音存在 `weight >= 100` 实证读音时，拦住 `weight <= 1` 单字参与 Tier 0 `.gram` 提权与 Tier 1 神经竞争并保留在后排可翻找（「`青爷在说喊你开店` + `me`」稳定 `#1 么`）。
  - **非汉字左边界 BPE Token Healing（空格/标点 + 汉字）**：修复 `ShurufaNeuralEngine.cpp` 此前仅对汉字末 token 做左边界 Token Healing 导致 Qwen3 词表 `78` 条 `" " + 汉字` 合并 token（如 `" 和"`、`" 的"`、`" 是"`、`" 在"`、`" 不"`）与 `5` 条 `"，" + 汉字` 合并 token 在英文接空格后被漏算并吃下 `-5~-8 nats` 伪影暴跌的问题（原「`turnitin ` + `he`」将「`和`」挤至第 2–3 页、`turnitin  + shi` 将「`是`」挤出首页）；补齐非汉字前导 token 索引、精确条件分母 `Z_last` 与单字愈合前缀和（`turnitin /ChatGPT /macOS ` 后接 `he`/`de`/`shi`/`zai` 全线 `#1`）。

## 2026-11-26

- **跨音节吞音恢复、免切换句内混输、`AppScope` 场景隔离与右边界句读闭合校准**：
  - **跨音节同辅音吞音恢复与 `*gn -> *n` 音系竞争（`pinyin/`、`quanpin/dictionary.rs`）**：引入 `geminate_boundary_syllable` 与三态 `GeminateCoda`（`-n+n-` / `-ng+g-` 吞音弧，`CHANNEL_GEMINATE_ELISION_PENALTY = 2.35`）及 `*gn -> *n` 动态音系候补（`2.95 nats`），严守 `autocorrect.rs` `correction_table` 静态行数（`807/4442/340/11939`）零改动；在 `merge_syllable_dag_competing_cuts` 中分离排序毫 nat 分与 `dict_weight`，恢复两音节降格吞音实词的正词典权重，做到 `chunuanhuakai -> 春暖花开`、`changjiangungun -> 长江滚滚`、`zhinengongju -> 智能工具`、`mohuyign -> 模糊音` 稳健召回且日常干净直拼零回归。
  - **开发者品牌英文规范词表叠加层与免切换句内同轨混输（`dictionary/english/`、`ime/code_switch.rs`、`ime/queries.rs`）**：新增编译期 `162` 条开发者/品牌标准大小写词表 `CANONICAL_TERMS` 叠加修复随包词库零权重/小写/缺失词（如 `ChatGPT`/`macOS`/`GitHub`/`DeepSeek`/`PyTorch`/`MLX`/`PR`）；在全拼整段不可完整拼读时启用 `CodeSwitchDecoder`（最多 `2` 个英文槽位、`O(n^2)` 全拼可达性、2 字母高置信白名单防误抢中文介词），支持 `woshiyongmacos -> 我使用macOS`、`woshiyongmacoshegithub -> 我使用macOS和GitHub`、`bangworeviewzhegepr -> 帮我review这个PR` 免切换同轨混输。
  - **前台应用 `AppScope` 场景隔离与 Rime `.dict.yaml` 自动识别（`session/`、`input-runtime/`、`host-api/`、`client-services/`、`platforms/macos/`）**：打通 `msime_client_set_app_bundle_id` → `InputSession::app_scope` 全链路分桶隔离与退格负反馈按桶回滚（仅内存映射 5 类粗粒度枚举且绝不落盘原始 `bundleIdentifier`，`syncFieldTraits` 先于 `syncAppScopeContextForClient:` 同步并在安全/隐私态强制复位 `"general"`）；为 `ImportFormat::for_text`、跨批导入通道与 macOS 词库窗口补齐 Rime `.dict.yaml` / `.yaml` / `.yml` 自动识别与格式保留。
  - **端侧右边界句读闭合校准与占位长词对称修复（`ShurufaNeuralEngine.cpp`、`merge.rs`、`neural_v2.rs`、`candidates.rs`）**：`ShurufaNeuralEngine` 无右文时在多字候选主树终点聚合 `，/。/？/！/\n` 五个闭合符对数概率（`kRightBoundaryClosureWeight = 0.80`、截断 `[-5.5, 0.0]`、单批预算不变且同步校准 Stage B）；`merge.rs` 去重命中未测频占位长词时保留原行并回填 `sentence_words`，`neural_v2.rs` 为榜首未测频占位长词恢复 `0.0` 相对静态先验，`candidates.rs` 共用 `sentence_tail_collocation_log_prob` 补齐占位长词句内末词 `.gram` 搭配衔接，消除无右文开放前缀虚高（`woshiyigebing` 在多类上文下稳定 `#1 我是一个兵`）。

- **多音节候选首词多样性、分段前缀直达与生僻字尾收敛**：
  - **词网格中间列防垄断与混淆网络 `[1, 1] -> [1, 1]` 联合重解码（`decode.rs`、`merge.rs`、`quanpin/dictionary.rs`）**：引入中间列按父假设单字出边上限 `PER_PARENT_DEST_CAP = 8`、`terminal_beam = 224`、末列 `[1, 1, ...]` 限流与 `MULTI_CHAR_HEAD_RESCUE_LIMIT = 3` 多字首词保底通道、`build_multi_head_seed_anchor` 首词种子锚点、算子 C2 相邻单字 `[1, 1] -> [1, 1]` 双向 `.gram` 联合重解码及多样性保护的前 6 上浮机制；3–4 字带汉字上文与 `.gram` 时后置 `weight == 100` 未测频占位词并修复 `has_exact_literal_dict_leader` 防止误纠行篡夺首位；实机「`还真是` + `xianzhezhishi`」在 Tier 0/Tier 1 首页同时呈献「`贤者之石`」与前缀「`闲着`/`贤者`」，`shurufa-spoken-v1.tsv` `immediate_top1 / top6` 达 `0.750 / 0.917`、`sentences-v2.tsv` 达 `0.599 / 0.837`。
  - **Tier 0/Tier 1 对齐 `4+2` (`2+1+1`) 首词多样性与第 2–4 页 `24` 席前缀直达（`ordering.rs`、`neural_v2.rs`、`host/session.rs`）**：传递排除隔音膨胀的 `lexical_head` / `has_lexical_head` 标记，统一由 `2+1+1` 3 遍选拔保障首页至少涵盖 2 种以上不同多字词首词读法，同时扩展 `MAX_RERANK_PATHS = 24` 与第 2–4 页 `24` 席前缀短词/常用单字（双音节精确词阈值收敛为 `4` 并在前 8 席后穿插 6 个常用单字）；单音节 `cand_cap = 24`（`ShurufaNeuralEngine.cpp` 同步）并上调 `NEURAL_PROVISIONAL_WINDOW` 至 `70 ms`（`MAX_FAST_SPACE_WAIT = 55 ms`）以避免热态 16 候选 Metal 推理提前锁槽 0。
  - **多音节尾部净化（`session/candidates.rs`）**：非 `Tab` 笔画筛选且非单字模式下，剔除多音节非主切分隔音单字与 `weight <= 0` 扩展区古汉字并收敛合成整句上限为 `16`（「`还真是` + `xianzhezhishi`」总页数由 `184` 页压至 `15` 页 / `89` 项）。

- **自造词学习闭环修复、轻量 `Tab` 笔画筛选恢复与第 1–5 位固定排位重构**：
  - **打通自造词与神经束候选学习闭环**：消除 `InputSession` 独立的第二条 `QuanpinEngine` 连接造成的双连接缓存脑裂，改为统一复用 `ProviderRegistry::quanpin_mut()` 并统一执行 `reset_all_caches()`；在自组/造词写库及 `InjectedBeam`（含 `Action::Command(Command::CommitCandidate)` 与分段前缀 `locked_prefix` 拼接）提交后同步记录 `personal_selection_events` 与 `last_overlay_learning`（保持 5 秒紧接退格回滚）；对静态首位 `< 500` 的弱先验键放开单次自造词即时首槽召回，不降低高频核心词 `1.8` 次防劫持门槛。
  - **在纯 macOS 统一流水线上恢复轻量 `Tab` 笔画与偏旁部件筛选**：基于 `scripts/fetch_stroke_dictionary.py` 随包内置 `msime-stroke.db`（`Contents/Resources/language-dictionaries/`）及引擎内置 `chaizi` 拆字表，恢复全拼/双拼 `Tab` 笔画（`h/s/p/n/z/d/x`）与部件首字母/全拼/俗称辅助筛选；激活筛选期间（`preedit` 含 `｜`）自动挂起 Tier 1 神经重排与内联 Emoji 投影，退出即恢复。
  - **重构并恢复第 1–5 位固定排位（与 L4 席位锁一致）**：贯穿 `WordItem`、`CandidateEntry`、`UnifiedScorer`、`SlotAllocator`、`PurePresenter`、`Runtime`、`host-api`（`msime_client_fix_candidate_position` / `msime_client_clear_candidate_position`）及 macOS 原生候选右键子菜单（`固定排位` → `第 1..5 位` 勾选态 + `取消固定`）；实现 `fixed_candidate_positions` 与 `pinned_candidates` 双向同步以防第 1 槽残影，统一槽位分配器让云候选、`CloudPending`、`AiSuggestion`、内联 Emoji 与神经注入候选绕行全部 L4 固定席位。

## 2026-11-25 — 11-26

- **纯 macOS 单产品减法与三段单向候选管线收敛**：
  - **物理减法收敛单产品**：物理移除 `windows / linux / android / ios / harmony` 五个平台目录、`apps/`、`packages/ui`、`plugins/`、`fuzz/`、多余 crates 与多 edition 契约（仅保留 8 个 Rust crate、`platforms/macos` 与 `ShurufaNeuralService`），移除非拼音方案、8 套辅助码表、`Tab` 笔画筛选、包内 4.5 MB / 25 MB 旧 `safetensors` 模型及 macOS 侧 Sparkle / Tauri / 云剪贴板 / 云词库 / 悬浮栏 / 屏幕键盘等死模块，收口 `scripts/run-checks.sh` 与 `scripts/verify-local.sh`；同步重写 `docs/ARCHITECTURE.md` 与 `AGENTS.md` 并归档架构决策笔记。
  - **三段单向纯管线与双边界神经算子**：围绕不可变 `CursorSnapshot`（含左右上下文与段内暂存前缀 `locked_prefix`）建立 `RecallPool<CandidateEntry> → UnifiedScorer → PurePresenter` 唯一单向管线，废除平行数组快照、`IndexRemap`、`usize::MAX` 假下标及多段原地洗牌；`ShurufaNeuralEngine` 收口为统一双边界条件对数概率算子（左 Token Healing + 右单字前缀边际化 + 句中右侧前瞻），候选右键菜单精简为「置顶到第 1 位」与「删除/屏蔽词条」。
  - **纯 Rust 托管 UDS 客户端与 `Provisional → Presented` 状态机**：将 `ShurufaNeuralService` 子进程生命周期、私有 UDS 二进制帧通信、`50 ms` 空闲预热、热态快敲空格有界等待及 `~28 ms` 微窗口延迟绘制与已绘出面板槽 0 硬锁全部下沉至 `crates/input-runtime`，物理删除 ObjC++ `ShurufaNeuralClient`，并修复候选窗已显示上一代时在 `~28 ms` `Provisional` 窗内按空格被旧面板身份栅栏吞键失灵的问题。
  - **单一 `config.json` 与零拷贝 `ShurufaViewSnapshot`**：废除历史 `NSUserDefaults + options.json` 双存储合并监听，收敛至单一 `~/Library/Application Support/Shurufa/config.json`（兼容首次幂等迁移既有配置）；按键与翻页热路径改用只读零拷贝结构体 `ShurufaViewSnapshot`，消除宿主侧 `_adaptiveCandidates` 影子分页状态。

## 2026-11-24 — 11-25

- **Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构**：
  - **Tier 0 统一真概率词格与混淆网络**：废除 `decode.rs` 的 `+47.63 nat` 人工断崖并清空 `separable_vo.rs` 原 356 条静态动宾硬编码表；自适应束宽（`56/40/32`）+ 零分配 `.gram` 查表 + 1-Span 混淆网络位置扰动消除候选后缀塌缩；3+ 字整句首屏调整为 `4+2` 读法布局。
  - **前端切分、简拼与物理错拼信道**：落地任意句长 A* 零声母隔音切分、长句边界词法剪枝、简全混拼与 4+ 音节纯简拼统一词网格解码、高频简拼与整串纠错概率竞争，以及统一 QWERTY 邻键/换位/缺后鼻音 `g`/两字母非法尾恢复信道。
  - **进程外 Metal 两阶段端侧神经解码（Tier 1）**：新增独立助手进程 `ShurufaNeuralService` 与 `libshurufa_neural.dylib`（私有 UDS 隔离显存，崩溃零掉键自动回退 Tier 0 并指数退避自愈，10 分钟空闲或内存压力自动卸载），默认自动发现本地 `Qwen3-0.6B-Base-Q4_K_M.gguf`（378 MB）；支持 512 字完整上文 KV-Cache 增量预热、Top-16 前缀树共享 KV 批量打分与惊奇度门控局部受限束搜索；纯 Rust `neural_v2.rs` 执行 3 参数线性融合与双兜底 80 ms 槽 0 锁（22 ms 延迟绘制窗，启用时关闭旧 25 MB 停顿精排）。
  - **防劫持个人学习与候选回归修复**：修复内联 Emoji 刷新吞词、非连续同音学习截断与双字 `1+1` 拼凑次选占位；单字键未置顶永不劫持第 1 槽、放开单字语境同音消歧并接入 `.gram` 跨界搭配；新增 5 秒内退格删光回滚 + 24 小时半衰期负反馈与有界 `[-1.5, +1.5]` nats 个人融合先验。

## 2026-11-20 — 11-23

- **全链路本地语音工程升级**：
  - 音频前端合并为单次重采样，新增 80 Hz 高通滤波、噪声门控温和 AGC、软限幅与纯静音误触判定；TextStream 升级全程 Silero VAD 状态机、256 ms preroll、18s/1200ms 软切段与按内存分级的空闲回收/内存压力释放，录音浮层区分「已定稿 / 实时草稿」双态预览。
  - Rust 共享层落地确定性后处理引擎（保缩进规整、块边界结巴与语气词/静音幻觉拦截、口述杠/横杠/斜杠/下划线/点合并、保守 ITN、中英盘古之白、技术词大小写纠正、跨语种个人词表 Top-K 筛选）、`<think>` 剥离与防幻觉清洗、OpenAI 兼容请求/SSE 解析（自动补全 `/chat/completions`）。
  - 新增本机托管 `llama-server`（仅监听 `127.0.0.1`、内存随机 API Key、录音开始异步预热、KV cache 固定前缀）与 `Ollama` 免 Token 本地润色闭环，新增 7.5 秒慢响应提示，支持选中文字口头改写，密码框实施分层隐私闸门。
  - 深化上下文感知：支持光标前后双向上下文读取、为 Chrome/Electron 安全激活 `AXEnhancedUserInterface`、在后台串行队列异步抓取焦点窗口可见屏幕文本（≤220 ms 预算，腾讯文档 `docs.qq.com/doc/` & `/sheet/` 纳入防卡黑名单）。
  - 落地 15 秒事后改错自动学词闭环：精准词边界差分提取、原子持久化 `voice-vocabulary.json`、屏幕右上角不抢焦点的毛玻璃学习气泡（带一键「撤销」与悬停暂停）、以及原生语音设置页「个人语音词表」增删管理卡片。
  - 补齐 IMK `KeyDown` 左右修饰键侧别判定，避免左 Option 误触右 Option 语音快捷键；将开始/结束提示音替换为干脆的 300ms 镜像双音；扩展 `voice_eval` 支持 TextStream 与润色评测。
  - 强化语音健壮性与防丢字闭环：起录首选麦不可用时自动回退至存活系统默认麦；录音中途监听 `AVAudioEngine` 配置变更、CoreAudio 默认/设备列表变化与采集卡死看门狗，支持跨采样率/声道数无缝重建重采样器并继续同一条录音与存档，彻底失麦时抢救已录语音；上屏前校验前台应用与 AX 聚焦元素可编辑性，焦点落到不可输入区域时绝不静默丢字而是复制并弹出不抢焦点的毛玻璃「结果已保留 · 一键复制」预览卡片；识别失败或超时且已留存音频时弹出「重试识别 / 听写历史」恢复卡片支持从加密存档原地重跑；上一条处理中再按快捷键给出「按 Esc 可取消」提示；`ctrl_v` 临时粘贴自动备份并在 220 ms 后无损恢复用户原剪贴板。
- **候选词决策链五层重构**：
  - Stage 0：补齐分阶段归因评测、停顿翻转统计与口语基准集。
  - Stage 1：落地纯函数 `SlotAllocator` 与重排后 4400+ 精选内联 Emoji 投影（带 MRU）。
  - Stage 2：修复万象语法搭配分数的 `LOG10_E` 错误衰减。
  - Stage 3：单趟复用逐键键盘神经分数、停顿精排锁定稳定榜首并标定语境同音词。
  - Stage 4：落地独立个人学习叠加层、30 天半衰期贝叶斯排序与紧接退格即时回滚。

## 2026-11-17 — 11-19

- 接入万象 LTS 语法搭配模型（`wanxiang-lts-zh-hans.gram`）：`install-shurufa.sh` 首次安装缺包时自动拉取并默认启用，设置页新增「万象语法搭配」开关。
- 组字开始前读取一次光标前的现成文本（支持 `IMKTextInput` 与已授权辅助功能的应用；空闲不弹窗，密码框和终端不读），剥离非汉字前缀后送入词格、万象语法窗口和神经重排，并按上文重排多字同音词。
- 中文组字补上离线英文单词前缀与大写起手自动补全（3～4 字母不可拆前缀、5 字母以上前缀、`Shift+字母` 起手，回车原样上屏），不设内部英文模式、Caps Lock 交给系统切 ABC 的边界不变。
- TextStream 本地流式语音模型升级为 `Qwen/Qwen3-ASR-1.7B`。
- 无权重导词（含 `.scel` 细胞词库、纯汉字词表等）与全拼自造词按同音键既有最高词频动态标定初始权重，不再固定 `10000` 空降压过低频固有词；离线词库构建器支持混入口语 N 元语料。
- 整句与调频收敛：补齐动宾离合词（如「许个愿」「请个假」）拆插解码；收紧全拼整句纠错门槛与重排席位，去掉候选窗重复的纠错原始按键回显；为短串神经重排引入静态先验赤字护栏与首选防抖阈值；为超高频核心词自动调频建立渐进护城河（防单次误选击穿，且不阻断强上文同音纠偏）。

## 2026-11-12 — 11-16

- 修复 Tab 偏旁与笔画筛选：进出或增删筛选码时回到第 1 页；支持常见部首俗称（如 `jiao` 绞丝、`dan` 单人）、偏旁全拼前缀、右/底部首与常用独体字；纯 `x` 只按部件筛，不再当全字库通配笔顺；收敛长句筛选时的次选整句。
- 开启本地神经排序时保留主切分的其余整句路径供翻页与停顿后精排使用；补齐指物代词「它」的对称短语与长句前缀多样性解码，优化次选整句排座。

## 2026-11-06 — 11-09

- 随包内置 25 MB 停顿后精排模型，设置页新增「停顿后精排」开关。
- 原生词库窗口支持导入搜狗细胞词库（`.scel`，含 `DCS` 和 `ECS` 布局），大词表改为整份分批导入。
- 候选右键菜单改用非激活面板绘制，不再抢走 Zen 等客户端的键盘焦点。
- 手动置顶的词不再被自动调频挤到第二位；置顶统一只记在引擎里，候选窗不再另按本机置顶表二次挪位。

## 2026-11-04

- 全拼、双拼打完拼音按 Tab，按第一个字的笔顺筛候选：`h` 横、`s` 竖、`p` 撇、`n` 或 `d` 点、`z` 折，`x` 代表任意一笔。也认部件的拼音首字母，比如口是 `k`、亻是 `r`、艹是 `c`。Backspace 逐个删掉筛选字母。包里因此带上了笔画词库，设置页有开关。
- 全拼里打小写 `v` 进入计算与数字：`v1+2*3` 出结果和算式，`v123` 出中文大小写数字和大写金额，`v2026.11.4` 出几种日期写法，`v3jin'g` 做单位换算。`v` 后面直接跟字母时和原来一样当拼音打。
- README 改写成个人 fork 的说明；删掉上游的 README 译文、贡献指南、行为准则、安全策略、issue/PR 模板、赞助配置、Dependabot、全部 GitHub Actions 工作流、隐私说明和代码签名策略。

## 2026-11-03

- 每次录音都写进本机的加密存档，设置页新增「听写历史」，可以设保留期限。密钥第一次录音时生成在登录钥匙串里，只在这台机器上有效。
- 录音期间的普通按键不再打断录音。
- 语音组合键在系统层吞掉时补上输入监控授权，设置页显示各项权限的状态。
- 补全词和前缀区间的候选不再当作已经打完的回答交给重排。
- 构建改用钥匙串里固定的本机自签名证书「Shurufa Local Signing」签名，重编重装后麦克风、辅助功能、输入监控的授权不用重新给。

## 2026-10-30 — 10-31

- 语音快捷键区分左右修饰键。
- 语音组合键在系统层吞掉，不再先触发前台应用里的快捷动作。
- 个人上下文重排不再让前缀候选压过整词。
- 安装后通知菜单栏重读输入源；构建时把源码修订写进 bundle；准备宿主失败时报出具体原因。

## 2026-10-29：收窄成「系统拼音 + 语音」

- 以 macOS 的输入源为唯一权威：不再自己切换中英文，英文交给系统 ABC，Caps Lock 交还系统。
- 只保留一个语音快捷键，去掉 Shift/Control 轻点、各种切换组合键和维护快捷键。
- 关掉快捷模式、临时英文、日期、Unicode、快捷短语、Emoji、颜文字、超级简拼、命令、提及、拆字、符号模式、混合候选、配对标点和智能标点，设置页里的对应入口一并移除。保留基本中文标点、简繁、全角。
- 方向键移动候选，数字和空格选词，中括号和减号、等号翻页，滚轮浏览，这些固定可用。
- 默认关掉辅助码，去掉候选窗上的中英状态标记。
- 语音默认只在浮层里预览实时文字；录音条改成一行。
- 整理：明确关闭整理时以它为准，补全整理接口地址，失败时提示已回退原文；不再把口述改写成列表。
- TextStream 不再把用户词库当作识别上下文。
- 切换输入源时，组字中的字母原样上屏。
- 安装：重装只登记新包、不再先停用输入源；刷新沙盒应用的输入源缓存；父输入源声明简体中文。

## 2026-10-28：Shurufa 成形

- 新增 `shurufa` 版本：独立的 bundle 标识、输入源和数据目录；自己的名字、图标和菜单图标，关于页不再指向上游。
- 新装默认关掉外围功能，强制关掉使用统计和云联想。
- 语音默认使用本地流式识别 TextStream（Qwen3-ASR + MLX），默认开启大模型整理，可以从整理服务拉取模型列表；可以从 OpenLess 一键导入整理配置。
- 可以录制语音快捷键，统一按住说话和切换两种触发方式；跨窗口录音时最终文本只投递一次，失败时有提示或落到剪贴板；语音上屏沿用引擎的简繁输出规则。
- 增加 Shurufa 的构建、安装和 bundle 审计脚本；安装后核实只有一个 Shurufa 服务，安装备份移出可卸载的数据目录。

## 2026-10-27

- 收入 TextStream 本地流式识别的工作进程，随包安装运行环境。
- 原生输入法 bundle 可以不带 Sparkle 构建。
