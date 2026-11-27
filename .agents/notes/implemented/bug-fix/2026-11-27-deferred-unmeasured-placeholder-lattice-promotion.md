# Agent Note: 未测频占位整词词网格升格、幻影异读单字护栏与非汉字左边界 BPE Token Healing

Status: implemented

## Problem

实机日常输入连续暴露出三类表面独立、实则均源于「词典静态权重语义与两阶段概率边界失配」的高频退化：

1. **带上文 3–4 字未测频占位整词去重自毁（`我看看 + zenmegeshi`、`yudedao`）**：
   - 输入「`我看看` + `zenmegeshi`」时，首页呈献 `1 怎么格式 / 2 怎么个十 / 3 怎么个使 / 4 怎么各事 / 5 怎么个事儿 / 6 怎么个`，日常高频词「`怎么个事`」被踢至第 29 位并完全漏出 `Qwen3-0.6B` 16 候选池，迫使用户拆选「`怎么`」「`个事`」；同理 `yudedao`（「`遇得到`」）亦在首页消失。
   - 主词库 `msime-pinyin.db` 中 **238,926 条 3–4 字 `weight == 100` 未测频占位词**（占 3 字词表 `33.0%`、4 字词表 `48.5%`，涵盖 `怎么个事`/`遇得到`/`答案就是`/`大爱无疆`/`恶狠狠地`/`擦边主播`/`阿鼻地狱`/`辩解者`/`不携带`/`把爱传递`/`恶意地`/`有毛病吧`/`按理来说`/`干得漂亮`）：`merge.rs` 为防 `先哲之石` 抢首槽将其后置，但当词网格 `decode_graph` 随后经多词组合与 `.gram` 以绝对优势解码出同一句子时，`merge_decoded_paths` 与 `generated_block_linear` 去重跳过发射，反而把 16 条更差生成句插到它前面；随后 `ordering.rs::full_len_dict_after_first` 将其放逐到第 29–43 位，`runtime.rs::take_neural_request` 满 16 条即止导致 `Qwen3-0.6B` 完全看不到该词。
2. **`weight <= 1` 古音/方言幻影单字反超现代实证字（`青爷在说喊你开店 + me`）**：
   - 输入「`青爷在说喊你开店` + `me`」时，首页呈献 `1 没 / 2 么 / 3 末 / 4 麽 / 5 me / 6 麼`。
   - `msime-pinyin.db` `tbl_1_*` 单字表中有 **552 条** 高频常用字（主读音权重 `>= 100,000`）在其它音节下挂有 `weight == 0` 幻影/古音异读（另有 `84` 条 `weight == 1` 极低权古音），如 `me -> 没(0)/末(0)`、`ti -> 是(0)`、`wei -> 有(0)`、`tuo -> 他(0)`、`gan -> 个(0)`、`ne -> 那(0)/哪(1)`、`e -> 啊(0)/哦(1)`、`shui -> 说(1)`。因 `.gram` 与 `Qwen3-0.6B` 只看汉字文本不感知用户所敲拼音，`pick_collocation_homophone` 给 `weight == 0` 赋 `prior = 0.0` 且 `neural_v2_metadata` 又将 `.gram` 净增益叠加进 `stat_log_prob`，导致 `没` 凭 `"开店没"` 的强语境直接击败唯一现代实证读音 `么 (9727)`。
