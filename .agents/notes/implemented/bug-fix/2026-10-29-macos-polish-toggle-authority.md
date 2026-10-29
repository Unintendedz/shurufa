# Agent Note: macOS 明确的整理开关优先于旧字段

Status: implemented

## Problem

原生设置页关闭整理只保存 `MSIMEClientVoicePolish=false`，旧 `MSIMEClientVoicePolishText` 仍可能为 true。系统语音、HTTP、豆包和本地模型的请求使用两者的 OR，导致关闭后仍向整理服务发送转写；TextStream 已单独同步两字段，未受此问题影响。

## Decision

原生请求参数在发现明确的布尔整理开关时，将 `polish_enabled` 和 `polish_text` 统一为该值。缺少明确开关时保留已有兼容默认值，不改出厂偏好。原生设置保存也同步旧字段，避免重新导出矛盾状态。共享 Rust 与其他平台的默认值和配置格式保持不变。

本篇补充[TextStream 语音](../feature/2026-10-28-macos-textstream-voice.md)已有的开关权威规则，将原生关闭操作落实到所有本机识别通道。

## Alternatives considered

只在保存时改旧字段可以修复新的关闭操作，但此前已经保存的 false/true 冲突仍会发送转写；因此请求边界同时尊重明确开关。

## Consequences

用户关闭整理后不会被旧字段重新开启。合成偏好回归覆盖五种本机识别通道，以及缺少显式值时的原有默认行为；不需要真实凭据、录音或网络请求。
