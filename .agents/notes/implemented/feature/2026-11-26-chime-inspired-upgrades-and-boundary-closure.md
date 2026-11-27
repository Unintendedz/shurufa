# Agent Note: 跨音节吞音恢复、免切换句内混输、AppScope 隔离与右边界句读闭合校准

Status: implemented

## Problem

在落地 [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](../architecture/2026-11-24-shurufa-v2-probabilistic-neural-ime.md) 与 [多音节候选首词多样性、分段前缀直达与生僻字尾收敛](../architecture/2026-11-26-candidate-diversity-and-prefix-hierarchy.md) 后，日常真实打字仍存在四组具体体验缺口：

1. **跨音节双辅音吞音与句尾错拼竞争不足**：汉语拼音跨音节同辅音连缀漏敲一个辅音时（如 `chunuanhuakai -> chun'nuan'hua'kai`「春暖花开」、`changjiangungun -> chang'jiang'gun'gun`「长江滚滚」、`zhinengongju -> zhi'neng'gong'ju`「智能工具」、`fanan -> fan'nan`「犯难」、`xianan -> xian'nan`「险难」、`kunan -> kun'nan`「困难」），原切分图无法从前音节韵尾还原后段声母导致漏召回（`chunuanhuakai` 落为「初年花开」）；同时句尾 `*gn` 错拼（如 `mohuyign -> 模糊音`）因 `yign -> yin` 缺少音系竞争路径而被 `yign -> ying`（换位）压至第 59 位。
2. **开发者英文大小写缺位与句内同轨混输中断**：随包 `msime-english.db` 中部分高频开发者词条权重为 `0`（如 `ChatGPT`/`GitHub`/`GitLab`/`OpenAI`/`SwiftUI`）、仅存全小写（`deepseek`/`pytorch`/`tensorflow`/`qwen`/`llm`）或缺失（`mlx`/`pr`/`mr`/`ux`/`visionOS`）；且 [Shurufa 中文输入下的英文单词自动补全](./2026-11-17-shurufa-english-word-autocomplete.md) 仅处理「整段串为英文词或前缀」，输入 `woshiyongmacos`、`woshiyongmacoshegithub`、`bangworeviewzhegepr` 等句内中英夹杂串时全部返回破碎拼音。
3. **个人学习场景桶悬空与 Rime `.dict.yaml` 自动识别缺失**：`overlay.rs` 的 `AppScope` 分桶虽已落地，会话层仍硬编码传入 `AppScope::General`，导致编程 IDE 术语与日常聊天习惯无法按前台应用隔离；导入带 `--- ...` YAML 头的 Rime `.dict.yaml` 时，在默认 `standard` 入口与跨批导入通道下无法自动识别为 `ImportFormat::Rime`，文件选择器也未放开 `.yaml`/`.yml`。
4. **无右文开放前缀偏置与未测频占位长词先验失配**：实机输入 `woshiyigebing` 时，尽管主词库 `tbl_5_w` 含有「我是一个兵」（`weight == 100` 占位长词），端侧却在首页输出 `1 我是一个病 / 2 我是一个饼 / 3 我是一个冰 / 4 我是一个并`。根因排查定位为四重失配叠加：
   - `ShurufaNeuralEngine.cpp` 在无右文时仅评估开放前缀条件概率 $\log P_{\text{LM}}(Y \mid C)$，「病」「并」作为「病人/病患」「并不/并非」高频首字获得虚高前缀分，而未在多字候选终点观测句读闭合概率；
   - `merge.rs` 去重命中已存在 `Database` 占位长词时丢弃了词网格 `SentencePath` 的 `sentence_words`（`["我是", "一个", "兵"]`）；
   - `candidates.rs::neural_v2_metadata` 仅对 `Generated | Collocation` 行按 `sentence_words.last()` 计算句内尾词 `.gram` 搭配分，`Database` 行在无全局中文左文时拿不到 `collocation_log_prob`（导致 `我是一个兵` 为 `0.0`，而 `我是一个饼` 拿到 `+3.36`）；
   - `neural_v2.rs` 给 `stat_log_prob == None` 一律赋值 `NEUTRAL_STAT_REL = -1.5`，使 Tier 0 榜首占位长词平白相对次位词网格（`0.0`）亏损 `1.5 nats`。

