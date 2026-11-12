# Agent Note: 键盘模型开着时保留其余整句路径

Status: implemented

## Problem

维护者报告：打 `daoshikan` 时「到时看」完全不在候选里，并说经常遇到这种违反直觉的缺席。

本地神经排序（`neural_keyboard`）开着时，词网格先解出 12 条整句（`MAX_RERANK_PATHS`），`lattice/merge.rs` 的 `reranked_block` 只留两条：词网格第一名（`Generated`）和键盘模型从 12 条里挑的那一条（`NeuralKeyboard`），其余 10 条直接丢弃。每个备选切分（`da'o'shi'kan`）各自也出这两条。两条都不对时，正确的整句根本不在列表里，停顿后精排也只能在已有候选之间挪。关掉神经排序时没有这个上限：每个切分出 6 条 `Generated`，由 `ordering.rs` 的 `runner_up_order` 留 3 条在前、其余挪到列表末尾。

在维护者用户目录的副本上复现：把「到时」的权重恢复到截图前，上文 seed 成「不过可以找其他」后，两页候选与截图逐条相同，整张列表没有「到时看」；它在 12 条路径里，但上文是「其他」时键盘模型挑的是「导师看」。

## Decision

- **保留次选整句**：`merge.rs` 的 `keyboard_runner_up_rows` 把整句块两条以外、还没出现在候选里的路径按键盘模型的顺序做成 `Generated` 行，`WordItem::keyboard_runner_up` 为真。只有键盘模型成功评过分时才有这些行；没有它的排列就不知道谁是次选。
- **只给主切分**：全拼 `query_exact` 只在主切分（`resolution.segmentation`）的 `query_primary_series` 里收集次选，备选切分和纠错、更贵的切分照旧只出两条。
- **接在整张列表末尾**：次选在所有切分、补全词、单字都合完之后用 `append_unique_words` 接到最后，哪几条回到第一页由现有的 `runner_up_order` 决定（3 字以上留 3 条读法，2 字留 1 条）。
- **逐键重排不比较它们**：`EngineSnapshot::candidate_keyboard_runner_up` 与候选同步排列（长度不符时整份作废，按全是假处理），runtime 逐键的 `rerank` 用 `keystroke_order_rows` 把它交给 `rerank_pick`，`rerank_pick` 把这些行排除出比较集合；停顿后精排（同步的 `rerank_settled` 和异步的 `score_settled_rerank`）用 `order_rows`，照常比较。首行若是次选，拒绝重排。
- 双拼、九键、网页引擎不收集次选：双拼和九键有各自的整句合并，网页引擎不开引擎内的键盘模型（`neural_keyboard: false`）。

## Evaluation

`convert_eval --keyboard-neural`，全拼，学习关；锁定资源 dict-v2.0.24 加 `sentence-model.safetensors`，「停顿后精排开」再加 `sentence-model-desktop.safetensors`。逐条配对，括号内为 McNemar 精确检验 p 值。产品实现与最后一个实验变体（分支 `exp/keyboard-sentence-tail` 的 `EXP_SENTENCE_TAIL=keyboard-end-primary-global-skipkey`，不合入）的候选列表逐条相同，只有 words 2791 条里 1 条不同，指标一致。

停顿后精排开（维护者的配置）：

| 集合 | 改前 首选 / 前五 / 找到 | 改后 首选 / 前五 / 找到 | 逐条 |
|---|---|---|---|
| neutral 1105 | 0.740 / 0.765 / 0.765 | 0.813 / 0.932 / 0.957 | 首选 +96 −16；前五 +185 −0 |
| harvested 312 | 0.590 / 0.622 / 0.622 | 0.673 / 0.878 / 0.974 | 首选 +31 −5；前五 +80 −0 |
| sentences-v1 60 | 0.817 / 0.817 / 0.817 | 0.817 / 0.850 / 0.850 | 前五 +2 −0 |
| particles 103 | 0.777 / 0.961 / 0.971 | 0.757 / 0.961 / 1.000 | 首选 +0 −2（0.5）；前五 +0 −0 |
| words 3000 取 2791 | 0.755 / 0.966 / 0.988 | 0.755 / 0.964 / 0.989 | 前五 +0 −5（0.06） |

停顿后精排关：neutral 前五 0.765 → 0.927（+179 −0），harvested 0.622 → 0.869（+77 −0），首选不变；particles 前五 +0 −0；words 前五 +0 −6（0.03）。

