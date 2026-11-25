//! Shurufa v2 Tier 1 两阶段异步神经解码协议与统一概率融合纯核心。
//!
//! 严守 `docs/ARCHITECTURE.md` 边界：全部融合公式、等跨度竞争、80 ms 槽 0 锁与去重并集排座
//! 均保留在纯 Rust `msime-engine` 内；`msime-input-runtime` 仅做代次编排，macOS/平台宿主仅做
//! 非阻塞 IPC 传输与 22 ms 延迟绘制计时，不含任何排序或融合数学。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::pinyin::graph::{
    build_syllable_graph, enumerate_kbest_complete_segmentations, SYLLABLE_GRAPH_PATH_LIMIT,
};
use crate::pinyin::syllables::is_intact;
use crate::text::is_all_han;
use crate::types::CandidateSource;

/// Tier 1 重排后的神经候选来源号（与 `CandidateSource::NeuralV2` 与基准脚本契约对齐）。
pub const SOURCE_NEURAL_V2: u8 = 19;
/// Tier 1 受限束搜索独有召回候选来源号（与 `CandidateSource::NeuralConstrained` 对齐）。
pub const SOURCE_NEURAL_CONSTRAINED: u8 = 20;

/// Tier 1 上文窗口上限（按 Unicode 标量字符数计，完整保留中文标点、空格、ASCII 与 Emoji）。
pub const NEURAL_CONTEXT_MAX_CHARS: usize = 512;
/// 单次发往 Tier 1 神经打分的去重等跨度 Tier 0 候选上限（稳定压在 <= 18 ms 零闪烁窗内）。
pub const DEFAULT_MAX_NEURAL_CANDIDATES: usize = 16;
/// 受限束搜索默认束宽。
pub const DEFAULT_BEAM_WIDTH: u32 = 3;
/// 受限束搜索默认最多注入的整句数。
pub const DEFAULT_MAX_BEAM_RESULTS: u32 = 4;
/// 受限束搜索单次最多携带的候选音节切分路径数。
pub const DEFAULT_MAX_SYLLABLE_PATHS: usize = 2;

/// 统一桶归一尺度常量（与 Tier 0 `lattice::decode` 保持同一真概率自然对数刻度）。
const UNIFIED_UNIGRAM_Z: f64 = 5e8;
const UNIFIED_MULTI_SYLLABLE_Z: f64 = 0.70 * UNIFIED_UNIGRAM_Z;

/// 隔音多切分路径每增加一处切分的通道先验代价（nats）。
const HIATUS_SPLIT_CHANNEL_PENALTY: f64 = -0.6;
/// 纠错候选默认信道代价（nats），要求神经语境优势明显大于误触发成本。
pub const DEFAULT_TYPO_CHANNEL_PENALTY: f64 = -2.8;
/// 模糊音候选默认信道代价（nats）。
pub const DEFAULT_FUZZY_CHANNEL_PENALTY: f64 = -0.8;
/// 用户个人词库条目的温和个人先验增益（nats）。
pub const DEFAULT_USER_DICTIONARY_LOG_PRIOR: f64 = 1.2;

fn default_true() -> bool {
    true
}

/// 全局 3 参数统一概率融合权重（仅允许在开发集上科学标定，严禁词级特判）。
///
/// 统一评分目标：
/// `Score(Y) = log P_channel(X | Y) + lambda_neural * log P_neural(Y | C) + lambda_stat * log P_stat(Y | C) + lambda_user * log P_user(Y | C)`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NeuralFusionWeights {
    pub lambda_neural: f64,
    pub lambda_stat: f64,
    pub lambda_user: f64,
}

impl Default for NeuralFusionWeights {
    fn default() -> Self {
        Self {
            lambda_neural: 1.0,
            lambda_stat: 0.35,
            lambda_user: 1.0,
        }
    }
}

impl NeuralFusionWeights {
    /// 读取可选环境变量覆盖（供离线标定脚本对 3 个全局权重做网格搜索，不改变线上默认值）。
    pub fn from_env_or_default() -> Self {
        let mut weights = Self::default();
        if let Ok(raw) = std::env::var("SHURUFA_NEURAL_LAMBDA_NEURAL") {
            if let Ok(value) = raw.parse::<f64>() {
                if value.is_finite() && value >= 0.0 {
                    weights.lambda_neural = value;
                }
            }
        }
        if let Ok(raw) = std::env::var("SHURUFA_NEURAL_LAMBDA_STAT") {
            if let Ok(value) = raw.parse::<f64>() {
                if value.is_finite() && value >= 0.0 {
                    weights.lambda_stat = value;
                }
            }
        }
        if let Ok(raw) = std::env::var("SHURUFA_NEURAL_LAMBDA_USER") {
            if let Ok(value) = raw.parse::<f64>() {
                if value.is_finite() && value >= 0.0 {
                    weights.lambda_user = value;
                }
            }
        }
        weights
    }
}

/// 发往 Tier 1 约束束搜索的一条候选音节路径。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeuralSyllablePath {
    pub syllables: Vec<String>,
    #[serde(default)]
    pub channel_log_prob: f64,
    #[serde(default = "default_true")]
    pub is_complete: bool,
}

/// 发往 Tier 1 批量打分与统一融合的单条 Tier 0 候选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeuralCandidateInput {
    /// 在剥离投影 Emoji 后的基础候选列表中的原始下标。
    pub index: usize,
    pub text: String,
    #[serde(default)]
    pub pinyin: String,
    #[serde(default)]
    pub source: u8,
    #[serde(default)]
    pub char_count: usize,
    /// 是否严格覆盖当前整段输入且非超长续写词（等跨度竞争核心判据）。
    #[serde(default)]
    pub exact_span: bool,
    /// 是否为严格匹配原始输入字母的未纠错词典整词（排除前缀补全，如 `meiganxi -> 没敢想` 为 `false`）。
    #[serde(default)]
    pub exact_literal_dict: bool,
    /// Tier 0 统一自然对数静态先验（nats）；非词格或缺失时为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stat_log_prob: Option<f64>,
    /// 输入信道对数概率（完全匹配为 0.0，隔音拆分/纠错/模糊音为负值）。
    #[serde(default)]
    pub channel_log_prob: f64,
    /// 个人词库/习惯对数先验增益（无个人偏好时为 0.0）。
    #[serde(default)]
    pub user_log_prob: f64,
    /// L4 硬约束：是否被用户显式固定位置或置顶。
    #[serde(default)]
    pub pinned: bool,
}

/// 引擎会话侧为单条候选提供的权威元数据（按引擎原始候选顺序对齐）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EngineNeuralCandidateMeta {
    pub stat_log_prob: Option<f64>,
    pub channel_log_prob: f64,
    pub user_log_prob: f64,
    pub exact_span: bool,
    pub exact_literal_dict: bool,
    pub pinned: bool,
}

/// 引擎会话侧为当前组字态提取的 Tier 1 辅助元数据。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EngineNeuralMetadata {
    pub raw_input: String,
    pub segmentation: String,
    pub syllable_paths: Vec<NeuralSyllablePath>,
    pub candidates: Vec<EngineNeuralCandidateMeta>,
}

/// Stage 1 -> Tier 1 神经解码请求（纯数据，可跨 FFI / UDS / 离线评测零损耗透传）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeuralDecodeRequest {
    pub session: u64,
    pub generation: u64,
    /// 光标前完整上文（至多 512 字，保留中文标点、空格、ASCII 与 Emoji）。
    pub context: String,
    pub raw_input: String,
    pub segmentation: String,
    #[serde(default)]
    pub syllable_paths: Vec<NeuralSyllablePath>,
    pub candidates: Vec<NeuralCandidateInput>,
    /// L4 固定/置顶席位清单：每项为 `(base_index, target_seat)`，
    /// 融合排座时强制把基础下标 `base_index` 锁在最终席位 `target_seat`，防止被等跨度重排或续写顺延挤离。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_seats: Vec<(usize, usize)>,
    #[serde(default = "default_beam_width")]
    pub beam_width: u32,
    #[serde(default = "default_max_beam_results")]
    pub max_beam_results: u32,
    /// 消融与降级开关：为 `false` 时仅执行前缀树批量打分，跳过受限束搜索。
    #[serde(default = "default_true")]
    pub enable_constrained_decode: bool,
}

fn default_beam_width() -> u32 {
    DEFAULT_BEAM_WIDTH
}

fn default_max_beam_results() -> u32 {
    DEFAULT_MAX_BEAM_RESULTS
}

/// Tier 1 返回的单条候选条件对数概率。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeuralScoredCandidate {
    pub text: String,
    pub neural_log_prob: f64,
}

