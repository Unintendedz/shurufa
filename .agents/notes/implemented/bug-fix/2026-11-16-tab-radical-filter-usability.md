# Agent Note: 修复 Tab 偏旁与笔画筛选的翻页残留、部件直觉匹配与长句排座

Status: implemented

## Problem

全拼和双拼在日常使用 Tab 笔画/偏旁筛选时（尤其在长句输入后试图按偏旁或笔顺找「第一个字」，或在候选窗翻页后再进入筛选，如 `daobi` → `dao'bi｜ps  2 / 3` 只剩 `倒必, 佻, 儔, 島, 軇, 鳥`），高频出现筛不到常用字而直接跳到生僻字的反直觉行为。排查确认由四处缺陷叠加导致：

1. **进出/编辑筛选码未重置已翻页码**：`Runtime::refresh()` 通过 `composition_held` 判断是否为不改变组字的旁路刷新（云联想、释义回包等）并保留当前页或钳制到末页。而 Tab 筛选不改 `editing_text` 与 `reading`，只改 `preedit`（`dao'bi` → `dao'bi｜` → `dao'bi｜ps`）。用户先翻页找字再按 Tab 筛选时，`composition_held` 误判为组字未变，把原本落在第 1 页的常用命中留在上一屏外，画面卡在第 2 页乃至末尾全是权重 0 生僻字的末页（九宫格筛选改 `nine_key_strokes` / `nine_key_single_character` 时也存在相同漏判）。
2. **长句筛选被大量 `keyboard_runner_up` 次选整句淹没或倒挂**：`decode_graph`（`terminal_beam = 64`）与隔音备选会产生数十条标为 `keyboard_runner_up` 挂在候选末尾的整句。进入筛选后：
   - 正常态下 `ordering::runner_up_order` 会把第 4 条及之后全部全键次选整句排在第 2～5 页，把短前缀词之外的单字（第一个字）挤到第 5～6 页生僻字边缘；两音节输入时合成次选整句（如 `倒必`）还会排进单字前面；
   - 若前排主整句全被筛掉、仅末尾的 `keyboard_runner_up` 整句命中，它留在所有权重 0 生僻单字之后，`runner_up_order` 的 `0..first_reading` 会把整串生僻单字垫在合法整句前面。
3. **部件筛选规则过窄**：旧实现 `chaizi::components_start_with` 只把拆字表每段拼音压成首字母做左前缀比对，导致四类高频直觉输入全部漏配并只留下生僻字：
   - **缺部首俗称**：绞丝旁 `纟` 不认 `j/jiao`（只认 `si`）、单人旁 `亻` 不认 `d/dan`、双人旁 `彳` 不认 `s/shuang`、反犬旁 `犭` 与反文旁 `攵` 不认 `f/fan`、立刀旁 `刂` 不认 `l/li`、双耳旁 `阝` 不认 `s/shuang`、四点底 `灬` 不认 `s/si`、秃宝盖 `冖` 不认 `t/tu`、单耳刀 `卩` 不认 `d/dan` 与 `e/er`、方框/国字框 `囗` 不认 `f/fang`、`g/guo`、`k/kuang`、三框 `匚` 不认 `k/kuang`、肉月旁 `月` 不认 `r/rou`、王/斜玉旁 `王` 不认 `y/yu`、三撇 `彡` 不认 `s/san`、`p/pie`、小字头 `⺌` 不认 `x/xiao`、春字头 `𡗗` 不认 `c/chun`；
   - **缺右偏旁与字底偏旁**：如立刀（`到, 利, 刻, 剑`）、页字旁（`顶, 领, 题`）、鸟字旁（`鸡, 鸭, 鹅`）、右耳刀（`都, 那, 部`）、反文（`放, 改, 教`）、欠字旁（`歌, 欢`）、心字底（`想, 思`）、四点底（`热, 照`）、皿字底（`盗, 盆`）等位于末部件，左前缀匹配一律漏掉；
   - **不支持偏旁拼音前缀/全拼**：输入 `jin`（金）、`shui`（水）、`zhu`（竹）、`jiao`（绞丝）等时，第 2 字母被当成第二个部件首字母或整字双部件缩略，甚至在 `push_filter_key` 里跳过不可接受字符只留畸形子串；
   - **缺独体部首字**：拆字表无拆法或仅拆笔画的常用独体字（`马`、`刀`、`口`、`心`、`车`、`牛`）无法按自身读音找到。
