# Agent Note: Stage 3 单遍 4.5MB 神经复用、停顿首选锁定与语境同音标定

Status: implemented

## Problem

在 Stage 2 修复 `LOG10_E` 搭配模型量纲后，Shurufa 的逐键与停顿重排仍存在三处结构性痛点：

1. **单键重复推理两遍 4.5MB 模型与双份权重副本**：`crates/engine/src/lattice/merge.rs` 逐键对最多 12 条整句路径运行一次 4.5MB 键盘模型，随后 `crates/input-runtime/src/runtime.rs::rerank()` 在同一按键上又调用一次 `ordering::rerank_pick` 重新评分 `[Collocation, NeuralKeyboard]`；同时 `crates/host-api/src/ffi.rs` 使用独立模型缓存，使引擎与运行时在内存中各自持有并推理一份 4.5MB `sentence-model.safetensors`，`rerank_latency` 实测 `reranker adds` 在 `p95` 额外增加 `+1.24 ms`、`p99 +2.36 ms`。此外引擎内 `NeuralReranker::score_order_into` 仍使用固定 `last_characters(64)` 滑窗而非步进窗口 `rerank_context`（`RERANK_CONTEXT_STEP = 16`），长文场景易与运行时前缀缓存错位。
2. **停顿精排延迟翻转空格首选与翻页后跳序**：150 ms 停顿后 25MB 大模型直接替换第 1 槽（`sentences-v2` 仍有 `30` 次首位翻脸，翻转率 `9.6%`），打断盲打空格肌肉记忆；用户已按方向键或翻页查看次选时（`highlighted > 0`），异步精排仍可能原地改排候选页。
3. **语境同音词典竞争双计先验与 1.5 nat 门槛过高**：`crates/engine/src/lattice/neural.rs::pick_dictionary_homophone` 将未标定的词库分档对数权重 `1.0 * ln(weight)` 与本身已含语料先验的条件神经概率 `ln P_LM(w | context)` 直接相加，并要求 `+1.5 nats` 净优势，导致旧词库存在人工分档断崖的常用同音词（如「登录 502k / 登陆 5k」「权利 4.19M / 权力 500k」）几乎无法在强上文下翻盘，`shurufa-spoken-v1` 的 `context-homophone` 仅 `0.588`（10/17）。

## Decision

落地 [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md) 的 Stage 3：

### 1. 同源共享 4.5MB 模型与单遍缓存复用（零额外堆分配）

- `crates/engine/src/lib.rs` 公开重导出 `shared_sentence_model`，`crates/host-api/src/ffi.rs::keyboard_reranker` 在加载随包默认键盘模型时复用引擎共享的 `Arc<SentenceModel>`，省去一份 4.5MB 权重副本并确保引擎与运行时指向同一模型实例指针。
- `crates/engine/src/lattice/neural.rs::score_order_with_means_into` 改用与 `ordering.rs` 完全一致的步进上文窗口 `ordering::rerank_context`，在完成 12 条路径评分的同时输出每条路径的逐字模型均值（毫 nat）及模型实例标识 `model_id`。
- `WordItem::keyboard_neural: Option<(usize, i32)>` 经 `StaticEvidence::keyboard_neural` 随既有 `candidate_static_evidence` 传入 `input-runtime`（不向 `EngineSnapshot` 增加新的平行 `Vec`，零额外堆分配）。
- `crates/engine/src/ordering.rs::rerank_pick` 检查全部参与比较的候选是否均携带与当前 `reranker.model()` 同源的 `keyboard_neural` 缓存分；全部命中时直接复用缓存分值跳过第二遍 4.5MB Transformer 前向计算，存在备选切分/纠错行或测试合成模型不匹配时自动回退正常推理。

### 2. 停顿精排锁定第 1 槽与导航冻结页序

- `crates/engine/src/ordering.rs` 新增 `settled_rerank_order_with_options(..., lock_leader: bool)`：`lock_leader = true` 时保证 `order[0] == 0` 恒成立，停顿 25MB 大模型仅对第 2 名及以后的可比候选排序（静态证据分支与无静态证据分支均遵守该不变量）；原有 `settled_rerank_order` 默认 `false` 保持向后兼容。
- `crates/input-runtime/src/runtime.rs` 新增 `set_settled_lock_leader(bool)`；开启后，只要用户已移动高亮或翻页（`self.highlighted > 0`），`settled_rerank_blocked` 与 `apply_settled_rerank` 立即冻结候选页，不再发起或应用异步重排。
- `crates/host-api/src/ffi/session.rs` 与 `crates/host-api/src/lib.rs` 对 `edition.id == "shurufa"` 默认开启 `set_settled_lock_leader(true)`。

### 3. 语境同音词典竞争去双计标定

