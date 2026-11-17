# Agent Note: 全拼整句纠错归一门槛、重排席位保护、中段切分多样性与纠错回显省略

Status: implemented

## Problem

在默认开启全拼邻键与乱序纠错（`autocorrect_neighbor`、`autocorrect_transposition`）的真实会话中，100% 合法、无拼写错误的完整音节长句仍会出现荒诞的错词整句抢占首选、正确切分从候选消失以及候选窗被整串原始字母撑宽四处耦合缺陷：

1. **多音节纠错边规避单字归一惩罚导致误纠错虚高**：`edge_log_prob` 对单字边扣减 `ln(unigram_z) = ln(1e6) ≈ 13.82` nats，对多音节词边则加 `(k - 1) * PHRASE_LENGTH_BONUS`（双字词 `+20.0`）且不扣 `ln(1e6)`。当字面最优路径包含合法常用单字（如 `xin'ji'qiu'kuai` 的「求 + 快」、`qiu'kan...` 的「求 + 看」），而邻键或漏键变体恰好拼成词库里的双字词（如 `qu'kuai`「区块」、`qi'kan`「期刊」、`lian'gang`「炼钢」、`yang'ren`「洋人」、`han'peng`「含硼」）时，`NEIGHBOR_COST = 20.0` 仅抵消了双字词的 `+20.0` 词长奖励，却净赚一份未做 `-ln(1e6)` 归一的 `+13.82` nats 尺度红利，使低频荒诞纠错句在 `decode_typo_on_graph` 中以 `+5..+10` nats 反超合法字面整句。
2. **外层重排绕过 `typo_lead_margin = INFINITY` 将附带纠错句提至首位**：引擎故意将 `LatticeOptions::typo_lead_margin` 设为无穷大，把 `TypoSentence` 置于字面整句块之后，避免音节替换纠错破坏干净句子的首选率；但 `ordering::rerank_pick` 与 `settled_rerank_order` 未区分切分层整体纠错与字面整句后的附带纠错句，把携带虚高静态证据的 `TypoSentence` 与字面整句等权混合（长句字均分进一步稀释局部语义差），反手将其旋至第 1 名；同时 `runner_up_order` 让 `TypoSentence` 挤占首页仅有的 3 个整句席位。
3. **长句中段多词同音笛卡尔积挤出并行切分**：`decode_graph_with_terminal` 的 `terminal_max_per_prefix` 仅按末词直接前驱 `hyp.prev` 限流。当长句中段某个错误分词骨架存在多组双字同音词笛卡尔积（如 4 × 2 组双字同音词汇入同一四字后缀），到达倒数第二列时已成为 8 个不同前驱假设，一口气占满 `nbest = 12` 的 11～12 个名额，将分差仅约 1.8 nats 的正统分词骨架整条挤出，使后续神经模型无从看见正确句子。
4. **长句纠错回显与隔音落选句污染版面**：未开启辅助码展示时，引擎把整串折叠按键 `corrected_from` 填入 `candidate_annotations`，macOS 宿主在已画 `*` 标记后又拼上 `(整串原始按键)`，与预编辑栏完全重复并撑爆候选窗；同时纠错模式下 `CutMode::Correction` 产生的五音节以上同音节数隔音备选（如 `sang'e`「丧饿」、`ren'ao`「人傲」）在落选后仍排在主切分次选之前占住第 2、3 名。

## Decision

在不改变词库格式、C ABI 与黄金基准权重的前提下，从词网格解码、全拼备选排座、独立重排器、评测工具与 macOS 候选显示五处收口契约：

1. **纠错整句合并单字时补齐 `ln(unigram_z)` 归一门槛（`lattice::decode::decode_typo_on_graph_with_policy`）**：
   - 在 `!allow_nonleading`（全拼策略）下统计 `literal_best` 与纠错最优路径 `best` 中单字词（`word.chars().count() == 1`）的个数；当 `typo_single_chars < literal_single_chars`（纠错路径通过合并单字规避了单字归一扣分）时，要求每条纠错边额外跨过 `ln(unigram_z)`（默认 `ln(1e6) ≈ 13.82` nats），即 `best.log_prob > literal_best.log_prob + z.ln() * (best.typo_edges as f64)`。
   - 同单字数的词组对词组替换（`typo_single_chars >= literal_single_chars`）与双拼非领先纠错（`allow_nonleading == true`）保持原有门槛不变；真实高频错拼（如 `meiganxi -> 没关系`，超出 `+21.33` nats）照常产出且 `weight` 不变。