/// Tier 1 受限束搜索生成的整句候选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeuralBeamCandidate {
    pub text: String,
    pub neural_log_prob: f64,
    #[serde(default)]
    pub path_index: usize,
    #[serde(default)]
    pub token_count: usize,
}

/// Tier 1 -> Stage 2 神经解码响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeuralDecodeResponse {
    #[serde(default = "default_true")]
    pub ok: bool,
    #[serde(default)]
    pub session: u64,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub candidate_scores: Vec<NeuralScoredCandidate>,
    #[serde(default)]
    pub beam_candidates: Vec<NeuralBeamCandidate>,
}

/// 离线同步评测与单元测试用的神经解码后端抽象。
pub trait NeuralDecoderBackend {
    fn prefill_context(&mut self, context: &str) -> Result<(), String>;
    fn decode(&mut self, request: &NeuralDecodeRequest) -> Result<NeuralDecodeResponse, String>;
}

/// 融合后的单个候选席位来源。
#[derive(Debug, Clone, PartialEq)]
pub enum FusedSeat {
    /// 指向原 Tier 0 基础候选列表的 `index` 项。
    Existing {
        index: usize,
        promoted_by_neural: bool,
    },
    /// Tier 1 受限束搜索独有召回的新候选（原 Tier 0 池中不存在）。
    InjectedBeam {
        text: String,
        pinyin: String,
        neural_log_prob: f64,
        channel_log_prob: f64,
    },
}

/// 统一概率融合输出。
#[derive(Debug, Clone, PartialEq)]
pub struct NeuralFusionOutcome {
    pub seats: Vec<FusedSeat>,
    pub moved: bool,
    pub slot0_locked: bool,
    pub tier1_rescued: bool,
    pub top1_text: Option<String>,
}

/// 截取光标前至多 `NEURAL_CONTEXT_MAX_CHARS`（512）个 Unicode 标量字符，
/// 完整保留中文标点、空格、ASCII 与 Emoji，绝不按标点截断。
pub fn neural_context_window(text: &str) -> &str {
    neural_context_window_limit(text, NEURAL_CONTEXT_MAX_CHARS)
}

/// 按字符数上限截取尾部窗口，始终落在合法 UTF-8 字符边界。
pub fn neural_context_window_limit(text: &str, max_chars: usize) -> &str {
    if text.is_empty() || max_chars == 0 {
        return "";
    }
    let mut count = 0usize;
    for (byte_idx, _) in text.char_indices().rev() {
        count += 1;
        if count >= max_chars {
            return &text[byte_idx..];
        }
    }
    text
}

/// 将词典权重映射到与 Tier 0 词网格一致的统一自然对数先验刻度（nats）。
pub fn dictionary_unified_stat_log_prob(weight: i64, syllables: usize) -> f64 {
    let w = if syllables >= 3 && weight == 100 {
        // 与 Tier 0 保持一致：未测频多字词占位权重 100 平滑折算，避免虚高。
        15.0
    } else {
        (weight.max(1)) as f64
    };
    let z = if syllables <= 1 {
        UNIFIED_UNIGRAM_Z
    } else {
        UNIFIED_MULTI_SYLLABLE_Z
    };
    w.ln() - z.ln()
}

/// 判断候选来源是否属于可参与 Tier 1 中文统一融合的来源。
pub fn is_neural_v2_source(source: u8) -> bool {
    source == CandidateSource::Database as u8
        || source == CandidateSource::UserDatabase as u8
        || source == CandidateSource::Generated as u8
        || source == CandidateSource::NeuralKeyboard as u8
        || source == CandidateSource::Collocation as u8
        || source == SOURCE_NEURAL_V2
        || source == SOURCE_NEURAL_CONSTRAINED
}

/// 从原始拼音与主切分构造供受限束搜索使用的候选音节路径（含主切分与零声母隔音备选切分）。
pub fn build_syllable_paths(
    raw_input: &str,
    primary_segmentation: &str,
    max_paths: usize,
) -> Vec<NeuralSyllablePath> {
    if max_paths == 0 {
        return Vec::new();
    }
    let mut paths = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let primary_pieces: Vec<String> = primary_segmentation
        .split('\'')
        .filter(|piece| !piece.is_empty())
        .map(|piece| piece.to_ascii_lowercase())
        .collect();
    let primary_len = primary_pieces.len();

    if !primary_pieces.is_empty() {
        let complete = primary_pieces.iter().all(|piece| is_intact(piece));
        let key = primary_pieces.join("'");
        seen.insert(key);
        paths.push(NeuralSyllablePath {
            syllables: primary_pieces,
            channel_log_prob: 0.0,
            is_complete: complete,
        });
    }

    // 若用户输入不含手动 `'`，使用任意句长 A* K-best 切分器枚举合法替代切分
    // （如 `xian -> xi'an`、`fangan -> fan'gan` 以及 10+ 音节长句中的零声母隔音切分）。
    let trimmed = raw_input.trim();
    if paths.len() < max_paths
        && !trimmed.is_empty()
        && !trimmed.contains('\'')
        && trimmed.bytes().all(|b| b.is_ascii_lowercase())
    {
        let graph = build_syllable_graph(trimmed);
        let alt_limit = SYLLABLE_GRAPH_PATH_LIMIT.min(16);
        for dag_path in enumerate_kbest_complete_segmentations(&graph, 2, alt_limit) {
            if paths.len() >= max_paths {
                break;
            }
            if dag_path.syllables.is_empty() {
                continue;
            }
            let key = dag_path.syllables.join("'");
            if !seen.insert(key) {
                continue;
            }
            let extra_splits = dag_path.syllables.len().saturating_sub(primary_len.max(1));
            let channel_log_prob = dag_path
                .channel_log_prob
                .min(HIATUS_SPLIT_CHANNEL_PENALTY * extra_splits as f64);
            paths.push(NeuralSyllablePath {
                syllables: dag_path.syllables,
                channel_log_prob,
                is_complete: dag_path.is_complete,
            });
        }
    }

    paths
}

#[derive(Debug, Clone)]
enum CompetitorKind {
    Tier0 {
        index: usize,
        original_rank: usize,
    },
    Beam {
        text: String,
        pinyin: String,
        neural_log_prob: f64,
        channel_log_prob: f64,
    },
}

#[derive(Debug, Clone)]
struct ScoredCompetitor {
    kind: CompetitorKind,
    text: String,
    total_score: f64,
    is_typo: bool,
}

