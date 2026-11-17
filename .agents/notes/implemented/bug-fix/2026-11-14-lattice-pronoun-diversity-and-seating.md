# Agent Note: 词网格指物代词对等、末列前缀多样性、隔音切分降级与次选排座精排

Status: implemented

## Problem

全拼整句输入在指物代词、长句尾词收敛、隔音备选切分和次选候选排座上同时存在四处相互放大的召回与排序缺陷：

1. **三单指物代词短语系统性缺失**：词库在许多包含 `"ta"` 的多音节短语里同时收录了「他」和「她」（同一跨度同字长位置仅这一字不同），却漏收对应的「它」短语。由于词网格 `edge_log_prob` 对单字边扣减 `ln(1e6)` 并缺少多字词组加分，「它」所在路径单步落后约 8.3–13.8 nats，既在束搜索阶段被剪枝，也进不到重排模型的比较集合。
2. **长句末列同前缀坍缩**：词网格 `decode_graph` 默认末列束宽与非末列相同（32），且回溯物化时不按多词前驱分散名额。四字及以上输入的前部存在合理并行切分、末段又有大量同音词时，单一前驱前缀会在几 nats 内占满全部候选预算，把其余合理前缀的路径整段挤出。
3. **多拆音节隔音备选切分静态分虚高**：当一段拼音存在比主切分多拆出音节的字面隔音备选时，多拆出的音节若能拼成双字词，会比主切分单字边少受一次 `ln(unigram_z)` 归一惩罚、并按更长词长在 `WordItem::static_evidence()` 中拿到多一份词长加分后再除以更大字数，静态均分反而压过主切分的合法短词或专名，进而在停顿精排和首屏排序中占住前排。
4. **次选读法漏识别、流放单字之后与停顿精排只提第 1 名**：`runner_up_order` 仅把 `source == LATTICE_SOURCE (8)` 且字符数严格等于首条读法字数的行视作全键读法，漏掉 `NeuralKeyboard (11)`、`Collocation (17)` 以及异字长全键整句；因此既会让神经键盘选中句“隐身”多占一格首页预算，又会把第 4 名及以后的合法全键整句一律挪到几十个单字末尾。同时，25 MB 停顿精排模型仅比较前 9 条 `answers_key` 候选且只把第 1 名旋到最前，忽略了第 2、3 名的精排结果与后排高质量次选。

## Decision

从建图、解码、全拼切分合并和呈现/精排四层协同落地以下契约：

1. **三单指物代词「它」按双证据补边（`lattice::decode::append_inanimate_pronoun_edges`）**：
   - 定义 `INANIMATE_PRONOUN_BACKOFF: f64 = 0.35`。
   - `build_graph` 与 `build_graph_borrowed` 在查询尚未缓存的多音节跨度（音节数 `>= 2` 且包含 `"ta"`）时，从该跨度的真实词条中检测同一字节位置是否存在一对同字长、分别含 `'他'` 与 `'她'` 且其余字符完全相同的词条；仅当两者同时存在且尚无对应的 `'它'` 词条时，合成一条 `'它'` 词边（频度取两者较低者乘以 `0.35`），随后连同原始词边写入 `CachedSpan`。仅单一性别的专名或普通词不触发补边，避免误派生。
2. **显式配置宽末列与三遍前缀多样性物化（`lattice::decode`）**：
   - `LatticeOptions` 增加 `terminal_beam: usize`（默认 `0`）、`terminal_max_per_prefix: usize`（默认 `0`）与 `terminal_diversity_margin: f64`（默认 `0.0`），`LatticeOptions::default()` 与纠错解码（`decode_typo_on_graph_with_policy` 显式清零这三项）保持原有契约不变。
   - 在通用整句表选项 `SentenceNgramTables::options` 与 `make_sentence_lattice_options`（全拼/双拼共用，不动九键 48 路并发契约）开启：`terminal_beam = SMALL_SENTENCE_PATHS`（`64`）、`terminal_max_per_prefix = 2`、`terminal_diversity_margin = 8.0`。
   - `decode_graph_with_terminal` 在 `terminal_max_per_prefix > 0 && take > terminal_max_per_prefix && n >= 4` 时按三遍次序从按分降序的末列假设物化：第一遍对多词前驱（`prefix.0 >= 3`）且未超出 `best_score - hyp.score <= terminal_diversity_margin` 窗口的假设每前驱最多取 `2` 条不同整句，超出窗口则记录 `pass1_end` 停止限流；第二遍按原分序排出第一遍暂缓的假设；第三遍用 `&last[pass1_end..]` 回填至 `take`。三音节及以下短句与单前驱起点的路径不受限流。
