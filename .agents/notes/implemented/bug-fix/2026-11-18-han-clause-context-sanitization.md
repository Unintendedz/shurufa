# Agent Note: 剥离非汉字上文 `<unk>` 污染并向词格传入连续汉字前文

Status: implemented

## Problem

在带表情符号、空格或标点之后继续输入汉字（如「🧐 培养`zhongshiguke`」）时，上下文传递链路存在两处偏差：

1. **神经模型把非汉字吃成 `<unk>` 词元**：`chinese-ime-lm` 的字级词汇表只在纯连续汉字上训练，不含表情符号、空格、ASCII 或标点。`set_rescoring_context` 原样保留了 `"🧐 培养"` 后缀，神经模型把 `'🧐'` 和 `' '` 编成 `<unk>`（低频生僻汉字槽位），导致前缀语义严重失真，反把 `"中式骨科"` 抬为神经第 1 名、压低 `"忠实顾客"`。
2. **词格解码忽略跨组字边界前文**：`decode_graph_with_terminal` 与 `rescore_with_trigram` 对每轮组字的第 0 列一律使用 `SENTENCE_START`（`\u{1}`），导致句中续写错吃高权重的句首二元/三元转移，既看不见前词转移，也无法在首词处给 `.gram` 字级搭配提供前驱汉字。

## Decision

统一以「光标前末尾连续汉字子句」作为前文语义单元，同时打通神经重排与词格解码两侧：

1. **连续汉字子句提取（`crates/engine/src/text.rs`）**：新增 `trailing_han_clause(text: &str) -> &str`，从末尾反向扫描连续 `is_han` 字符并返回零分配切片；一旦遇到空格、标点、数字、字母或表情符号即截断（因此 `"🧐 培养"` 提取 `"培养"`，`"你好，"` 提取 `""`）。
2. **神经入口统一清洗（`crates/engine/src/ordering.rs`、`crates/engine/src/lattice/neural.rs`）**：`rerank_pick`、`settled_rerank_order`、`NeuralReranker::score_order_into` 与 `NeuralReranker::pick_dictionary_homophone` 在截取窗口前先经 `trailing_han_clause` 过滤。保留 `pub fn rerank_context` 本身签名与字符边界截断契约不变。
3. **词格消费 `preceding_context` 与按需缓存键（`crates/engine/src/lattice/decode.rs`、`quanpin/dictionary.rs`、`shuangpin/dictionary.rs`）**：
   - `LatticeOptions` 新增 `preceding_context: &'a str`（默认 `""`）。非空时：第 0 列静态二元查找改以 `last_characters(preceding_context, 2)` 替代 `SENTENCE_START`（个人模型 `personal.context` 仍维持 `SENTENCE_START` 以守住句首习惯迁移边界）；`collocation_tail` 在路径累计码点不足 8 位时向前补足紧邻汉字前文；`rescore_with_trigram` 在第 0 词跳过伪句首三元查询，并在第 1 词改用 `preceding_word`。`preceding_context` 为空时路径与零分配统计维持原样。
   - `QuanpinDictionary` 与 `ShuangpinDictionary` 通过 `sentence_reads_context()` 判断当前是否启用了读取上文的组件（`!rerankers.is_empty()` 或非零权重 `.gram`），仅在该条件为真时将 `trailing_han_clause(&rescoring_context)` 写入 `options.preceding_context`、系列槽位键与 `sentence_decode_key`，保证无模型配置下的槽位所有权、在线候选留存与缓存分配计数不变。

## Historical audit

- [组字开始前同步光标前现成文本并按前文重排首组字典候选](../feature/2026-11-17-surrounding-context-association.md)：直接上游并双向互链。该篇打通 `rescoring_context` 读取与同音提首，本篇净化其非汉字前缀污染并把干净前文继续接入词格静态转移与 `.gram` 尾窗。
- [Rime octagram `.gram` 语法模型读取端与逐边打分](../feature/2026-10-21-octagram-grammar-model.md)：职责叠加不冲突。该篇定义 `collocation_tail` 的 8 码点尾窗与逐边打分；本篇在其之前补足跨组字边界的前置汉字，并同步将读取 `.gram` 时的上文依赖纳入 `series_slot_key`。

## Alternatives considered

- **在 `Runtime::set_surrounding_context` 层面直接丢弃非汉字**：会让依赖光标前紧邻符号做判断的智能标点、前缀识别或混合上下文失去原始前文；在神经评分和词格接口边界做无分配切片既保留宿主全量 `rescoring_context`，又消除 `<unk>` 污染。
- **在 `ordering::rerank_context` 内部调用 `trailing_han_clause`**：最紧凑，但会破坏已有 `rerank_context_cuts_on_a_character_boundary_and_can_empty` 对混排串的既定窗口契约；在各模型调用入口包一层零分配过滤零冲突。
- **对所有配置无条件把 `rescoring_context` 编入 `series_slot_key`**：会破坏无模型时的缓存槽位复用约定（跨普通上屏保留在线结果）；绑定 `sentence_reads_context()` 仅在真实消费前文时切槽位。

## Consequences

- **收益**：表情符号、空格和标点后的中文输入不再被 `<unk>` 扭曲语义；标点末尾正确视作新子句起点而重新吃句首统计；连续汉字后接打整句不再误拿 `<bos>` 奖励，并同步为静态二元、三元和 `.gram` 语法搭配提供跨边界前文。
- **代价与边界**：开启神经或 `.gram` 模型且前文持续变化时，`sentence_decode_cache` 按清洗后的前文切片细分键值；当光标前以英文或数字紧邻汉字输入（无空格隔开，如 `"VIP会员"`）时，`trailing_han_clause` 会在非汉字处截断、不保留跨英文字词之前的汉字，这是刻意为避免跨词序误导做的保守边界。