2. **重排与首页排座遵守未纠错整句优先（`ordering`）**：
   - 定义 `speculative_typo_sentence(leader, row)`：当首行已是未纠错的整句读法（`!leader.corrected && SENTENCE_READING_SOURCES.contains(&leader.source)`）时，后排带 `corrected` 的整句读法不进入 `rerank_pick` 与 `settled_rerank_order` 的比较集合，严守 `typo_lead_margin = INFINITY` 不抢字面整句席位；首行本身已带 `corrected`（如 `sahnghao` 切分纠错）或词典行居首的场景保持原重排能力。
   - `visit_runner_up_order` / `runner_up_order` 在首条整句未纠错时，首页 `keep` 个读法名额与次选区均先排未纠错字面读法、名额未满再补纠错整句，保持无变化零分配与幂等不变量。
3. **末列第一遍叠加词边界骨架限流与长句隔音后置（`lattice::decode` + `quanpin::dictionary`）**：
   - `decode_graph_with_terminal` 在启用 `terminal_max_per_prefix` 且 `n >= 4` 时，零堆分配回溯计算假设的词起始列位掩码 `segmentation_signature`，在 `terminal_diversity_margin`（`8.0` nats）窗口内限制同一分词骨架第一遍最多物化 `max_per_segmentation = (take / 3).max(max_per_prefix * 2)` 条（`nbest = 12` 时为 `4` 条），超额假设按原分序暂缓至第二遍回填，防止单一错误切分的同音笛卡尔积挤出并行正统切分。
   - `QuanpinDictionary::merge_alternatives_with_runner_ups` 在挂载重排器且主切分音节数 `> MAX_SYLLABLES_FOR_MULTIPLE_SEGMENTATIONS (4)` 时，将 `CutMode::Correction` 产生的同音节数隔音整句放入 `extra_syllable_runner_ups`（保持 `keyboard_runner_up = false`），使模型选中的真隔音句（如 `fang'an -> 方案`）仍能提至首位，而落选的误隔音句退居主切分次选之后。
   - `convert_eval` 新增可选 `--autocorrect` 开关，支持在评测集上直接度量开启全拼邻键与乱序纠错时的真实端到端表现。
4. **macOS 候选显示层省略重复的裸按键回显（`platforms/macos/src/input/InputController.mm`）**：
   - 新增 `MSIMEVisibleCandidateAnnotation`：当 `candidate[@"corrected"]` 为真（字面已追加 `*`）且 `annotation` 仅为纯小写 ASCII 裸串（`corrected_from`）时省略该重复回显；五笔混输反查形如 `"sahnghao wqvb"` 时剥除前段 `corrected_from` 仅保留 `"wqvb"`；带括号的辅助码（如 `"(aB)"`）及非 ASCII 注释原样保留。

## Evaluation

使用 `convert_eval`（`--keyboard-neural`，学习关）测量开启 `--autocorrect` 前后的端到端表现，并核对关闭 `--autocorrect` 的既有基线与逐键时延。

### 开启全拼纠错（`--keyboard-neural --autocorrect`）

| 集合 | 改前 `top1 / top5 / top9 / mrr` | 改后 `top1 / top5 / top9 / mrr` | 变化说明 |
|---|---|---|---|
| `sentences-v1` (60，停顿精排开) | `0.850 / 0.883 / 0.883 / 0.867` | `0.850 / 0.883 / 0.883 / 0.867` | 真隔音 `我们要研究这个方案` 继续保持第 1 |
| `sentences-v2` (`harvested`, 312，停顿精排开) | `0.679 / 0.894 / 0.952 / 0.789` | `0.692 / 0.910 / 0.968 / 0.803` | `top1 +1.3 pp`，`top5 +1.6 pp`，`top9 +1.6 pp`，`mrr +1.4 pp`，并消除误纠错污染 |
| `sentences-particles-v1` (103，停顿精排开) | `0.893 / 0.951 / 0.961 / 0.919` | `0.893 / 0.951 / 0.961 / 0.919` | 零退化 |
| `sentences-neutral-v1` (`--limit 250`, 221，4.5 MB 模型) | `0.710 / 0.937 / 0.955 / 0.814` | `0.729 / 0.950 / 0.968 / 0.832` | `top1 +1.9 pp`，`top5 +1.3 pp`，`mrr +1.8 pp`，8 处误纠错/误隔音全部清零 |
| `quanpin-words-v1` (`--limit 3000`, 2791，停顿精排开) | `0.755 / 0.967 / 0.984 / 0.836` | `0.755 / 0.967 / 0.984 / 0.836` | 词级指标完全持平 |

### 关闭全拼纠错基线（`--keyboard-neural`）与逐键时延