3. **多拆音节隔音备选静态证据扣分与次选后置（`types` + `quanpin::dictionary`）**：
   - `WordItem` 新增 `pub extra_syllables: u8`（默认 `0`），记录比主切分多拆出的字面隔音音节数（同步由 `page_copy`、`path_page` 和 `session::candidates::clone_candidate_rows` 复制传递，不改变 `WordItem::weight`）。
   - `WordItem::static_evidence()` 在现有 `PHRASE_LENGTH_BONUS * 1000.0 * syllables` 基础上，再按 `PHRASE_LENGTH_BONUS * 1500.0 * f64::from(self.extra_syllables)`（每多一音节额外扣除 30,000 毫 nat = 30 nats）扣减多音节隔音奖励，消除其虚假每字均值优势并保留真实高频语境晋升能力。
   - `QuanpinDictionary::merge_alternatives_with_runner_ups` 对 `path.len() > primary_segments.len()` 的字面切分 `sentence_association` 行写入 `extra_syllables`；当 `!self.rerankers.is_empty()` 且调用方提供了 `extra_syllable_runner_ups` 缓冲时，将其标为 `keyboard_runner_up = true` 并从 `query_exact` 追加在主切分 `runner_ups` 之后，避免占去逐键重排和首屏正统切分的名额。
4. **修复 `runner_up_order` 座位预算并引入停顿精排全序 `settled_rerank_order`（`ordering` + `input-runtime`）**：
   - `SENTENCE_READING_SOURCES` 扩展为 `[LATTICE_SOURCE (8), 11 (NeuralKeyboard), 17 (Collocation)]`。`visit_runner_up_order` / `runner_up_order` 在存在 `answers_key` 为真的读法时，把全部来源属于 `SENTENCE_READING_SOURCES`、字符数 `>= 2` 且 `answers_key` 为真的行统称为全键读法（仅当没有任何读法设置 `answers_key` 时退回旧的同字长判定）。
   - 自 `first_reading` 起，首条之后先保留首页名额内的前 `keep` 条读法（首条读法字数 `>= 3` 时 `keep = 3`，否则 `keep = 1`），再保留最多 `front_limit` 条非读法前置行（字数 `>= 3` 时最多 `3` 条字长 `>= 2` 的前缀词，否则最多 `4` 条任意长度行），紧接着排入**全部剩余全键次选读法**（第二页起即紧接在首页前缀词之后，不再垫到所有单字结尾），最后才排剩余前缀词与单字；保持无变化零分配与幂等不变量。
   - `settled_rerank_order` 使用独立常量 `SETTLED_STATIC_RERANK_LIMIT = 16` 比较停顿阶段前 16 条回答完整按键的候选；在全部比较行都有静态证据时，以 `0.5 * (statik + neural)` 对比较窗口内候选做稳定降序完整重排（静态证据缺失时退回仅将最佳候选旋前）；`Runtime::settled_model_order` 与 `apply_settled_order` 在同步 `rerank_settled` 与异步 `score_settled_rerank` 中复用这一排列，之后仍经过 `demote_runner_up_readings` 执行首页 `3 + 3` 配额。逐键 `rerank_pick` 继续保持 `STATIC_RERANK_LIMIT = 9`，不增加逐键热路径比较开销。

## Evaluation

