# Agent Note: 词网格动宾离合词拆插补边与三字短词悬殊次选首屏让位

Status: implemented

## Problem

输入 `xu'ge'yuan`（期望候选「许个愿」）时，候选首屏出现 `1 许各院 / 2 许哥原 / 3 徐各院 / 4 许个愿望 / 5 许哥 / 6 许个`，「许个愿」整句完全不在候选中。根因有两层：

1. **三音节与四音节动宾离合词拆插表达系统性缺失**：出货词库 `msime-pinyin.db`（`dict-v2.0.24`）收有双音节离合词 `xu'yuan -> 许愿` 以及前缀词条 `xu'ge -> 许个`、`xu'ge'yuan'wang -> 许个愿望`，却仅收录极少数带中缀的三音节动宾短语（如 `睡个觉`、`洗个澡`、`帮个忙`、`许了个愿`），在 `tbl_3_*` 与 `tbl_4_*` 中普遍缺失常见二字动宾离合词的 `V + 个/了/过/着 + O` 与 `V + 了个 + O` 形式（如 `许个愿`、`请个假`、`加个班`、`充个电`、`排个队`、`转个账`、`退个款`、`备个份`、`鞠个躬`、`睡了个觉`）。
2. **`edge_log_prob` 尺度差与首页无差别三席配额放大偏切拼凑**：
   - 在 `edge_log_prob` 中，单字边扣减 `ln(unigram_z) = ln(1e6) ≈ 13.82` nats，双字词边加 `phrase_length_bonus = 20.0`。三音节离合词缺失整词边时，「许个 + 愿」与「许 + 个 + 愿」被迫携带单字惩罚，而偏切 `[1, 2]`（`许 + 各院`、`徐 + 各院`）和 `[2, 1]`（当用户导入或学到前缀双字词如 `INSERTED_WEIGHT = 10_000` 时）凭借双字词奖励与同音字笛卡尔积一口气占满 `nbest = 12`，把「许个愿」整条挤出，使重排模型无从看见它。
   - 同时，`ordering::runner_up_order` 对 `width >= LONG_SENTENCE_CHARACTERS (3)` 一律在首页固定保留 `keep = 3` 个全键读法席位，即使次选读法的 `StaticEvidence::mean()` 已经比领先整句低 `4～6` nats/字（明显属于缺失整词后的拼凑句），仍占用第 2、3 格首屏席位；而 `crates/engine-wasm/src/host.rs` 调用 `runner_up_order` 时未传入 `ScoredOrderRows`，网页端与原生桌面端行为不一致。

## Decision

在词网格建图阶段与次选排座阶段分别收口两处契约：

1. **按双端单字证据补齐动宾离合词边（`crates/engine/src/lattice/separable_vo.rs`、`crates/engine/src/lattice/decode.rs`）**：
   - 定义有序静态词表 `SEPARABLE_VO_COMPOUNDS: &[SeparableVoEntry]`，收录 `356` 条常见二字动宾离合词（按 `(s0, s2, Reverse(weight), v, o)` 排序，含 `催单`、`退单`、`露脸`、`露面`、`还钱`、`还愿` 等多音与常用现代组合），并通过 `lookup_vo_entries(s0, s2)` 做零分配二分查找。
   - 在 `build_graph` 与 `build_graph_borrowed` 完成常规音节建图后调用 `append_separable_vo_edges(syllables, &mut graph, options)`：仅当存在中缀音节 `ge | le | guo | zhe`（三音节）或连续 `le + ge`（四音节），且 `graph` 对应列已存在该中缀的单字边以及 `V`、`O` 两个端点的单字边时，才按栈缓冲（`MAX_PENDING_PER_COLUMN = 8`）去重并在 `options.span_limit` 内合成完整 `3` 或 `4` 音节 `Generated`（`typo: false`）词边；中缀或单字证据不命中时不发生任何堆分配、不改写 `Vec` 容量，也不发起额外 SQLite 查询或改动 `PinyinDatabase::query_lattice_span` / `cached_lattice_span`。
   - 有效词频通过 `effective_vo_weight` 换算为与词库原生拆插短语同等量纲：`ge` 与 `le'ge` 取 `(weight / 300).clamp(60, 1500)`，`le | guo | zhe` 取 `(weight / 2000).clamp(8, 250)`，再交 `edge_log_prob` 计算三/四音节边分。
