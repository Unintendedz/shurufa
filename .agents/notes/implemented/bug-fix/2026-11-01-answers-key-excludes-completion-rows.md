# Agent Note: 补全词和前缀区间行不算回答键入的读音

Status: implemented

## Problem

快照里的 `candidate_answers_key` 决定哪些候选当作「回答了当前键入」交给神经重排模型。它原来只看 `selection_completes_composition`：选中这一行能用完全部按键就算回答。补全词和前缀区间行都满足这一条，回答的却是别的键。全拼键入 `zhemefa` 时，`这么发展下去`、`这么发展`（读音比键入的长）和 `这么烦`（末音节 `fa` 被扩成完整音节 `fan`）都被标成回答，于是和 `这么发` 一起交给重排器比较，重排器可能把它们排到真正回答 `zhemefa` 的 `这么发` 前面。

## Decision

`candidate_answers_key` 在原有条件之外再要求 `InputSession::reading_answers_key(item)` 为真。这个判定逐音节比较键入的拼写和这一行词库读音 `canonical_pinyin`，两边音节数必须相等：

- 键入的音节是完整音节（`is_intact`）时，读音的对应音节必须逐字相等（不区分大小写）。
- 键入的音节不完整（如末尾的 `f`）或是简拼首字母时，读音的对应音节以它为前缀即可。
- 两边都按 `normalized_syllable` 归一化后再比一次，覆盖同一音节的不同写法。

全拼用这一行的 `pinyin`（键入的分段）作为键入拼写；双拼用请求的 `normalized_segmentation`。以下行不做判断、一律视为回答：非词库行、`canonical_pinyin` 为空的行、模糊音行、纠错行、带辅码的双拼请求，以及全拼和双拼之外的方案。这些行的键入读音与词库读音本来就不逐音节对应，判定没有依据。

## Alternatives considered

- 只比较音节数（读音音节数等于键入音节数就算回答）：能排除 `这么发展下去` 这类长补全，但排除不了 `这么烦`，它和 `这么发` 一样是三个音节。因此否决。
- 在重排器一侧按字数或权重过滤：重排器拿不到读音和键入的分段，只能猜，而引擎侧已有结构化的读音数据。因此否决。

## Consequences

收益：补全词和前缀区间行不再和真正回答键入的候选一起进神经重排，`zhemefa` 的 `这么发` 不会被 `这么烦` 或长补全压过。键入不完整时（`zhemef`）`这么烦` 仍算回答；简拼（`zmf`）和歧义切分（`xian` 的 `先` 与 `西安`）不受影响。

代价：模糊音、纠错和带辅码的双拼行仍按旧规则一律视为回答，同样的问题在这些行上没有处理。

测试：`session::tests` 新增 `completion_and_prefix_range_rows_do_not_answer_the_typed_key`、`an_incomplete_final_piece_is_answered_by_its_prefix_range_row`、`jianpin_and_ambiguous_segmentations_still_answer_the_typed_key`、`shuangpin_prefix_range_rows_do_not_answer_the_typed_key`；引擎 lib 全套 2697 个通过。关联：[个人上下文重排首组只收用完全部按键的字典候选](2026-10-31-personal-context-rerank-prefix-rows.md)部分重叠，它用的是同一个 `selection_completes_composition` 信号，但没有加本篇的读音判定，个人重排首组仍可能收进前缀区间行。
