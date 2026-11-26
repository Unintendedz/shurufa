//! 编译期零 I/O 现代开发者、AI 与 Apple 生态标准英文词表：
//! 修正随包 `msime-english.db` 中缺失、`weight = 0` 或仅剩全小写的高频技术术语大小写与排序权重，
//! 供 [`super::EnglishDictionary`] 与码表无关句内中英混输快速检索使用。

/// 单条规范化英文词条。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CanonicalTerm {
    pub word: &'static str,
    pub display: &'static str,
    pub weight: i64,
}

/// 全局规范化英文词表，严格按 `word` ASCII 字节序升序排列以支持二分与前缀范围切片。
/// 仅收录明确的品牌、专有框架/工具与技术缩写，绝不收录如 `review`、`rust`、`test`、`apple`、`python`、`mac` 等通用英文小写词。
pub(crate) const CANONICAL_TERMS: &[CanonicalTerm] = &[
    CanonicalTerm {
        word: "abi",
        display: "ABI",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "acl",
        display: "ACL",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "aes",
        display: "AES",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "ai",
        display: "AI",
        weight: 120_000_000,
    },
    CanonicalTerm {
        word: "airdrop",
        display: "AirDrop",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "airplay",
        display: "AirPlay",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "airpods",
        display: "AirPods",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "anthropic",
        display: "Anthropic",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "api",
        display: "API",
        weight: 110_000_000,
    },
    CanonicalTerm {
        word: "appkit",
        display: "AppKit",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "appstore",
        display: "AppStore",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "archlinux",
        display: "ArchLinux",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "ascii",
        display: "ASCII",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "aws",
        display: "AWS",
        weight: 80_000_000,
    },
    CanonicalTerm {
        word: "azure",
        display: "Azure",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "bitbucket",
        display: "Bitbucket",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "carplay",
        display: "CarPlay",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "cd",
        display: "CD",
        weight: 100_000_000,
    },
    CanonicalTerm {
        word: "chatgpt",
        display: "ChatGPT",
        weight: 160_000_000,
    },
    CanonicalTerm {
        word: "ci",
        display: "CI",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "clang",
        display: "Clang",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "claude",
        display: "Claude",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "cli",
        display: "CLI",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "clickhouse",
        display: "ClickHouse",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "clion",
        display: "CLion",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "clippy",
        display: "Clippy",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "cloudflare",
        display: "Cloudflare",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "cloudkit",
        display: "CloudKit",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "cmake",
        display: "CMake",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "colab",
        display: "Colab",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "copilot",
        display: "Copilot",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "coredata",
        display: "CoreData",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "coreml",
        display: "CoreML",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "cors",
        display: "CORS",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "cpu",
        display: "CPU",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "crud",
        display: "CRUD",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "csrf",
        display: "CSRF",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "css",
        display: "CSS",
        weight: 90_000_000,
    },
    CanonicalTerm {
        word: "csv",
        display: "CSV",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "cuda",
        display: "CUDA",
        weight: 80_000_000,
    },
    CanonicalTerm {
        word: "cudnn",
        display: "cuDNN",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "cursor",
        display: "Cursor",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "db",
        display: "DB",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "debian",
        display: "Debian",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "deepseek",
        display: "DeepSeek",
        weight: 150_000_000,
    },
    CanonicalTerm {
        word: "deno",
        display: "Deno",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "devops",
        display: "DevOps",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "dns",
        display: "DNS",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "docker",
        display: "Docker",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "dom",
        display: "DOM",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "duckdb",
        display: "DuckDB",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "eslint",
        display: "ESLint",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "fastapi",
        display: "FastAPI",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "fedora",
        display: "Fedora",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "ffi",
        display: "FFI",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "gcc",
        display: "GCC",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "gcp",
        display: "GCP",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "gdb",
        display: "GDB",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "gemini",
        display: "Gemini",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "gguf",
        display: "GGUF",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "gitea",
        display: "Gitea",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "github",
        display: "GitHub",
        weight: 150_000_000,
    },
    CanonicalTerm {
        word: "gitlab",
        display: "GitLab",
        weight: 80_000_000,
    },
    CanonicalTerm {
        word: "gitops",
        display: "GitOps",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "golang",
        display: "Golang",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "gpu",
        display: "GPU",
        weight: 105_000_000,
    },
    CanonicalTerm {
        word: "graphql",
        display: "GraphQL",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "grpc",
        display: "gRPC",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "healthkit",
        display: "HealthKit",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "hmac",
        display: "HMAC",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "homebrew",
        display: "Homebrew",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "homekit",
        display: "HomeKit",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "html",
        display: "HTML",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "http",
        display: "HTTP",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "https",
        display: "HTTPS",
        weight: 90_000_000,
    },
    CanonicalTerm {
        word: "huggingface",
        display: "HuggingFace",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "iaas",
        display: "IaaS",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "icloud",
        display: "iCloud",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "ide",
        display: "IDE",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "imac",
        display: "iMac",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "intellij",
        display: "IntelliJ",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "io",
        display: "IO",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "ios",
        display: "iOS",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "ipad",
        display: "iPad",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "ipados",
        display: "iPadOS",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "ipc",
        display: "IPC",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "iphone",
        display: "iPhone",
        weight: 120_000_000,
    },
    CanonicalTerm {
        word: "iterm",
        display: "iTerm",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "javascript",
        display: "JavaScript",
        weight: 110_000_000,
    },
    CanonicalTerm {
        word: "jax",
        display: "JAX",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "jdk",
        display: "JDK",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "jetbrains",
        display: "JetBrains",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "js",
        display: "JS",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "json",
        display: "JSON",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "jsonl",
        display: "JSONL",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "jupyter",
        display: "Jupyter",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "jvm",
        display: "JVM",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "jwt",
        display: "JWT",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "kafka",
        display: "Kafka",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "kms",
        display: "KMS",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "kubernetes",
        display: "Kubernetes",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "langchain",
        display: "LangChain",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "linux",
        display: "Linux",
        weight: 120_000_000,
    },
    CanonicalTerm {
        word: "lldb",
        display: "LLDB",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "llm",
        display: "LLM",
        weight: 85_000_000,
    },
    CanonicalTerm {
        word: "llvm",
        display: "LLVM",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "lora",
        display: "LoRA",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "macbook",
        display: "MacBook",
        weight: 85_000_000,
    },
    CanonicalTerm {
        word: "macos",
        display: "macOS",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "mapkit",
        display: "MapKit",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "mcp",
        display: "MCP",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "mlx",
        display: "MLX",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "mongodb",
        display: "MongoDB",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "mr",
        display: "MR",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "mvc",
        display: "MVC",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "mvvm",
        display: "MVVM",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "mysql",
        display: "MySQL",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "ndk",
        display: "NDK",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "neovim",
        display: "Neovim",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "nextjs",
        display: "NextJS",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "nginx",
        display: "Nginx",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "nodejs",
        display: "NodeJS",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "nosql",
        display: "NoSQL",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "npu",
        display: "NPU",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "numpy",
        display: "NumPy",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "nuxtjs",
        display: "NuxtJS",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "oauth",
        display: "OAuth",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "objc",
        display: "ObjC",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "oidc",
        display: "OIDC",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "ollama",
        display: "Ollama",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "onnx",
        display: "ONNX",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "openai",
        display: "OpenAI",
        weight: 145_000_000,
    },
    CanonicalTerm {
        word: "openrouter",
        display: "OpenRouter",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "orm",
        display: "ORM",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "os",
        display: "OS",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "paas",
        display: "PaaS",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "pandas",
        display: "Pandas",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "pdf",
        display: "PDF",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "png",
        display: "PNG",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "pnpm",
        display: "pnpm",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "posix",
        display: "POSIX",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "postgresql",
        display: "PostgreSQL",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "pr",
        display: "PR",
        weight: 90_000_000,
    },
    CanonicalTerm {
        word: "prettier",
        display: "Prettier",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "py",
        display: "Py",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "pycharm",
        display: "PyCharm",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "pydantic",
        display: "Pydantic",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "pytest",
        display: "pytest",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "pytorch",
        display: "PyTorch",
        weight: 120_000_000,
    },
    CanonicalTerm {
        word: "qwen",
        display: "Qwen",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "rag",
        display: "RAG",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "raycast",
        display: "Raycast",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "rbac",
        display: "RBAC",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "redis",
        display: "Redis",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "rest",
        display: "REST",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "rpc",
        display: "RPC",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "rsa",
        display: "RSA",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "rustc",
        display: "rustc",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "rustrover",
        display: "RustRover",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "rustup",
        display: "rustup",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "saas",
        display: "SaaS",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "saml",
        display: "SAML",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "scipy",
        display: "SciPy",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "scss",
        display: "SCSS",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "sdk",
        display: "SDK",
        weight: 85_000_000,
    },
    CanonicalTerm {
        word: "serde",
        display: "Serde",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "sftp",
        display: "SFTP",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "sglang",
        display: "SGLang",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "sla",
        display: "SLA",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "slo",
        display: "SLO",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "sql",
        display: "SQL",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "sqlite",
        display: "SQLite",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "ssh",
        display: "SSH",
        weight: 80_000_000,
    },
    CanonicalTerm {
        word: "ssl",
        display: "SSL",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "storekit",
        display: "StoreKit",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "supabase",
        display: "Supabase",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "svelte",
        display: "Svelte",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "svg",
        display: "SVG",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "swiftdata",
        display: "SwiftData",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "swiftui",
        display: "SwiftUI",
        weight: 80_000_000,
    },
    CanonicalTerm {
        word: "tailwindcss",
        display: "TailwindCSS",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "tcp",
        display: "TCP",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "tensorflow",
        display: "TensorFlow",
        weight: 90_000_000,
    },
    CanonicalTerm {
        word: "terraform",
        display: "Terraform",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "testflight",
        display: "TestFlight",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "tls",
        display: "TLS",
        weight: 70_000_000,
    },
    CanonicalTerm {
        word: "tokio",
        display: "Tokio",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "toml",
        display: "TOML",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "ts",
        display: "TS",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "tsv",
        display: "TSV",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "tvos",
        display: "tvOS",
        weight: 50_000_000,
    },
    CanonicalTerm {
        word: "typescript",
        display: "TypeScript",
        weight: 105_000_000,
    },
    CanonicalTerm {
        word: "ubuntu",
        display: "Ubuntu",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "udp",
        display: "UDP",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "ui",
        display: "UI",
        weight: 90_000_000,
    },
    CanonicalTerm {
        word: "uikit",
        display: "UIKit",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "uri",
        display: "URI",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "url",
        display: "URL",
        weight: 110_000_000,
    },
    CanonicalTerm {
        word: "uuid",
        display: "UUID",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "ux",
        display: "UX",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "vercel",
        display: "Vercel",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "visionos",
        display: "visionOS",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "vite",
        display: "Vite",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "vllm",
        display: "vLLM",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "vram",
        display: "VRAM",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "vscode",
        display: "VSCode",
        weight: 95_000_000,
    },
    CanonicalTerm {
        word: "vue",
        display: "Vue",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "wasm",
        display: "WASM",
        weight: 60_000_000,
    },
    CanonicalTerm {
        word: "watchos",
        display: "watchOS",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "webassembly",
        display: "WebAssembly",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "webp",
        display: "WebP",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "websocket",
        display: "WebSocket",
        weight: 65_000_000,
    },
    CanonicalTerm {
        word: "webstorm",
        display: "WebStorm",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "widgetkit",
        display: "WidgetKit",
        weight: 40_000_000,
    },
    CanonicalTerm {
        word: "windsurf",
        display: "Windsurf",
        weight: 55_000_000,
    },
    CanonicalTerm {
        word: "xcode",
        display: "Xcode",
        weight: 80_000_000,
    },
    CanonicalTerm {
        word: "xctest",
        display: "XCTest",
        weight: 45_000_000,
    },
    CanonicalTerm {
        word: "xml",
        display: "XML",
        weight: 75_000_000,
    },
    CanonicalTerm {
        word: "yaml",
        display: "YAML",
        weight: 70_000_000,
    },
];

