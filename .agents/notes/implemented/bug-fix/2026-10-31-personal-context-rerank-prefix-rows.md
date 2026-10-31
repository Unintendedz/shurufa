# Agent Note: 个人上下文重排首组只收用完全部按键的字典候选

Status: implemented

## Problem

全拼下输入 `gei` 后按空格，在刚提交过上一个词（8 秒提交链内，例如我、以、给、给我之后）时上屏 `个` 并留下标记文本 `i`，而不是 `给`。没有上文、提交你好之后或停顿超过 8 秒后都正常。

机制：提交链有上一个词时，`update_mixed_candidates` 调 `personal_context_rerank_order`。首组 `leading_group_len` 取「与首行字数相同的连续字典候选」。`gei` 的候选依次是 给（`gei`，2939384）、給（`gei`，0）、个（`ge`，14790406）、哥（`ge`，14789906）……全是单字字典行，只回答 `ge` 的前缀行因此进入首组。`blended_order_indices` 按 `weight / weight_sum` 混合各行份额，不同读音的权重混在同一个分母里，`ge` 的高权重让 个 的份额约 0.7、给 约 0.14；个人模型只要对首组有任何记忆（`mu > 0` 且 `personal_sum > 0`），个 就压过 给，哪怕个人模型本身偏向 给。选中 个 只消耗 `ge`，剩下的 `i` 留在输入里。

## Decision

首组只收同字数、且选中即用完全部按键的字典候选。「选中即用完全部按键」直接取引擎已有的结构化判定 `InputSession::selection_completes_composition`，与 `candidate_answers_key`（神经重排用它排除前缀行）同源。`personal_context_rerank_order` 新增参数 `completes`，`leading_group_len` 的 `take_while` 多一条 `completes(item)`。首选本身不满足该条件时首组长度为 0，不做个人重排，与神经重排拒绝不回答整段输入的首选一致。

光标前缀解码（`prefix_active`）时候选来自光标前那段的独立查询，整段输入的判定对它们没有意义，`completes` 恒真，行为与此前一致；这条路径上同样的前缀行问题没有处理，因为判定需要前缀查询的请求，而前缀候选缓存目前不保存它。

这与 C++ 参考实现（`R/core/personal_context_rerank.cpp` 的「首组同长度字典候选」）不同，黄金场景 `crates/engine/tests/golden/scenarios/personal_context_rerank.json` 即从它移植；该场景的候选都回答整段输入，仍然通过。

## Alternatives considered

- 首组限定为 `canonical_pinyin` 相同的行：最直接，读音相同的权重才可比。但模糊拼音下读音不同、却同样回答整段输入的行会被挤出个人重排，因此否决。
- 为每行计算精确消耗长度并按长度分组：语义最准。但引擎没有一个函数能在全拼、双拼、辅助码和纠错之间给出统一的消耗长度，而「是否用完」已是神经重排使用的现成信号，因此否决。

## Consequences

收益：前缀行不再进入首组，`gei` 在提交链内稳定是 给；其余个人上下文重排行为不变。代价：首选本身只回答前缀时不再有个人重排；光标前缀路径沿用旧分组，仍可能被前缀行影响。

测试：`session::tests::prefix_rows_do_not_displace_the_whole_key_leader_in_the_personal_context` 在修复前失败（首位为 个），修复后通过；`personal_rerank` 单测覆盖前缀行不入组和不完整行终止首组；golden 场景与引擎全套通过。关联：[全局调频首选优先于个人上下文重排](2026-10-24-global-learning-before-personal-context.md)部分重叠，它持有调频首选与重排的优先级，本篇持有首组成员资格。
