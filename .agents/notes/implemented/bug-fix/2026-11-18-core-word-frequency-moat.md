# Agent Note: 超高频核心词渐进调频护城河与语境同音竞争解耦

Status: implemented

## Problem

在默认「促进词频」（`Promote`、`trigger_count = 1`）下，两道规则叠加会让核心高频词因一次误选而在无意间长期被生僻同音词压在下面：

1. **自动调频无核心词护城河**：基准词库里超级核心词与低频同音词往往相差数十倍（如 `508,964` 与 `20,030`）。`plan_weights` 的 `target == 0` 直接把选中词写到 `leader + 1000`，`target == 1` 把第三名写到榜首与次席的中点；一次手误点中第 2 候选，它就立刻超过 50 万权重的常用词并成为全局榜首。
2. **自动调频同时关掉了前文同音纠偏**：`InputSession::contextual_dictionary_promote` 原先调用与个人词序链共用的 `leader_is_pinned_or_user_weighted`，一旦榜首在 `user_dictionary_operations` 里存在当前权重的 `upsert`（包括那一次自动调频），整组同音词就不再生效逐键神经模型的前文重排，即使紧接极强匹配上文、神经模型给出数千倍概率优势也无法把核心词换回第 1 位。

## Decision

把「全局调频防误击穿」与「前文同音纠偏准入」拆开处理，同时保留既有个人词序链与手动置顶契约：

1. **核心高频词渐进调频护城河（`crates/engine/src/user_dictionary/ranking.rs`）**：仅在拼音非强制置顶且非 `Pin` 模式的自动调频下生效。当榜首权重不低于 `CORE_LEADER_MOAT_WEIGHT`（`200_000`）且 `leader_weight > selected_weight * 8` 时：`target == 0` 不一步跃过榜首，而是按 `max(selected * 3, selected + 15_000)` 并封顶在 `leader_weight - 1` 渐进抬升（约 3 次明确选择才跨过护城河）；`target == 1` 且次席仍在 `leader_weight / 8` 下方、未触发重平衡阶梯时，把权重限制在 `max(weights[1] * 2, weights[1] + REBALANCE_GAP)` 与 `(leader_weight / 8) - 1` 之间，避免选第 3 名时中点公式直接越过护城河。`Pin`、`force_top` 与低于 8 倍悬殊差的常规高频词调频保持原步长。
2. **语境同音提首仅让位于手动置顶（`crates/engine/src/session/candidates.rs`）**：`contextual_dictionary_promote` 改用 `is_pinned_candidate` 检查榜首。手动置顶（`pinned_candidates`）依然锁定第 0 位，不受前文覆盖；普通自动调频只通过词条本身的新 `weight` 参与 `ln(weight) + neural_nats` 混合打分，弱上文与无上文下保持调频结果，强匹配汉字前文时可纠偏回贴合语境的同音词。
3. **个人词序链优先级维持不变**：`personal_context_rerank_order` 仍保留 `Self::leader_is_pinned_or_user_weighted`（见[全局调频首选优先于个人上下文重排](2026-10-24-global-learning-before-personal-context.md)），确保无额外前文上下文约束的单码个人词序链仍优先尊重用户全局调频。

## Historical audit

- [全局调频首选优先于个人上下文重排](2026-10-24-global-learning-before-personal-context.md)：部分重叠并互链。该篇守卫 `personal_context_rerank_order` 对单码全局调频的保护，本篇将其与 `contextual_dictionary_promote` 解耦。
- [手动置顶不被自动调频挤掉](2026-11-06-manual-pin-survives-frequency-learning.md)：互补并互链。该篇保护手动 `pinned_candidates` 不被 `adjust_candidate_ranking` 挤落，本篇保护非置顶高频词不被单次误选击穿。
- [组字开始前同步光标前现成文本并按前文重排首组字典候选](../feature/2026-11-17-surrounding-context-association.md)：部分事实更新并互链，将同音提首跳过条件由「未经手动置顶与个人调频覆盖」收敛为「未经手动置顶覆盖」。

## Alternatives considered

- **全部禁用 `Promote` 默认单次调频或把默认 `trigger_count` 调高**：能减少误选伤害，但会把所有普通近频词（如 40 万 vs 50 万的常规人名、专名、双音词）一起变钝，违背现有「选一次常见相当词立刻记住」的默认手感，且也会影响 `initial_learning` 系列契约。护城河只约束 `>= 200,000` 且倍率超过 8 倍的悬殊词对，近频词仍一次生效。
- **直接删掉 `leader_is_pinned_or_user_weighted` 中的 `has_matching_user_weight` 检查**：会让两处逻辑更一致，但会直接破坏「单码全局调频胜过个人二元旧历史」的明确契约并导致 `initial_learning` 回归；按两处调用点职责拆分代价最低且边界清晰。
- **把护城河阈值写进 `plan_weights` 纯算术函数**：`plan_weights` 的既有单测依赖无门槛中点公式；放在 `adjust_candidate_ranking` 并仅作用于 `!force_top && mode != Pin && Pinyin` 可以完整保留既有重平衡单测与 `force_top` 语义。

## Consequences

- **收益**：超高频核心词不再因单次手误被低频词击穿；即便此前已因自动调频落到第 2 位，只要未手动置顶，在强匹配汉字上文下也能由神经同音竞争恢复第 1 位；确实反复选用专名（连续 3 次及以上）依然可以正常学会。
- **代价与边界**：在无前文或普通弱前文输入场景中，用户若希望冷僻字仅凭选 1 次就永久置顶在超高频词前面，需使用手动置顶（快捷键/候选菜单置顶）；护城河仅保护拼音词库且仅对增量调频生效，已有历史用户库中遗留的越界记录需经单独的一次性回退修复处理。
