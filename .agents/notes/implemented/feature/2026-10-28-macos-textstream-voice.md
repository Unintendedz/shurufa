# Agent Note: macOS 本地流式识别 TextStream 作为默认语音服务

Status: implemented

## Problem

macOS 宿主的本地识别只有 `msime-voice-local`（sherpa-onnx）一条路，需要用户先下载模型目录，而且识别和整理的效果不如用户平时在 OpenLess 里用的 TextStream（Qwen3-ASR + MLX，边说边出字）加大模型整理。要让 Shurufa 在本机直接获得同样的体验，并且不让输入法进程自己持有模型。

## Decision

- 新增 ASR 服务 `textstream`，并作为未设置时的默认服务。识别在独立进程里：`platforms/macos/textstream/shurufa-textstream` 启动 `textstream_worker.py`，协议逐条兼容 `msime-voice-local`，所以宿主复用 `MSIMELocalVoiceRequest`/`MSIMELocalVoiceHelper`，只换可执行文件和模型目录。`MSIMELocalVoiceHelper` 变成按通道各一个单例（`shared` 与 `textStream`），两种本地服务切换时各用各的进程，互不顶替。
- 运行环境不进包：`install-runtime.sh` 随包，由设置窗口的“安装语音模型”调用，用 uv 建 Python 3.12 虚拟环境并按带哈希的 `requirements.lock` 安装，模型固定为 `Qwen/Qwen3-ASR-1.7B`（Hugging Face `7278e1e70fe206f11671096ffdd38061171dd6e5` / ModelScope `d69410f1c275f2b0fa60cbb9960edfcdb0ae0aec`，Apache-2.0），装在 `~/Library/Application Support/Shurufa/textstream/model`。安装脚本先清理模型目录里不在当前清单中的残留文件（防止从旧版 `0.6B` 升级时遗留的 `model.safetensors` 被 `mlx_qwen3_asr` 一并加载导致权重覆盖冲突，同时提前释放空间）；OpenLess 已装同哈希文件时 APFS 克隆，否则默认优先从 ModelScope 固定修订下载并在失败时回退 Hugging Face（显式设置 `HF_ENDPOINT` 时只走该地址），全部文件逐一校验 SHA-256 后写 `.ready`。宿主 `MSIMETextStreamReady` 按 `.ready`、`config.json`、`model.safetensors.index.json` 和两个权重分片（`model-00001-of-00002.safetensors`、`model-00002-of-00002.safetensors`）判定就绪，worker 在存在 `model.safetensors.index.json` 时同时拒绝残留 `model.safetensors`，不用 `msime-model.json`。
- 解码与资源预算：`textstream_worker.py` 将 Metal 内存软上限调为 `min(8 GiB, memory_bytes // 3, recommended_bytes)` 以容纳 `1.7B` 参数；解码间隔仍为 0.5 秒（textstream-asr 默认 2.5 秒）。升级到 `Qwen3-ASR-1.7B` 后在本机（Apple M3 Max / 64 GiB）跑 `test_protocol.py`（`chunk_sec=0.5`）实测：11 项协议用例全过，加载并 `start` 耗时 `0.39` 秒、开口后首个 partial `0.21` 秒、`finish` 到 `final` `0.15` 秒、三句合成普通话音频 CER `0.0`、流式期间单核 CPU 占 `19.3%`、子进程峰值内存 `peak_rss_mb_rusage = 4232 MB`（`ps` 采样 RSS 峰值 `271 MB`）。
- 整理沿用现有 `polish_*` 链路，`textstream` 下默认开启（用户没存过开关时按开，仍要求有凭据才发请求）；整理失败或超时原样上屏识别文字。新增只能调小的 `polish_timeout_ms` 选项，仅为测试超时路径。
- 从 OpenLess 导入是输入法设置窗口里由用户触发的动作：读登录钥匙串里 `com.openless.app`/`credentials.v2` 的凭据（系统会弹授权框），读 `preferences.json`、`style-packs.json`，把当前整理风格的提示词写进自定义方案 1 并选中。不读历史、录音、速记。提示词取值顺序固定：内置风格的用户覆盖值，其次 style-packs.json 里同 id 的包，最后 `styleSystemPrompts`。

