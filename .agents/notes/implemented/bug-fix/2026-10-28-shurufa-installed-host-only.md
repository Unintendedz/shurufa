# Agent Note: Shurufa 只运行安装目录里的输入法服务

Status: implemented

## Problem

安装脚本保留旧进程，把备份保存成完整 `.app`，也没有注销构建目录。现场同时存在安装目录的 PID 2439 和构建目录的 PID 36906；LaunchServices 登记了相同 bundle id 的两份程序，IMK 激活只到构建目录进程。不能仅由这条证据断言所有窗口切换症状都来自重复登记。

## Decision

Shurufa 常驻服务只接受 `~/Library/Input Methods/Shurufa.app` 和 `/Library/Input Methods/Shurufa.app`（解析符号链接后精确比较）。构建完成后注销产物；误启动产物时再次注销并退出，不创建 IMKServer。只读身份与注册查询不启动服务。安装前枚举同 bundle id 的登记、精确核对进程可执行路径，注销全部副本并结束全部 Shurufa 主进程。备份可逆地改为非应用目录，并把 `Contents` 改为 `SavedContents`，保留全部旧文件。安装保留构建产物原签名，避免无意义重签造成身份漂移。

## Alternatives considered

- 只杀构建目录的进程：即时有效，但下一次系统仍会按登记拉起它。
- 只添加 `.metadata_never_index`：减少 Spotlight 索引，但不能阻止 LaunchServices 显式注册或误启动，不足以作为运行边界。
- 只改备份的 `.app` 扩展名：修改最小，但包内仍有完整应用结构，因此同时改内部目录名，恢复时反向改名即可。

## Testing

原生启动路径测试覆盖用户级、系统级、构建和备份路径；构建目录无常驻服务器；安装后登记和进程只指向安装目录，CDHash 与构建产物一致。测试不选择系统输入源，也不注入键盘事件。

## Consequences

系统实际的跨应用切换手感仍交由用户验收。仅对 Shurufa 生效，其他 edition 保持原来的开发启动路径。

故障时段 06:20–06:50 的日志只有构建目录 PID 36906 的 82 条激活/停用事件，首条在 06:32:41。用户说恢复的时间早于本轮首次修改和注销；没有捕获当时的 TIS 选择失败或连接拒绝，故重复登记与症状的直接因果仍未确认。按应用模式记忆默认是 app，没有证据表明它阻止系统选择输入源，本轮不改它。
