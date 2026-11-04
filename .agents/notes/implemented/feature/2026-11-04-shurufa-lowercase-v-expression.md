# Agent Note: Shurufa 恢复计算与数字，从全拼的小写 v 进入

Status: implemented

## Problem

维护者想要搜狗拼音的 V 模式：在全拼里直接打 `v`，后面接算式、数字或日期，候选给出结果、中文大小写数字、大写金额和日期格式。搜狗只在双拼下才用 Shift+V，因为双拼占了 `v` 键；全拼没有以 v 开头的音节，小写 `v` 是空着的。

引擎早就有 V 模式（`crates/engine/src/local/expression.rs`：算式、中文数字、金额、`YYYY.M.D` 日期、单位换算），但只认 Shift+V。[Shurufa 对齐系统拼音](../simplification/2026-10-29-shurufa-system-pinyin.md) 又在运行时副本里关掉了包括计算在内的全部快捷模式和 Shift+字母总开关，所以 Shurufa 里完全打不出 V 模式。

维护者说明：要小写 `v`；函数、`%` 当百分号这类和搜狗的差别他很少用到，不必照搬，怎么方便怎么来。

## Decision

- 引擎的 `LocalModeOptions` 新增 `expression_lowercase_entry`（宿主侧 `EngineOptions::local_expression_lowercase_entry`），默认关。它和 `expression` 都开着、方案是全拼、没有组字时，小写 `v` 进入 V 模式（`InputSession::local_mode_for_lowercase_v`）。双拼的 `v` 是声母或韵母（小鹤 `v` 是 zh），五笔的 `v` 是编码，都不进入。这个入口不经过 Shift+字母总开关 `shift_letter_entry`。
- 预编辑和原样上屏照用户打的写：`v12*` 回车上屏 `v12*`，只有 `v` 时空格上屏 `v`。
- 小写 `v` 后面紧跟字母（`vi`、`vs`、`video`）时，`v` 和这枚字母一起转回普通组字。这和 Shift 入口后再打一个大写字母的旧规则走同一个分支（`handle_local_character` 里 `entry.len() == 1` 的那段），结果和没有这个入口时逐键打出来的一样，测试逐键比两种会话的返回值、预编辑和候选。数字之后的字母照旧是单位（`v3jin'g`）。
- Shurufa 的运行时策略 `apply_shurufa_input_policy` 把 `local_modes.expression` 固定为开，host-api 只对 `shurufa` 版本把 `local_expression_lowercase_entry` 设为开（与 `local_url` 同一处）。Shift+字母总开关仍然关着，所以 Shift+V 还是大写字母，V 模式只有小写 `v` 一个入口。设置页不加开关，存储的偏好文档不变。
- 其他版本（full 等）不受影响：`local_expression_lowercase_entry` 恒为关，V 模式照旧只认 Shift+V、照旧由偏好控制。
- 功能本身不改：算式、数字、日期、单位换算的行为与 Shift+V 时完全一样；`%` 仍是取余，不加 `sqrt`、`sin` 这类函数，也不加罗马数字、千分位和更多日期写法。

## Alternatives considered

### 不另加选项，在所有版本里让小写 v 都能进入 V 模式

改动最少，full 版用户也能用上。不采用：full 版的 Windows TSF 在 Server 回答之前就要给按键分类，它靠键击缓冲第一个字符是不是 `V` 判断是否在 V 模式（`platforms/windows/tsf/Global/LocalModeKeyPolicy.h` 的 `IsExpressionModeComposition`），而且 TIP 不知道当前是全拼还是双拼，小写 `v` 之后的数字会被当成选词；full 版开着中英混输时，以 v 开头的英文词也会多一次转换。这两件事都要单独验证，Shurufa 只跑在 macOS 上，宿主按快照的 `spelling_symbols` 路由数字和运算符，不需要改宿主。

### 给 Shurufa 加一个设置开关

能让维护者随时关掉。不采用：维护者要的就是开着，`v` 只占全拼里空着的位置，后接字母时手感和原来一样；Shurufa 的设置页已经收掉了全部快捷模式开关，为一项加回整套偏好、原生设置控件和保存校验不划算。

### 同时恢复 Shift+V

和 full 版一致。不采用：Shift+V 要靠 Shift+字母总开关，打开总开关就会让 Shurufa 里其他 Shift+字母模式的问题重新出现；小写 `v` 已经够用，Shift+V 留给大写字母。

### 照搬搜狗的函数、百分号和罗马数字

能和搜狗一一对上。不采用：维护者明确说很少用；`%` 改成百分号还会改变现有取余的含义，函数名要把字母放进 V 模式的输入里，与单位换算的字母规则交叉，等到真要用时再单独做。

## Consequences

- Shurufa 全拼里打 `v` 再接数字、运算符就是 V 模式；`v` 后直接打字母与改动前一样。以前全拼的 `v` 只开一个没有候选的组字，现在它后面的数字和运算符成了算式；只打 `v` 再按空格仍上屏 `v`。
- V 模式开着时引擎在后台加载一次 rink 的单位定义（`units::warm_up_in_background`），Shurufa 进程因此多占一些内存。
- 新入口是引擎能力位，其他宿主想用时要先确认自己按 `spelling_symbols` 路由数字和运算符；Windows 还要把小写 `v` 告诉 TIP 才能用。

## Testing

- 引擎 `session::tests`：`lowercase_v_opens_the_expression_mode_in_quanpin`（进入、算式结果、大写金额上屏、回车原样上屏小写、只有 `v` 时上屏 `v`、总开关关着时 Shift+V 不进入）、`a_letter_after_lowercase_v_types_the_letters_as_before`（`vi`、`vs`、`vV`、`video` 与没有入口的会话逐键一致；去掉回退分支时该测试失败）、`lowercase_v_needs_quanpin_an_empty_composition_and_both_switches`（组字中的 `lv`、两个开关缺一、小鹤双拼都不进入）。
- host-api `shurufa_tests::removed_input_modes_stay_unreachable_after_legacy_preferences_and_updates`：Shurufa 会话的 `local_expression` 和小写入口都开着，Shift+U/T/K/E/M/J/Y/R/V/C/S、`/`、`@` 仍不进入任何模式，`v1+2` 进入 V 模式且首选是 `3`。
- `cargo test -p msime-engine` 与 `-p msime-host-api --lib` 里另有 4 个失败（golden 的 `scripted_scenarios_match_the_reference`、`listing_reports_every_pack_and_checks_the_state_root`、`a_command_gets_the_local_time_to_the_minute`、`shurufa_macos_voice_hotkey_survives_runtime_validation`），去掉本次改动在同一提交上照样失败，与本次无关。