### 设置编辑与模型列表

所有通过 `MSIMEPresentWindow` 展示的原生设置窗口安装标准编辑菜单，Command+V/C/X/A/Z 经 AppKit 响应链到当前文本框。保留系统文本框的右键菜单和安全文本框对复制密钥的保护。测试进程使用独立剪贴板，不读写用户剪贴板。

整理模型行提供「获取模型列表」按钮，后台线程复用 `msime_client_ai_models` 请求当前地址旁的 `/models`。按 JSON 的 `data[].id` 确定性读取并保留服务顺序，不猜模型、不内置列表；选择后走原保存入口，手动填写始终可用。请求期间禁用按钮；地址或密钥改变后丢弃旧结果。原有 FFI 调用保留粗粒度错误码，新界面用可选 `detailed_errors` 获得中文鉴权、不支持、超时提示，不透传服务原文或密钥。

本地假服务覆盖方舟 `/api/v3`、DeepSeek 根路径、硅基流动 `/v1` 三种路径、鉴权和不支持状态、空列表与响应体超时。此覆盖证明兼容接口路径和格式，不代表各供应商的实时可用性。

## Alternatives considered

- 把 MLX 识别直接放进输入法进程：最强的理由是省一个进程和一层协议。但输入法被加载进每个接受文字的应用，模型和 Metal 内存会拖慢前台应用，崩溃会带走打字；AGENTS 和 `msime-voice-local` 的设计都是为了避免这一点。
- 新写一套宿主请求类给 TextStream：最强的理由是不碰 `LocalVoiceRequest.mm`。但协议相同，重写只会复制进程管理、超大行和取消处理，两份实现日后要分别修。
- 继续沿用 `Qwen/Qwen3-ASR-0.6B`：最强的理由是体积只有约 1.9 GB、内存更小；但同构开源系列里 `1.7B` 在识别准确率与中英混说/方言上表现更好，且 worker 已有空闲 120 秒自动释放模型的设计，在 16 GiB 及以上 Mac 上换用 `1.7B` 无需改任何编解码代码。
- 改为闭源 `Qwen-Audio-3.0-ASR`：它是阿里云百炼上的商用闭源模型，没有开源权重，只能走云端 API，无法继续提供本机离线流式识别；需要使用百炼云端识别的用户仍可在设置里选「阿里云百炼 · 千问」。
- 把 Python 运行环境打进 app 包：最强的理由是首次使用免安装。但依赖含 Metal 运行库约 100 MB，模型约 4.7 GB 无论如何要另装，签名和公证也要处理 Python 的二进制；先走随包脚本安装。
- 用 JSON 外的启发式去猜 OpenLess 凭据载荷：不做。凭据载荷按 JSON 对象树里的键名读取（`ark.api_key` 等），读不到就报错，不猜。

## Consequences

默认服务从 `local` 变成 `textstream`：新用户未安装模型时，第一次按语音热键会提示去设置里安装；已显式选择 `local` 的用户不受影响。`textstream` 需要 `uv`（Homebrew 或 ~/.local/bin）和网络（约 4.7 GB 模型文件）。之前只装过旧版 `0.6B` 单文件的环境升级后，因 `MSIMETextStreamReady` 改查分片文件而显示「未安装」，在设置窗口点一次「安装语音模型」即可自动清理旧单文件、复用共用的分词器与配置文件并补齐 `1.7B` 分片。凭据载荷的真实结构在开发机上没能核对（读取需要用户在钥匙串授权框点“允许”），解析按键名在对象树里查找，结构与预期不符时导入会报“没有完整的 Ark 配置”，需要按真实结构调整 `arkFieldsFromPayload`。
