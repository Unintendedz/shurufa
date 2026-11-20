# Agent Note: Stage 4 独立个人学习叠加层与确定性退格回滚

Status: implemented

## Problem

既有自动调频（`crates/engine/src/user_dictionary/ranking.rs::plan_weights`）把用户的每次选词直接写成 `UPDATE tbl_* SET weight=?1` 覆写主词库 `msime-pinyin.db` 的静态权重列，造成三类结构性缺陷：

1. **静态先验污染整句解码**：被调频抬高的整数 `weight` 会直接流进词网格 `edge_log_prob` 的 `ln(w)`，一次低频词选词可能长期扭曲整句切分；为了缓解误选击穿，不得不继续叠加 `CORE_LEADER_MOAT_WEIGHT = 200_000 / 8×` 等启发式护城河。
2. **无法区分「首屏召回」与「竞争首位」**：要么一次选词直接夺首位（误选破坏盲打习惯），要么多次选择才能从第 5 页慢慢爬回首屏（冷门人名/专名体感迟钝）。
3. **无时间衰减、无即时回滚**：一年前误选或临时打过的专名永久留在主词库，紧接按退格删掉刚误选的词也无法自动撤销学习写入。

## Decision

落地 [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md) 的 Stage 4，为 Shurufa 启用完全独立于主词库静态权重的个人学习叠加层（`SessionOptions::decoupled_personal_learning`，在 `host-api` 中对 `edition.id == "shurufa"` 默认开启）：

### 1. 追加式衰减事件日志（永不改写 `msime-pinyin.db`）

- `crates/engine/src/user_dictionary/journal.rs` 在 `msime_user.db` 新增 `personal_selection_events(id, entry_key, value, selected_at)` 表与索引（`CREATE TABLE IF NOT EXISTS`，完全向后兼容 v4 日志且由 `reset_learned_data` 自动清空）。
- `crates/engine/src/session/learning.rs::adjust_candidate_frequency` 在 `decoupled_personal_learning = true` 时彻底绕开 `adjust_candidate_ranking` / `plan_weights` 与 `msime-pinyin.db` 写操作：
  - 空格接受默认首位（`index == 0`）照旧不记任何事件；
  - 明确改选次选拼音词典词时，仅向 `personal_selection_events` 追加一行事件并记录自增 `event_id`；写入每满 64 次自动裁剪超过 180 天或超出 50,000 条的陈旧记录；
  - 手动置顶（`pin_candidate`）仅写 `pinned_candidates` 表，同样不改写主词库静态权重。

### 2. 30 天半衰期贝叶斯平滑叠加与 L4 硬置顶（`crates/engine/src/user_dictionary/overlay.rs`）

- 对开头连续匹配完整 `entry_key` 的同音词典行，按 30 天半衰期计算有效明确选择次数 $n_{\text{user}}(w) = \sum 2^{-\Delta t / (30\text{d})}$，与静态概率分布 $P_{\text{static}}(w)$ 计算贝叶斯平滑后验：
  $$P_{\text{post}}(w) \propto \alpha \cdot P_{\text{static}}(w) + n_{\text{user}}(w), \quad \alpha = 3.0$$
- **区分首屏召回与竞争首位**：
  - 只有当 $n_{\text{user}}(w) \ge 1.8$（至少 2 次近期明确选择）且后验超过静态榜首时，候选才允许竞争第 1 槽（对占比 ~90% 的超高频核心词如「一个」，先验质量约 `2.70`，自然需要 3 次明确选择才反超；单次误选永远不会击穿核心词，天然取代 `200k / 8×` 硬编码护城河）；
  - 首次明确选择埋在多页之后的冷门专名（$n_{\text{user}} = 1.0$），因跨全部未选词的静态先验总质量为 $\alpha = 3.0$，至多只有 2 个静态占比超 1/3 的词能排在它前面，从而自然将其召回到**首屏第 2–4 位**；
  - 30 天半衰期使旧选择随时间自然衰减回落，避免一次性专名永久占榜；
  - 显式「置顶」（`pinned_candidates`）在叠加层最前作为 L4 硬约束无条件锁定第 1 槽。
- `crates/engine/src/session/candidates.rs::merge_mixed_candidates` 在读取引擎候选后、个人上下文与语境同音重排之前执行 `apply_personal_overlay`，全程只重排顺序、绝不改动任何 `WordItem::weight`。

### 3. 紧接退格确定性回滚与显式撤销入口

- `InputSession` 维护 `last_overlay_learning: Option<LastOverlayLearning { event_id, remaining_chars }>`：
  - 上屏后若用户在空闲态紧接按 `Command::Backspace` 删光刚上屏词的全部字符数（`remaining_chars` 归零），立即按 `event_id` 确定性删除 `personal_selection_events` 中的该条事件；
  - 一旦输入下一个字符、标点、非退格命令或重置上下文，立即清空该待回滚句柄，防止误撤更早的提交；
  - 同时在 `Session` 公开 `undo_last_personal_learning(&mut self) -> bool` 显式撤销入口。

## Historical audit

- [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md)：本篇落地最终阶段 Stage 4（独立个人学习叠加层与退格回滚）。至此提案中的 Stage 0 → Stage 4 全部完成。
- [超高频核心词渐进调频护城河与语境同音竞争解耦](../bug-fix/2026-11-18-core-word-frequency-moat.md)：在 Shurufa 路径下已被本篇贝叶斯先验平滑自然取代（非 Shurufa 上游兼容路径暂保留原实现以守住历史单测）。

## Alternatives considered

### 直接修改 `plan_weights` 的步长并新增回滚快照
最强理由是复用既有权重列。未采用，因为只要学习状态继续覆写 `msime-pinyin.db` 的静态 `weight`，整句词网格的 `ln(w)` 静态先验就仍然会被个人选词污染，且无法做自然的时间衰减。

### 用内存 LRU 缓存代替 SQLite 事件表
最强理由是读盘为零。未采用，因为重启输入法或切换应用进程后学习记忆会全部丢失；使用 `msime_user.db` 的线程本地复用连接并建 `(entry_key, value)` 索引，仅在同音词典查询命中时做一次微秒级索引查询且无日志文件时不创建文件。

## Consequences

- **收益**：Shurufa 主词库 `msime-pinyin.db` 的静态词频在日常使用与手动置顶下彻底保持只读纯净；单次误选不击穿核心高频词、冷门专名 1 次明确选择即进首屏第 2–4 位、连续 2–3 次自然竞争首位、30 天自然衰减、紧接退格删光即确定性回滚学习事件。
- **边界**：独立个人叠加层当前覆盖 Shurufa 的全拼/双拼常规拼音词典候选；英文词典与五笔码表继续沿用既有调频路径；旧 `user_dictionary_operations` 历史权重不受破坏，未来若需随本地备份导出 `personal_selection_events` 可直接并入 `habits.rs`。
