# Agent Note: 全拼两阶段纠错纪律、隔音偏移补偿与动宾离合词拆插恢复

Status: implemented

## Problem

基于 [端侧两阶段真实模型自动化评测门禁](../testing/2026-11-27-tier1-neural-end-to-end-eval-gate.md) 对 `417` 条真实语境样本的端到端诊断，暴露出三项相互耦合的 Tier 0 → Tier 1 契约失衡：

1. **干净 4 字全拼首槽误纠劫持高达 `26.1%`（`24/92`）且存在双错字幻觉**：
   - `decode_typo_on_graph_with_policy` 未限制单句累计纠错边数，`4` 音节干净输入 `gongxunyongji` 同时启用两条邻键纠错边拼出荒诞双错字整句 `通讯统计*`；
   - `SentenceNgramTables::options` 将 `typo_lead_margin` 设为 `1.0`（而非 `INFINITY`），导致任意不含整句词典条目的多词字面句（如 `阡陌之上`、`界定不准`、`隔日周一`、`时节如流`）只要被常见邻键词借 `.gram` 搭配超出 `1.0 nat` 即在 Tier 0 篡夺槽 0；即便主切分判定 `leads = false`，后续 `merge_alternative_sentences_by_score` 仍按裸 `weight` 全局降序重排整句池，反手把纠错整句重新顶回槽 0（如 `想着只是*`、`前模式上*`）；一旦 Tier 1 超出 `70 ms` 触发 `lock_slot_0` 或未挂载模型，误纠错字即被永久焊死在首位。
   - 反过来，当 `ordering.rs` 正常将非领跑纠错句延后至前缀词之后（第 `35–45` 位），`Runtime::take_neural_request` 满 16 条即止会导致真漏键错拼（如 `meiganxi -> 没关系*`）掉出 `Qwen3-0.6B` 视野。
2. **`.gram` 边偏移令多拆零声母隔音句白拿红利污染首页**：
   - 挂载万象 `.gram` 时，每条词边叠加 `collocation_edge_offset = +3.0 nats`；比主切分多拆出单字元音边界的隔音路径（如 `yu'de'da'o -> 余地大哦`、`gu'a'kai'yi'kan -> 姑阿开一看`、`ta'o'shi'jian'zhang -> 他哦时间长`）仅付 `-0.60 nats` 信道代价却多拿一份边偏移红利，频繁挤入首页。
3. **清空 `separable_vo.rs` 导致动宾离合词拆插在 Tier 0 束搜索中失召回**：
   - `msime-pinyin.db` 普遍缺失 `V + 个/了/过/着 + O` 与 `V + 了个 + O` 词条，且宾语语素（如 `觉(jiao)`、`账(zhang)`、`款(kuan)`、`份(fen)`）单字频显著低于同音常用字（`叫/长/分`），中缀隔断二元转移后在 Tier 0 即被剪出 `nbest`（`spoken-v1` 中 `睡了个觉`/`转个账`/`退个款`/`备个份` 全军覆没，`Qwen3-0.6B` 完全不可见）。

## Decision

围绕「**Tier 0 高召回且严守字面首槽干净，Tier 1 保底可见并结合完整语境裁决**」统一收口四处契约：

1. **收紧 Tier 0 投机纠错准入并消除隔音合并反超（`decode.rs`、`quanpin/dictionary.rs`、`session/candidates.rs`）**：
   - `decode_typo_on_graph_with_policy` 在全拼路径限制单句最大纠错边数 `max_typo_edges = (n / 3).max(1)`（`2..=5` 音节至多 `1` 处纠错边），并记录 `TypoSentence::literal_has_multi_char_word`；
   - `QuanpinDictionary::merge_sentences_with_typos` 扫描前 `64` 行检查是否存在覆盖全键的精确词典整词（含后置占位整词），并对 `3+` 音节已含多字词的合法字面路径令 `effective_margin = f64::INFINITY`（纯单字零散拼句要求 `>= 5.5 nats`），将投机纠错句稳定置于字面整句之后；`merge_alternatives_with_runner_ups` 的弱词典后置纠错提首严格收窄至双音节低权词（`weight < 5_000`）；`merge_alternative_sentences_by_score` 在主切分字面整句领跑时保持首槽仍为未纠错整句（不影响 `merge_syllable_dag_competing_cuts` 的跨音节吞音恢复 `chunuanhuakai -> 春暖花开`）。
   - `session/candidates.rs` 将 `has_exact_literal_dict` 扫描窗扩至 `64` 并识别 `3+` 完整音节多字词字面整句作为 `has_strong_literal_backing`，同步按替换字母数缩放邻键信道代价（`-4.6 nats`/处，上限 2 处）。
2. **Tier 1 神经池延后纠错句尾槽保底（`crates/input-runtime/src/runtime.rs`）**：
   - 在 `take_neural_request` 中，当 `2+` 音节输入的前 16 条入池候选中尚未包含任何纠错候选（`channel_log_prob <= -2.0`）且 `0..128` 后排存在被 `ordering.rs` 延后的整串纠错句时，允许置换 `candidates[4..]` 末尾至多 `MAX_DEFERRED_TYPO_NEURAL_RESCUE = 1` 个非词典普通生成槽位（绝不触碰前 4 席），实现「Tier 0 首屏零误纠污染 + Tier 1 真错字（`meiganxi -> 没关系*`）100% 入池救回」。