- `crates/engine/src/lattice/neural.rs::pick_dictionary_homophone` 将静态权重降为低频防噪稳定项 `0.25 * ln(weight)`（避免重复计算神经模型已学得的语料先验，并削平旧词库 100× 分档断崖），同时将净领先门槛从 `1.5 nats` 标定至 `0.35 nats`；仅在非空汉字上文下生效，零上文词级排序与手动置顶保持不变。

### 4. 实测验证数据

#### A. Apple Silicon 逐键延迟（`rerank_latency --limit 80 --keyboard-neural`，`sentences-v2.tsv`，1,665 键）

| 指标 | Stage 2（单键双跑 4.5MB） | Stage 3（单遍缓存复用） | 变化 |
|---|---|---|---|
| `reranker adds` mean | `+0.13 ms` | **`-0.01 ms`** | **消除额外均值开销** |
| `reranker adds` p95 | `+1.24 ms` | **`+0.28 ms`** | **-77%（4.4× 降低）** |
| `reranker adds` p99 | `+2.36 ms` | **`+0.90 ms`** | **-62%** |
| 端到端 `with model` p95 | `15.82 ms` | **`15.79 ms`** | **稳定低于 16 ms 帧预算** |

#### B. 候选质量与首位抖动率（完整管线：万象 `.gram` + 4.5MB + 25MB + 纠错 + `--settled-lock-leader`，页宽 6）

| 评测集 | Stage 0 基线 | Stage 2（修 `LOG10_E`） | Stage 3（单遍 + 锁首选 + 语境标定） |
|---|---|---|---|
| `shurufa-spoken-v1` (n=120) | `imm=0.725 / t1=0.758 / flp=6` | `imm=0.783 / t1=0.792 / flp=2` | **`imm=0.800 / t1=0.800 / t5=0.958 / fnd=0.992 / mrr=0.868 / flp=0`** |
| ↳ `context-homophone` (n=17) | `0.588` | `0.588` | **`0.706` (+11.8%, `top5=1.000`)** |
| `sentences-v2` (n=312) | `imm=0.663 / t1=0.740 / flp=49` | `imm=0.740 / t1=0.779 / flp=30` | **`imm=0.740 / t1=0.740 / t5=0.933 / fnd=0.984 / flp=0`** |
| `sentences-neutral-v1` (n=150) | — | `imm=0.784 / t1=0.842 / flp=9` | **`imm=0.784 / t1=0.784 / t5=0.957 / fnd=0.978 / flp=0`** |
| `quanpin-words-v1` (n=2,791) | `t1=0.755` | `t1=0.755` | **`imm=0.755 / t1=0.755 / t5=0.967 / fnd=0.989 / flp=0`** |

注：`pronoun-ta`（`0/6`，`found=1.000`）仍受限于 4.5MB 字级模型对跨句长距指物代词的语义容量，按 Stage 0 约定不为其引入启发式特判。

## Historical audit

- [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md)：本篇落地其中的 Stage 3（单遍 4.5MB 复用、停顿首选锁定与语境同音标定）。
- [Stage 2 搭配模型量纲修复与万象词库 2×2 消融审计](2026-11-20-stage2-collocation-scale-and-lexicon-audit.md)：延续其自然对数域搭配底座，在 `imm_top1` 无回退前提下将首位抖动归零并显著拉升语境同音准确率。

## Alternatives considered

### 直接移除 `input-runtime` 侧的逐键 `Reranker`
最强理由是代码最简。未采用，因为当存在多切分（如 `xian` 的隔音备选 `xi'an`）或整句纠错行跨出主词网格时，只有 `input-runtime` 侧能看到合并后的完整候选集合；通过 `StaticEvidence::keyboard_neural` 携带引擎已算分并校验同源模型指针，既在常规单切分热路径上完全免除第二遍推理，又无损保留跨切分重排能力。

### 停顿精排保留高阈值首位翻转而非完全锁定第 1 槽
最强理由是在 `sentences-v2` / `neutral` 上可挽回约 4–5% 的静态离线 `top1`。未采用，因为三方共识明确以真实打字稳定性优先：停顿 150 ms 后空格首选突然跳变是盲打最破坏手感的体验缺陷；锁定第 1 槽并让大模型精排第 2–6 槽，使首位翻转率彻底归零，同时保证用户扫一眼首屏即可在 2–6 位找到大模型优选次句。

## Consequences

- **收益**：Shurufa 省去一份 4.5MB 模型内存副本，逐键额外神经延迟 `p95` 下降 77%（`+1.24 ms → +0.28 ms`），停顿首位翻转率全集归零（`0.0%`），`shurufa-spoken-v1` 即时首选率达到 `80.0%`（相对 Stage 0 基线累计 `+7.5%`）、`context-homophone` 提升 `+11.8%`。
- **权衡**：离线静态指标中依赖停顿后翻盘的 `settled_top1` 回落到与 `immediate_top1` 相等，符合本阶段「宁稳勿抖、用逐键首绘能力而非延迟翻脸赢取首选」的产品契约。
