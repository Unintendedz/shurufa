# Agent Note: Shurufa 本机构建用固定的自签名证书签名

Status: implemented

## Problem

`platforms/macos/build-shurufa.sh` 原来按 ad-hoc 签名（`codesign --sign -`）。ad-hoc 签名的 designated requirement 就是这次构建的 `cdhash`，每次重编都变。麦克风、辅助功能这类 TCC 授权和钥匙串条目的访问控制都按 designated requirement 认应用，所以每次重编重装之后，系统都把 Shurufa 当成一个新应用，要求重新授权。语音组合键依赖辅助功能授权，授权失效时又会回到被修过的那个问题（见 [语音组合键在系统层吞掉](../bug-fix/2026-10-31-macos-voice-shortcut-event-tap.md) 的 Consequences）。

## Decision

登录钥匙串里放一张本机自签名的代码签名证书「Shurufa Local Signing」，`build-shurufa.sh` 按它的 SHA-1 签名。这样签出的 designated requirement 是 `identifier "dev.shurufa.inputmethod.Shurufa" and certificate leaf = H"<证书 SHA-1>"`，只要证书不换，重编后授权照样有效。

- 证书由 `platforms/macos/scripts/create-shurufa-signing-identity.sh` 建：用系统自带的 LibreSSL 生成 RSA 2048 密钥和 10 年有效期、`extendedKeyUsage = codeSigning` 的自签名证书，打成 PKCS#12 后用 `security import -T /usr/bin/codesign` 导入，codesign 用私钥不弹窗。钥匙串里已有同名证书时脚本什么也不做。
- 证书不加信任设置。`codesign` 用不受信任的自签名身份照样能签，`codesign --verify --strict` 也通过；`security find-identity -v` 不列它，这是预期的，不影响签名。
- 签名身份按 SHA-1 传给 `codesign`，不按名字，避免同名证书并存时报 `ambiguous`。证书名可以用 `SHURUFA_SIGNING_NAME` 覆盖。
- 钥匙串里没有证书（新的 Mac）时 `build-shurufa.sh` 先自己跑建证书的脚本再签名；建不成才退回 ad-hoc 并在 stderr 说明，不让构建失败。

## Alternatives considered

- Apple Developer ID 证书：designated requirement 同样稳定，还能公证。但需要付费的开发者账号，本机没有；只为本机自用的输入法不值得。
- 继续 ad-hoc，每次重编后手动重新授权：零配置。但每次重装都要去系统设置里删掉再加回授权，语音组合键在授权失效期间不可用，维护者已经明确不接受。

## Consequences

收益：重编重装 Shurufa 不再丢麦克风、辅助功能等授权。

代价：第一次换成证书签名时，designated requirement 从 `cdhash` 变成证书，需要再授权一次。证书和私钥只在这台机器的登录钥匙串里，换机器或删掉证书后要重新跑脚本，新证书的 SHA-1 不同，又要再授权一次。签名没有 Team ID，不能公证，也不能分发给别人。