使用 `cargo build --release -p msime-input-runtime --example convert_eval --example rerank_latency`（开启 `--keyboard-neural`，学习关），分别对照未改 `main` 冻结报告（`target/eval-exp/out/resources-*-impl-*.json`）测量。

### 停顿后精排开（`resources-settled`，`settled.lock.json` 含 25 MB 停顿模型）

| 集合 | 改前 `top1 / top5 / top9 / mrr` | 改后 `top1 / top5 / top9 / mrr` | 变化说明 |
|---|---|---|---|
| `sentences-v1` (60) | `0.817 / 0.850 / 0.850 / 0.833` | `0.833 / 0.867 / 0.867 / 0.850` | 全项 +1.6～+1.7 pp |
| `sentences-v2` (`harvested`, 312) | `0.673 / 0.878 / 0.878 / 0.758` | `0.676 / 0.894 / 0.952 / 0.787` | `top5 +1.6 pp`，`top9 +7.4 pp`，`mrr +2.9 pp` |
| `sentences-particles-v1` (103) | `0.757 / 0.961 / 0.971 / 0.839` | `0.893 / 0.951 / 0.961 / 0.919` | `top1 +13.6 pp`，`mrr +8.0 pp`，`top5` -1 例 |
| `sentences-neutral-v1` (1105) | `0.813 / 0.932 / 0.932 / 0.864` | `0.804 / 0.932 / 0.941 / 0.865` | `top5` 持平，`top9 +0.9 pp`，`mrr +0.1 pp` |
| `quanpin-words-v1` (`--limit 3000`, 2791) | `0.755 / 0.964 / 0.984 / 0.835` | `0.755 / 0.967 / 0.984 / 0.836` | `top5 +0.3 pp`，`mrr +0.1 pp` |
| `sentences-nine-key-v1` (`--nine-key`, 81) | — | `0.679 / 0.704 / 0.704 / 0.688` | `found = 0.741` |

### 停顿后精排关（`resources`，仅 4.5 MB 键盘模型）

| 集合 | 改前 `top1 / top5 / top9 / mrr` | 改后 `top1 / top5 / top9 / mrr` | 变化说明 |
|---|---|---|---|
| `sentences-v1` (60) | `0.817 / 0.850 / 0.850 / 0.833` | `0.833 / 0.867 / 0.867 / 0.850` | 全项 +1.6～+1.7 pp |
| `sentences-v2` (`harvested`, 312) | `0.490 / 0.869 / 0.869 / 0.656` | `0.490 / 0.843 / 0.923 / 0.664` | 见下文预算效应：`top9 +5.4 pp`，`mrr +0.8 pp` |
| `sentences-particles-v1` (103) | `0.738 / 0.961 / 0.971 / 0.831` | `0.874 / 0.942 / 0.951 / 0.905` | `top1 +13.6 pp`，`mrr +7.4 pp` |
| `sentences-neutral-v1` (1105) | `0.710 / 0.927 / 0.927 / 0.806` | `0.709 / 0.907 / 0.937 / 0.803` | `top9 +1.0 pp` |
| `quanpin-words-v1` (`--limit 3000`, 2791) | `0.754 / 0.965 / 0.984 / 0.835` | `0.755 / 0.967 / 0.984 / 0.836` | `top1 +0.1 pp`，`top5 +0.2 pp` |
| `sentences-nine-key-v1` (`--nine-key`, 81) | — | `0.642 / 0.667 / 0.691 / 0.657` | `found = 0.741` |

### 逐键时延（`rerank_latency`，`resources-settled`，`sentences-v1` 60 例 / 1320 键）

- 仅开外层重排（不加 `--keyboard-neural`）：`with model` `mean = 1.17 ms`，`p50 = 0.17 ms`，**`p95 = 4.78 ms`**，`p99 = 7.13 ms`，`max = 11.46 ms`，超 16 ms 预算 `0 / 1320`（`0.0%`）。
- 同时打开 `--keyboard-neural`：`with model` `mean = 2.68 ms`，`p50 = 0.25 ms`，**`p95 = 11.20 ms`**，`p99 = 21.32 ms`，`max = 45.88 ms`，显著低于 `16.00 ms` 门限。

