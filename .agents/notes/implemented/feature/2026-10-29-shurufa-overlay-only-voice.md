# Agent Note: Shurufa 实时语音默认只在浮层预览

Status: implemented

## Problem

用户在 d00089c 试用中发现录音先把实时文字写进编辑框，切窗口后才转到浮层。当前本机及共享 `voice_input.stream_inline_preedit` 都是 true；控制器据此在失焦前使用 marked text。该表现来自已存在的偏好，不是跨窗口录音被取消。

## Decision

按用户明确要求，Shurufa 的已有 `stream_inline_preedit` 默认改为 false，本机和共享文档通过一次性迁移改为 false；不增加设置项、不改变其他 edition 默认值。实时结果从首个 partial 就更新浮层，最终结果仍经过既有整理与一次投递路径。已有控件只澄清“编辑框实时显示”与浮层预览的区别。

本决定取代[跨窗口录音](../../implemented/feature/2026-10-28-shurufa-single-voice-session.md)中 Shurufa 失焦前默认写 marked text 的选择；所有者、代次、复制与取消规则保留。迁移在获准安装时执行，截图审查前不改正在使用的本机配置。

## Alternatives considered

- 无条件硬编码 Shurufa 禁止 inline：能屏蔽所有旧配置，但绕过已有偏好与恢复默认流程；用户明确要求优先使用现有字段，因此采用默认值和一次性迁移。

## Testing

- Shurufa 原生与共享默认均关闭；其他 edition 保留原默认。
- 迁移只改变此字段，保存失败不写本机镜像/标记，重复安装不覆盖后续选择。
- 录音从首个 partial 起只更新浮层，原编辑框无 marked text、无中间提交；最终整理文本仅插入一次。
- 持久录音控制器与一次性迁移测试 2/2 退出 0，共享 Shurufa 默认值测试退出 0。原生浮层截图经监督者批准后，与其他已批准修复一次安装。

本机镜像继续通过 NSUserDefaults 的 boolForKey 读取显式值；新增默认值 helper 不直接向任意对象发送 boolValue。合成数组、字典和数据沿用系统读取行为，避免改变原有偏好兼容性。

## Consequences

仍沿用现有偏好；用户以后显式开启编辑框实时显示时，该已有能力仍可用。不修改其他出厂值。
