# Agent Note: 短串神经重排引入长度感知静态先验护栏与首选防抖阈值

Status: implemented

## Problem

在跨边界上下文打通（[清洗非汉字前文消除神经模型 &lt;unk&gt; 污染并打通词格跨边界上下文](2026-11-18-han-clause-context-sanitization.md)）后，`ordering::rerank_pick`、`ordering::settled_rerank_order` 与 `NeuralReranker::score_order_into` 仍按 `0.5 * (statik + neural)` 无护栏等权线性混合且以 `0.0` 为晋升门槛：

1. 对 2–5 字词组或双词短语（例如 `zhongshiguke` 的「重视顾客 / 中式顾客 / 忠实顾客 / 中式骨科」），候选本身上下文窗口极短，小参数字级神经模型的局部概率波动方差大，静态权重低出 `> 0.50 nats/字`（约 `7.4x` 以上词频断崖）的生僻拼凑幻觉仍容易被抬进前排甚至反超首位；
2. 当挑战者的静态先验低于当前首行时，`0.0` 翻盘门槛会导致极微弱的模型波动就改写第 1 名，引发首选抖动；
3. 若对所有句长一刀切使用 `0.50 nats/字` 赤字上限与 `0.06 nats/字` 首选门槛，实测会在 `sentences-neutral-v1`（全量 1,105 条均在 6–16 字的完整子句）上把 `top1` 从 `0.711 / 0.712` 拉低至 `0.697 / 0.700`：因为完整子句内部已具远距语义约束，且单点同音纠偏产生的总分优势会被 6–16 的句长分母稀释到 `< 0.06 nats/字`，专名等合法词频差也会被短串阈值误拦。

## Decision

在 `crates/engine/src/ordering.rs` 定义按字符长度区分短词组与完整子句（分界 `FULL_CLAUSE_MIN_CHARACTERS = 6`）的共享静态先验赤字护栏与首选防抖阈值，并在 `crates/engine/src/lattice/neural.rs` 的 `NeuralReranker::score_order_into` 中以同一组常量和函数复用：

- **长度感知免费赤字上限（`static_prior_free_deficit_mean_nats`）**：
  - `< 6` 字短词组：`0.50 nats/字`（常见合理同音词如 `忠实` 与 `重视` 相差约 `0.37–0.39 nats/字`，完全落在免罚窗口内）；
  - `>= 6` 字完整子句：`2.00 nats/字`（仅在极端低频崩塌时干预，放行正常专名与跨词组消歧）。
- **超额赤字惩罚（`STATIC_PRIOR_EXCESS_PENALTY = 0.50`）**：对相对同组最大静态均值分 `max_statik` 超出免费窗口的赤字 `excess`，在等权均值 `(statik + neural) * 0.5` 后再扣 `0.50 * excess`，要求深静态赤字幻觉必须具备双倍神经对数似然优势才可能竞争。`NeuralReranker::score_order_into` 在送入 `rerank_order_into` 前按 `(STATIC_PRIOR_EXCESS_PENALTY / RERANK_LAMBDA) * excess * chars` 提前折算，保持无静态重排 `rerank_order` 裸签名与零堆分配不变。
- **首选防抖晋升阈值（`static_leader_promote_margin_mean_nats`）**：
  - 仅当挑战者的静态分严格低于首行（`statik[challenger] < statik[0]`）时检查最小净优势；若挑战者静态分高于或等于首行则门槛恒为 `0.0`（不阻碍静态更优候选翻盘）；
  - `< 6` 字短词组首行要求挑战者综合均值分领先 `>= 0.06 nats/字`（`settled_rerank_order` 与 `score_order_into` 未达门槛时只把原首行旋回第 0 席，保留后续候选题序）；`>= 6` 字完整子句门槛为 `0.0`，避免按字倍增伤及长句单点同音纠偏。

## Alternatives considered

