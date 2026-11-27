# Agent Note: 主词库只读校准边车与离线权重重标定管线（`SLXC` v1）

Status: implemented

## Problem

对锁定主词库 `msime-pinyin.db`（70 万+词条）的全表统计暴露出三类根源性脏先验，导致运行时排序各层被迫叠加大量防御性特判：

1. **`tbl_1_*` 单字零权方言/讹误幻影（`947` 条）**：高频常用汉字在非普通话读音下带有 `weight == 0` 的方言或抓取讹误行（如 `me -> 没(0)/末(0)`、`ti -> 是(0)`、`wei -> 有(0)`、`ao -> 棍(0)`、`an -> 盒(0)`、`bo -> 怕(0)`、`bing -> 平(0)`），不仅曾触发上下文模型误提权，还会直接在短候选键（如 `me`）首页尾部露出荒诞错读单字。
2. **`tbl_2_*` 高权非词汇化「`X的`」组合伪词（`557` 条）**：`中的(503k)`、`上的(502k)`、`到的(500k)`、`鱼的(72.5k)`、`雨的(39.6k)` 等语法组合片段在双字词表中占据数万至五十万权重，在三音节输入（如 `yudedao`）中既作为高权 2 字边拼出 `鱼的倒`/`雨的到`，又霸占前缀词席位挤走真正词汇（`欲得`/`育德`）。
3. **`tbl_3..tbl_5` 大规模未测频 `weight == 100` 悬崖（`31.1` 万条）**：`怎么个事`、`遇得到`、`按理来说`、`有毛病吧`、`把爱传递`、`干得漂亮`、`恶狠狠地` 等真实高频口语习语与 `先哲之石` 等生僻未验证占位词共享硬编码 `weight == 100`，使 Tier 0 缺少区分度。

同时，`EngineResources/msime-pinyin.db` 的精确 SHA-256 由 `resources/editions/shurufa.lock.json` 与 `ResourceStore::verify` 强校验（且禁止在 `EngineResources/` 下混入锁外文件），而会话工作库 `user_data/dictionaries/<content_id>/msime-pinyin.db` 又承载着用户自造词与调频回放。

## Decision

在不改动 `msime-pinyin.db` 原始字节与资源锁哈希、不干扰用户学习写库的前提下，建立「**离线构建 `SLXC` v1 校准边车 + `PinyinDatabase` 读边界零分配叠加**」架构：

1. **`SLXC` v1 二进制校准表（`crates/engine/src/dictionary/calibration.rs`）**：
   - 文件随 `msime-stroke.db` 一并置于 `EngineResources` 同级的 `language-dictionaries/shurufa-lexicon-calibration.bin`，由 `RuntimePaths` 自动解析并通过 `OnceLock<Mutex<LruCache<...>>>` 进程内只读共享（单测临时目录无该文件时返回 `None`，保持全部既有单测与零分配基准不变）。
   - 记录定长 `16` 字节 `(hash64: u64, raw_weight: i32, calibrated_weight: i32)`，按 `hash64` 严格升序排列；构建期在整库 `70` 万+ `(key, value)` 对上验证选定零碰撞 `seed`。
   - **尊重用户学习不变量**：运行时 `LexiconCalibration::apply` 比对当前行权重与 `raw_weight`——若用户已通过学习或导词改写权重（超过标定值或修改被降权词），立即退让保持用户值；仅对未改动的出厂行执行压制（`calibrated_weight == -1`）或更新权重。
2. **`PinyinDatabase` 读边界零分配注入（`crates/engine/src/dictionary/pinyin.rs`）**：
   - `QuanpinDictionary::new` 与 `ShuangpinDictionary::new` 统一经 `PinyinDatabase::open_for_runtime(paths)` 挂载校准表；
   - `visit_rows_until` 在行产出时原地过滤 `Suppress` 行并替换标定权重；`rows` 仅在本页出现逆序时执行原地稳定插入排序（零堆分配，严格保持同权 SQLite 行序）。
