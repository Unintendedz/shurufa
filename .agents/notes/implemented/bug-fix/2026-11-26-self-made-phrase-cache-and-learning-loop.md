# Agent Note: 自造词双连接缓存一致性与神经束候选学习闭环

Status: implemented

## Problem

在启用纯 macOS 统一流水线与 [Stage 4 独立个人学习叠加层](../architecture/2026-11-20-stage4-decoupled-personal-learning-overlay.md) 后，用户顺序逐字组出非词典二字词（例如打 `aili` 分两次选 `爱` + `梨` 上屏「爱梨」），甚至多次重复提交该词后，再次输入完整拼音 `aili` 时新造词仍长期难以出现在首页，且部分情况下仅因点击候选框外置操作而丢失学习回流：

1. **写入/读取双 `QuanpinEngine` 实例缓存脑裂**：`InputSession` 的 `canonical_phrase_engine` 会单独打开并缓存另一份独立的全拼词库连接写入新词；在双拼或混合读取路径下，虽然写侧调用了 `reset_cache`，主解码侧的 `QuanpinEngine` 缓存（或跨方案共享的拼音缓存）未在同一 `ProviderRegistry` 实例上统一失效，导致随后重新键入全码时查询仍命中旧缓存页。
2. **弱静态先验场景下 `1.8` 次门槛误拦首次自造词**：在独立个人叠加层（`apply_personal_overlay_scoped`）中，若目标编码静态槽 0 只是低频冷门残留词（`weight < 500`，如 `aili` 下的冷门候选）且用户从未选中过，原本统一的 `SLOT_ZERO_MIN_DECAYED_PICKS = 1.8` 门槛会阻止首次刚组出的自造词进入槽 0，而神经受限束搜索或次选单字拼凑又会继续把真正的自造词压到后面。
3. **神经注入候选与 `Command::CommitCandidate` 未走统一学习闭环**：`Runtime::commit_neural_injected` 提交 `CandidateOrigin::InjectedBeam`（`CandidateSource::NeuralConstrained`）时只上屏文本而未回写用户词库与个人叠加层；同时 `Action::Command(Command::CommitCandidate)` 直接透传给底层 `Engine::command(CommitCandidate)`，绕过了运行时的 `select_seat(0)`、分段前缀拼接以及神经注入候选学习逻辑，导致经快捷键/菜单按默认首位提交的候选不会记入个人学习与自造词闭环。
4. **选词组成并落盘新词后未写个人叠加层事件**：逐字选词凑出整词并写入主/用户词库（`create_word_from_canonical_pinyin` / `store_user_phrase_from_canonical_pinyin`）后，未向 `personal_selection_events` 补充一条选择事件，使启用 `decoupled_personal_learning` 的会话无法在二次键入时通过个人叠加层优先召回刚造出的词。

## Decision

打通从逐字组词 / 神经束搜索上屏到二次输入召回的完整学习闭环（提交 `8359cd59d2`），与 [Shurufa 纯 macOS 单产品减法与三段单向候选管线收敛](../architecture/2026-11-25-shurufa-pure-macos-pipeline-convergence.md) 的统一流水线保持一致：

1. **消除 `QuanpinEngine` 双实例分裂，统一由 `ProviderRegistry::quanpin_mut` 管理**：
   - 移除 `InputSession` 独立的第二条 `canonical_phrase_engine` 实例；`ProviderRegistry` 持有唯一可按需初始化的 `QuanpinEngine`（`quanpin_mut()`）并暴露 `reset_all_caches()` 与 `create_word_from_canonical_pinyin()`。
   - 无论当前激活全拼还是双拼，凡写入自造词/规范拼音短语后，立即在同一引擎连接上落盘并调用 `reset_all_caches()` 同步失效所有拼音查询缓存。
2. **补充叠加层选词事件并在弱静态首位时允许首轮升格**：
   - 新增 `InputSession::record_learned_phrase_overlay`，在 `store_user_phrase_from_canonical_pinyin` 与 `learn_injected_candidate` 成功写库后向 `personal_selection_events` 写入一条 `personal_selection_events` 记录并更新 `last_overlay_learning`（保持 5 秒内紧接退格删光确定性回滚能力）。
   - 在 `crates/engine/src/user_dictionary/overlay.rs::apply_personal_overlay_scoped` 中保持高频核心词 `SLOT_ZERO_MIN_DECAYED_PICKS`（`1.8`）防单次误选护城河不变，仅当静态首位权重 `< 500.0` 且用户从未选过该首位（`user_counts[0] < 0.9`）时，允许刚形成/明确选择一次（`user_counts[pos] >= 0.9` 且无 24 小时负反馈抑制）的候选直接升格到槽 0。
3. **补齐神经束搜索候选与 `CommitCandidate` 的运行时学习入口**：
   - 在 `Session` 与 `InputEngine` 新增 `learn_injected_candidate(&mut self, canonical_pinyin, word)`：将 `Runtime::commit_neural_injected` 提交的多字束搜索/受限解码候选（含 `locked_prefix` 前缀拼接后的完整规范拼音与整词）直接写入用户词库、个人叠加层与个人上下文模型。
   - 在 `Runtime::dispatch_action` 中拦截 `Action::Command(Command::CommitCandidate)`（非联想、非整句改字且存在候选时），统一改走 `select_seat(0)`，使空格、`CommitCandidate` 与神经注入候选共享同一条选词与学习闭环。

## Alternatives considered

### 仅靠提高 `user_phrase` SQLite 初始权重压过同音词

- **最强理由**：无需改动 `overlay.rs` 的槽 0 竞争条件，也无需同步写入 `personal_selection_events`。
- **放弃原因**：在 Shurufa 的 `decoupled_personal_learning = true` 路径下，个人叠加层直接锚定并保护静态权重分布；静态首位权重哪怕只有 `400`，若不写 `personal_selection_events` 或放宽弱先验门槛，`overlay.rs` 仍会把新词留在后面；同时盲目抬高物理权重又会重蹈「静态先验污染整句词网格解码」的老路。

### 让 `canonical_phrase_engine` 写完后向主会话发异步重刷信号

- **最强理由**：可以继续隔离自造词写入连接与主输入查询连接，避免共享可变状态。
- **放弃原因**：SQLite WAL 事务之外，主问题正是两个各自持有 LRU/页缓存的 `QuanpinEngine` 实例造成内存态缓存脑裂；既然主会话本来就在同一线程依次执行按键和选词，让 `ProviderRegistry::quanpin_mut()` 成为唯一拼音写词库入口并立刻 `reset_all_caches()`，既省掉一条长驻连接和双倍内存缓存，又能在同步返回前确定性消除陈旧缓存。

## Consequences

- 在 `decoupled_personal_learning` 开启下，无论用户逐字选中「爱」「梨」凑出「爱梨」、或从 `Qwen3-0.6B` 受限束搜索直接上屏非词典 OOV 短语，再次键入完整全拼时均可从同一非陈旧缓存与个人叠加层稳定召回；且由于 500 静态权重门槛与 `last_overlay_learning` 句柄继续生效，高频核心词单次误选不篡夺首位、紧接退格删光仍可确定性撤回。
- 对话式测试覆盖：`crates/engine/src/session/tests.rs` 与 `crates/input-runtime/src/tests.rs`（`real_runtime_commit_candidate_commits_slot0_and_learns_injected_beam_phrase`）固定验证本闭环。