2. **三字短词悬殊次选让出首屏席位与网页宿主对齐（`crates/engine/src/ordering.rs`、`crates/engine-wasm/src/host.rs`）**：
   - 定义 `RUNNER_UP_MAX_MEAN_GAP_NATS = 3.8`。在 `runner_up_order` 中，仅对三字输入（`width == LONG_SENTENCE_CHARACTERS`，恰位于两字 `keep = 1` 边界之上）读取首条读法的 `StaticEvidence::mean()`；若某条次选读法的均分落后首条超过 `3.8` nats/字（`far_below_leader`），`visit_runner_up_order` 不将其计入首页 `keep = 3` 个读法名额，并借 `expand_front_with_unused_keep = width >= LONG_SENTENCE_CHARACTERS` 把未用完的读法席位补给前排非读法候选（如 `许个愿望`、`许个`、`许哥` 等前缀词/短词）以凑满首屏 6 格；被延后的次选按相对原序排在第 2 页次选区（近分次选之后、剩余单字之前），保持零分配恒等快速路径与幂等性。
   - `width != 3`（含四字短语与五字及以上长句，如 `毛后失宠`、`走班教学`、`算尽天下`）不启用 `far_below_leader`，完整保留首页 3 个读法席位。
   - `crates/engine-wasm/src/host.rs` 的 `WebHost` 调用 `runner_up_order` 时构造 `&ScoredOrderRows::new(&rotated, &rotated_evidence)`，与 `crates/input-runtime` 的 `Runtime::demote_runner_up_readings` 保持一致。

## Evaluation

使用 `cargo build --release -p msime-input-runtime --example convert_eval --example rerank_latency`（开启 `--keyboard-neural --autocorrect`，学习关），搭配 `target/eval-exp/resources-settled` 与 `settled.lock.json`（含 `4.5 MB` 键盘模型与 `25 MB` 停顿模型）实测：

### 定向场景与 36 例动宾离合词集

- `xugeyuan`（`vo-001`）：`[(1, '许个愿', 8), (2, '许个愿望', 0), (3, '许个', 0), (4, '旭哥', 0), (5, '许哥', 0), (6, '许', 0), (7, '许各院', 8), (8, '徐各院', 8), (9, '许各园', 8), (10, '许个原', 8)]`。「许个愿」居第 1，悬殊偏切「许各院 / 徐各院」退居第 2 页开头（`#7` 起）。
- `shuilegejiao`（`vo-002`）：`[(1, '睡了个觉', 8), (2, '睡了硌脚', 8), (3, '睡了割胶', 8), (4, '睡了', 0), (5, '水了', 0), (6, '水', 0)]`（第 1）。
- `woxiangxugeyuan`（`vo-003`）：`[(1, '我想许个愿', 8), (2, '我像许个愿', 8), (3, '我向许个愿', 8), (4, '我想学', 0), (5, '我想', 0), (6, '我向', 0)]`（第 1）。
- 36 例动宾离合词合成集（含 `许个愿`、`请个假`、`加个班`、`充个电`、`排个队`、`转个账`、`退个款`、`备个份`、`鞠个躬`、`睡了个觉`、`加了个班`、`洗了个澡`、`帮了个忙`、`吃过饭`、`洗过脸`、`睡了觉`、`退个单`、`催个单`、`露个脸`、`还个钱`、`还个愿`、`许个愿吧` 等）：`found = 1.000 (36/36)`、`top5 = 1.000 (36/36)`、`top1 = 0.972 (35/36)`、`mrr = 0.986`（`vo-ge` `27/27`、`vo-lege` `4/4`、`vo-guo` `2/2`、`vo-le` `1/1` 全第 1，仅 `xugeyuanba` 居 `许个院坝` 后第 2）。

### 既有基准与逐键时延

| 集合 | `top1 / top5 / top9 / mrr` | `found` | 对照未改 `main` 说明 |
|---|---|---|---|
| `sentences-v1` (60) | `0.850 / 0.883 / 0.883 / 0.867` | `0.883` | 与基线完全持平 |
| `sentences-v2` (312) | `0.692 / 0.910 / 0.968 / 0.803` | `0.981` | `top1 / top5 / top9 / mrr` 持平，`found` 较 `0.974` 升 `+0.7 pp` |
| `sentences-particles-v1` (103) | `0.893 / 0.951 / 0.961 / 0.919` | `1.000` | 与基线完全持平 |
| `sentences-neutral-v1` (1105) | `0.814 / 0.954 / 0.962 / 0.880` | `0.966` | 长句召回与 `top5` 保持最优水平 |
| `quanpin-words-v1` (`--limit 3000`, 2791) | `0.755 / 0.967 / 0.984 / 0.836` | `0.989` | 词级指标与基线完全持平 |

- 逐键时延（`rerank_latency --keyboard-neural`，`sentences-v1` 60 例 / 1320 键）：`engine only` `mean = 2.57 ms`、`p95 = 11.61 ms`；`with model` `mean = 2.71 ms`、`p50 = 0.23 ms`、**`p95 = 11.95 ms`**（低于 `16.00 ms` 预算门限）、`p99 = 21.58 ms`；`reranker adds` `mean = 0.14 ms`、`p95 = 1.16 ms`。

## Historical audit