/// 按精确小写拼写二分查找规范词条。
pub(crate) fn lookup_exact(word: &str) -> Option<CanonicalTerm> {
    let idx = CANONICAL_TERMS
        .binary_search_by(|term| term.word.cmp(word))
        .ok()?;
    Some(CANONICAL_TERMS[idx])
}

/// 返回所有以 `prefix` 开头的规范词条连续切片；无命中时返回空切片且零堆分配。
pub(crate) fn lookup_prefix(prefix: &str) -> &'static [CanonicalTerm] {
    if prefix.is_empty() {
        return &[];
    }
    let start = CANONICAL_TERMS.partition_point(|term| term.word < prefix);
    if start >= CANONICAL_TERMS.len() || !CANONICAL_TERMS[start].word.starts_with(prefix) {
        return &[];
    }
    let rest = &CANONICAL_TERMS[start..];
    let len = rest.partition_point(|term| term.word.starts_with(prefix));
    &rest[..len]
}

#[cfg(test)]
mod tests {
    use super::{lookup_exact, lookup_prefix, CANONICAL_TERMS};

    #[test]
    fn canonical_terms_are_strictly_sorted_unique_and_valid() {
        assert!(
            (120..=220).contains(&CANONICAL_TERMS.len()),
            "count={}",
            CANONICAL_TERMS.len()
        );
        for term in CANONICAL_TERMS {
            assert!(!term.word.is_empty());
            assert!(term
                .word
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
            assert!(!term.display.is_empty());
            assert!(term.weight > 0, "{}", term.word);
        }
        for pair in CANONICAL_TERMS.windows(2) {
            assert!(
                pair[0].word < pair[1].word,
                "out of order or duplicate: {} >= {}",
                pair[0].word,
                pair[1].word
            );
        }
    }

    #[test]
    fn exact_and_prefix_lookup_work_and_exclude_common_words() {
        assert_eq!(lookup_exact("chatgpt").map(|t| t.display), Some("ChatGPT"));
        assert_eq!(lookup_exact("macos").map(|t| t.display), Some("macOS"));
        assert_eq!(lookup_exact("mlx").map(|t| t.display), Some("MLX"));
        assert_eq!(lookup_exact("pr").map(|t| t.display), Some("PR"));
        assert!(lookup_exact("review").is_none());
        assert!(lookup_exact("rust").is_none());
        assert!(lookup_exact("test").is_none());
        assert!(lookup_exact("apple").is_none());
        assert!(lookup_prefix("alpha").is_empty());
        let macs = lookup_prefix("mac");
        assert_eq!(macs.len(), 2);
        assert_eq!(macs[0].display, "MacBook");
        assert_eq!(macs[1].display, "macOS");
    }
}