3. **非汉字左边界（英文+空格/标点）单字 BPE Token Healing 缺位（`turnitin  + he`）**：
   - 输入「`turnitin ` + `he`」时，首页呈献 `1 合 / 2 核 / 3 呵 / 4 喝 / 5 he / 6 贺`，「`和`」被压至第 2–3 页；同样 `turnitin  + shi` 把「`是`」挤出首页、`macOS  + de` 让「`的`」输给「`地`/`得`」。
   - Qwen3 词表共有 **20 个非汉字前导 token、223 条「非汉字 + 汉字」合并 BPE token**，其中仅 ASCII 空格 `" "` 就有 **78 条**（精准包含 `" 和"`、`" 的"`、`" 是"`、`" 在"`、`" 不"`、`" 上"`、`" 下"`、`" 对"` 等最高频汉字，而不含 `" 合"`/`" 核"`/`" 地"`/`" 识"`/`" 再"`），中文逗号 `"，"` 有 **5 条**（`"，请"`/`"，在"` 等）。原 `ShurufaNeuralEngine.cpp` 仅在 `decode_first_utf8_hanzi(cached_ctx_last_piece)` 为真时计算分母 `Z_last` 并执行单字前缀和愈合，同时第 `1501` 行跳过单 token 合并单字的 `healed_root_id` 插入，导致 `" 和"`/`" 的"`/`" 是"`/`" 在"` 完全未被计分，只能按 BPE 禁忌拆切 `[token(" "), token("和")]` 吃下 `-5~-8 nats` 伪影暴跌，而未合并的 `" 合"`/`" 核"` 反获裸空格高条件概率虚高。

## Decision

围绕「**守住词典实证来源语义 + 全边界概率守恒**」，在引擎、运行时与端侧神经内核落地三项精确收口：

### 1. 未测频占位整词的词网格组合升格与 Tier 1 尾槽保底（`types.rs`、`lattice/merge.rs`、`ordering.rs`、`session/candidates.rs`、`neural_v2.rs`、`input-runtime/src/runtime.rs`）

- **`WordItem` 增设 `lattice_log_prob_milli_nats: Option<i64>`**：保留 `CandidateSource::Database` 来源与 `weight == 100`（不破坏右键置顶/固定第 1–5 位/删词、`personal_cache_log_bonus`、`has_lexical_head` 与 `prune_multi_syllable_tail` 契约），同时令 `WordItem::static_evidence()` 返回真实词网格得分。
- **`merge.rs` 仅在多词组合验证时原地升格**：先计算 `at = whole_sentence_insert_position_with_options(...)`，在哈希与线性双路径中，仅当 `existing_idx >= at`、`path.words.len() >= 2`（严格要求经多词组合与 `.gram` 独立验证，杜绝单边 `[4]` 占位边经 `decode.rs` Pass 2 新骨架通道自举回流，确保 `先哲之石` 继续后置）且满足 `is_promotable_deferred_placeholder` 时，将原 `Database` 行克隆移入 `block` 真实名次、回填 `sentence_words` 与 `lattice_log_prob_milli_nats` 并降序移除原后置槽位；`existing_idx < at`（如 5 字 `我是一个兵` 与空上文）保持原地回填 `sentence_words`。
- **`ordering.rs` 区分已验证升格词与未验证占位词**：`runner_up_order` 将 `is_lattice_scored_dict`（`Database && static_evidence.is_some()`）纳入主读法集合，将 `full_len_dict_after_first` 收窄为 `static_evidence.is_none()`；并修复 `visit_runner_up_order` 的 `leader_head_cap`（当 `cap >= 3 && (first_reading == 0 || head_keys[..head_len].contains(&leader_head))` 时维持同首词合计 `2` 席），避免已测频领跑词后的同首升格词（如 `恶狠狠的` 后的 `恶狠狠地`）被异首词挤出首页。
- **`candidates.rs` / `neural_v2.rs` / `runtime.rs` 独立分桶与尾槽保底**：`contextual_dictionary_promote` 跳过 `lattice_log_prob_milli_nats.is_some()` 的榜首；新增 `stat_from_lattice: bool`（`#[serde(default)]`）令升格占位词复用句内尾词 `.gram` 搭配分、归入 `lattice_max` 桶独立归一（不污染 `dict_max`）且不触发 `exact_dict_len_mask` / `-1.8 nats` 拆字罚分；`runtime.rs::take_neural_request` 在 `primary_syllables >= 3` 且满 16 条时继续扫描 `0..128`，将至多 `MAX_DEFERRED_DICT_NEURAL_RESCUE = 2` 条未纠错精确后置词典整词置换入 `candidates[4..]` 尾槽（绝不触碰前 4 席），使未进词网格前 16 的真实占位词（如 `恶狠狠地`、`阿鼻地狱`、`遇得到`）同样获得 `Qwen3-0.6B` 裁决。