- **全长度统一 `0.50 / 0.06`（或退一步的 `0.75 / 0.03`）**：实现最简且无需区分句长；但 `convert_eval` 实测 `0.50 / 0.06` 使 `sentences-neutral-v1` `top1` 跌 `0.711/0.712 -> 0.697/0.700`，调宽至 `0.75 / 0.03` 后 `neutral-v1` 仍掉 `-0.006/-0.005`（`0.705/0.707`）且同步削弱 `particles-v1` 的正收益，故不采用。
- **直接调小混合系数 `RERANK_LAMBDA`（如从 `0.5` 降到 `0.3`）**：会让所有正常近频语境消歧整体变钝，违反既有混合系数契约，放弃。

## Consequences

- 短串场景（双字–五字词组与语气短句）的低频拼凑幻觉与微弱抖动显著减少，`sentences-v2` 与 `sentences-particles-v1` 的 `top1` 与 `MRR` 明显上升；
- 6 字及以上完整子句保留原有神经长距消歧敏锐度，在 `sentences-neutral-v1` 实现零退步；代价是若出现 6 字以上的超罕见幻觉且静态赤字不足 `2.00 nats/字`，该护栏不会介入。

## Verification

- 单元测试通过：`ordering::tests` 26 passed（1 ignored，含 `static_prior_guardrail_blocks_low_frequency_hallucination_and_marginal_jitter`）、`lattice::neural::tests` 14 passed（含 `score_order_into_applies_static_prior_guardrail_and_leader_margin`）、`msime-input-runtime` `static_evidence` 与 `settled_async` 共 10 passed。
- `cargo clippy -p msime-engine -p msime-input-runtime -p msime-engine-wasm --all-targets -- -D warnings` 0 警告；全量 `msime-engine`、`msime-input-runtime` 与 `msime-engine-wasm` 单元测试通过。
- 在 `target/resources/7356c4a452d8ba15475c0e57f134fccf7b7cbe267440461541c74d09b11b6ffe` 跑完 5 套 `convert_eval`（格式为 `默认 / --keyboard-neural`）：
  - `sentences-v1`：`found=0.867/0.867`、`mrr=0.844/0.85`、`top1=0.833/0.833`、`top5=0.85/0.867`（与 Slice 2 持平）。
  - `sentences-v2`：`found=0.933/0.981`、`mrr=0.667/0.672`、`top1=0.51/0.503`（相对 Slice 2 `0.506/0.497` 提升 `+0.004/+0.006`）、`top5=0.821/0.843`。
  - `sentences-particles-v1`：`found=1.0/1.0`、`mrr=0.944/0.923`、`top1=0.913/0.903`（相对 Slice 2 `0.883/0.874` 提升 `+0.030/+0.029`）、`top5=0.981/0.951`。
  - `sentences-neutral-v1`：`found=0.933/0.966`、`mrr=0.795/0.812`、`top1=0.711/0.712`、`top5=0.876/0.923`（相对 Slice 2 完整零退步）。
  - `quanpin-words-v1 --limit 3000`：`found=0.989/0.989`、`mrr=0.833/0.835`、`top1=0.755/0.755`、`top5=0.96/0.967`。

## Related

- [核心高频词调频建立渐进护城河并放开语境同音纠偏](2026-11-18-core-word-frequency-moat.md)：个人调频侧对超高频核心词的护城河。
- [清洗非汉字前文消除神经模型 &lt;unk&gt; 污染并打通词格跨边界上下文](2026-11-18-han-clause-context-sanitization.md)：清理输入上下文噪声并打通跨句边界。
- [离合词与三字整句次选后移阈值](2026-11-18-separable-vo-compounds-and-runner-up-gating.md)：同属 `ordering.rs` 对短串次选席位的静态先验约束。
- [Shurufa 原生接入万象 LTS 语法搭配模型](../feature/2026-11-18-shurufa-wanxiang-grammar-model.md)：语法搭配信号与神经重排协同。