4. **纯 `x` 误当全字库通配笔顺**：`stroke_keys` 把单独的 `"x"` 或 `"xx"` 送进 `msime-stroke.db` 的 `GLOB '?*'`，命中全字库，导致 `Tab + x`（心字旁/穴宝盖/小字头）丧失筛选作用。

## Decision

从交互分页、部件索引、笔顺通配与筛选态整句排座四处协同修复：

1. **预编辑与九宫格筛选变化时回到首页（`crates/input-runtime/src/runtime.rs`）**：
   - `Runtime::refresh()` 的 `composition_held` 必须同时核对 `preedit == previous.preedit`、`nine_key_strokes == previous.nine_key_strokes` 与 `nine_key_single_character == previous.nine_key_single_character`。进入筛选、增删筛选码或退出筛选时，`self.highlighted` 归 `0` 回到第 1 页第 1 项；`preedit` 与组字均不变的云联想/释义刷新继续保留已翻开的页。
2. **升级拆字部件筛选接口为 `chaizi::matches_component_filter`（`crates/engine/src/local/chaizi.rs`）**：
   - 保留零磁盘读取的 `OnceLock` 构建方式，从 `chaizi.tsv` 与 `chaizi-chars.tsv` 为每个字按拆法部件数分组构建 `ComponentChain`：
     - **顺序首字母与全拼前缀匹配（`matches_chain`）**：每个位置去重并展开常见部首俗称别名（如 `ren`→`dan`、`si`（非 `wang` 四字头）→`jiao`、`chi+ren`→`shuang`、`quan+gou`→`fan`、`pu+wen`→`fan`、`dao`→`li`、`er+fu`→`shuang`、`biao+huo`→`si`、`bao`（非 `mian`）→`tu`、`jie`→`dan/er`、`shan`→`san/pie`、`yue`→`rou`、`wang`→`yu`、`kou+wei`→`fang/guo/kuang`、首位 `fang`→`kuang`、首位 `shang`→`xiao`、首位 `shou` 接 `fu`→`chun`），支持每部件输入首字母或任意长度拼音前缀（`k`、`kou`、`kz`、`kouzhi`、`jin`、`jiao`、`shui`、`zhu`）。
     - **后置（右/底）部首独立匹配与同音声旁排除（`trailing_radicals`）**：两部件及以上拆法的末位置中，明确后置部首（`ALWAYS_TRAILING_RADICALS` 及 `pu+wen`、`er+fu`、`biao+huo`、`quan+gou` 共现签名）直接纳入；易作形声声旁的部件（`NON_PHONETIC_TRAILING_RADICALS`：`li, yi, shi, yu, he, mi, si, tian, che, zhou, yang, er`）仅当其不等于 `chaizi-chars.tsv` 中该字去调读音时才纳入，从而命中 `动 l`、`袋 y`、`磨 s`、`紫 j/s` 而不误中 `理 l`、`亿 y`、`试 s`。
     - **跨中间笔画的首尾组合**：三部件及以上拆法支持 `code` 拆成两段分别前缀匹配首部件与后置部首（如 `岛 bs`/`baoshan`、`照 rh`）。
     - **常用独体部首字（`STANDALONE_RADICALS`）**：收录 152 个常用独体字与成字部首自身读音，与 `chaizi.tsv` 拆法并集生效（`马 m/ma`、`心 x/xin`、`牛 n/niu`）；不泛化扩展区等表外生僻字。
3. **纯 `x` 不查笔顺（`crates/engine/src/session/stroke_filter.rs`）**：
   - `stroke_keys` 要求码串至少含一笔具体笔画（`keys.bytes().any(|key| key != b'x')`）；单独 `x`/`xx` 仅走部件筛选，`hsx` 等组合中的 `x` 仍作任意一笔通配。