## Decision

在严格守住 [ARCHITECTURE.md](../../../../docs/ARCHITECTURE.md) 职责边界与零词级静态规则硬编码原则的前提下，落地四项架构与算法升级：

### 1. 跨音节同辅音吞音恢复与 `*gn -> *n` 音系竞争（`crates/engine/src/pinyin/`、`quanpin/dictionary.rs`）

- **纯音系规则建边（零静态词条硬编码）**：`pinyin/typos.rs` 新增 `CHANNEL_GEMINATE_ELISION_PENALTY = 2.35`、`CHANNEL_TRANSPOSED_EXTRA_NASAL_G_PENALTY = 2.95`、`geminate_boundary_syllable(prev_syllable, remainder)`（当前音节以单 `n` 韵尾结尾时补 `n-`、以 `ng` 韵尾结尾时补 `g-`，构造合法 `n-`/`g-` 声母音节）与 `is_transposed_extra_nasal_g(typed, intended)`（识别 `*agn`/`*egn`/`*ign -> *an/*en/*in`）。
- **保持静态纠错表不变**：`pinyin/autocorrect.rs` 严格保持 `correction_table` 静态行数（`807 / 4442 / 340 / 11939`）零改动，在 `correction_targets_for_piece` 中当 `INSERTION` 开启时动态补充 `*gn -> *n` 目标与 `*gn -> *ng`（`2.40 nats`）公平竞争。
- **三态 A* DAG 与正词典权重不变量**：`pinyin/graph.rs` 引入 `GeminateCoda::{None, SingleN, NasalNg}` 标注并在 `DELETION` 开启时生成受控吞音恢复弧，提供跳过 `DagEdgeKind::Initial` 边的 `dag_complete_candidate_paths`；`quanpin/dictionary.rs` 通过零堆分配前哨检查 `may_have_geminate_boundary` / `may_have_transposed_extra_nasal_g` 门控触发 `merge_syllable_dag_competing_cuts`，使用 `DagPooledCandidate { item, score_milli, dict_weight }` 彻底分离排序毫 nat 分与原始正词频，确保两音节直拼实词（如 `发难`、`先按`、`苦难`）稳居首位时，降格进入 `deferred_two_char_dict` 的吞音实词（`犯难`、`险难`、`困难`）恢复 `CandidateSource::Database` 与原始正词典权重 `raw_weight`，不污染下游 `dictionary_unified_stat_log_prob` 与个人叠加层先验。

### 2. 现代开发者品牌规范词表叠加层与免切换句内中英同轨混输（`crates/engine/src/dictionary/english/`、`ime/`）

- **编译期零 I/O 规范词表叠加层（`canonical_terms.rs` + `english.rs`）**：内嵌排序常量表 `CANONICAL_TERMS`（覆盖 `162` 条高频 AI / Apple / 现代开发者品牌与缩写规范大小写及校准权重，明确排除 `review`/`rust`/`test`/`apple`/`python`/`mac`/`he` 等通用小写英文词）；`EnglishDictionary::query_exact` 与 `query_prefix_into` 在规范区间叠加修复零权重/仅小写/缺失词条，非规范区间保持零额外分配快路径并兼容合成小权重测试夹具。
- **无模式句内中英同轨混输（`code_switch.rs` + `ime/queries.rs`）**：在 `SchemeType::Quanpin`、`LocalInputMode::None`、`!dedicated_english`、`english.mixed_candidates`、`5 <= len <= 40`、全小写 ASCII、整段非完整合法全拼（`!is_complete_pinyin_input`）且非完整英文单词时触发 `CodeSwitchDecoder`：
  - 栈上 `O(n^2)` 计算所有子串的全拼可达性，允许至多 `2` 个英文词槽位（`CODE_SWITCH_SLOT_PENALTY = 16.0`、`CANONICAL_TERM_BONUS = 6.0`、2 字母高置信白名单 `SHORT_TECH_ENGLISH_ALLOWLIST` 排除 `SHORT_STOP_PREPOSITIONS` 中 `an`/`do`/`he`/`in`/`me`/`no`/`so`/`to` 等常见介词/代词，避免抢占纯拼音边界）；
  - 懒加载只读 `PinyinDatabase` 与 `SentenceNgramTables` 解码中文片段，直接输出紧凑无空格文本（如 `我使用macOS`、`我使用macOS和GitHub`、`帮我review这个PR`），上屏排版由宿主 `chinese_english_spacing` 按用户偏好处理；通过 `insert_code_switch_rows` 插入首位（避开前置固定/置顶/在线云候选及整键用户词），且不向纯汉字学习日志写入混输词条。