3. **三类离线清洗与标定准则（`crates/engine/examples/build_lexicon_calibration.rs`）**：
   - **单字零权幻影压制**：仅压制 `tbl_1_*` 中 `weight == 0`、该字主读音 `weight >= 20,000` 且当前音节最高权重 `>= 500` 的方言/讹误行（`947` 条），完整保留 `n/ng -> 嗯(0)` 零权叹词音节与 `weight == 1` 的《新华字典》真实生僻多音字（如 `参差 cen/ci`、`单于 chan`、`提防 di`、`龟裂 jun`）。
   - **组合伪词「`X的`」降权**：保留 `28` 个已词汇化代词/评价词（`我的/你的/他的/她的/它的/您的/谁的/有的/别的/是的/对的/好的/真的/假的/似的` 等），其余 `557` 条 `tbl_2_*` 高权 `*'de -> *的` 统一降权至 `80`。
   - **多字 `weight == 100` 语料对数线性标定**：依托 `GramDb::exact_phrase_ln_count` 读取万象 `.gram` 双数组 Trie 精确整串对数频次（含 `+14.0` 存储偏移校准，3 字要求 `>= 13.2`、4–5 字要求整串 `>= 13.5` 或全部重叠 3-gram 均 `>= 14.5`），将高频口语词平滑映射至 `600..16,000`（如 `遇得到 -> 701`、`把爱传递 -> 3573`、`有毛病吧 -> 4640`、`怎么个事 -> 8949`、`干得漂亮 -> 10866`、`按理来说 -> 11608`），而无语料实证的孤立占位词（`先哲之石`）保持 `100` 不动；故意排除 2 字 `weight == 100` 行，防止跨词边界高频二元字对（如「有时|得到」中的 `时得`）误升权。
4. **构建与装包门禁闭环（`build-shurufa.sh`、`stage-resources.sh`、`CMakeLists.txt`、`verify_shurufa_bundle.py`）**：构建时自动生成并暂存校准边车，CMake 装配与包校验脚本强制核验 `Contents/Resources/language-dictionaries/shurufa-lexicon-calibration.bin` 在位。

## Historical audit

- [端侧两阶段真实模型自动化评测门禁](../testing/2026-11-27-tier1-neural-end-to-end-eval-gate.md)：由 `tier1_eval` 417 例端到端门禁直接核验校准前后的 Tier 0 / Tier 1 命中率、违禁幻影数与延迟。
- [全拼两阶段纠错纪律、隔音偏移补偿与动宾离合词拆插恢复](2026-11-27-speculative-typo-discipline-and-separable-vo-recall.md)：校准边车将已验证高频口语短语从源头移出 `weight == 100` 占位态，同时保留对未验证 `weight == 100` 词条的运行时护栏，形成离线清洗与在线护栏双保险。

## Alternatives considered

### 1. 直接原地改写 `msime-pinyin.db` 并重签 `shurufa.lock.json`

- **最强理由**：SQLite 表内天然按新权重索引排序，运行时无需边车结构。
- **放弃原因**：每次微调校准曲线都要重写并分发 74 MB SQLite 二进制、破坏 `shurufa.lock.json` 与上游锁定基线的可审计性，且老用户已生成的 `user_data/dictionaries/<content_id>/msime-pinyin.db` 若不强制推倒重建就拿不到修复；全库统计表明 `99.9987%`（`311,390 / 311,394`）的 `weight == 100` 多字词在同键下位于前 16 行内，读边界轻量边车即可无损覆盖且立刻对存量代次生效。

### 2. 无差别压制全部 `tbl_1_*` `weight <= 1` 单字，或对 2 字 `weight == 100` 同样跑 `.gram` 升权

- **最强理由**：规则更简短。
- **放弃原因**：实测审计发现 `tbl_1_*` 中 `weight == 1` 的 `97` 条记录包含大量合法文言/书面多音字（`参差`、`单于`、`句读`、`巷道`、`倔强`、`龟裂`），仅 `weight == 0` 才是混入的方言/错读噪音；而 `.gram` 是字符级 n-gram 而非词边界模型，对 2 字词升权会把跨词边界高频相邻字误判为高权词。

## Consequences

- **单测与门禁实测**：`msime-engine` 全部 `1,587` 项单测（含 SQLite 页复用与零分配断言单测）零退化通过；`yudedao` 首屏净化为 `#1 遇得到 / #2 欲得到 / #3 于得到 / #4 欲得 / #5 育德`（彻底清除 `鱼的`/`雨的`/`鱼的倒`/`预测到*`），`me` 首屏彻底清除 `没`/`末` 噪音；`spoken-v1` Tier 1 `Top6` 进一步升至 **`98.3%`（`118/120`）**，`guardrails-v1` `unmeasured-dict` Tier 0 `Top1` 升至 **`80.0%`**、Tier 1 `Top6` 升至 **`100%`（`15/15`）**，`boundary-single`（144 例）与 `phantom-reading`（14 例）Tier 1 `Top6` 全线达 **`100%`**。