4. **筛选态整句收敛与次选归位（`InputSession::compact_filtered_sentences`）**：
   - 筛选保留后若存在完整消费组字的整句行：三字及以上长句最多保留前 `LONG_FILTERED_SENTENCE_LIMIT`（`2`）条整句，丢弃其余 `keyboard_runner_up` 尾词排列组合；两音节在存在主整句时仅留主整句，仅有 `keyboard_runner_up` 时仅当列表开头无精确整键词典词（`exact_dictionary_lead == 0`）才保留 1 条（有精确词典词时丢弃，如 `daobi｜ps` 下的 `倒必`）。
   - 保留的整句行清除 `keyboard_runner_up` 标记，凡 `fixed_position == 0` 的行统一按原相对顺序插回 `exact_dictionary_lead` 处，确保筛选后长句前两格即命中的整句、随后是短前缀词与首字单字，不会出现生僻单字垫在次选整句之前。

## Historical audit

- 部分取代 [Tab 笔画与部件筛选](../feature/2026-11-04-tab-stroke-filter.md)：该文保留进入条件、预编辑标记、`ranking_candidates` 保留机制及偏好/打包边界；本篇取代其「部件只比首字母且不算独体字」「`x` 单独也查笔顺」「筛选列表只做 `retain` 不收敛整句」三项事实，已在原笔记就地同步并互链。
- 区分于 [词网格指物代词、末列前缀多样性与次选排座](./2026-11-14-lattice-pronoun-diversity-and-seating.md)：正常输入态仍由 `ordering::runner_up_order` 在首页保 3 条整句 + 3 条前缀并将全部全键次选留在第 2 页起；本篇仅作用于进入 Tab 筛选后的 `mixed_candidates`（且完整原列表仍保存在 `ranking_candidates` 中供学习记名次与退格恢复）。

## Alternatives considered

- **无差别允许拆字表任意位置部件做子序列匹配**：最强理由是不需要维护 `ALWAYS_TRAILING_RADICALS` 与 `NON_PHONETIC_TRAILING_RADICALS` 词表。但以 `li` 为例，常见 88 个字里有 60 个以 `l` 声旁结尾（`理, 莉, 丽` 等），在 `li｜l` 下几乎无法收窄；区分显式后置部首与非同音声旁可同时命中 `利, 刻, 动` 并屏蔽声旁干扰。
- **把所有拆字表外的单字都按其拼音词典读音参与匹配**：最强理由是不必手写 152 个 `STANDALONE_RADICALS`。但 `msime-pinyin.db` 中大量扩展区或生僻异体单字不在拆字表里（如 `㠀`），按自身拼音首字母放行会把生僻字大量卷入筛选，违背「减少生僻字干扰」的目标。
- **继续修改 `ordering::runner_up_order` 在全局限制次选整句数**：最强理由是正常态和筛选态逻辑统一。但正常长句停顿精排和第 2 页次选找句依赖保留第 4 条及之后的整句候选；只在 `apply_stroke_filter` 后局部收敛能把影响严格限制在 Tab 筛选态，且退格删光筛选码立即恢复完整次选。

## Consequences

- **直觉匹配与前排命中显著改善**：翻页后再按 Tab 或改筛选码固定回到第 1 页第 1 项；`jin`/`shui`/`zhu`/`jiao`/`dan`/`fan` 等俗称与偏旁全拼、右/底部首和独体字均可直达；长句筛选时第 1 页即可看到整句、短词与首个单字，双音节已有精确词不再出现 `倒必` 这类合成次选；`Tab + x` 真正筛心/穴/小部件。
- **筛选态局部收敛范围与可逆性**：收敛仅改 `mixed_candidates`，`ranking_candidates` 仍保留原完整列表，学习记名次和退格退出筛选不受影响。
- **验证覆盖**：
  - `crates/input-runtime/src/tests.rs` 新增 `preedit_filter_change_resets_open_page_while_same_preedit_keeps_it`（验证 `preedit` 变化回第 1 页而 `preedit` 不变的旁路刷新留在原页）。
  - `crates/engine/src/local/chaizi.rs` 更新 `matches_component_filter_covers_initials_full_pinyin_aliases_trailing_and_standalone`（覆盖顺序前缀/全拼、俗称别名、右/底部首与声旁区分、首尾组合与独体部首字）。
  - `crates/engine/src/session/tests/stroke_filter.rs` 新增 `radical_filter_accepts_full_pinyin_aliases_trailing_and_pure_x` 与 `filtered_long_sentence_caps_runner_ups_and_promotes_tail_sentences`。
