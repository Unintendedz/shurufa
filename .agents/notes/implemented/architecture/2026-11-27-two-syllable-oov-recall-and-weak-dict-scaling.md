# Agent Note: 双音节词库缺失复合词 `3×3` 保底入池、低频孤立词典壁垒软化与惊奇度去噪

Status: implemented

## Problem

实机日常输入暴露出一类典型的**双音节现代常用复合词（如 `yuankong -> 远控`、`laqun -> 拉群`、`laliu -> 拉流`、`wenzhen -> 稳帧`、`yunkong -> 云控`）完全缺席首页**的系统性盲区，四层耦合根因如下：

1. **主词库覆盖缺口叠加双音节 `[1, 1]` 拆字首尾垄断与隔音重排截断**：
   - `msime-pinyin.db` 的 `tbl_2_y` 仅收有冷僻机械术语 `yuan'kong -> 圆孔(9550)` 而漏收现代高频词 `远控`；
   - `decode_graph_with_terminal` 此前仅在 `n >= 3` 时启用 `head_cap` 与 `suffix_cap`，`insert_two_syllable_split_pool` 将新增拆字附在全部 32 条词格句之后，且在存在隔音切分（如 `yu'an'kong`）时 `merge_alternative_sentences_at` 按纯静态权重重排使 13 条高一元频 `*空` 垄断前 16 席，直接被 `Session::prune_multi_syllable_tail`（`MAX_GENERATED_SENTENCES = 16`）裁掉，导致 `yuan[1] × kong[2]`（`远+控`）掉出 Stage A 前缀树。
2. **孤立低频词典词在 `UnifiedScorer` 中冒充高频核心词筑起 `-1.80 nats` 壁垒**：
   - `neural_v2.rs` 此前用布尔位掩码 `exact_dict_len_mask` 无差别判定成词保护：哪怕同键下唯一一条双字词典词是低频词 `圆孔(9550)`（甚至 `八孔(27)`、`壮歌`），桶内相对归一 `s - dict_max` 仍给它 `rel_stat = 0.0` 满分，并额外对所有 `[1, 1]` 组合（`远控`、`装个`）一刀切扣减 `-1.80 nats` `BASE_NON_LEXICAL_SPLIT_PENALTY`。
3. **Stage B 受限束搜索惊奇度门控被错拼项掩盖**：
   - `ShurufaNeuralEngine.cpp` 计算 `best_avg_char_lp` 与 `best_min_edge_lp` 时未过滤错拼候选（`channel_log_prob <= -1.0`，如 `管控*`），高概率错拼词掩盖了全部字面候选的高惊奇度，阻断了双音节长尾 OOV（如 `稳帧`）的 1 步束搜索救回。

## Decision

围绕「**高频单字正交网格零额外解码成本进 Stage A + 成词保护按真实词频连续退让 + 惊奇度门控仅看干净字面项**」三层闭环修复：

1. **双音节词网格首尾多样性、`3×3` 常用单字网格前置保底与跨切分防截断（`lattice/decode.rs`、`quanpin/dictionary.rs`、`input-runtime/src/runtime.rs`）**：
   - 将 `decode_graph_with_terminal` 的 `head_cap`（`3`）与单字 `suffix_cap`（`3`）扩至 `n >= 2`（不影响 `max_per_prefix == 0` 的全部历史冻结单测）；
   - `collect_two_syllable_split_pool` 容量扩至 `12`：先保留至多 `4` 条首尾去重（每字上限 `2`）的上下文高分 `[1, 1]` 路径，紧接着强制补齐已预热单字缓存中前 `3×3`（`weight >= 1,000`）常用正字组合（如 `{原,远,员} × {空,恐,控}`），由 `insert_two_syllable_split_pool` 前置排入 `MAX_GENERATED_SENTENCES = 16` 窗口，并在 `merge_alternative_sentences_at` 跨切分重排时同步施加首尾 `3×3` 多样性延后；同时在 `Runtime::take_neural_request` 将延后纠错尾槽保底阈值对齐至 `channel_log_prob <= -1.0`（覆盖 `-1.85 nats` 漏 `g` 纠错如 `penyou->朋友`、`yijin->已经`）且保护双音节前 `10` 席字面候选。由于因果语言模型前缀树共享首字 KV，同首字的 3 个次字在同一次 softmax 中零额外 token 同步出分，时延保持 `~21 ms`。