### 3. 前台应用 `AppScope` 场景隔离全链路与 Rime `.dict.yaml` 导入识别（`session/`、`input-runtime/`、`host-api/`、`client-services/`、`platforms/macos/`）

- **粗粒度 `AppScope` 内存映射与安全态强制复位**：在 `host-api`（`msime_client_set_app_bundle_id`）→ `input-runtime`（`Runtime::set_app_bundle_id` / `set_app_scope_key`）→ `engine::host::Session` → `InputSession::app_scope` 打通前台应用标识透传，`learning.rs`、`candidates.rs` 与 `last_overlay_learning` 全线改为消费会话当前 `app_scope`（同桶 `1.0` / 跨桶 `0.5`）。严格隐私硬边界：宿主与 Rust 层**绝不持久化或打印原始 `bundleIdentifier`**，仅在内存中由 `AppScope::from_bundle_id` 映射为 `general` / `im_chat` / `code_ide` / `docs_writing` / `browser_search` 5 个粗粒度标签；`InputController.mm::syncFieldTraits` 先调用 `[_session setFieldTraits:traits]` 再调用 `syncAppScopeContextForClient:`，并在 `TRAIT_SECURE` / `TRAIT_PRIVATE` / `set_private_session(true)` / `secureEventInputActive` / `isSecureField` 时于宿主与 Rust 双端强制复位为 `"general"`，离开安全输入后首个按键即可恢复原场景桶。
- **Rime `.dict.yaml` 自动识别与跨批保持**：`crates/client-services/src/dictionary/import.rs::ImportFormat::for_text` 识别 `---` 至 `...` 之间含非制表符 YAML 配置行的文本为 `ImportFormat::Rime`；`crates/host-api/src/settings_dictionary.rs` 在分批导入前依据 `for_text` 检测结果启用 YAML 头剥离并在跨批 `template["format"]` 保持 `"rime"`（与 `Gboard` 对等）；`DictionaryWindowController.mm` 放开 `.yaml`/`.yml`/`.scel` 与 `UTTypeYAML` 选择并自动按 `"rime"` 格式导入。

### 4. 端侧右边界句读闭合校准与占位长词元数据/先验对称修复（`ShurufaNeuralEngine.cpp`、`merge.rs`、`neural_v2.rs`、`candidates.rs`）

- **无右文句读闭合概率校准（`ShurufaNeuralEngine.cpp`）**：初始化缓存 `{"，", "。", "？", "！", "\n"}` 5 个单 token 闭合符到 `boundary_closure_tokens`。在 `!has_right_suffix && im_boundary_calibration` 模式下，仅对多字候选的主树终点 `primary_end` 标记 `need_boundary_eval` 并纳入单批 `base_internal_leaves` / `assign_branches`（不展开 `comma` 子树，总分支严格受 `max_chunk_seqs` 单批预算限制），聚合 `boundary_eos_lp = log_sum_exp(logits[boundary_closure_tokens]) - log_z` 截断至 `[-5.5, 0.0]` 并以 `kRightBoundaryClosureWeight = 0.80` 叠加至 `total_lp`；Stage B 门控仍沿用 `raw_total_lp / eval_chars`，`beam_candidates` 亦对齐同一右边界闭合刻度，消除开放前缀虚高同时不误伤短语内部连续语境。
- **占位长词 `sentence_words` 回填、榜首先验对称与句内末词 `.gram` 衔接**：
  - `crates/engine/src/lattice/merge.rs` 在 `merge_decoded_paths` 与 `generated_block_linear` 去重跳过命中 `is_unmeasured_dict_placeholder`（`Database && weight == 100 && chars >= 3 && corrected_from.is_empty() && sentence_words.is_empty()`）时保留原行与来源并回填 `path.words` 至 `sentence_words`；
  - `crates/engine/src/neural_v2.rs` 当 `is_dict_bucket(cand.source) && stat_log_prob.is_none() && dict_max.is_none() && *orig_rank == 0` 时赋 `0.0` 相对静态先验（其余 `None` 维持 `NEUTRAL_STAT_REL = -1.5`）；
  - `crates/engine/src/session/candidates.rs` 抽出共用辅助函数 `sentence_tail_collocation_log_prob` 与 `capped_collocation_bonus`，使已回填 `sentence_words` 的未测频词典占位长词与 `Generated | Collocation` 整句一致按 `sentence_words.last()` 计算 `collocation_log_prob`，且不重复叠进 `stat_log_prob`。