3. **挂载 `.gram` 时抵消隔音多拆边的偏移红利（`quanpin/dictionary.rs`、`neural_v2.rs`、`session/candidates.rs`）**：
   - 保持 `pinyin/graph.rs` 的 `DAG_HIATUS_SPLIT_COST = -0.60` 不变以守住长句零声母 A* K-best 召回（`西安`/`方案`/`定案`）；在 `merge_alternatives_with_runner_ups` 激活 `.gram` 时按额外切分数扣回 `-2.20 nats` 边偏移红利，并将 Tier 1 `HIATUS_SPLIT_CHANNEL_PENALTY` 同步校准至 `-1.4 nats`。
4. **在 Shurufa v2 统一对数概率尺度上恢复零分配动宾离合词拆插边（`lattice/separable_vo.rs`、`session/candidates.rs`）**：
   - 恢复 `356` 条有序静态动宾词表 `SEPARABLE_VO_COMPOUNDS` 与零查库、栈缓冲补边 `append_separable_vo_edges`，按 v2 `p99_ref(3)=85k / p99_ref(4)=45k` 标定有效权重（`ge`/`le'ge`: `(w / 25).clamp(800, 18_000)`；`le`/`guo`/`zhe`: `(w / 120).clamp(150, 4_500)`）；
   - 在 `sentence_tail_collocation_log_prob` 中为空上文单跨度 `3+` 字合成词补齐词内尾字 `.gram` 搭配查询（如 `"请个" -> "假"`），与 `[2, 1]` 拆词候选保持对称标尺。

## Historical audit

- [全拼整句纠错归一门槛、重排席位保护、中段切分多样性与纠错回显省略](../bug-fix/2026-11-17-typo-sentence-gating-and-lattice-segmentation-diversity.md)：继承其「字面整句优先、投机纠错退居次席」核心原则，补上隔音合并重排绕过与 `take_neural_request` 尾槽入池闭环。
- [词网格动宾离合词拆插补边与三字短词悬殊次选首屏让位](../bug-fix/2026-11-18-separable-vo-compounds-and-runner-up-gating.md)：将其零分配双端单字证据补边机制重新标定至 Shurufa v2 统一对数概率空间，修复 v2 清空该表后出现的 `睡了个觉`/`转个账`/`退个款`/`备个份` 搜索剪枝失召回。
- [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](2026-11-24-shurufa-v2-probabilistic-neural-ime.md)：修正其早期假设「纯单字路径无需离合词表即可自然竞争」在低单字频约束语素（`觉/账/款/份`）上的盲区。

## Alternatives considered

### 1. 全局把邻键/换位物理信道罚分从 `-2.8` 直接拉高到 `-8.0 nats`

- **最强理由**：无需区分字面路径是否成词，也无需改动 `merge_alternative_sentences_by_score`。
- **放弃原因**：全局重罚会无差别扼杀双音节与真错字召回（`diannai -> 电脑`、`kuaisi -> 快递`、`wenri -> 问题`），破坏精确率-召回率平衡；按「3+ 音节成词字面路径禁止 Tier 0 抢首 + Tier 1 尾槽保底 1 席由 `Qwen3-0.6B` 结合上下文裁决」可在零损失真错字召回的前提下将干净句误纠劫持压降 24 倍。

### 2. 直接把 `pinyin/graph.rs` 的 `DAG_HIATUS_SPLIT_COST` 调至 `-2.0 nats`

- **最强理由**：一处常量即可压低所有隔音多拆候选。
- **放弃原因**：实测单测 `kbest_complete_segmentations_preserve_zero_initial_splits_in_long_sentences` 立即捕获退化——含两处真零声母词的长句（`womendexinfanganyaojinkuaidingan`）在前端切分图被过早重罚挤出；保持前端图 `-0.60` 召回、仅在挂载 `.gram` 合并备选整句时扣回虚增的边偏移红利，兼顾真零声母词召回与首页抗噪。

## Consequences

- **417 例端侧 `Qwen3-0.6B` 真实模型门禁前后对比（总耗时 `13.68 s`）**：
  - `sentences-v2-4char`（92 例）：Tier 0 首槽误纠劫持由 **`24/92 (26.1%)` 暴降至 `1/92 (1.1%)`**（下降 **24 倍**），Tier 0 `Top1` 由 `0.522` 跃升至 **`0.663 (+14.1 pp)`**；Tier 1 `Top1` 由 `0.891` 升至 **`0.913`**、`Top6` 升至 **`0.967`**，Tier 1 误纠劫持降至 **`1/92 (1.1%)`**。
  - `spoken-v1`（120 例）：`separable-vo`（17 例）Tier 0/1 达 **`0.941..1.000 Top1 / 1.000 Top6`**（`睡了个觉`/`转个账`/`退个款`/`备个份`/`请个假` 全线修复）；全集 Tier 0 由 `0.750/0.917` 升至 **`0.808/0.967`**（误纠劫持 `0/120`），Tier 1 由 `0.875/0.942` 升至 **`0.908/0.975`**（误纠劫持 `0/120`、倒退 `0` 例）。
  - `guardrails-v1`（205 例）：干净样本 Tier 0/1 误纠劫持均为 **`0/185`**，同时 `typo-genuine` 真错字控制组保持 **`85.0%`（`17/20`）零回退**（`meiganxi -> #1 没关系*` 经尾槽保底稳健救回）。