## Historical audit

- [键盘模型开着时保留其余整句路径](../feature/2026-11-10-keyboard-rerank-sentence-tail.md)：保留将主切分次选接在列表末尾并由 `runner_up_order` 收拢、且逐键重排跳过 `keyboard_runner_up` 的设计；本篇修复 `runner_up_order` 对 `NeuralKeyboard` / `Collocation` 来源与异字长全键读法的漏识别及次选落在单字之后的问题，并对多拆音节隔音备选整句沿用相同 `keyboard_runner_up` 降级通道。
- [Shurufa 随包提供停顿后精排模型](../feature/2026-11-08-shurufa-settled-rerank.md)：停顿后 25 MB 模型接入与异步流转保持不变；本篇把停顿阶段从单一第 1 名提升（`rerank_pick`）改为比较前 16 条回答全键候选并做完整重排（`settled_rerank_order`）。
- [独立句子排序保留词网格的静态证据](2026-10-14-rerank-static-evidence.md)：保留 `WordItem::static_evidence()` 对普通句子的口径与 `STATIC_RERANK_LIMIT = 9` 逐键预算；仅叠加 `extra_syllables` 扣除以修正隔音多音节虚高，不改动原始 `weight`。
- [九宫格切分路径按音节频度截断](2026-10-08-nine-key-path-ranking.md)：借鉴跨切分扣除 `PHRASE_LENGTH_BONUS * 1000` 的对齐原则并在隔音多切分场景取 `1.5` 倍惩罚以抵消双字词归一差；通过 `SentenceNgramTables::options` 与 `extra_syllables` 隔离改动范围，不改变九键 `split_paths` 48 路约束与内存预算。
- [词网格给句末语气词加分](2026-10-23-sentence-final-particles.md)：保留句末语气词加分，三遍前缀多样性物化减少末段同音词对前驱的挤压，使 `particles` 集 `top1` 在两条资源线分别从 `0.738 / 0.757` 升至 `0.874 / 0.893`。
- [词网格末列按最终预算流式保留假设](../testing/2026-10-17-lattice-terminal-stream.md) 与 [整句候选恒等排列避免临时分配](../testing/2026-10-08-runner-up-order-identity.md)：`LatticeOptions` 新增字段默认关闭，保留所有既有 `decode_terminal` / `decode_return` / `decode_materialization` 快照与峰值分配契约；`runner_up_order` 继续保留无变化零分配恒等快路径。
- [全拼整句纠错归一门槛、重排席位保护、中段切分多样性与纠错回显省略](2026-11-17-typo-sentence-gating-and-lattice-segmentation-diversity.md)：后续在此基础上为第一遍物化叠加词边界骨架限流，防止中段同音笛卡尔积绕过末词前驱限流，并隔离附带纠错整句的重排与首页席位。

## Alternatives considered

- **全局修改 `WordItem::static_evidence()` 仅对多字词扣减 `20 * syllables`**：
  最强理由是可以从根源统一单字与双字词的静态均分尺度。但如 [九宫格切分路径笔记](2026-10-08-nine-key-path-ranking.md) 实测所证，词库单字权重与多字词权重不在同一量纲，全量改口径会导致未挂重排模型时整句 top-1 大跌（`0.233 -> 0.150`），且容易让无上文时的低频单字拆句压过常见双字词，因此只对 `extra_syllables > 0` 的隔音备选整句施加额外扣除。
- **把隔音备选切分的惩罚直接写回 `WordItem::weight`**：
  最强理由是不用为 `WordItem` 增加字段。但直接改写 `weight` 会连带扰动词网格原本的显示/排序比较链以及缓存页复用时的原始权重语义；记入 `extra_syllables` 并只在 `static_evidence()` 扣减能把影响严格收敛到静态重排尺度上。