- [词网格指物代词对等、末列前缀多样性、隔音切分降级与次选排座精排](2026-11-14-lattice-pronoun-diversity-and-seating.md)：直接沿袭其「不改出货词库文件、按词网格已有证据在构图期补边」的模式与 `runner_up_order` 首页配额设计；本篇进一步为三字读法增加基于静态均分落差的拼凑句退让规则，并保持 `width >= 4` 的原有三席配额。
- [全拼整句纠错归一门槛、重排席位保护、中段切分多样性与纠错回显省略](2026-11-17-typo-sentence-gating-and-lattice-segmentation-diversity.md)：延续 `visit_runner_up_order` 的首屏名额与次选优先级顺序，在未纠错优先与纠错滞后之间插入 `far_below_leader` 分层。
- [独立句子排序保留词网格的静态证据](2026-10-14-rerank-static-evidence.md) 与 [整句候选恒等排列避免临时分配](../testing/2026-10-08-runner-up-order-identity.md)：继续复用 `StaticEvidence::mean()`，并在 `visit_runner_up_order` 保持无变化零分配恒等快速路径。

## Alternatives considered

- **在 `PinyinDatabase::query_lattice_span` / `cached_lattice_span` 中拦截三/四音节跨度并重查 `(s0, s2)`**：
  最强理由是可以和词库查询共用缓存结构。但 `warm_lattice_span_cache` 仅预热连续跨度，跳字跨度在缓存严格模式下会触发断言失败，且 `lattice_span_key_plan_tests`、`lattice_span_cache_tests` 与 `aggregate_page_reuse_tests` 冻结了查库计划与分配次数；改为基于已构造的 `graph` 单字边与编译期排序小表 `SEPARABLE_VO_COMPOUNDS` 在 `build_graph` / `build_graph_borrowed` 末尾零分配补边，完全不触碰查库冻结契约。
- **直接将合成离合词标记为 `CandidateSource::Database`**：
  最强理由是可以按精确词典词条直接豁免重排器竞争。但 `InputSession::learn_ranked_candidate` 在候选位置为 0 且来源为 `Database` 时会跳过整句学习持久化；保留 `Generated`（`8`）来源既能让神经模型参与同音歧义裁决（如「许个愿吧」与「许个院坝」），也能在空格上屏时通过 `learn_sentence_candidate` 写回用户个人词库。
- **将 `far_below_leader` 同时应用于四字及以上短语（`width <= 4`）**：
  最强理由是可以一并清理四字离合词（如 `睡了个觉`）后方的拼凑四字读法。但实测在 `sentences-v2`（312 例）上，`width == 4` 的合法 `[1, 1, 2]` 短语（如 `毛后失宠`、`走班教学`、`算尽天下`）常与 `[2, 2]` 同音词（`毛猴失宠`、`走板教学`、`算进天下`）拉开超过 `3.8` nats/字的静态均分差，放行到 `width <= 4` 会把这批四字正解挤出首屏并使 `sentences-v2` `top5` 从 `0.910` 跌至 `0.904`（-2 例）；而 `睡了个觉` 本身已稳定位居第 1。因此将 `far_below_leader` 严格限定在 `width == LONG_SENTENCE_CHARACTERS (3)`，兼顾三字短语首屏干净度与四字及以上长句的 `top5` 召回。

## Consequences

- **收益**：高频三/四音节动宾离合词拆插不再因出货词库缺词而被偏切双字名词挤出 `nbest`，`xugeyuan` 等三字输入首位直接命中正统离合词且首屏不再残留「许各院 / 许哥原」等高分差拼凑项，Web 与桌面宿主在静态证据排座上完全一致。
- **代价与边界**：仅覆盖 `SEPARABLE_VO_COMPOUNDS` 收录的 `356` 个现代常见二字动宾离合词与 `ge / le / guo / zhe / le'ge` 五类中缀（含 `max_phrase_syllables >= 3/4` 且 `graph` 含对应单字边时生效）；未列入静态表的新造动宾组合仍依赖用户首次手动选取后的词库学习。
- **验证覆盖**：
  - `msime-engine --lib` `2734 passed; 0 failed; 57 ignored`（含 `separable_vo_table_is_sorted_unique_and_valid`、`separable_vo_edges_are_synthesized_from_single_char_evidence_and_lead_competitors`、`runner_up_order_defers_far_weaker_readings_from_first_page_and_keeps_idempotence`，以及全部 640 例 `graph_ownership_tests` 与查库缓存契约测试）。
  - `msime-input-runtime` `408 passed; 0 failed`；`msime-engine-wasm` `72 passed; 0 failed`。
  - `msime-engine --test golden` `30 passed; 1 failed`（仅 `scripted_scenarios_match_the_reference` 保留 `main` 既有的 10 处 `answers_key` 历史基线差异，真实句集与 290/300 剧本零回归）。
  - 后续在 Shurufa v2 统一对数概率尺度下的重新标定与两阶段纠错纪律联动见 [全拼两阶段纠错纪律、隔音偏移补偿与动宾离合词拆插恢复](../architecture/2026-11-27-speculative-typo-discipline-and-separable-vo-recall.md)。
