# Agent Note: 收敛 `stat_log_prob` 重复计分并消除翻页扩容覆写已落定神经排序

Status: implemented

## Problem

在完成离线词库校准与误纠纪律收口后，`tier1_eval` 端到端评测（`417` 例）进一步揭示出两处 Tier 0 ↔ Tier 1 跨层契约不一致：

1. **翻页扩容（`expand_cached_candidates`）清空已落定的 Tier 1 `Qwen3-0.6B` 排序与束搜索注入项（`expand_wiped = 2`）**：
   - 当 Tier 1 神经响应已完成融合（`apply_neural_response` 重排了前排候选或注入了 `CandidateOrigin::InjectedBeam` 新项）后，用户翻至后页或宿主请求全量候选（`Runtime::all_candidates()`）会调用 `Runtime::expand_cached_candidates()`；
   - 原实现直接用 `self.engine.snapshot()` 覆盖 `self.cached` 并重跑纯 Tier 0 的 `self.order_presentation()`，把已落定的 `Qwen3-0.6B` 首屏/前排顺序无条件打回 Tier 0 初始序，并彻底弄丢全部受限束搜索注入项。
2. **字典候选 `.gram` 搭配分跨字段重复计权与空上文多字词尾字衔接不对称**：
   - `Session::neural_v2_metadata` 在 `han_clause` 非空时同时执行 `stat_log_prob = stat_log_prob.map(|s| s + capped)` 与 `collocation_log_prob = 0.28 * capped`，将同一份万象 `.gram` 搭配分重复灌入静态先验和搭配先验两个通道，污染了 `UnifiedScorer` 桶内纯静态词频基准 `dict_max`；
   - 反之，在空上文（`han_clause.is_empty()`）下，`3+` 字 `Generated` 词网格整句（如 `我是一个饼`）通过 `sentence_tail_collocation_log_prob` 获得词内末字 `.gram` 搭配加分，而同字数 `Database` 词典短语（如 `5` 字词 `我是一个兵`）既拿不到空上文词内尾字 `.gram` 衔接，又因 `neural_v2.rs` 的 `exact_dict_len_mask` 仅覆盖 `2..=4` 字而失去对同长度拼凑句的成词结构保护。

## Decision

1. **在 `Runtime` 维护 `neural_settled_epoch` 并在扩容时无损保留已落定前排（`crates/input-runtime/src/runtime.rs`）**：
   - 新增 `neural_settled_epoch: Option<u64>`：在 `apply_neural_response` 成功落定后记录当前 `composition_epoch`，在 `refresh`、`replace_engine_with_touch_layout` 与 `set_neural_v2_enabled(false)` 时同步清空；
   - `expand_cached_candidates()` 在当前 `composition_epoch` 已落定 Tier 1 排序时，先提取去除了内联 Emoji 投影的前排已落定序列，再在引擎完成 `expand_initial_candidates()` + `order_presentation()` 后，将前排每项按 `(text, code)` 映射回扩容后快照的新 `CandidateOrigin::EngineRow(new_idx)`（保证后续按位选词 `select_seat` 100% 指向扩容后的正确引擎行），原样保留 `CandidateOrigin::InjectedBeam` 束注入项与 `tier1_neural_log_prob`，最后顺序追加尚未出现的新增后排扩充项并统一执行 `apply_slot_allocator()`。
   - 在 `crates/input-runtime/examples/tier1_eval.rs` 将 `expand_wiped == 0` 升级为全集零容忍硬门禁。
2. **净化 `stat_log_prob` 职责边界并对齐 `2..=5` 字成词与尾字搭配对称性（`session/candidates.rs`、`neural_v2.rs`）**：
   - 从 `Session::neural_v2_metadata` 移除向 `stat_log_prob` 叠加 `capped` 的重复路径，令 `stat_log_prob` 严格表达纯词典/词网格静态先验，`.gram` 搭配信号唯一经 `collocation_log_prob` 流入 `UnifiedScorer`；
   - 空上文下 `3+` 字 `Database` 词典短语同步计算词内尾字 `.gram` 衔接（与 `Generated` 整句完全对称），并将 `neural_v2.rs` 的 `exact_dict_len_mask` 覆盖范围由 `2..=4` 扩展至 `2..=5` 字。

## Historical audit

- [端侧两阶段真实模型自动化评测门禁](../testing/2026-11-27-tier1-neural-end-to-end-eval-gate.md)：正是该评测器新增的 `all_candidates()` 对比探针首次量化捕获了 `expand_wiped = 2` 的隐蔽状态覆盖缺陷，现转为 `expand_wiped == 0` 硬门禁闭环。
- [主词库只读校准边车与离线权重重标定管线（`SLXC` v1）](2026-11-27-offline-lexicon-calibration-sidecar.md)：多字口语短语与 5 字词校准出 `weight == 100` 后走入标准词典桶，此次对齐空上文词内尾字衔接与 `2..=5` 字成词掩码消除了校准后暴露的跨桶不对称。

## Alternatives considered

### 1. 在 `expand_cached_candidates()` 中直接保留旧 `CandidateEntry` 的 `EngineRow(old_idx)` 而不重绑新快照下标

- **最强理由**：只需把新增词 `extend` 到旧数组末尾，代码最少。
- **放弃原因**：`Session::expand_initial_candidates()` 重新查询词库放宽上限后，底层引擎快照中的行下标会发生重排偏移；若沿用扩容前的 `EngineRow(old_idx)`，用户翻页后点选候选会选错底层词条。按 `(text, code)` 从扩容后快照认领最新 `CandidateEntry` 可同时保住 Tier 1 展示顺序与底层 `EngineRow` 精确映射。

## Consequences

- `msime-input-runtime` 全部 `232` 项单测（含新增扩容保序与 `InjectedBeam` 保留单测）全部通过；
- `417` 例端侧 `Qwen3-0.6B` 评测集上 `expand_wiped` 由 `2` 彻底归 **`0`**，空上文 `woshiyigebing -> #1 我是一个兵` 恢复首槽，`stat_log_prob` 与 `collocation_log_prob` 实现正交解耦。