/// 纯函数统一概率融合与排座：
/// - 将 `Tier 0 ∪ Tier 1` 按去重并集在统一自然对数目标下竞争；
/// - 严格保障等跨度主候选不被超长续写词或单字前缀词抢占首槽；
/// - 当 `lock_slot_0 == true`（迟到 >80 ms 或用户已翻页/移高亮）或首槽为置顶/非中文候选时，严格锁定槽 0 不动，仅升级槽 `1..`；
/// - 保证基础列表 `0..total_base_len` 中每一条原候选均完整保留、零丢失、零重复。
pub fn fuse_neural_response(
    total_base_len: usize,
    request: &NeuralDecodeRequest,
    response: &NeuralDecodeResponse,
    weights: NeuralFusionWeights,
    lock_slot_0: bool,
) -> NeuralFusionOutcome {
    let identity = |locked: bool| NeuralFusionOutcome {
        seats: (0..total_base_len)
            .map(|index| FusedSeat::Existing {
                index,
                promoted_by_neural: false,
            })
            .collect(),
        moved: false,
        slot0_locked: locked,
        tier1_rescued: false,
        top1_text: request
            .candidates
            .iter()
            .find(|c| c.index == 0)
            .map(|c| c.text.clone()),
    };

    if !response.ok || total_base_len == 0 {
        return identity(lock_slot_0);
    }

    // 1. 汇总前缀树批量打分结果（仅采纳有限浮点值）。
    let mut neural_scores: HashMap<&str, f64> =
        HashMap::with_capacity(response.candidate_scores.len());
    for scored in &response.candidate_scores {
        if !scored.text.is_empty() && scored.neural_log_prob.is_finite() {
            neural_scores
                .entry(scored.text.as_str())
                .and_modify(|prev| {
                    if scored.neural_log_prob > *prev {
                        *prev = scored.neural_log_prob;
                    }
                })
                .or_insert(scored.neural_log_prob);
        }
    }

    // 2. 索引 Tier 0 中所有出现过的文本，确保 `Tier 0 ∪ Tier 1` 严格去重。
    let mut tier0_texts: HashSet<&str> = HashSet::with_capacity(request.candidates.len());
    for cand in &request.candidates {
        tier0_texts.insert(cand.text.as_str());
    }

    // 3. 校验并分类受限束搜索结果：命中 Tier 0 同文本则升级其神经分，未命中则作为 Tier 1 独有注入候选。
    let mut unique_beams: Vec<(String, String, f64, f64)> = Vec::new();
    let mut seen_beam_texts: HashSet<&str> = HashSet::new();
    for beam in &response.beam_candidates {
        if beam.text.is_empty() || !beam.neural_log_prob.is_finite() || !is_all_han(&beam.text) {
            continue;
        }
        let path = request
            .syllable_paths
            .get(beam.path_index)
            .or_else(|| request.syllable_paths.first());
        let Some(path) = path else {
            continue;
        };
        let path_allows_beam = path.is_complete
            || (!request.context.is_empty() && (4..=8).contains(&path.syllables.len()));
        if !path_allows_beam || beam.text.chars().count() != path.syllables.len() {
            continue;
        }
        if tier0_texts.contains(beam.text.as_str()) {
            neural_scores
                .entry(beam.text.as_str())
                .and_modify(|prev| {
                    if beam.neural_log_prob > *prev {
                        *prev = beam.neural_log_prob;
                    }
                })
                .or_insert(beam.neural_log_prob);
            continue;
        }
        if !seen_beam_texts.insert(beam.text.as_str()) {
            continue;
        }
        unique_beams.push((
            beam.text.clone(),
            path.syllables.join("'"),
            beam.neural_log_prob,
            path.channel_log_prob,
        ));
    }

    // 4. 筛选可参与等跨度竞争的 Tier 0 候选，并决定槽 0 是否必须锁定。
    let slot0_candidate = request.candidates.iter().find(|c| c.index == 0);
    let slot0_eligible = slot0_candidate.is_some_and(|c| {
        c.exact_span
            && !c.pinned
            && is_neural_v2_source(c.source)
            && neural_scores.contains_key(c.text.as_str())
    });
    // 若槽 0 被用户置顶或为英文/表情/非等跨度行，强制锁定槽 0 不允许抢占。
    let slot0_protected =
        slot0_candidate.is_none_or(|c| c.pinned || !is_neural_v2_source(c.source));
    let effective_lock_slot_0 = lock_slot_0 || slot0_protected;

    let mut competing_tier0: Vec<(usize, &NeuralCandidateInput, f64)> = Vec::new();
    for (rank, cand) in request.candidates.iter().enumerate() {
        if cand.index >= total_base_len
            || !cand.exact_span
            || cand.pinned
            || !is_neural_v2_source(cand.source)
        {
            continue;
        }
        if let Some(&neural_lp) = neural_scores.get(cand.text.as_str()) {
            competing_tier0.push((rank, cand, neural_lp));
        }
    }

    if competing_tier0.is_empty() && unique_beams.is_empty() {
        return identity(effective_lock_slot_0);
    }

    // 5. 跨来源静态先验分桶相对归一：
    //    `Database/UserDatabase` 与 `Lattice(Generated/Collocation/NeuralKeyboard)` 的原始绝对偏移不同
    //    （后者含 `.gram` 边常数偏移），按来源桶各自减去桶内最大值并截断至 `[-10.0, 0.0]`，
    //    彻底消灭跨来源加性常数错位；未测频词典占位条目（`stat_log_prob == None`）给予温和中性相对先验 `-1.5`，
    //    而未进入 Tier 0 词网格主束的 `UniqueBeam` 独有候选按已观测 Tier 0 下界钳位至 `[-6.5, -3.5]`，
    //    既杜绝束搜索次优路径凭虚高静态分反超在池真词，又让强神经语境优势的真实 OOV 词稳定翻盘。
    const STAT_REL_FLOOR: f64 = -10.0;
    const NEUTRAL_STAT_REL: f64 = -1.5;
    const BEAM_OOV_STAT_REL_CEILING: f64 = -3.5;
    const BEAM_OOV_STAT_REL_FLOOR: f64 = -6.5;

    let is_dict_bucket = |source: u8| -> bool {
        source == CandidateSource::Database as u8 || source == CandidateSource::UserDatabase as u8
    };
    let mut dict_max: Option<f64> = None;
    let mut lattice_max: Option<f64> = None;
    for &(_, cand, _) in &competing_tier0 {
        if let Some(s) = cand.stat_log_prob.filter(|v| v.is_finite()) {
            let slot = if is_dict_bucket(cand.source) {
                &mut dict_max
            } else {
                &mut lattice_max
            };
            *slot = Some(slot.map_or(s, |cur| cur.max(s)));
        }
    }

    // 6. 按唯一统一概率公式为 `Tier 0 ∪ Tier 1` 计分并排序。
    // (a) 非成词拆字先验：当同池已存在 `2..=4` 字未纠错词典整词（如全拼 `但凡` 或混拼 `que'g'x -> 缺根弦`）时，
    //     同字数非词典拼接（如 `但反`、`缺钙性`）因独立分桶归零会抹平成词结构优势，对其施加结构先验惩罚；
    //     若输入含简拼/混拼残缺声母（如 `que'g'x` 中 `g`/`x`），自由拼接的信道分支熵远高于精确词典整词，
    //     每枚残缺声母叠加 `-1.2 nats`（上限 3 枚）。真实语境必需拆字（`+6..+15 nats` 神经优势）仍可稳定胜出。
    // (b) 单字纠错跨字数信息量补偿：联合对数概率按每字约 `-4.8 nats` 衰减；当同池存在 2+ 字未纠错词典整词
    //     （如 `shj -> 时间/手机`、`shh -> 上海`、`zhw -> 中文/这位`）时，单字纠错候选（`char_count == 1`，如 `是`/`这`/`书`）
    //     会凭少 1 字白赚约 `+4.5..+5.0 nats` 联合概率反压多字简拼整词，须补足 `-4.8 nats` 信息量差；
    //     多字纠错候选（`char_count >= 2`，如跨音节换位 `jintina -> 今天*`、`meiwentj -> 没问题*`）不施加此惩罚。
    // (c) 空上文精确字面词信道保守边际：当上文为空、输入为完整全拼且同池存在严格匹配原始字母的未纠错词典整词
    //     （`exact_literal_dict == true`，如 `"" + naizi -> 奶子`，排除 `meiganxi -> 没敢想` 这类前缀补全词）时，
    //     缺乏语境证据佐证错拼，对错拼候选额外施加 `-2.5 nats` 信道边际：常规字面词不被无条件高频邻键词劫持首位，
    //     而有上文语义佐证（如 `他是个好 + naizi -> 孩子*`）或空上文生僻碰撞大额先验差仍可正常救回。
    const BASE_NON_LEXICAL_SPLIT_PENALTY: f64 = -1.8;
    const BARE_INITIAL_SPLIT_EXTRA_PER_SYLLABLE: f64 = -1.2;
    const SINGLE_CHAR_TYPO_ENTROPY_COMP: f64 = -4.8;
    const EMPTY_CONTEXT_EXACT_LITERAL_TYPO_PENALTY: f64 = -2.5;
    let bare_initial_count = request
        .segmentation
        .split('\'')
        .filter(|piece| !piece.is_empty() && !is_intact(piece))
        .count();
    let mut exact_dict_len_mask: u8 = 0;
    let mut has_multi_char_exact_dict = false;
    let mut has_verified_exact_literal_dict = false;
    for &(_, c, _) in &competing_tier0 {
        if is_dict_bucket(c.source) && c.channel_log_prob == 0.0 {
            if (2..=4).contains(&c.char_count) {
                exact_dict_len_mask |= 1u8 << c.char_count;
            }
            if c.char_count >= 2 {
                has_multi_char_exact_dict = true;
            }
            if c.exact_literal_dict {
                has_verified_exact_literal_dict = true;
            }
        }
    }
    let empty_ctx_literal_typo_penalty =
        if request.context.is_empty() && bare_initial_count == 0 && has_verified_exact_literal_dict
        {
            EMPTY_CONTEXT_EXACT_LITERAL_TYPO_PENALTY
        } else {
            0.0
        };

    let mut competitors: Vec<ScoredCompetitor> =
        Vec::with_capacity(competing_tier0.len() + unique_beams.len());
    let mut observed_min_rel_stat: Option<f64> = None;

    for (orig_rank, cand, neural_lp) in &competing_tier0 {
        let rel_stat = match cand.stat_log_prob.filter(|v| v.is_finite()) {
            Some(s) => {
                let bucket_max = if is_dict_bucket(cand.source) {
                    dict_max.unwrap_or(s)
                } else {
                    lattice_max.unwrap_or(s)
                };
                (s - bucket_max).clamp(STAT_REL_FLOOR, 0.0)
            }
            None => NEUTRAL_STAT_REL,
        };
        observed_min_rel_stat =
            Some(observed_min_rel_stat.map_or(rel_stat, |cur| cur.min(rel_stat)));
        let has_same_len_exact_dict = (2..=4).contains(&cand.char_count)
            && (exact_dict_len_mask & (1u8 << cand.char_count)) != 0;
        let non_lexical_split_penalty = if has_same_len_exact_dict
            && !is_dict_bucket(cand.source)
            && cand.channel_log_prob == 0.0
        {
            BASE_NON_LEXICAL_SPLIT_PENALTY
                + BARE_INITIAL_SPLIT_EXTRA_PER_SYLLABLE * (bare_initial_count.min(3) as f64)
        } else {
            0.0
        };
        let shorter_typo_len_comp =
            if cand.channel_log_prob <= -2.0 && cand.char_count == 1 && has_multi_char_exact_dict {
                SINGLE_CHAR_TYPO_ENTROPY_COMP
            } else {
                0.0
            };
        let empty_ctx_typo_adj = if cand.channel_log_prob <= -2.0 {
            empty_ctx_literal_typo_penalty
        } else {
            0.0
        };
        let total = cand.channel_log_prob
            + non_lexical_split_penalty
            + shorter_typo_len_comp
            + empty_ctx_typo_adj
            + weights.lambda_neural * (*neural_lp)
            + weights.lambda_stat * rel_stat
            + weights.lambda_user * cand.user_log_prob;
        competitors.push(ScoredCompetitor {
            kind: CompetitorKind::Tier0 {
                index: cand.index,
                original_rank: *orig_rank,
            },
            text: cand.text.clone(),
            total_score: total,
            is_typo: cand.channel_log_prob <= -2.0,
        });
    }

    let beam_stat_rel = observed_min_rel_stat
        .unwrap_or(BEAM_OOV_STAT_REL_CEILING)
        .clamp(BEAM_OOV_STAT_REL_FLOOR, BEAM_OOV_STAT_REL_CEILING);
    for (text, pinyin, neural_lp, channel_lp) in unique_beams {
        let char_count = text.chars().count();
        let has_same_len_exact_dict =
            (2..=4).contains(&char_count) && (exact_dict_len_mask & (1u8 << char_count)) != 0;
        let non_lexical_split_penalty = if has_same_len_exact_dict && channel_lp == 0.0 {
            BASE_NON_LEXICAL_SPLIT_PENALTY
                + BARE_INITIAL_SPLIT_EXTRA_PER_SYLLABLE * (bare_initial_count.min(3) as f64)
        } else {
            0.0
        };
        let total = channel_lp
            + non_lexical_split_penalty
            + weights.lambda_neural * neural_lp
            + weights.lambda_stat * beam_stat_rel;
        competitors.push(ScoredCompetitor {
            kind: CompetitorKind::Beam {
                text: text.clone(),
                pinyin,
                neural_log_prob: neural_lp,
                channel_log_prob: channel_lp,
            },
            text,
            total_score: total,
            is_typo: channel_lp <= -2.0,
        });
    }

    competitors.sort_by(|a, b| {
        b.total_score
            .total_cmp(&a.total_score)
            .then_with(|| match (&a.kind, &b.kind) {
                (
                    CompetitorKind::Tier0 {
                        original_rank: ra, ..
                    },
                    CompetitorKind::Tier0 {
                        original_rank: rb, ..
                    },
                ) => ra.cmp(rb),
                (CompetitorKind::Tier0 { .. }, CompetitorKind::Beam { .. }) => {
                    std::cmp::Ordering::Less
                }
                (CompetitorKind::Beam { .. }, CompetitorKind::Tier0 { .. }) => {
                    std::cmp::Ordering::Greater
                }
                (CompetitorKind::Beam { .. }, CompetitorKind::Beam { .. }) => a.text.cmp(&b.text),
            })
    });

    // 当同池已存在未纠错的精确词典整词（如 `xiongtai -> 兄台`）时，在 Tier 1 完成全池打分之后，
    // 首页前 4 个等跨度席位至多保留 1 条得分最高的纠错候选（保留其原有胜出名次，包括 `#1` 语境救回），
    // 将其余次优纠错候选（如 `性态*`/`邢台*`）顺延至第 5 席之后，做到零召回损失、零 Top-1 损失且不刷屏首页。
    let has_exact_literal_dict_any = competing_tier0
        .iter()
        .any(|(_, c, _)| is_dict_bucket(c.source) && c.channel_log_prob == 0.0);
    if has_exact_literal_dict_any && competitors.len() > 2 {
        const PAGE1_EQUAL_SPAN_CAP: usize = 4;
        const MAX_TYPOS_IN_PAGE1: usize = 1;
        let mut head: Vec<ScoredCompetitor> = Vec::with_capacity(PAGE1_EQUAL_SPAN_CAP);
        let mut deferred_typos: Vec<ScoredCompetitor> = Vec::new();
        let mut rest: Vec<ScoredCompetitor> = Vec::new();
        let mut typos_in_head = 0usize;
        for comp in competitors {
            if head.len() < PAGE1_EQUAL_SPAN_CAP {
                if comp.is_typo {
                    if typos_in_head < MAX_TYPOS_IN_PAGE1 {
                        typos_in_head += 1;
                        head.push(comp);
                    } else {
                        deferred_typos.push(comp);
                    }
                } else {
                    head.push(comp);
                }
            } else {
                rest.push(comp);
            }
        }
        let mut reordered = Vec::with_capacity(head.len() + deferred_typos.len() + rest.len());
        reordered.extend(head);
        reordered.extend(deferred_typos);
        reordered.extend(rest);
        competitors = reordered;
    }

    // 若要求锁槽 0 且原槽 0 在竞争集合中，将其牢牢固定在竞争列表首位，其余席位按融合分升序更新。
    if effective_lock_slot_0 && slot0_eligible {
        if let Some(pos) = competitors
            .iter()
            .position(|c| matches!(c.kind, CompetitorKind::Tier0 { index: 0, .. }))
        {
            if pos > 0 {
                competitors[..=pos].rotate_right(1);
            }
        }
    }

    // 7. 统一 L4 固定/置顶席位约束与等跨度无损回填：
    //    先对全部未固定行执行等跨度前 4 连续排座与束搜索溢出回填，再把每个 L4 固定/置顶行精确合入其目标席位，
    //    保证用户固定在位置 `k` 的候选（如 `ma -> 嘛` 固定在第 4 位）绝不被神经重排、非等跨度顺延或束注入挤离原位。
    let mut pinned_index_set: HashSet<usize> = HashSet::new();
    let mut raw_pinned: Vec<(usize, usize)> = Vec::new();
    for &(orig_idx, target_seat) in &request.pinned_seats {
        if orig_idx < total_base_len && pinned_index_set.insert(orig_idx) {
            raw_pinned.push((target_seat, orig_idx));
        }
    }
    for cand in &request.candidates {
        if cand.pinned && cand.index < total_base_len && pinned_index_set.insert(cand.index) {
            raw_pinned.push((cand.index, cand.index));
        }
    }

    let mut exact_slots: Vec<usize> = competing_tier0
        .iter()
        .map(|(_, c, _)| c.index)
        .filter(|idx| !pinned_index_set.contains(idx))
        .collect();
    exact_slots.sort_unstable();
    exact_slots.dedup();

    let extra_beams = competitors.len().saturating_sub(exact_slots.len());
    let final_len = total_base_len + extra_beams;

    // 为所有 L4 固定/置顶行分配最终目标席位（冲突时向后顺延至最近空位）。
    raw_pinned.sort_by_key(|&(target, orig)| (target, orig));
    let mut pinned_by_seat: Vec<Option<usize>> = vec![None; final_len];
    for (target, orig_idx) in raw_pinned {
        let mut seat = target.min(final_len.saturating_sub(1));
        while seat < final_len && pinned_by_seat[seat].is_some() {
            seat += 1;
        }
        if seat >= final_len {
            if let Some(free) = pinned_by_seat.iter().position(|entry| entry.is_none()) {
                seat = free;
            } else {
                continue;
            }
        }
        pinned_by_seat[seat] = Some(orig_idx);
    }

    let unpinned_base_indices: Vec<usize> = (0..total_base_len)
        .filter(|idx| !pinned_index_set.contains(idx))
        .collect();
    let mut unpinned_seats: Vec<FusedSeat> = Vec::with_capacity(final_len);

    if exact_slots.is_empty() {
        // 极端情况：Tier 0 没有任何等跨度候选（例如仅剩前缀单字），而 Tier 1 受限束搜索召回了完整整句。
        let lock_first_unpinned =
            effective_lock_slot_0 && pinned_by_seat.first().is_none_or(|p| p.is_none());
        let insert_at = if lock_first_unpinned {
            1.min(unpinned_base_indices.len())
        } else {
            0
        };
        for &idx in &unpinned_base_indices[..insert_at] {
            unpinned_seats.push(FusedSeat::Existing {
                index: idx,
                promoted_by_neural: false,
            });
        }
        for comp in &competitors {
            if let CompetitorKind::Beam {
                text,
                pinyin,
                neural_log_prob,
                channel_log_prob,
            } = &comp.kind
            {
                unpinned_seats.push(FusedSeat::InjectedBeam {
                    text: text.clone(),
                    pinyin: pinyin.clone(),
                    neural_log_prob: *neural_log_prob,
                    channel_log_prob: *channel_log_prob,
                });
            }
        }
        for &idx in &unpinned_base_indices[insert_at..] {
            unpinned_seats.push(FusedSeat::Existing {
                index: idx,
                promoted_by_neural: false,
            });
        }
    } else {
        let exact_slot_set: HashSet<usize> = exact_slots.iter().copied().collect();
        let mut comp_iter = competitors.into_iter();
        let first_exact_pos = unpinned_base_indices
            .iter()
            .position(|idx| exact_slot_set.contains(idx))
            .unwrap_or(0);
        let last_exact_pos = unpinned_base_indices
            .iter()
            .rposition(|idx| exact_slot_set.contains(idx))
            .unwrap_or(0);
        // 保证首页前 4 席未固定等跨度主候选连续呈现，
        // 防止长词续写（如 `淡饭粗茶`/`淡饭黄齑`/`单反相机`）楔入首页中间把真实等跨度词（如 `但凡`）挤到第 2 页；
        // L4 固定席位已提前剔除并独立锁位，绝不会被误判为续写词顺延。
        let front_exact_count = exact_slots.len().min(4);
        let contiguous_front_end =
            (first_exact_pos + front_exact_count).min(unpinned_base_indices.len());
        let displaced_non_exact: Vec<usize> = unpinned_base_indices
            [first_exact_pos..contiguous_front_end]
            .iter()
            .copied()
            .filter(|idx| !exact_slot_set.contains(idx))
            .collect();
        let mut overflow_inserted = false;

        let make_seat =
            |comp: ScoredCompetitor, target_slot: usize, track_promoted: bool| match comp.kind {
                CompetitorKind::Tier0 { index, .. } => FusedSeat::Existing {
                    index,
                    promoted_by_neural: track_promoted && index != target_slot,
                },
                CompetitorKind::Beam {
                    text,
                    pinyin,
                    neural_log_prob,
                    channel_log_prob,
                } => FusedSeat::InjectedBeam {
                    text,
                    pinyin,
                    neural_log_prob,
                    channel_log_prob,
                },
            };

        // 1. 首个等跨度未固定槽位之前的非等跨度行原位保留。
        for &base_idx in &unpinned_base_indices[..first_exact_pos] {
            unpinned_seats.push(FusedSeat::Existing {
                index: base_idx,
                promoted_by_neural: false,
            });
        }

        // 2. 从 `first_exact_pos` 起连续回填前 `front_exact_count` 名等跨度胜出者。
        for &target_slot in &unpinned_base_indices[first_exact_pos..contiguous_front_end] {
            if let Some(comp) = comp_iter.next() {
                unpinned_seats.push(make_seat(comp, target_slot, true));
            }
        }
        // 若所有原始等跨度槽位均已落在前段内，立即接上束搜索溢出项，避免丢失。
        if last_exact_pos < contiguous_front_end && !overflow_inserted {
            overflow_inserted = true;
            for extra in comp_iter.by_ref() {
                unpinned_seats.push(make_seat(extra, contiguous_front_end, false));
            }
        }

        // 3. 将原本夹在未固定前 4 段内的非等跨度续写词顺延至前 4 等跨度之后。
        for idx in displaced_non_exact {
            unpinned_seats.push(FusedSeat::Existing {
                index: idx,
                promoted_by_neural: false,
            });
        }

        // 4. 其余未固定区间按原槽位无损回填剩余等跨度与非等跨度候选。
        for (pos, &base_idx) in unpinned_base_indices
            .iter()
            .enumerate()
            .skip(contiguous_front_end)
        {
            if exact_slot_set.contains(&base_idx) {
                if let Some(comp) = comp_iter.next() {
                    unpinned_seats.push(make_seat(comp, base_idx, true));
                }
                if pos == last_exact_pos && !overflow_inserted {
                    overflow_inserted = true;
                    for extra in comp_iter.by_ref() {
                        unpinned_seats.push(make_seat(extra, base_idx, false));
                    }
                }
            } else {
                unpinned_seats.push(FusedSeat::Existing {
                    index: base_idx,
                    promoted_by_neural: false,
                });
            }
        }
    }

    // 二次兜底校验：若 `effective_lock_slot_0` 为真且槽 0 未被其他固定行占据，硬性保证未固定流首位必为原索引 0，
    // 在合并固定席位之前旋转，绝不连带平移任何 L4 固定席位。
    if effective_lock_slot_0
        && !pinned_index_set.contains(&0)
        && pinned_by_seat.first().is_none_or(|p| p.is_none())
    {
        if let Some(pos) = unpinned_seats
            .iter()
            .position(|s| matches!(s, FusedSeat::Existing { index: 0, .. }))
        {
            if pos > 0 {
                unpinned_seats[..=pos].rotate_right(1);
            }
        }
    }

    // 合并 L4 固定席位与未固定流，得到最终零丢失、零重复且严格遵守全部固定位置的席位表。
    let mut seats: Vec<FusedSeat> = Vec::with_capacity(final_len);
    let mut unpinned_iter = unpinned_seats.into_iter();
    for pinned_slot in &pinned_by_seat {
        if let Some(orig_idx) = *pinned_slot {
            seats.push(FusedSeat::Existing {
                index: orig_idx,
                promoted_by_neural: false,
            });
        } else if let Some(seat) = unpinned_iter.next() {
            seats.push(seat);
        }
    }
    seats.extend(unpinned_iter);

    let moved = seats.len() != total_base_len
        || seats
            .iter()
            .enumerate()
            .any(|(pos, seat)| !matches!(seat, FusedSeat::Existing { index, .. } if *index == pos));

    let tier1_rescued = seats
        .iter()
        .take(4)
        .any(|seat| matches!(seat, FusedSeat::InjectedBeam { .. }));

    let top1_text = seats.first().and_then(|seat| match seat {
        FusedSeat::InjectedBeam { text, .. } => Some(text.clone()),
        FusedSeat::Existing { index, .. } => request
            .candidates
            .iter()
            .find(|c| c.index == *index)
            .map(|c| c.text.clone()),
    });

    NeuralFusionOutcome {
        seats,
        moved,
        slot0_locked: effective_lock_slot_0,
        tier1_rescued,
        top1_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_candidate(
        index: usize,
        text: &str,
        exact_span: bool,
        stat_log_prob: Option<f64>,
    ) -> NeuralCandidateInput {
        NeuralCandidateInput {
            index,
            text: text.to_owned(),
            pinyin: "zheng'le'xie'qian".to_owned(),
            source: CandidateSource::Generated as u8,
            char_count: text.chars().count(),
            exact_span,
            exact_literal_dict: false,
            stat_log_prob,
            channel_log_prob: 0.0,
            user_log_prob: 0.0,
            pinned: false,
        }
    }

    fn base_request(candidates: Vec<NeuralCandidateInput>) -> NeuralDecodeRequest {
        NeuralDecodeRequest {
            session: 1,
            generation: 10,
            context: "这个兄弟 应该多少还是".to_owned(),
            raw_input: "zhenglexieqian".to_owned(),
            segmentation: "zheng'le'xie'qian".to_owned(),
            syllable_paths: vec![NeuralSyllablePath {
                syllables: vec!["zheng".into(), "le".into(), "xie".into(), "qian".into()],
                channel_log_prob: 0.0,
                is_complete: true,
            }],
            candidates,
            pinned_seats: Vec::new(),
            beam_width: 5,
            max_beam_results: 4,
            enable_constrained_decode: true,
        }
    }

    #[test]
    fn unified_fusion_promotes_contextual_homophone() {
        let req = base_request(vec![
            sample_candidate(0, "整了血铅", true, Some(-11.4)),
            sample_candidate(1, "整了些钱", true, Some(-12.1)),
            sample_candidate(2, "挣了些钱", true, Some(-12.5)),
            sample_candidate(3, "整", false, Some(-4.0)),
        ]);
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "整了血铅".into(),
                    neural_log_prob: -24.0,
                },
                NeuralScoredCandidate {
                    text: "整了些钱".into(),
                    neural_log_prob: -14.2,
                },
                NeuralScoredCandidate {
                    text: "挣了些钱".into(),
                    neural_log_prob: -11.3,
                },
                NeuralScoredCandidate {
                    text: "整".into(),
                    neural_log_prob: -2.0,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(4, &req, &resp, NeuralFusionWeights::default(), false);
        assert!(out.moved);
        assert!(!out.slot0_locked);
        assert_eq!(out.top1_text.as_deref(), Some("挣了些钱"));
        assert!(matches!(out.seats[0], FusedSeat::Existing { index: 2, .. }));
        assert!(matches!(out.seats[1], FusedSeat::Existing { index: 1, .. }));
        assert!(matches!(out.seats[2], FusedSeat::Existing { index: 0, .. }));
        // 非等跨度单字前缀留在原地，绝不抢占等跨度主候选位。
        assert!(matches!(out.seats[3], FusedSeat::Existing { index: 3, .. }));
    }

    #[test]
    fn tier1_constrained_beam_rescues_missing_sentence_without_losing_tier0_rows() {
        let req = base_request(vec![
            sample_candidate(0, "整了血铅", true, Some(-11.4)),
            sample_candidate(1, "整了些钱", true, Some(-12.1)),
            sample_candidate(2, "整", false, Some(-4.0)),
        ]);
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "整了血铅".into(),
                    neural_log_prob: -24.0,
                },
                NeuralScoredCandidate {
                    text: "整了些钱".into(),
                    neural_log_prob: -15.0,
                },
            ],
            beam_candidates: vec![NeuralBeamCandidate {
                text: "挣了些钱".into(),
                neural_log_prob: -11.1,
                path_index: 0,
                token_count: 3,
            }],
        };
        let out = fuse_neural_response(3, &req, &resp, NeuralFusionWeights::default(), false);
        assert!(out.moved);
        assert!(out.tier1_rescued);
        assert_eq!(out.top1_text.as_deref(), Some("挣了些钱"));
        assert_eq!(out.seats.len(), 4);
        assert!(matches!(
            &out.seats[0],
            FusedSeat::InjectedBeam { text, .. } if text == "挣了些钱"
        ));
        // 原 0, 1, 2 三条 Tier 0 候选必须无一丢失。
        let kept: HashSet<usize> = out
            .seats
            .iter()
            .filter_map(|s| match s {
                FusedSeat::Existing { index, .. } => Some(*index),
                FusedSeat::InjectedBeam { .. } => None,
            })
            .collect();
        assert_eq!(kept, HashSet::from([0, 1, 2]));
    }

    #[test]
    fn beam_candidate_duplicate_of_tier0_is_deduplicated_not_injected() {
        let req = base_request(vec![
            sample_candidate(0, "整了些钱", true, Some(-12.0)),
            sample_candidate(1, "挣了些钱", true, Some(-12.4)),
        ]);
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "整了些钱".into(),
                    neural_log_prob: -14.0,
                },
                NeuralScoredCandidate {
                    text: "挣了些钱".into(),
                    neural_log_prob: -11.5,
                },
            ],
            beam_candidates: vec![NeuralBeamCandidate {
                text: "挣了些钱".into(),
                neural_log_prob: -11.2,
                path_index: 0,
                token_count: 3,
            }],
        };
        let out = fuse_neural_response(2, &req, &resp, NeuralFusionWeights::default(), false);
        assert_eq!(out.seats.len(), 2);
        assert!(!out.tier1_rescued);
        assert!(matches!(out.seats[0], FusedSeat::Existing { index: 1, .. }));
        assert!(matches!(out.seats[1], FusedSeat::Existing { index: 0, .. }));
    }

    #[test]
    fn late_arrival_lock_slot_0_strictly_preserves_leader_and_upgrades_tail() {
        let req = base_request(vec![
            sample_candidate(0, "整了血铅", true, Some(-11.4)),
            sample_candidate(1, "政了血铅", true, Some(-12.0)),
            sample_candidate(2, "挣了些钱", true, Some(-12.5)),
        ]);
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "整了血铅".into(),
                    neural_log_prob: -24.0,
                },
                NeuralScoredCandidate {
                    text: "政了血铅".into(),
                    neural_log_prob: -26.0,
                },
                NeuralScoredCandidate {
                    text: "挣了些钱".into(),
                    neural_log_prob: -11.0,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(3, &req, &resp, NeuralFusionWeights::default(), true);
        assert!(out.slot0_locked);
        assert!(out.moved);
        // 槽 0 严格保持原首候选不动，槽 1 立即升级为神经首选。
        assert!(matches!(out.seats[0], FusedSeat::Existing { index: 0, .. }));
        assert!(matches!(out.seats[1], FusedSeat::Existing { index: 2, .. }));
        assert!(matches!(out.seats[2], FusedSeat::Existing { index: 1, .. }));
    }

    #[test]
    fn overlong_continuation_never_hijacks_equal_span_slot_0() {
        let mut req = base_request(vec![
            sample_candidate(0, "现在", true, Some(-6.5)),
            sample_candidate(1, "显在", true, Some(-11.0)),
            sample_candidate(2, "现在已经", false, Some(-5.0)),
        ]);
        req.raw_input = "xianzai".into();
        req.segmentation = "xian'zai".into();
        req.syllable_paths = vec![NeuralSyllablePath {
            syllables: vec!["xian".into(), "zai".into()],
            channel_log_prob: 0.0,
            is_complete: true,
        }];
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "现在".into(),
                    neural_log_prob: -6.0,
                },
                NeuralScoredCandidate {
                    text: "显在".into(),
                    neural_log_prob: -12.0,
                },
                NeuralScoredCandidate {
                    text: "现在已经".into(),
                    neural_log_prob: -3.0,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(3, &req, &resp, NeuralFusionWeights::default(), false);
        assert!(!out.moved);
        assert!(matches!(out.seats[0], FusedSeat::Existing { index: 0, .. }));
    }

    #[test]
    fn pinned_slot_0_is_never_displaced() {
        let mut pinned = sample_candidate(0, "整了血铅", true, Some(-11.4));
        pinned.pinned = true;
        let req = base_request(vec![
            pinned,
            sample_candidate(1, "挣了些钱", true, Some(-12.0)),
        ]);
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "整了血铅".into(),
                    neural_log_prob: -25.0,
                },
                NeuralScoredCandidate {
                    text: "挣了些钱".into(),
                    neural_log_prob: -10.0,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(2, &req, &resp, NeuralFusionWeights::default(), false);
        assert!(out.slot0_locked);
        assert!(matches!(out.seats[0], FusedSeat::Existing { index: 0, .. }));
    }

    #[test]
    fn context_window_preserves_punctuation_spaces_ascii_and_emoji() {
        let ctx = "昨天刚聊完，这个兄弟 API 2.0 应该多少还是🙂，";
        assert_eq!(neural_context_window(ctx), ctx);
        let long: String = (0..600)
            .map(|i| if i % 5 == 0 { '，' } else { '字' })
            .collect();
        let clipped = neural_context_window(&long);
        assert_eq!(clipped.chars().count(), NEURAL_CONTEXT_MAX_CHARS);
        assert!(clipped.contains('，'));
    }

    #[test]
    fn build_syllable_paths_enumerates_hiatus_alternatives_with_channel_cost() {
        let paths = build_syllable_paths("xian", "xian", 4);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].syllables, vec!["xian"]);
        assert_eq!(paths[0].channel_log_prob, 0.0);
        assert_eq!(paths[1].syllables, vec!["xi", "an"]);
        assert!(paths[1].channel_log_prob < 0.0);
    }

    #[test]
    fn cross_source_bucket_alignment_prevents_collocation_and_rare_word_bias() {
        // 模拟 Collocation(+2.60) 与未测频 Database(None) 同台竞争，以及 UniqueBeam 与极冷僻词(-19.67) 共池。
        let mut coll = sample_candidate(0, "我没高兴", true, Some(2.60));
        coll.source = CandidateSource::Collocation as u8;
        let mut dict_unmeasured = sample_candidate(1, "我没关系", true, None);
        dict_unmeasured.source = CandidateSource::Database as u8;
        let mut dict_rare = sample_candidate(2, "我每管席", true, Some(-19.67));
        dict_rare.source = CandidateSource::Database as u8;

        let mut req = base_request(vec![coll, dict_unmeasured, dict_rare]);
        req.syllable_paths = vec![NeuralSyllablePath {
            syllables: vec!["wo".into(), "mei".into(), "guan".into(), "xi".into()],
            channel_log_prob: 0.0,
            is_complete: true,
        }];
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "我没高兴".into(),
                    neural_log_prob: -12.5,
                },
                NeuralScoredCandidate {
                    text: "我没关系".into(),
                    neural_log_prob: -9.3,
                },
                NeuralScoredCandidate {
                    text: "我每管席".into(),
                    neural_log_prob: -26.0,
                },
            ],
            beam_candidates: vec![NeuralBeamCandidate {
                text: "我没观息".into(),
                neural_log_prob: -11.0,
                path_index: 0,
                token_count: 3,
            }],
        };
        let out = fuse_neural_response(3, &req, &resp, NeuralFusionWeights::default(), false);
        assert_eq!(out.top1_text.as_deref(), Some("我没关系"));
        // UniqueBeam 不受极冷僻词 -19.67 拖累，稳居第二。
        assert!(matches!(
            &out.seats[1],
            FusedSeat::InjectedBeam { text, .. } if text == "我没观息"
        ));
    }

    #[test]
    fn two_char_dictionary_word_beats_non_lexical_split_and_stays_contiguous_before_continuations()
    {
        // 真实双字词典整词 `但凡` 与非成词 `1+1` 拆字 `但反`、长词续写 `淡饭粗茶`/`淡饭黄齑` 共池时：
        // 1. `但反` 不再因独立分桶归零而以微小神经波动反超真实词典整词 `但凡`；
        // 2. 前 4 个等跨度主候选在首页首段连续呈现，长词续写顺延其后，不会将真实双字词挤到第 2 页。
        let mut dict_a = sample_candidate(0, "但凡", true, Some(-9.66));
        dict_a.source = CandidateSource::Database as u8;
        let mut dict_b = sample_candidate(1, "单反", true, Some(-9.93));
        dict_b.source = CandidateSource::Database as u8;
        let mut dict_c = sample_candidate(2, "胆矾", true, Some(-12.66));
        dict_c.source = CandidateSource::Database as u8;
        let mut cont_1 = sample_candidate(3, "淡饭粗茶", false, Some(-15.0));
        cont_1.source = CandidateSource::Database as u8;
        let mut cont_2 = sample_candidate(4, "淡饭黄齑", false, Some(-15.0));
        cont_2.source = CandidateSource::Database as u8;
        let mut split_a = sample_candidate(5, "但反", true, Some(-11.81));
        split_a.source = CandidateSource::Collocation as u8;

        let mut req = base_request(vec![dict_a, dict_b, dict_c, cont_1, cont_2, split_a]);
        req.raw_input = "danfan".into();
        req.segmentation = "dan'fan".into();
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "但凡".into(),
                    neural_log_prob: -12.91,
                },
                NeuralScoredCandidate {
                    text: "单反".into(),
                    neural_log_prob: -16.31,
                },
                NeuralScoredCandidate {
                    text: "胆矾".into(),
                    neural_log_prob: -23.73,
                },
                NeuralScoredCandidate {
                    text: "但反".into(),
                    neural_log_prob: -12.82,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(6, &req, &resp, NeuralFusionWeights::default(), false);
        assert_eq!(out.top1_text.as_deref(), Some("但凡"));
        // 前 4 席全为等跨度主候选（0, 5, 1, 2），续写词（3, 4）顺延至第 5–6 席且全员无损保留。
        let seat_indices: Vec<usize> = out
            .seats
            .iter()
            .map(|s| match s {
                FusedSeat::Existing { index, .. } => *index,
                FusedSeat::InjectedBeam { .. } => usize::MAX,
            })
            .collect();
        assert_eq!(seat_indices, vec![0, 5, 1, 2, 3, 4]);
    }

    #[test]
    fn exact_literal_word_caps_secondary_typos_on_page_one_without_dropping_recall() {
        // 输入 `xiongtai` 存在精确字面词典整词 `兄台` 与 3 条纠错候选 `形态*`/`性态*`/`邢台*` 时：
        // Tier 1 融合后首页前 4 席仅留最强 1 条纠错（`形态*`），`性态*`/`邢台*` 顺延至第 5 席之后且零丢失；
        // 若最强纠错本身赢下 `#1`，仍稳居 `#1` 不受影响。
        let mut dict_exact = sample_candidate(0, "兄台", true, Some(-11.0));
        dict_exact.source = CandidateSource::Database as u8;
        let mut typo_1 = sample_candidate(1, "形态", true, Some(-8.0));
        typo_1.source = CandidateSource::Generated as u8;
        typo_1.channel_log_prob = -7.0;
        let mut typo_2 = sample_candidate(2, "性态", true, Some(-9.5));
        typo_2.source = CandidateSource::Generated as u8;
        typo_2.channel_log_prob = -7.0;
        let mut typo_3 = sample_candidate(3, "邢台", true, Some(-9.0));
        typo_3.source = CandidateSource::Generated as u8;
        typo_3.channel_log_prob = -7.0;
        let mut lit_2 = sample_candidate(4, "熊太", true, Some(-13.0));
        lit_2.source = CandidateSource::Generated as u8;
        let mut lit_3 = sample_candidate(5, "凶太", true, Some(-13.5));
        lit_3.source = CandidateSource::Generated as u8;

        let req = base_request(vec![dict_exact, typo_1, typo_2, typo_3, lit_2, lit_3]);
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "兄台".into(),
                    neural_log_prob: -17.6,
                },
                NeuralScoredCandidate {
                    text: "形态".into(),
                    neural_log_prob: -11.1,
                },
                NeuralScoredCandidate {
                    text: "性态".into(),
                    neural_log_prob: -12.0,
                },
                NeuralScoredCandidate {
                    text: "邢台".into(),
                    neural_log_prob: -12.5,
                },
                NeuralScoredCandidate {
                    text: "熊太".into(),
                    neural_log_prob: -20.5,
                },
                NeuralScoredCandidate {
                    text: "凶太".into(),
                    neural_log_prob: -21.0,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(6, &req, &resp, NeuralFusionWeights::default(), false);
        assert_eq!(out.top1_text.as_deref(), Some("兄台"));
        let seat_indices: Vec<usize> = out
            .seats
            .iter()
            .map(|s| match s {
                FusedSeat::Existing { index, .. } => *index,
                FusedSeat::InjectedBeam { .. } => usize::MAX,
            })
            .collect();
        // 前 4 席：0(兄台), 1(形态*), 4(熊太), 5(凶太)；次优纠错 2(性态*), 3(邢台*) 顺延至后且无损保留。
        assert_eq!(seat_indices, vec![0, 1, 4, 5, 2, 3]);
    }

    #[test]
    fn mixed_jianpin_exact_dictionary_word_beats_non_lexical_stitch_and_shorter_typo() {
        // 1. 混拼 `que'g'x` 中，真实三字词典整词 `缺根弦`（Database）胜过非词典拼接 `缺钙性`（Generated）。
        let mut exact_idiom = sample_candidate(0, "缺根弦", true, Some(-14.81));
        exact_idiom.source = CandidateSource::Database as u8;
        let mut stitch = sample_candidate(1, "缺钙性", true, Some(-7.28));
        stitch.source = CandidateSource::Generated as u8;
        let mut req = base_request(vec![exact_idiom, stitch]);
        req.raw_input = "quegx".into();
        req.segmentation = "que'g'x".into();
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "缺根弦".into(),
                    neural_log_prob: -20.71,
                },
                NeuralScoredCandidate {
                    text: "缺钙性".into(),
                    neural_log_prob: -19.98,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(2, &req, &resp, NeuralFusionWeights::default(), false);
        assert_eq!(out.top1_text.as_deref(), Some("缺根弦"));

        // 2. 简拼 `shj` / 混拼 `tu'ran'j` 中，更短字数纠错候选（`是`/`突然`）须补偿缺失字数信息量，不反超完整字面词。
        let mut short_typo = sample_candidate(0, "是", true, Some(-2.77));
        short_typo.source = CandidateSource::Database as u8;
        short_typo.channel_log_prob = -2.8;
        let mut exact_jp = sample_candidate(1, "时间", true, Some(-6.54));
        exact_jp.source = CandidateSource::Database as u8;
        let mut req2 = base_request(vec![short_typo, exact_jp]);
        req2.raw_input = "shj".into();
        req2.segmentation = "shj".into();
        let resp2 = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "是".into(),
                    neural_log_prob: -3.12,
                },
                NeuralScoredCandidate {
                    text: "时间".into(),
                    neural_log_prob: -7.31,
                },
            ],
            beam_candidates: vec![],
        };
        let out2 = fuse_neural_response(2, &req2, &resp2, NeuralFusionWeights::default(), false);
        assert_eq!(out2.top1_text.as_deref(), Some("时间"));

        // 3. 空上文完整全拼存在严格精确词典整词（`exact_literal_dict == true`，如 `"" + naizi -> 奶子`）时，
        //    邻键高频错拼（`孩子*`）在 `-4.6 - 2.5 = -7.1 nats` 信道边际下不得劫持首位；
        //    而仅存在前缀补全词典词（`exact_literal_dict == false`，如 `"" + meiganxi` 下的 `没敢想`）时，
        //    真实错拼（`没关系*`）照常胜出。
        let mut lit_word = sample_candidate(0, "奶子", true, Some(-10.52));
        lit_word.source = CandidateSource::Database as u8;
        lit_word.exact_literal_dict = true;
        let mut high_freq_typo = sample_candidate(1, "孩子", true, Some(-12.53));
        high_freq_typo.source = CandidateSource::Generated as u8;
        high_freq_typo.channel_log_prob = -4.6;
        let mut req3 = base_request(vec![lit_word, high_freq_typo]);
        req3.context.clear();
        req3.raw_input = "naizi".into();
        req3.segmentation = "nai'zi".into();
        let resp3 = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "奶子".into(),
                    neural_log_prob: -15.09,
                },
                NeuralScoredCandidate {
                    text: "孩子".into(),
                    neural_log_prob: -8.73,
                },
            ],
            beam_candidates: vec![],
        };
        let out3 = fuse_neural_response(2, &req3, &resp3, NeuralFusionWeights::default(), false);
        assert_eq!(out3.top1_text.as_deref(), Some("奶子"));

        let mut prefix_dict = sample_candidate(0, "没敢想", true, Some(-12.0));
        prefix_dict.source = CandidateSource::Database as u8;
        prefix_dict.exact_literal_dict = false;
        let mut real_typo = sample_candidate(1, "没关系", true, Some(-10.0));
        real_typo.source = CandidateSource::Generated as u8;
        real_typo.channel_log_prob = -2.8;
        let mut req3b = base_request(vec![prefix_dict, real_typo]);
        req3b.context.clear();
        req3b.raw_input = "meiganxi".into();
        req3b.segmentation = "mei'gan'xi".into();
        let resp3b = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "没敢想".into(),
                    neural_log_prob: -15.0,
                },
                NeuralScoredCandidate {
                    text: "没关系".into(),
                    neural_log_prob: -7.5,
                },
            ],
            beam_candidates: vec![],
        };
        let out3b = fuse_neural_response(2, &req3b, &resp3b, NeuralFusionWeights::default(), false);
        assert_eq!(out3b.top1_text.as_deref(), Some("没关系"));

        // 4. 有语境 3–8 音节简拼（如 `现在是北京时间上午 + j'd's's'f`）允许采纳 Stage B 受限束搜索救回候选。
        let mut t0 = sample_candidate(0, "决定是十分", true, Some(5.12));
        t0.source = CandidateSource::Collocation as u8;
        let mut req4 = base_request(vec![t0]);
        req4.context = "现在是北京时间上午".into();
        req4.raw_input = "jdssf".into();
        req4.segmentation = "j'd's's'f".into();
        req4.syllable_paths = vec![NeuralSyllablePath {
            syllables: vec!["j".into(), "d".into(), "s".into(), "s".into(), "f".into()],
            channel_log_prob: 0.0,
            is_complete: false,
        }];
        let resp4 = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![NeuralScoredCandidate {
                text: "决定是十分".into(),
                neural_log_prob: -30.7,
            }],
            beam_candidates: vec![NeuralBeamCandidate {
                text: "九点三十分".into(),
                neural_log_prob: -6.56,
                path_index: 0,
                token_count: 3,
            }],
        };
        let out4 = fuse_neural_response(1, &req4, &resp4, NeuralFusionWeights::default(), false);
        assert_eq!(out4.top1_text.as_deref(), Some("九点三十分"));

        // 5. 同池已存在 2–4 字精确词典整词时，Stage B 生成的同字数非词典拼接
        //    必须承担与 Tier 0 非词典拼接一致的结构先验惩罚，不得绕过惩罚反超精确词典整词。
        let mut dict_four = sample_candidate(0, "不可或缺", true, Some(-12.2));
        dict_four.source = CandidateSource::Database as u8;
        let mut req5 = base_request(vec![dict_four]);
        req5.context = "这件事对他来说".into();
        req5.raw_input = "bkhuoq".into();
        req5.segmentation = "b'k'huo'q".into();
        req5.syllable_paths = vec![NeuralSyllablePath {
            syllables: vec!["b".into(), "k".into(), "huo".into(), "q".into()],
            channel_log_prob: 0.0,
            is_complete: false,
        }];
        let resp5 = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![NeuralScoredCandidate {
                text: "不可或缺".into(),
                neural_log_prob: -14.2,
            }],
            beam_candidates: vec![NeuralBeamCandidate {
                text: "不可或取".into(),
                neural_log_prob: -11.5,
                path_index: 0,
                token_count: 2,
            }],
        };
        let out5 = fuse_neural_response(1, &req5, &resp5, NeuralFusionWeights::default(), false);
        assert_eq!(out5.top1_text.as_deref(), Some("不可或缺"));
    }

    #[test]
    fn pinned_fixed_position_never_displaced_by_neural_fusion() {
        // 模拟 `差价 + ma` 中「嘛」被用户固定在第 4 位（下标 3），
        // 即便「嘛」因 `pinned = true` 不发往神经打分，也不得被前 4 连续等跨度逻辑误判为续写词挤到第 5 位。
        let mut c0 = sample_candidate(0, "吗", true, Some(-4.0));
        c0.source = CandidateSource::Database as u8;
        let mut c1 = sample_candidate(1, "马", true, Some(-4.5));
        c1.source = CandidateSource::Database as u8;
        let mut c2 = sample_candidate(2, "妈", true, Some(-5.0));
        c2.source = CandidateSource::Database as u8;
        // 下标 3 为用户固定的「嘛」（不在 candidates 中，通过 pinned_seats = [(3, 3)] 锁位）
        let mut c4 = sample_candidate(4, "麻", true, Some(-5.5));
        c4.source = CandidateSource::Database as u8;
        let mut c5 = sample_candidate(5, "码", true, Some(-6.0));
        c5.source = CandidateSource::Database as u8;
        let mut req = base_request(vec![c0, c1, c2, c4, c5]);
        req.context = "差价".into();
        req.raw_input = "ma".into();
        req.segmentation = "ma".into();
        req.pinned_seats = vec![(3, 3)];
        let resp = NeuralDecodeResponse {
            ok: true,
            session: 1,
            generation: 10,
            candidate_scores: vec![
                NeuralScoredCandidate {
                    text: "吗".into(),
                    neural_log_prob: -2.0,
                },
                NeuralScoredCandidate {
                    text: "码".into(),
                    neural_log_prob: -2.2,
                },
                NeuralScoredCandidate {
                    text: "马".into(),
                    neural_log_prob: -3.0,
                },
                NeuralScoredCandidate {
                    text: "麻".into(),
                    neural_log_prob: -3.5,
                },
                NeuralScoredCandidate {
                    text: "妈".into(),
                    neural_log_prob: -4.0,
                },
            ],
            beam_candidates: vec![],
        };
        let out = fuse_neural_response(6, &req, &resp, NeuralFusionWeights::default(), false);
        let order: Vec<usize> = out
            .seats
            .iter()
            .map(|s| match *s {
                FusedSeat::Existing { index, .. } => index,
                FusedSeat::InjectedBeam { .. } => usize::MAX,
            })
            .collect();
        // 下标 3（嘛）稳坐第 4 位（位置 3），其余未固定候选按神经融合分顺流排在 0,1,2,4,5。
        assert_eq!(order, vec![0, 5, 1, 3, 4, 2]);

        // 即使迟到锁槽 0（lock_slot_0 = true），下标 3 也绝不能被连带平移。
        let out_locked = fuse_neural_response(6, &req, &resp, NeuralFusionWeights::default(), true);
        assert!(matches!(
            out_locked.seats[0],
            FusedSeat::Existing { index: 0, .. }
        ));
        assert!(matches!(
            out_locked.seats[3],
            FusedSeat::Existing { index: 3, .. }
        ));
    }
}