## Historical audit

检索 `.agents/notes/` 后，本笔记与以下既有决策存在继承、补全或互链关系：

- [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](../architecture/2026-11-24-shurufa-v2-probabilistic-neural-ime.md)：在其统一信道、`Qwen3-0.6B-Base` 双边界概率与 `overlay.rs` `AppScope` 存储之上，完成宿主 `bundleIdentifier` 全链路透传与安全态复位、跨音节吞音恢复、`*gn -> *n` 竞争以及右边界句读闭合校准，已在原笔记同步互链。
- [Shurufa 纯 macOS 单产品减法与三段单向候选管线收敛](../architecture/2026-11-25-shurufa-pure-macos-pipeline-convergence.md) 与 [多音节候选首词多样性、商业级分段前缀直达与生僻字尾收敛](../architecture/2026-11-26-candidate-diversity-and-prefix-hierarchy.md)：继续严守 `RecallPool -> UnifiedScorer -> PurePresenter` 与 L4 固定排位不变量；`CodeSwitchDecoder` 作为 `CandidateQueries::mixed` 产出的 `Generated` 候选由既定展示链护送首槽，且不修改 `PurePresenter` 契约。
- [Shurufa 中文输入下的英文单词自动补全](./2026-11-17-shurufa-english-word-autocomplete.md)：保留其整词/短前缀与大写起手英文补全边界及「无内部英文模式、Caps Lock 交还系统 `ABC`」定位，向上叠加开发者品牌规范词表与句内中英同轨混输。
- [全拼、双拼的 Tab 笔画与部件筛选](./2026-11-04-tab-stroke-filter.md)：明确保持 `Tab` 键专属于轻量笔画/部件筛选，不引入古诗或大模型 `Tab` 续写抢占按键习惯。
- [导入搜狗细胞词库（.scel）](./2026-10-13-scel-import.md)：保留 macOS 原生词库窗口分批导入管道，为 `.dict.yaml` 补齐文本头自动识别与跨批 `"rime"` 格式保留。
- 后续演进见 [未测频占位整词词网格升格、幻影异读单字护栏与非汉字左边界 BPE Token Healing](../bug-fix/2026-11-27-deferred-unmeasured-placeholder-lattice-promotion.md)：将未测频占位长词的 `sentence_words` 与句内尾词 `.gram` 衔接机制完整延伸至 3–4 字带上文后置场景，补齐词网格组合升格、`stat_from_lattice` 独立分桶、Tier 1 尾槽精准保底及非汉字左边界 BPE 愈合。

## Alternatives considered

### 1. 将 28 层进程外 `Qwen3-0.6B-Base` 替换为 Chime 6 层裁词表 MLX 蒸馏小模型

- **最强理由**：6 层裁词表 MLX 蒸馏模型参数量、磁盘体积与单步矩阵计算量更小，理论上可降低推理内存占用并压缩端侧前向耗时。
- **放弃原因**：大幅裁剪层数与词表会明显削弱模型对现代中文新词、专有名词、长距世界知识以及 512 字中英混杂上文（标点/ASCII/Emoji）的概率建模精度，导致同音消歧与高惊奇度受限束救回出现结构性退化；当前 `ShurufaNeuralService` 基于 `llama.cpp` Apple Metal 前缀树共享 KV、`70 ms` Provisional 防闪微窗口与进程外故障隔离，实测单批打分保持在 `~17–22 ms` 中位数，无需以牺牲 36T 基座泛化分布换取小幅延迟余量。