- **离线重打 SQLite 词库直接补齐「它」词条**：
  最强理由是运行期零额外查边与比较成本。但这需要修改词库构建产物并要求所有用户升级词库，无法覆盖已发布资源包；运行期仅在命中 `"ta"` 的未缓存多音节跨度里核对「他 / 她」双证据并写回 `CachedSpan`，误触发为零且重复跨度命中缓存。
- **直接无条件更改 `decode_graph` 默认末列束宽与物化限流**：
  最强理由是减少 `LatticeOptions` 配置字段。但这会破坏使用 `LatticeOptions::default()` 的分配与峰值内存快照门禁，也会把九键 48 路并发解码末列扩大一倍；经 `LatticeOptions` 显式启用并在纠错解码中关闭可精确控制影响面。
- **放宽首屏读法配额或把 `NeuralKeyboard` 继续排除在 `SENTENCE_READING_SOURCES` 外以保住未停顿 `top5`**：
  最强理由是可以避免未停顿状态下 `harvested` / `particles` 的 `top5` 数字名义回落约 1.9–2.6 pp。但把 `NeuralKeyboard` 排除在外本质上只是漏洞导致的隐形第 4 格（挤压首页最多 3 条前缀词的座位、把其余全键次选挤去百位单字末尾）；收口为严格的「首屏至多 3 条整句 + 至多 3 条前缀、第 4 条起整句紧跟前缀之后」才是符合直觉与既定排座设计的一致行为，`top9` 也随之上扬 1.0–7.4 pp。

## Consequences

- **召回与排座显著改善**：多音节含 `"ta"` 长句在「他/她」双证据下稳定产出「它」边；四字及以上长句在末段同音词密集时保留跨前缀多样性；隔音误切不再压过主切分正统词汇与专名；第 4 名及以后的全键读法固定排在首页前缀词之后、全部单字之前（第 2 页即可触达）；停顿精排可以把大模型选出的第 2、3 名整句与 `10..16` 位候选一同排入前席。
- **未停顿 `top5` 口径效应**：因首页严格执行 `3` 条整句配额，`NeuralKeyboard` 不再额外占用第 4 格，导致未启用停顿模型时的 `harvested`（`-2.6 pp`）与 `particles`（`-1.9 pp`）以及 `neutral`（`-2.0 pp`）`top5` 名义略降，同时全部四项整句集的 `top9` 均持平或提升（`neutral +1.0 pp`、`harvested +5.4 pp`），开启停顿精排后 `harvested` `top5` 提升 `+1.6 pp`、`top9` 提升 `+7.4 pp`。
- **单测与门禁覆盖**：
  - `msime-engine --lib` `2721 passed; 0 failed; 57 ignored`（含 `inanimate_pronoun_edges_are_synthesized_only_when_both_genders_exist`、`terminal_prefix_diversity_preserves_runner_up_prefix_and_backfills_single_prefix`、`runner_up_order_recognizes_neural_and_collocation_sources_and_is_idempotent`、`runner_up_order_uses_answers_key_for_different_lengths_and_caps_front_non_readings_with_suffix_fallback`、`settled_rerank_order_sorts_up_to_sixteen_static_candidates_and_falls_back_without_evidence`、`extra_syllable_literal_cut_sentences_follow_primary_runner_ups_with_length_deduction` 与同步更新的 `literal_alternative_series_reuses_prefix_rows_across_dense_paths`）。
  - `msime-input-runtime` `406 passed; 0 failed`（含 `only_readings_that_answer_the_whole_key_are_counted`，以及覆盖 `Runtime::settled_model_order` / `apply_settled_order` 流程的停顿精排回归）。
  - `msime-engine-wasm` `72 passed; 0 failed`。
  - `msime-host-api --lib` `1017 passed; 3 failed; 5 ignored`（失败项与未改动 `main` 完全相同）。
  - 说明：`crates/engine/tests/golden/real/*.jsonl` 录制于无 `sentence-model*.safetensors` 的历史 `dict-v2.0.1` 基线；`scripted` golden 仅保留既有 `answers_key` 10 处历史差异（与 `2026-11-10` 笔记记录一致）。
