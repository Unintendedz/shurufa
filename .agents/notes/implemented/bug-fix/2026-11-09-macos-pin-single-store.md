# Agent Note: macOS 置顶只留 Engine 一份

Status: implemented

## Problem

维护者报告：Shurufa 里每次打 `you`，候选窗第一格是置顶的「有」，亮着的却是第二格的「又」，空格上屏的也是「又」。

置顶原来存了两份：
- macOS 宿主一份：NSUserDefaults 的 `MSIMEClientPinnedCandidates`，按预编辑记一张有序表，画候选窗、数字键、展开网格、右键菜单之前都用它把置顶词挪到前面（`MSIMEReorderedPinnedCandidates`）。
- Engine 一份：`pinned_candidates` 表加一次 `force_top` 抬权重（`session/candidates.rs` 的 `pin_candidate`）。调频的保护靠这份（[手动置顶不被自动调频挤掉](2026-11-06-manual-pin-survives-frequency-learning.md)），视图里的 `highlighted` 和空格上屏（`MSIME_COMMIT_CANDIDATE`）也跟着 Engine 的顺序。

本机数据显示，用户日志 `msime_user.db` 和两份工作词库在 11-08 09:51:07 这一秒同时重建，这是清除学习数据（`reset_learned_data`）的特征。重建后 Engine 那份置顶清空了，宿主那份留着，`you`、`de`、`le`、`ma`、`na'ge`、`shi`、`w`、`z'd` 从此只在宿主那份里有。Engine 查不到 `you` 的置顶，`promote` 调频选一次「又」就把它写成「当前最高 + 500」（日志里「又」17850513，「有」17850013）。于是 Engine 的首行和高亮都是「又」，宿主只把「有」挪到了第一格，没挪高亮。

在用户目录的副本上经 C 接口核过：停顿后精排和本地神经排序的四种开关组合下 Engine 都给出 `*又 有 …`，精排一行都没动；在副本里补一次 Engine 置顶后再选「又」三次，「有」一直在首位、一直高亮。

## Decision

宿主不再自己记置顶，也不再挪候选：删掉 `MSIMEClientPinnedCandidates` 的读写和 `MSIMEReorderedPinnedCandidates` 的全部调用，候选窗、数字键、方向键、释义、装饰词、展开网格和删除快捷键的位置一律按 Engine 给的顺序。右键菜单的「置顶」只派发 `pinGeneration:`，菜单始终显示「置顶」，与 Linux 相同（`CandidateActionPolicy.h`）。

NSUserDefaults 里旧的 `MSIMEClientPinnedCandidates` 不迁移也不删：没有代码再读它，留着无害。本机 8 条只在宿主里的置顶，有 6 条 Engine 的首选本来就是同一个词；`you`→「有」、`zd`→「知道」两条要在候选窗里各重新置顶一次。

## Alternatives considered

- **宿主挪位时把高亮一起挪，清除学习数据时同时清 NSUserDefaults**：改动最小，保留「取消置顶」和每个编码多条置顶的顺序。但两份数据仍然各写各的：清除只是这次让它们分家的一条路，导入习惯（`habits.rs` 合并 `pinned_candidates`）、删除词条（`removal.rs` 同时撤掉置顶）、换机器都同样会让两边对不上，而且空格上屏和调频永远只认 Engine，宿主挪出来的顺序本身就和上屏不是同一个词。
- **开机时把宿主的表重放进 Engine 再删掉**：用户感觉不到变化。但 Engine 现在没有按「编码 + 词」置顶的入口，只能按某一代候选的下标置顶，要新开一条 host-api 动作，只为迁移一次本机 8 条数据；`pinned_candidates` 每个上下文也只存一条，宿主那种多条有序的表本来就放不进去。
- **按 [Windows 对齐笔记](../feature/2026-10-10-windows-mac-parity-leftovers.md)里的共享机制做全套**（多条有序置顶、视图带 `pinned`、取消置顶动作）：这是让所有宿主都有「取消置顶」的正路，但要定「取消时撤不撤词频抬高」这个产品决定，和这个缺陷无关，这次不做。

## Consequences

- 画面第一格、高亮、空格上屏、数字键 1 从此是同一个词。
- macOS 失去「取消置顶」。旧的「取消置顶」只撤宿主那份顺序，Engine 的抬高和记录都不撤，被取消的词通常仍排在前面，所以实际损失很小；要让别的词排到前面，置顶那个词或用固定排位。
- 每个编码只能置顶一个词，后置顶的替换先前的（`record_pinned_candidate` 是 upsert），与其他宿主一致。
- 测试：`shortcut-test` 里写入一条旧的 `MSIMEClientPinnedCandidates` 记录后，第一格仍是 Engine 的首行「测试」，第二格「布局」的菜单是「置顶」；这三条断言在旧实现上失败。`shortcut` 这个 CTest 在未改动的 `main` 上就停在第 14450 行的外部皮肤描边颜色断言，与本改动无关；本机临时跳过那一行后整套通过。
