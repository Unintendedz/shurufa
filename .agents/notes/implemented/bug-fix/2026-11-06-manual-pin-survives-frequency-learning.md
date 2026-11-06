# Agent Note: 手动置顶不被自动调频挤掉

Status: implemented

## Problem

维护者报告：手动置顶过的单字，有时空格上屏的是第二个候选，有时又是第一个。只读查看本机用户日志：置顶写入了 `pinned_candidates`，同时按 `force_top` 把该词的权重提到最高；之后在 `promote`、触发次数 1 下，同一读音里另一个候选只被选中一次，就被 `adjust_candidate_ranking` 写到「当前最高权重 + 500」，置顶词退到第二位。再选回置顶词又把它提回去，所以表现为时好时坏。`pinned_candidates` 只在个人上下文重排里被读（见[全局调频首选优先于个人上下文重排](2026-10-24-global-learning-before-personal-context.md)），调频不看它，置顶实际上只是一次性的权重提升。

## Decision

拼音调频（`PersonalDictionaryKind::Pinyin`，非 `force_top`）计算目标位置后，在同一个日志事务里按 `context_key` 读 `pinned_candidates`；置顶词在按权重排好的集合里排在被选候选之前时，目标位置下限是置顶词的下一位。下限把目标抬到不小于原位置时不写任何权重，只清选中计数。手动置顶（`force_top`）不受限制，并且 `record_pinned_candidate` 让新的置顶替换旧的，所以置顶词只让给下一次手动置顶。

保护对 `promote`、`pin`、`halve`、`linear` 四种自动模式一致。五笔码表和英文行不记录置顶，不受影响。已经被挤下去的置顶词不会自动恢复，重新置顶一次即可。

## Alternatives considered

- 把置顶改写成固定位置第 1 位（`fixed_candidate_positions`）：位置靠槽位而不是权重，调频天然动不了它；但固定位置会拒绝该词自身的调频、占用五个槽位之一，还和已有的「固定到第 N 位」菜单语义重合，老用户的置顶记录也要迁移。
- 在候选合并时按 `pinned_candidates` 把置顶词强行挪到首位：能顺带修复已经被挤下去的记录；但在权重之外又加一层排序，和固定位置、个人上下文重排、在线座位的先后都要重新约定，调频仍会按显示顺序之外的权重继续上抬别的候选。
- 不改，告诉用户关掉调频或改用固定位置：零代码；但置顶的字面含义就是留在首位，默认 `promote` 下一次误选即失效，不符合用户预期。

## Consequences

收益：置顶词在自动学习下稳定留在首位，其余候选照常按调频上移，最多到第二位。代价：被选候选紧挨在置顶词后面时再选不会再动，置顶词在场时同读音里别的词不能靠选择超过它；每次真正需要挪位的调频多一次按主键的日志查询。

验证：`session::learning::tests::automatic_learning_never_displaces_a_manual_pin` 在旧实现上失败（`promote` 下选六次后置顶词退到第二），新实现四种模式均通过；`msime-engine` 单元测试 2714 项通过。在本机用户日志的临时副本上重新置顶后选另一个候选三次，旧实现第一次就换了首位，新实现置顶词保持首位，副本用完即删，没有带出任何数据。golden 的 10 个 `answers_key` 差异和 host-api 的 3 项失败在未改动的 `main` 上同样存在，与本改动无关。macOS 输入法真机未重装验收。