### 2. 引入 Chime 静态 `closed_grammar.yaml` 虚词/句式规则表

- **最强理由**：通过维护静态虚词与句尾闭合配置表，可以低成本针对典型句式（如 `我是一个兵`）直接规则加分修复排序。
- **放弃原因**：静态虚词/句式补丁表会重演词级硬编码维护困局，治标不治本。真实设备排查已证实 `woshiyigebing -> 我是一个病 / 我是一个饼 / 我是一个并` 的根因并非缺少虚词表，而是：（1）无右文时单算开放前缀对数概率使高频词首字（`病/并`）占优；（2）`merge.rs` 去重丢弃了占位长词的 `sentence_words` 导致 `candidates.rs` 未能对其末尾词查询 `.gram` 搭配分；（3）`neural_v2.rs` 给榜首占位长词施加了 `-1.5 nats` 的不对称静态先验。采用右边界 5 闭合符句读校准叠加占位长词元数据与先验对称修复后，从概率建模根源一并消除整类开放前缀虚高现象，全程零新增静态词级规则表。

### 3. 复用 `Tab` 键触发古诗续写或大模型整句续写

- **最强理由**：在需要诗词或长文续写时，直接用 `Tab` 触发上下文生成交互最快捷。
- **放弃原因**：Shurufa 已将全拼/双拼组字态下的 `Tab` 键固定分配给轻量笔画（`h/s/p/n/z/d/x`）与偏旁部件筛选（见 [全拼、双拼的 Tab 笔画与部件筛选](./2026-11-04-tab-stroke-filter.md)），更改 `Tab` 语义会直接破坏同音难字筛选的既定肌肉记忆并引入额外模态复杂度。

## Consequences

- **正收益**：
  - 跨音节 `-n+n-`/`-ng+g-` 吞音（`chunuanhuakai -> 春暖花开`、`changjiangungun -> 长江滚滚`、`zhinengongju -> 智能工具`）与 `*gn -> *n`（`mohuyign -> 模糊音`）按统一音系概率信道融入词网格，不破坏 `correction_table`（`807/4442/340/11939`）与日常干净拼音默认首位。
  - 日常中文输入下无缝获得标准大小写开发者品牌词（`ChatGPT`/`macOS`/`GitHub`/`DeepSeek`/`PyTorch`/`MLX`/`PR`）及免切换句内混输（`我使用macOS`、`我使用macOS和GitHub`、`帮我review这个PR`），无需反复切换系统 `ABC`。
  - 个人学习与候选打分正式按前台应用粗粒度 `AppScope` 隔离，密码框与隐私态硬隔离并即时恢复；Rime `.dict.yaml` 在默认格式下即可自动识别并完整跨批导入。
  - 无右文时的开放前缀虚高得到系统性校准，占位长词与词网格整句在 `.gram` 尾词搭配与相对静态先验上恢复完全对称（`woshiyigebing` 在空上文、中文标点上文与英文终端上文下均稳定由端侧模型选出「我是一个兵」）。
- **代价与维护约束**：
  - `canonical_terms.rs` 维护 `162` 条编译期开发者品牌规范词表，后续扩充词条须继续避开通用小写英文词以免抬高普通单词误覆盖，并只在全量测试下核验与 `msime-english.db` 的排序兼容性。
  - `ShurufaNeuralEngine.cpp` 无右文多字候选开启 `need_boundary_eval` 后，每个多字叶子节点会纳入单批前缀树求 logits（受 `max_chunk_seqs` 单批上限保护）；若未来扩大候选上限，须同步核验单批序列余量与延迟预算。
  - `CodeSwitchDecoder` 懒加载只读 `PinyinDatabase` 与 `SentenceNgramTables` 仅在触发句内混输条件时发生一次；若资源目录缺主词库则静默退回零候选。