### 2. 单音节 `weight <= 1` 幻影异读护栏（`crates/engine/src/session/candidates.rs`、`crates/engine/src/lattice/octagram.rs`）

- 仅当当前单音节存在至少一条实证现代读音（`CandidateSource::Database && weight >= 100`，不影响全零权方言音节如 `ng`/`n` 或小权测试夹具）时：
  - Tier 0 的 `contextual_dictionary_promote`、`find_single_char_prefix_promote` 与 `octagram::pick_collocation_homophone` 跳过 `item.weight <= 1` 的单字异读条目（用户显式学习提权除外）；
  - Tier 1 的 `neural_v2_metadata` 对 `weight <= 1 && user_log_prob <= 0.0` 的未实证单字条目不叠加 `.gram` 搭配加分，并置 `exact_span = false` 使其保留在候选后排可翻找，但不进入 `Qwen3-0.6B` 排序池抢占现代实证字。

### 3. 非汉字左边界 BPE Token Healing 与精确分母 `Z_last`（`platforms/macos/src/neural/ShurufaNeuralEngine.cpp`）

- 在 `build_bpe_prefix_index()`（启动一次性执行，`< 3 ms`）新增 `find_first_hanzi_after_non_han_prefix`，全词表自动索引全部「非汉字前导 token + 汉字」合并项到 `non_han_hanzi_prefix_tokens[prefix_tid][han_cp]`（Qwen3 全表仅 20 个前导 token、223 条合并项），并为这 20 个前导 token 预建 `non_han_prefix_z_tokens[prefix_tid]`；
- 在 `prefill_context_locked()` 中，当 `cached_ctx_last_token` 命中 `non_han_prefix_z_tokens`（如 `" "`、`"，"`、`"（"`、`"”"`）时，按 `cached_ctx_prev_log_probs` 一次性精确计算 `Z_last = log sum_{t starts with s_last} P(t | C_base)` 与 `unmerged_shift = min(0.0, cached_ctx_last_lp - Z_last)`，彻底消除裸空格条件概率虚高；
- 在 `score_candidates_with_diag_locked()` 中，对单字候选补齐 `cached_ctx_last_has_non_han_heal` 分支汇总 `non_han_hanzi_prefix_tokens[cached_ctx_last_token][han_cp]`（同步沿用 `kSingleCharRightExtStopPenalty` 折算多字延伸如 `" 是否"`）并减去 `Z_last`，再经 `log_sum_exp_pair(lp_dir, lp_heal)` 汇入主概率；同时收紧第 `1630` 行兜底条件 `covered_by_prefix_sum = cached_ctx_last_has_han_first || cached_ctx_last_has_non_han_heal`，杜绝任何未建索引的单 token 合并单字被静默丢弃。

## Historical audit

检索 `.agents/notes/` 后，本修复直接闭环并已与以下既有决策笔记双向互链：

- [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](../architecture/2026-11-24-shurufa-v2-probabilistic-neural-ime.md)：将其左边界 BPE Token Healing 从「仅汉字末 token」推广至全部「汉字 + 非汉字（空格/标点）」BPE 合并边界，补齐贝叶斯条件分母 `Z_last` 守恒。
- [多音节候选首词多样性、分段前缀直达与生僻字尾收敛](../architecture/2026-11-26-candidate-diversity-and-prefix-hierarchy.md)：保留 3–4 字未测频占位词后置护栏，修补其与 `merge.rs` 去重、`ordering.rs::full_len_dict_after_first` 叠加产生的组合胜出反遭放逐缺陷。
- [跨音节吞音恢复、免切换句内混输、AppScope 隔离与右边界句读闭合校准](../feature/2026-11-26-chime-inspired-upgrades-and-boundary-closure.md)：沿用未测频占位长词 `sentence_words` 回填与句内尾词 `.gram` 衔接，并将其延伸至带上文 3–4 字升格占位词。

## Alternatives considered

### 1. 将命中词网格的 `weight == 100` 后置占位词直接改写为 `CandidateSource::Collocation` / `Generated`（Option 1A）