words 剩下的 5–6 条都是被截断的四字词补全（`fangshujiang` → 防暑降温），主切分多出的读法占了座位，补全词从第 4 位落到第 6 位；这是多给整句座位的直接代价。

延迟：`rerank_latency --keyboard-neural`（两臂都开引擎内的键盘模型，量逐键重排在它之上多花多少）。最初让逐键重排也比较次选时，sentences-v1 每键 p95 多 6.08 ms（11.82 → 17.09），neutral 取 277 条多 7.79 ms（16.17 → 23.32）。拆开后：两臂都不挂逐键重排时这个增量只剩 p95 0.17 / 0.26 ms，引擎一侧几乎不花时间；计数显示逐键重排每次比较的行从 1.6–1.8 条（约 9–11 字）涨到 9 条（约 52–59 字），整串读法都回答当前按键，`chinese-ime-lm` 的比较上限 9（`COMPARED`）被它们占满。排除之后增量是 p95 0.18 / 0.25 ms，评测的候选列表逐条不变：在这个摆放下，逐键重排给次选打的分从没改变过结果。改后 sentences-v1 上逐键重排的 p95 是 11.71 ms，在 16 ms 预算内。`verify-local.sh` 的 `reranker latency` 阶段多跑一遍 `--keyboard-neural`。

## Alternatives considered

- **保留现状的两条**：首选和延迟都不动；但 neutral 上 19% 的用例正确整句根本不在列表里，用户只能分段重打。
- **其余路径直接放进整句块**（`keyboard`）：语气词短句首选 +14 −3，因为逐键重排能从更多行里挑；但词级前五 −25 到 −44，两字键会在第二位冒出生造的读法。
- **接在各自切分的末尾**（`keyboard-end`）：整句收益相同；但停顿后精排开时词级前五 −20，语气词前五 −4：另一切分的正确读法（`xieshanga` 的「协商啊」）在列表里排到主切分多出的读法后面，被 `runner_up_order` 挪去末尾。
- **只给主切分、仍接在切分末尾**（`keyboard-end-primary`）：词级 −14，语气词 −4，第二种代价没有消失，说明问题在放的位置而不在离谱切分。
- **按词网格顺序而不是键盘模型顺序**（`lattice`）：neutral 前五少 40 条（0.890 对 0.927）。
- **让逐键重排也比较次选**：理由是逐键重排能在词典行和整句之间重新挑首选；但在全局末尾的摆放下它从没改变过结果，每键 p95 却多 6–8 ms。
- **把次选标成 `answers_key` 为假**：不用新增字段；但停顿后精排也读 `answers_key`，改后精排开时的首选收益（neutral +96 −16）来自它比较这些行，而且它们确实回答整个按键串，`answers_key` 还被补全词、前缀行的判断共用。
- **新增候选来源值**：来源是冻结的线上值，宿主按它分支，次选在显示和上屏上与 `Generated` 完全相同，没有理由让宿主看到区别。

## Consequences

- 开着本地神经排序时，长句的正确读法大多能在候选里找到；停顿后精排开时首选也明显变好。
- 截断输入的四字词补全偶尔从第 4 位落到第 6 位。
- 测试：`lattice::merge::tests::keyboard_runner_ups_are_the_other_paths_in_keyboard_order`（次选的集合与顺序），`tests::static_evidence::keyboard_runner_ups_leave_the_comparison_only_when_the_flags_line_up`（排除、首行是次选、长度不符），`tests::candidate_transform::keystroke_rerank_skips_keyboard_runner_ups_and_the_settled_pass_compares_them`（逐键不提、同步与异步的停顿精排照常提、标记随行旋转）。`host::tests::hidden_helpcode_snapshot_does_not_recompute_annotations` 的分配上限加一，是新数组本身。
- `msime-engine` 的 golden 回放有 10 个 `answers_key` 差异、`msime-host-api` 有 3 项失败，在未改动的 `main` 上同样存在，与本改动无关。九键、14 键、双拼不受影响，没有重跑它们的评测。

## Related

- [手动置顶不被自动调频挤掉](../bug-fix/2026-11-06-manual-pin-survives-frequency-learning.md)：同一时期记录过 golden 回放的那 10 个差异。
- [Shurufa 随包提供停顿后精排模型](2026-11-08-shurufa-settled-rerank.md)：停顿后精排的来历。