- 未开纠错下，因中段切分多样性修复，`sentences-v2` 从 `0.679 / 0.894 / 0.952 / 0.789` 升至 `0.692 / 0.910 / 0.968 / 0.803`，`sentences-neutral-v1`（`--limit 250`）从 `0.719 / 0.941 / 0.955 / 0.821` 升至 `0.729 / 0.950 / 0.968 / 0.832`；`sentences-v1` 与 `sentences-particles-v1` 持平。
- `rerank_latency --keyboard-neural`（`sentences-v1`，60 例 / 1320 键）：`with model` `mean = 2.60 ms`，`p50 = 0.22 ms`，**`p95 = 11.34 ms`**（低于 `16.00 ms` 预算门限），`reranker adds` `mean = 0.12 ms`、`p95 = 1.07 ms`。

## Historical audit

- [词网格指物代词对等、末列前缀多样性、隔音切分降级与次选排座精排](2026-11-14-lattice-pronoun-diversity-and-seating.md)：直接扩展。该篇引入的 `terminal_max_per_prefix` 只覆盖末词前驱，本篇在此基础上补齐中段词边界骨架限流、五音节以上隔音备选后置以及附带纠错整句在 `rerank_pick` / `settled_rerank_order` / `runner_up_order` 中的席位隔离。
- [双拼非首选整句纠错候选保留](2026-10-24-shuangpin-nonleading-typo-candidate.md)：职责正交。双拼调用 `allow_nonleading = true` 并在引擎与 `ordering.rs` 两侧原本就禁止纠错领先，本篇的单字归一差门槛仅作用于 `!allow_nonleading` 的全拼路径，双拼行为完全不变。
- [全拼纠错候选按原始按键推进](2026-10-12-quanpin-correction-consumption.md)：完全保留。引擎内部 `WordItem::corrected_from` 与 `SessionSnapshot::candidate_annotations` 不变，保证选中纠错候选时的原始按键消费长度与黄金基准完全一致；仅在 macOS 显示层省略已由 `*` 表达的重复裸串回显。
- [独立句子排序保留词网格的静态证据](2026-10-14-rerank-static-evidence.md)：补充边界。保持字面整句的 `static_evidence` 计算公式不变，将首行未纠错时的附带 `TypoSentence` 排除出模型比较集合，防止带不同惩罚量纲的纠错分污染等权混合。

## Alternatives considered

- **直接把全局 `NEIGHBOR_COST` / `TRANSPOSITION_COST` 从 `20.0` 调高到 `34.0`**：
  最强理由是无需比较字面路径与纠错路径的单字个数。但全局抬高基础罚分会同时扼杀不涉及单字合并的真实双字词对双字词纠错，并改写全部 `typo_gating_*`、`typo_learning*` 与 `typo_upgrade_survival` 黄金基准中的候选分数；按 `typo_single_chars < literal_single_chars` 精确补齐缺失的 `ln(unigram_z)` 归一项即可从量纲根源消除单字合并红利，且不影响真实纠错基准。
- **在引擎 `Session::candidate_annotations` 中直接清空 `corrected_from` 回填**：
  最强理由是一次改动同时覆盖所有平台宿主。但 `host::tests` 与数十份脚本化 golden 基准明确断言未开辅助码时 `SessionSnapshot::candidate_annotations` 携带 `corrected_from` 字符串，且部分测试依赖该快照契约核对纠错状态；在 macOS 宿主呈现层过滤纯小写 ASCII 裸串回显既能根治长句撑宽候选窗，又不破坏跨层快照契约。
- **将五音节以上同音节数隔音备选整句也标为 `keyboard_runner_up = true`**：
  最强理由是与多拆音节隔音备选完全共用同一标记。但标为 `keyboard_runner_up = true` 会使其被逐键 `rerank_pick` 跳过，在未开启停顿精排时无法将 `womenyaoyanjiuzhegefangan -> 我们要研究这个方案` 提至第 1 名；保持 `keyboard_runner_up = false` 仅将其排在主切分次选之后，可兼顾真隔音提首与误隔音降级。

## Consequences

- **收益**：完整合法的全拼长句不再因末端或中段单字被误判为邻键/漏键错词而冒出「区块」「期刊」「炼钢」「含硼」等荒诞首选；中段多词同音笛卡尔积不再把正统分词挤出 `nbest`；真实产生纠错候选时仅以 `*` 简洁提示，不再拖带数十字符的整句拼音尾巴。
- **代价与边界**：`decode_graph_with_terminal` 在 `n >= 4` 且开启前缀多样性时每条扫描到的末列窗口内假设多一次栈指针回溯算 `u64` 位掩码（最多 `n` 步，纳秒级，零堆分配）；对于单字词确系用户漏打声母韵母、但目标双字词在词库中词频极低（未能跨过 `ln(unigram_z)` 归一差）的罕见生僻词错拼，全拼不再强行猜测纠错整句，避免在日常正常输入中制造高比例误报。