- **最强理由**：无需在 `WordItem` 与 `NeuralCandidateInput` 中新增 `lattice_log_prob_milli_nats` / `stat_from_lattice` 字段，天然归入 `lattice_max` 桶。
- **放弃原因**：丢弃 `CandidateSource::Database` 来源会破坏 `is_editable_source`（右键置顶/固定/删词）、`personal_cache_log_bonus`、`has_exact_literal_dict_leader`、`has_lexical_head`、尾部截断保护及 `-1.8 nats` 拆字罚分豁免；保留 `Database` 并独立携带 `lattice_log_prob_milli_nats` 与 `stat_from_lattice` 可一次守住全部契约。

### 2. 面对 `turnitin  + he` 时，在送入 `ShurufaNeuralEngine` 前直接剥离上文末尾空格

- **最强理由**：改动仅需一行 `trim_end()`，可立即绕开 `" "` + 汉字合并问题。
- **放弃原因**：直接删掉上文末尾空格会篡改真实语境（例如英文单词后接空格与直接连写、句末逗号后换行与句中连续），无法修复 `"，"` 等中文标点左边界的 BPE 合并（`"，请"`/`"，在"`），且破坏 `prefill_context_locked` 的增量 KV-Cache 前缀复用；按贝叶斯精确公式补齐非汉字左边界 Token Healing 与分母 `Z_last`，一次解决空格、逗号及全部 20 个非汉字前导 token 的 BPE 边界伪影，且零额外 GPU 前向推理。

### 3. 直接在主词库查询层硬删除 `weight <= 1` 单字行，而非仅拦住 `.gram` / Tier 1 重排

- **最强理由**：连后排翻页也完全不出现古音异读，单音节列表更短。
- **放弃原因**：部分生僻字或异体字在 `tbl_1_*` 中本就以 `weight == 0` 收录，且个别用户翻页查找古音/方言/繁体异体（如 `嚒`/`麼`）仍依赖原词典召回；仅在存在 `weight >= 100` 实证读音时禁止 `weight <= 1` 条目参与 `.gram` 与 `Qwen3-0.6B` 提权，既 100% 杜绝「`me -> 没`」类劫持，又零损失保留完整字库翻找能力。

## Consequences

- **端侧 `ShurufaNeuralService`（`Qwen3-0.6B-Base-Q4_K_M.gguf` + `wanxiang-lts-zh-hans.gram`）34 组实机回归验证**：
  - **占位整词（Fix #1）**：「`我看看` + `zenmegeshi`」Tier 0/Tier 1（`43.6 ms`）双 `#1 怎么个事`；`yudedao` 在空上文与「`我们`/`哪里`/`没想到`」上文下均稳定入首页 `#2–#3 遇得到`；`答案就是`、`恶狠狠地`、`阿鼻地狱`、`把爱传递`、`按理来说` 全部 Tier 1 `#1`；控制组「`还真是` + `xianzhezhishi`」保持 `贤者之石` 首页且 `先哲之石` 不劫持。
  - **幻影异读单字（Fix #2）**：「`青爷在说喊你开店` + `me`」、「`你去吃饭` + `me`」、「`这是什` + `me`」Tier 0/Tier 1 全部 `#1 么`；`ne` 稳居 `#1 呢`；`ti`/`wei`/`e`/`shui` 彻底消除 `是`/`有`/`啊`/`哦`/`说` 劫持；控制组「`还真是贤者之` + `shi`」保持 `#1 石`。
  - **非汉字左边界 BPE Token Healing（Fix #3）**：`turnitin  + he`、`我用 turnitin  + he`、`ChatGPT  + he`、`macOS  + he`、`' ' + he` 全部 `#1 和`（`30–50 ms`）；`turnitin  + de` / `macOS  + de` 全 `#1 的`；`turnitin  + shi` 全 `#1 是`；`turnitin  + zai` 全 `#1 在`；`turnitin  + hezuo` 保持 `#1 合作`。
- **内存与时延开销**：`non_han_prefix_z_tokens` 与 `non_han_hanzi_prefix_tokens` 全词表仅占 `~220 KiB` 内存，启动建表增耗 `< 1.5 ms`；上文末尾为空格时单次 `prefill` 计算 `Z_last` 约 `< 0.1 ms`，零额外 Metal 推理开销。