2. **完整全拼双音节按真实静态词频平滑缩放成词保护（`neural_v2.rs`）**：
   - 记录各长度已测频词典最高静态对数先验 `max_dict_stat_by_len`：3+ 字短语/成语与简拼/混拼维持 `1.0` 完整成词结构保护；完整全拼双音节（`len == 2 && bare_initial_count == 0`）在高频核心词（`stat >= -8.8`，`weight >= ~52k`）维持 `1.0` 完整 `-1.80 nats` 保护，低频冷门词在 `[-11.8, -8.8]` 区间线性退让（`stat <= -11.8` 归零），同时保留词典桶自身 `0.0` 相对基准以抵御错拼词劫持字面词（如 `兄台` 抵御 `形态*`、`圆孔` 抵御 `管控*`）；
   - 带上下文双音节场景将 Stage B 独有束候选静态基准定为 `-3.8`，而空上文与 `3+` 音节维持原 `observed_min_rel_stat` 护栏（确保空上文 `beigefen -> 备个份` 与四字句零回退）。
3. **Stage B 惊奇度门控剥离错拼候选并支持带语境双音节极端惊奇触发（`ShurufaNeuralEngine.cpp`）**：
   - 仅在未纠错字面候选（`channel_log_prob > -1.0`）上统计 `best_avg_char_lp` / `best_min_edge_lp`，避免 `管控*` 等错拼词误关束搜索；
   - 在非空上文 `primary_n == 2` 且字面最佳平均对数概率 `< -7.5` 时放行 1 步受限束搜索，救回第 4 位以外同音字组成的双音节 OOV（如 `稳帧`）。
4. **扩充护栏集第 6 组 `oov-2char`（`shurufa-tier1-guardrails-v1.tsv`、`tier1_eval.rs`、`test-tier1-eval-suite.py`）**：新增 10 例全合成双音节 OOV / 极低权现代常用词回归护栏并纳入 `>= 90%` 硬门禁。

## Historical audit

- [端侧两阶段真实模型自动化评测门禁](../testing/2026-11-27-tier1-neural-end-to-end-eval-gate.md)：门禁集由 `205` 例（5 组）升级至 `215` 例（6 组），同时正是 `spoken-v1` 的 `reg == 0` 与 `typo-genuine >= 85%` 硬门禁在迭代中当场拦截了空上文 `beigefen -> 贝格芬` 与缺 `g` 尾音纠错尾槽过滤的潜在副作用。
- [收敛 `stat_log_prob` 重复计分并消除翻页扩容覆写已落定神经排序](2026-11-27-unified-stat-collocation-and-expand-settlement.md)：依托已净化的纯静态 `stat_log_prob`，`max_dict_stat_by_len` 得以直接反映真实词频而非混入 `.gram` 偏移。

## Alternatives considered

### 1. 仅往离线词库校准表手工补录「远控」等个别缺失双字词

- **最强理由**：改一行配置即可让 `yuankong` 出 `远控`。
- **放弃原因**：治标不治本——双音节现代缩略词与技术热词（`拉群`、`拉流`、`稳帧`、`云控` 等）层出不穷，逐词打补丁永远追不上新词；从机制上打通「`3×3` 常用单字网格保底进 Stage A + 低频词典壁垒线性退让 + 去噪 Stage B 束救回」可一次性覆盖整类双音节复合词。

### 2. 对空上文与 3+ 音节无差别放宽 Stage B 束搜索阈值至 `-6.2` 并固定 `beam_stat_rel = -3.5`

- **最强理由**：分支更少。
- **放弃原因**：`tier1_eval` 实测当场捕获该方案在空上文 `beigefen` 上误触发音译外来词幻觉 `贝格芬` 压过离合词 `备个份`，并将 `p90` 时延从 `28 ms` 推高至 `40 ms`；限定于「非空上文 + 双音节字面高惊奇度 `< -7.5`」既零损离合词与长句准确率，又保持 `21 ms` 级低时延。

## Consequences

- `yuankong` 在各类上文及空上文下均稳定召回 `远控`（带语境稳居 `#1`，空上文进首页前排），`laqun -> #1 拉群`、`laliu -> #1 拉流`、`wenzhen -> #1 稳帧`、`bakong -> #1 把控`、`guobu -> #1 国补`、`yunkong -> #1 云控` 及中英混排 `brew  + zhuangge -> #1 装个` 全线修复（`oov-2char` 组 Tier 1 `Top6` 达 `100.0%`）；
- 全套 `427` 例真实 `Qwen3-0.6B` 端到端门禁一次性通过（`guardrails-v1` `93.0% / 98.6%`、真错字组 `85.0%`、`spoken-v1` `91.7% / 98.3%` 零倒退），零违禁幻影、零翻页覆写。
