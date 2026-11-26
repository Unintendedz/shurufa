//! 音节图与统一概率音节有向无环图 `SyllableDag`（quanpin.md §4.1 + Shurufa v2 前端）。
//!
//! 保留既有 `SyllableGraph` 与 `enumerate_complete_segmentations` 供短串及 `normalize_full_pinyin` 使用，
//! 新增支持任意句长的零声母 A* K-best 切分器，以及统一编码合法切分、简全混拼与受控键盘纠错信道的 `SyllableDag`。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use super::autocorrect::{correction_targets_for_piece, MAX_AUTOCORRECT_INPUT_LENGTH};
use super::jianpin::is_valid_initial_segment;
use super::syllables::{intact_piece, is_intact, normalized_syllable, MAX_SYLLABLE_LENGTH};
use super::typos::{
    geminate_boundary_syllable, probabilistic_syllable_typos, two_letter_neighbor_typos,
    CHANNEL_GEMINATE_ELISION_PENALTY,
};
use crate::types::autocorrect_type;

pub const SYLLABLE_GRAPH_PATH_LIMIT: usize = 32;

/// 零声母多切分每多拆出一个音节边界的小额真实信道代价（自然对数 nat，绝不使用 `-30 nats` 死刑惩罚）。
pub const DAG_HIATUS_SPLIT_COST: f64 = -0.60;
/// 长句 A* 搜索中，句中单字母零声母 `a`/`o`（如 `gu'o`、`xi'a`）的额外排序偏好偏移（仅用于先让出 `xi'an`/`ti'an`/`ji'e` 等真实零声母切分）。
const INTERIOR_BARE_AO_SEARCH_BIAS_MILLI: i32 = 750;
/// 简拼单声母或 `zh/ch/sh` 声母边的省略信道代价（自然对数 nat）。
pub const DAG_INITIAL_EDGE_COST: f64 = -1.60;
/// `SyllableDag` 每个字节跨度保留的最高概率纠错弧上限。
pub const DAG_MAX_TYPO_EDGES_PER_SPAN: usize = 3;
/// 单条路径允许累计的最大键盘纠错边数。
pub const DAG_MAX_TYPO_EDGES_PER_PATH: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyllableEdge {
    pub end: usize,
    pub syllable: &'static str,
}

/// `edges[start]` lists the intact syllables starting at `start`, longest first, pruned to those from which the end is reachable. Empty input or input containing `'` has no edges.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyllableGraph {
    pub input_length: usize,
    pub edges: Vec<Vec<SyllableEdge>>,
}

/// 统一概率音节 DAG 中的边类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DagEdgeKind {
    /// 严格字面完整音节边。
    Exact,
    /// 零声母多切分产生的合法音节边。
    HiatusSplit,
    /// 单字母或 `zh/ch/sh` 简拼声母边。
    Initial,
    /// 受控键盘噪声信道纠错边（含合法音节纠错与非法拼写纠错）。
    Typo,
}

/// 跨音节同辅音吞音恢复边对前一音节韵尾的约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeminateCoda {
    None,
    /// 前一音节须以单 `n`（非 `ng`）结尾，当前边起点补回 `n` 构成 `n-` 声母音节。
    SingleN,
    /// 前一音节须以 `ng` 结尾，当前边起点补回 `g` 构成 `g-` 声母音节。
    NasalNg,
}

impl GeminateCoda {
    fn index(self) -> usize {
        match self {
            Self::None => 0,
            Self::SingleN => 1,
            Self::NasalNg => 2,
        }
    }

    fn matches(self, trailing: Self) -> bool {
        self == Self::None || self == trailing
    }
}

/// `SyllableDag` 中的一条有向边，覆盖原始输入的 `[start, end)` 字节区间。
#[derive(Debug, Clone, PartialEq)]
pub struct DagEdge {
    pub start: usize,
    pub end: usize,
    /// 规范音节或简拼声母（已将 `lue/nue/jv` 等别名归一）。
    pub syllable: String,
    pub kind: DagEdgeKind,
    /// 若为跨音节吞音恢复边，要求前一合法音节匹配的韵尾类别。
    pub required_coda: GeminateCoda,
    /// 该边的对数信道概率 $\log P_{\text{channel}}(X_{\text{start..end}} \mid \text{syllable}) \le 0$（单位：nat）。
    pub channel_log_prob: f64,
}

/// 统一概率音节有向无环图：节点为字节偏移 `0..=input_length`，边为合法切分、简拼声母与受控纠错边。
#[derive(Debug, Clone, Default)]
pub struct SyllableDag {
    pub input_length: usize,
    pub edges: Vec<Vec<DagEdge>>,
}

/// 从 `SyllableDag` 或 `SyllableGraph` 提取的一条完整覆盖 `[0, input_length)` 的候选音节路径。
#[derive(Debug, Clone, PartialEq)]
pub struct DagPath {
    pub syllables: Vec<String>,
    /// 该路径所有切分/简拼/纠错信道代价总和（自然对数 nat，<= 0）。
    pub channel_log_prob: f64,
    /// 相对最小音节数多拆出的额外音节边界数（用于零声母多切分追踪）。
    pub extra_splits: usize,
    /// 路径中简拼声母边数量。
    pub initial_edges: usize,
    /// 路径中键盘纠错边数量。
    pub typo_edges: usize,
    /// 路径中跨音节吞音恢复边数量。
    pub geminate_edges: usize,
    /// 是否全部为完整合法音节（供 `decode_constrained` 跳过半截声母受限展开）。
    pub is_complete: bool,
}

/// QU:245-285.
pub fn build_syllable_graph(pinyin: &str) -> SyllableGraph {
    let bytes = pinyin.as_bytes();
    let length = bytes.len();
    let mut graph = SyllableGraph {
        input_length: length,
        edges: vec![Vec::new(); length + 1],
    };
    if pinyin.is_empty() || pinyin.contains('\'') {
        return graph;
    }
    for start in 0..length {
        let bucket_capacity = MAX_SYLLABLE_LENGTH.min(length - start);
        for end in (start + 1..=length.min(start + MAX_SYLLABLE_LENGTH)).rev() {
            if let Some(syllable) = intact_piece(&bytes[start..end]) {
                let edges = &mut graph.edges[start];
                if edges.capacity() == 0 {
                    edges.reserve(bucket_capacity);
                }
                edges.push(SyllableEdge { end, syllable });
            }
        }
    }
    let mut reaches_end = vec![false; length + 1];
    reaches_end[length] = true;
    for start in (0..length).rev() {
        let edges = &mut graph.edges[start];
        edges.retain(|edge| reaches_end[edge.end]);
        reaches_end[start] = !edges.is_empty();
    }
    graph
}

/// Depth-first from 0 in edge order, stopping at `path_limit` paths (QU:288-325).
pub fn enumerate_complete_segmentations(
    graph: &SyllableGraph,
    path_limit: usize,
) -> Vec<Vec<String>> {
    fn visit(
        graph: &SyllableGraph,
        position: usize,
        path_limit: usize,
        current: &mut Vec<&'static str>,
        result: &mut Vec<Vec<String>>,
    ) {
        if result.len() >= path_limit {
            return;
        }
        if position == graph.input_length {
            if result.is_empty() {
                result.reserve_exact(path_limit);
            }
            result.push(
                current
                    .iter()
                    .map(|syllable| (*syllable).to_owned())
                    .collect(),
            );
            return;
        }
        for edge in &graph.edges[position] {
            if current.capacity() == 0 {
                current.reserve_exact(graph.input_length);
            }
            current.push(edge.syllable);
            visit(graph, edge.end, path_limit, current, result);
            current.pop();
            if result.len() >= path_limit {
                return;
            }
        }
    }

    let mut result = Vec::new();
    if path_limit == 0 || graph.input_length == 0 || graph.edges.len() != graph.input_length + 1 {
        return result;
    }
    visit(graph, 0, path_limit, &mut Vec::new(), &mut result);
    result
}

/// 面向任意句长的 A* K-best 完整切分枚举器：先出全部最小音节数切分（如 `fang'an`/`fan'gan`、`ming'e`/`min'ge`、`dang'an`/`dan'gan`），
/// 再按真实小额边界信道代价（`DAG_HIATUS_SPLIT_COST = -0.60 nats`）优先展开 `xi'an`/`pi'ao`/`ti'an`/`ji'e`/`li'an` 等零声母多切分。
pub fn enumerate_kbest_complete_segmentations(
    graph: &SyllableGraph,
    max_extra_splits: usize,
    path_limit: usize,
) -> Vec<DagPath> {
    let n = graph.input_length;
    if path_limit == 0 || n == 0 || graph.edges.len() != n + 1 {
        return Vec::new();
    }
    const STEP_MILLI: i32 = 600;
    let edge_step_milli = |start: usize, edge: &SyllableEdge| -> i32 {
        let bare_interior_ao = start > 0 && edge.end < n && matches!(edge.syllable, "a" | "o");
        STEP_MILLI
            + if bare_interior_ao {
                INTERIOR_BARE_AO_SEARCH_BIAS_MILLI
            } else {
                0
            }
    };

    // 反向 DP 求出每个起点到终点的最小音节数与最小 A* 搜索代价（精确可采纳启发）。
    let mut min_segments = vec![usize::MAX; n + 1];
    let mut min_cost = vec![i32::MAX; n + 1];
    min_segments[n] = 0;
    min_cost[n] = 0;
    for pos in (0..n).rev() {
        for edge in &graph.edges[pos] {
            if min_segments[edge.end] == usize::MAX {
                continue;
            }
            min_segments[pos] = min_segments[pos].min(1 + min_segments[edge.end]);
            let candidate = edge_step_milli(pos, edge).saturating_add(min_cost[edge.end]);
            if candidate < min_cost[pos] {
                min_cost[pos] = candidate;
            }
        }
    }
    let global_min = min_segments[0];
    if global_min == usize::MAX {
        return Vec::new();
    }
    let max_segments = global_min.saturating_add(max_extra_splits);

    // 使用前驱池存储部分路径，避免堆中反复复制整条 `Vec<&str>`。
    struct Node {
        pos: usize,
        segments: usize,
        g_cost: i32,
        parent: usize,
        syllable: &'static str,
    }
    let mut nodes: Vec<Node> = Vec::with_capacity(path_limit.saturating_mul(16).min(1024));
    nodes.push(Node {
        pos: 0,
        segments: 0,
        g_cost: 0,
        parent: usize::MAX,
        syllable: "",
    });
    // 堆键：(f_cost, extra_segments_lower_bound, arrival_seq, node_id)
    let mut heap: BinaryHeap<Reverse<(i32, usize, usize, usize)>> = BinaryHeap::new();
    heap.push(Reverse((min_cost[0], 0, 0, 0)));

    let mut result = Vec::with_capacity(path_limit.min(16));
    let mut seen: HashSet<Vec<&'static str>> = HashSet::with_capacity(path_limit.min(16));
    let max_expansions = path_limit.saturating_mul(64).max(512);
    let mut expansions = 0usize;
    let mut arrival = 1usize;

    while let Some(Reverse((_, _, _, node_id))) = heap.pop() {
        if result.len() >= path_limit || expansions >= max_expansions {
            break;
        }
        expansions += 1;
        let pos = nodes[node_id].pos;
        let segments = nodes[node_id].segments;
        let g_cost = nodes[node_id].g_cost;
        if pos == n {
            let mut path = Vec::with_capacity(segments);
            let mut cursor = node_id;
            while nodes[cursor].parent != usize::MAX {
                path.push(nodes[cursor].syllable);
                cursor = nodes[cursor].parent;
            }
            path.reverse();
            if seen.insert(path.clone()) {
                let extra_splits = segments.saturating_sub(global_min);
                result.push(DagPath {
                    syllables: path.into_iter().map(str::to_owned).collect(),
                    channel_log_prob: (extra_splits as f64) * DAG_HIATUS_SPLIT_COST,
                    extra_splits,
                    initial_edges: 0,
                    typo_edges: 0,
                    geminate_edges: 0,
                    is_complete: true,
                });
            }
            continue;
        }
        for edge in &graph.edges[pos] {
            let next_segments = segments + 1;
            let remaining_min = min_segments[edge.end];
            if remaining_min == usize::MAX || next_segments + remaining_min > max_segments {
                continue;
            }
            let next_g = g_cost.saturating_add(edge_step_milli(pos, edge));
            let f_cost = next_g.saturating_add(min_cost[edge.end]);
            let next_id = nodes.len();
            nodes.push(Node {
                pos: edge.end,
                segments: next_segments,
                g_cost: next_g,
                parent: node_id,
                syllable: edge.syllable,
            });
            let lb_extra = (next_segments + remaining_min).saturating_sub(global_min);
            heap.push(Reverse((f_cost, lb_extra, arrival, next_id)));
            arrival += 1;
        }
    }
    result
}

/// 构建统一概率音节有向无环图 `SyllableDag`：同时承载合法完整音节、零声母多切分、简全混拼声母与受控键盘纠错弧。
pub fn build_syllable_dag(raw: &str, autocorrect_types: u32) -> SyllableDag {
    let bytes = raw.as_bytes();
    let length = bytes.len();
    let mut dag = SyllableDag {
        input_length: length,
        edges: vec![Vec::new(); length + 1],
    };
    if raw.is_empty()
        || raw.contains('\'')
        || length > MAX_AUTOCORRECT_INPUT_LENGTH
        || !bytes.iter().all(u8::is_ascii_lowercase)
    {
        return dag;
    }

    // 记录每个起点是否可由完整音节直接抵达终点，辅助标注零声母切分与避免冗余展开。
    let complete_graph = build_syllable_graph(raw);
    let mut exact_reachable = vec![false; length + 1];
    exact_reachable[length] = true;
    for pos in (0..length).rev() {
        exact_reachable[pos] = !complete_graph.edges[pos].is_empty();
    }

    let mut has_single_n_coda = vec![false; length + 1];
    let mut has_ng_coda = vec![false; length + 1];

    for start in 0..length {
        let bucket = &mut dag.edges[start];
        // 1. 合法完整音节边（最长优先）。
        for end in (start + 1..=length.min(start + MAX_SYLLABLE_LENGTH)).rev() {
            let piece = &bytes[start..end];
            if let Some(syllable) = intact_piece(piece) {
                let normalized = normalized_syllable(syllable);
                if normalized.ends_with("ng") {
                    has_ng_coda[end] = true;
                } else if normalized.ends_with('n') {
                    has_single_n_coda[end] = true;
                }
                let kind = if start > 0
                    && matches!(
                        syllable,
                        "a" | "ai" | "an" | "ang" | "ao" | "e" | "ei" | "en" | "er" | "o" | "ou"
                    ) {
                    DagEdgeKind::HiatusSplit
                } else {
                    DagEdgeKind::Exact
                };
                bucket.push(DagEdge {
                    start,
                    end,
                    syllable: normalized,
                    kind,
                    required_coda: GeminateCoda::None,
                    channel_log_prob: 0.0,
                });
            }
        }

        // 2. 简拼声母边（单字母合法声母以及 `zh`/`ch`/`sh` 双字母声母）。
        let single = &raw[start..start + 1];
        if !is_intact(single) && is_valid_initial_segment(single) {
            bucket.push(DagEdge {
                start,
                end: start + 1,
                syllable: single.to_owned(),
                kind: DagEdgeKind::Initial,
                required_coda: GeminateCoda::None,
                channel_log_prob: DAG_INITIAL_EDGE_COST,
            });
        }
        if start + 2 <= length {
            let pair = &raw[start..start + 2];
            if matches!(pair, "zh" | "ch" | "sh") {
                bucket.push(DagEdge {
                    start,
                    end: start + 2,
                    syllable: pair.to_owned(),
                    kind: DagEdgeKind::Initial,
                    required_coda: GeminateCoda::None,
                    channel_log_prob: DAG_INITIAL_EDGE_COST,
                });
            }
        }

        // 3. 受控键盘噪声信道纠错边（每跨度 Top-3；合法音节与非法片段均纳入）。
        if autocorrect_types != 0 {
            for end in (start + 2..=length.min(start + MAX_SYLLABLE_LENGTH)).rev() {
                let piece = &raw[start..end];
                if is_intact(piece) {
                    for typo in probabilistic_syllable_typos(
                        piece,
                        autocorrect_types,
                        DAG_MAX_TYPO_EDGES_PER_SPAN,
                    ) {
                        bucket.push(DagEdge {
                            start,
                            end,
                            syllable: typo.syllable.to_owned(),
                            kind: DagEdgeKind::Typo,
                            required_coda: GeminateCoda::None,
                            channel_log_prob: -typo.penalty,
                        });
                    }
                } else if piece.len() >= 3 {
                    for (syllable, penalty) in correction_targets_for_piece(
                        piece,
                        autocorrect_types,
                        DAG_MAX_TYPO_EDGES_PER_SPAN,
                    ) {
                        bucket.push(DagEdge {
                            start,
                            end,
                            syllable: normalized_syllable(syllable),
                            kind: DagEdgeKind::Typo,
                            required_coda: GeminateCoda::None,
                            channel_log_prob: -penalty,
                        });
                    }
                } else if piece.len() == 2
                    && length >= 4
                    && autocorrect_types & autocorrect_type::NEIGHBOR != 0
                {
                    for typo in two_letter_neighbor_typos(piece, DAG_MAX_TYPO_EDGES_PER_SPAN) {
                        bucket.push(DagEdge {
                            start,
                            end,
                            syllable: typo.syllable.to_owned(),
                            kind: DagEdgeKind::Typo,
                            required_coda: GeminateCoda::None,
                            channel_log_prob: -typo.penalty,
                        });
                    }
                }
            }
        }

        // 4. 跨音节同辅音吞音恢复边（`-n + n-` 与 `-ng + g-`）。
        if autocorrect_types & autocorrect_type::DELETION != 0
            && (has_single_n_coda[start] || has_ng_coda[start])
        {
            for end in (start + 1..=length.min(start + MAX_SYLLABLE_LENGTH - 1)).rev() {
                let remainder = &raw[start..end];
                for (enabled, required_coda, rep_coda) in [
                    (has_single_n_coda[start], GeminateCoda::SingleN, "an"),
                    (has_ng_coda[start], GeminateCoda::NasalNg, "ang"),
                ] {
                    if !enabled {
                        continue;
                    }
                    if let Some(syllable) = geminate_boundary_syllable(rep_coda, remainder) {
                        bucket.push(DagEdge {
                            start,
                            end,
                            syllable: syllable.to_owned(),
                            kind: DagEdgeKind::Typo,
                            required_coda,
                            channel_log_prob: -CHANNEL_GEMINATE_ELISION_PENALTY,
                        });
                    }
                }
            }
        }
    }

    // 反向可达性剪枝，仅保留可抵达终点的边。
    let mut reaches_end = vec![false; length + 1];
    reaches_end[length] = true;
    for start in (0..length).rev() {
        let edges = &mut dag.edges[start];
        edges.retain(|edge| reaches_end[edge.end]);
        reaches_end[start] = !edges.is_empty();
    }
    dag
}

fn edge_out_coda(edge: &DagEdge) -> GeminateCoda {
    if matches!(edge.kind, DagEdgeKind::Exact | DagEdgeKind::HiatusSplit) {
        if edge.syllable.ends_with("ng") {
            GeminateCoda::NasalNg
        } else if edge.syllable.ends_with('n') {
            GeminateCoda::SingleN
        } else {
            GeminateCoda::None
        }
    } else {
        GeminateCoda::None
    }
}

/// 从 `SyllableDag` 提取按总信道对数概率降序排列的前 `path_limit` 条全跨度候选音节路径（受每句最多 2 处纠错约束）。
pub fn dag_candidate_paths(dag: &SyllableDag, path_limit: usize) -> Vec<DagPath> {
    extract_dag_paths(dag, path_limit, true)
}

/// 仅沿完整音节与受控纠错弧（跳过半截简拼声母边）从 `SyllableDag` 提取前 `path_limit` 条完整候选路径。
/// 供跨音节吞音恢复（`-n+n-`/`-ng+g-`）与多字母错拼尾音节校准使用，防止单字母简拼拆分（如 `fa'n'an`）挤占完整切分额度。
pub fn dag_complete_candidate_paths(dag: &SyllableDag, path_limit: usize) -> Vec<DagPath> {
    extract_dag_paths(dag, path_limit, false)
}

fn extract_dag_paths(dag: &SyllableDag, path_limit: usize, allow_initial: bool) -> Vec<DagPath> {
    let n = dag.input_length;
    if path_limit == 0 || n == 0 || dag.edges.len() != n + 1 {
        return Vec::new();
    }

    // 计算无纠错、全完整切分的最小音节数（用于标注 `extra_splits`）；不可完整切分时以全图最小步数为基准。
    let mut min_complete_segs = vec![usize::MAX; n + 1];
    min_complete_segs[n] = 0;
    for pos in (0..n).rev() {
        for edge in &dag.edges[pos] {
            if matches!(edge.kind, DagEdgeKind::Exact | DagEdgeKind::HiatusSplit)
                && min_complete_segs[edge.end] != usize::MAX
            {
                min_complete_segs[pos] =
                    min_complete_segs[pos].min(1 + min_complete_segs[edge.end]);
            }
        }
    }
    let baseline_segs = min_complete_segs[0];

    // 反向 DP 求出每 `(位置, 前驱韵尾)` 到终点的最小负对数代价（单位：毫 nat，越小越好）。
    const HIATUS_STEP_MILLI: i32 = 600;
    let edge_loss_milli = |edge: &DagEdge| -> i32 {
        let channel_loss = (-edge.channel_log_prob * 1000.0).round() as i32;
        // 完整合法音节边统一附带每段 `HIATUS_STEP_MILLI` 边界基数，使多拆出 1 个零声母边界恰好比最小切分多付 `0.60 nats`。
        let split_base = HIATUS_STEP_MILLI;
        let bare_interior_ao = edge.start > 0
            && edge.end < n
            && matches!(edge.kind, DagEdgeKind::Exact | DagEdgeKind::HiatusSplit)
            && matches!(edge.syllable.as_str(), "a" | "o");
        channel_loss
            .saturating_add(split_base)
            .saturating_add(if bare_interior_ao {
                INTERIOR_BARE_AO_SEARCH_BIAS_MILLI
            } else {
                0
            })
    };

    let mut min_loss = vec![[i32::MAX; 3]; n + 1];
    min_loss[n] = [0; 3];
    for pos in (0..n).rev() {
        for edge in &dag.edges[pos] {
            if !allow_initial && edge.kind == DagEdgeKind::Initial {
                continue;
            }
            let tail = min_loss[edge.end][edge_out_coda(edge).index()];
            if tail == i32::MAX {
                continue;
            }
            let cand = edge_loss_milli(edge).saturating_add(tail);
            for in_coda in [
                GeminateCoda::None,
                GeminateCoda::SingleN,
                GeminateCoda::NasalNg,
            ] {
                if edge.required_coda.matches(in_coda) {
                    let idx = in_coda.index();
                    if cand < min_loss[pos][idx] {
                        min_loss[pos][idx] = cand;
                    }
                }
            }
        }
    }
    let start_h = min_loss[0][GeminateCoda::None.index()];
    if start_h == i32::MAX {
        return Vec::new();
    }

    struct State {
        pos: usize,
        segments: usize,
        g_loss: i32,
        channel_sum: f64,
        trailing_coda: GeminateCoda,
        initial_edges: usize,
        typo_edges: usize,
        geminate_edges: usize,
        parent: usize,
        edge_pos: usize,
        edge_idx: usize,
    }
    let mut states: Vec<State> = Vec::with_capacity(path_limit.saturating_mul(16).min(1024));
    states.push(State {
        pos: 0,
        segments: 0,
        g_loss: 0,
        channel_sum: 0.0,
        trailing_coda: GeminateCoda::None,
        initial_edges: 0,
        typo_edges: 0,
        geminate_edges: 0,
        parent: usize::MAX,
        edge_pos: 0,
        edge_idx: 0,
    });
    let mut heap: BinaryHeap<Reverse<(i32, usize, usize)>> = BinaryHeap::new();
    heap.push(Reverse((start_h, 0, 0)));

    let mut result = Vec::with_capacity(path_limit.min(32));
    let mut seen: HashSet<Vec<String>> = HashSet::with_capacity(path_limit.min(32));
    let max_expansions = path_limit.saturating_mul(64).max(512);
    let mut expansions = 0usize;
    let mut arrival = 1usize;

    while let Some(Reverse((_, _, state_id))) = heap.pop() {
        if result.len() >= path_limit || expansions >= max_expansions {
            break;
        }
        expansions += 1;
        let pos = states[state_id].pos;
        let segments = states[state_id].segments;
        let g_loss = states[state_id].g_loss;
        let channel_sum = states[state_id].channel_sum;
        let trailing_coda = states[state_id].trailing_coda;
        let initial_edges = states[state_id].initial_edges;
        let typo_edges = states[state_id].typo_edges;
        let geminate_edges = states[state_id].geminate_edges;
        if pos == n {
            let mut syllables = Vec::with_capacity(segments);
            let mut cursor = state_id;
            while states[cursor].parent != usize::MAX {
                let st = &states[cursor];
                syllables.push(dag.edges[st.edge_pos][st.edge_idx].syllable.clone());
                cursor = st.parent;
            }
            syllables.reverse();
            if seen.insert(syllables.clone()) {
                let extra_splits =
                    if baseline_segs != usize::MAX && initial_edges == 0 && typo_edges == 0 {
                        segments.saturating_sub(baseline_segs)
                    } else {
                        0
                    };
                let is_complete = initial_edges == 0 && syllables.iter().all(|s| is_intact(s));
                result.push(DagPath {
                    syllables,
                    channel_log_prob: channel_sum + (extra_splits as f64) * DAG_HIATUS_SPLIT_COST,
                    extra_splits,
                    initial_edges,
                    typo_edges,
                    geminate_edges,
                    is_complete,
                });
            }
            continue;
        }
        for (edge_idx, edge) in dag.edges[pos].iter().enumerate() {
            if !allow_initial && edge.kind == DagEdgeKind::Initial {
                continue;
            }
            if !edge.required_coda.matches(trailing_coda) {
                continue;
            }
            let out_coda = edge_out_coda(edge);
            let tail_loss = min_loss[edge.end][out_coda.index()];
            if tail_loss == i32::MAX {
                continue;
            }
            let next_typos = typo_edges + usize::from(edge.kind == DagEdgeKind::Typo);
            if next_typos > DAG_MAX_TYPO_EDGES_PER_PATH {
                continue;
            }
            let next_initials = initial_edges + usize::from(edge.kind == DagEdgeKind::Initial);
            let next_geminates =
                geminate_edges + usize::from(edge.required_coda != GeminateCoda::None);
            let next_g = g_loss.saturating_add(edge_loss_milli(edge));
            let f_loss = next_g.saturating_add(tail_loss);
            let next_id = states.len();
            states.push(State {
                pos: edge.end,
                segments: segments + 1,
                g_loss: next_g,
                channel_sum: channel_sum + edge.channel_log_prob,
                trailing_coda: out_coda,
                initial_edges: next_initials,
                typo_edges: next_typos,
                geminate_edges: next_geminates,
                parent: state_id,
                edge_pos: pos,
                edge_idx,
            });
            heap.push(Reverse((f_loss, arrival, next_id)));
            arrival += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(pinyin: &str, limit: usize) -> Vec<String> {
        enumerate_complete_segmentations(&build_syllable_graph(pinyin), limit)
            .iter()
            .map(|path| path.join("'"))
            .collect()
    }

    #[test]
    fn complete_segmentations_come_longest_edge_first() {
        assert_eq!(paths("xian", SYLLABLE_GRAPH_PATH_LIMIT), ["xian", "xi'an"]);
        assert_eq!(
            paths("fangan", SYLLABLE_GRAPH_PATH_LIMIT),
            ["fang'an", "fan'gan"]
        );
        assert_eq!(
            paths("xianxian", 3),
            ["xian'xian", "xian'xi'an", "xi'an'xian"]
        );
        assert!(paths("nihz", SYLLABLE_GRAPH_PATH_LIMIT).is_empty());
        assert!(paths("xi'an", SYLLABLE_GRAPH_PATH_LIMIT).is_empty());
        assert!(paths("", SYLLABLE_GRAPH_PATH_LIMIT).is_empty());
        assert!(paths("xian", 0).is_empty());
    }

    #[test]
    fn kbest_complete_segmentations_preserve_zero_initial_splits_in_long_sentences() {
        let long_cases = [
            (
                "woxiageyueyaoquxianchuchai",
                "wo'xia'ge'yue'yao'qu'xi'an'chu'chai",
            ),
            ("piaotebie", "pi'ao'te'bie"),
            ("jinxingletian", "jin'xing'le'ti'an"),
            ("guodujie", "guo'du'ji'e"),
            ("helianpai", "he'li'an'pai"),
            (
                "womendexinfanganyaojinkuaidingan",
                "wo'men'de'xin'fang'an'yao'jin'kuai'ding'an",
            ),
            (
                "woyouyigexianpengyouzhoumolaiwan",
                "wo'you'yi'ge'xi'an'peng'you'zhou'mo'lai'wan",
            ),
        ];
        for (raw, expected) in long_cases {
            let graph = build_syllable_graph(raw);
            let kbest = enumerate_kbest_complete_segmentations(&graph, 2, 16);
            let joined: Vec<String> = kbest.iter().map(|path| path.syllables.join("'")).collect();
            assert!(
                joined.iter().any(|item| item == expected),
                "{raw} 缺少零声母切分 {expected}: {joined:?}"
            );
            let found = kbest
                .iter()
                .find(|path| path.syllables.join("'") == expected)
                .unwrap();
            assert!(found.is_complete);
            assert!(
                found.channel_log_prob >= -1.5,
                "零声母边界信道代价不应过重: {} ({})",
                found.channel_log_prob,
                raw
            );
        }
    }

    #[test]
    fn syllable_dag_covers_mixed_jianpin_and_legal_typos() {
        let all = autocorrect_type::TRANSPOSITION
            | autocorrect_type::NEIGHBOR
            | autocorrect_type::MISSING_OR_EXTRA;

        // 简全混拼完整覆盖，绝不截断。
        for (raw, expected) in [
            ("wxiangquchifan", "w'xiang'qu'chi'fan"),
            ("nzmhaimeilai", "n'z'm'hai'mei'lai"),
            ("dengwoyx", "deng'wo'y'x"),
            ("womeigx", "wo'mei'g'x"),
            ("mingtianzs", "ming'tian'z's"),
            ("wobzd", "wo'b'z'd"),
            ("jtwanshang", "j't'wan'shang"),
        ] {
            let dag = build_syllable_dag(raw, 0);
            let candidates = dag_candidate_paths(&dag, 8);
            assert!(
                candidates
                    .iter()
                    .any(|path| path.syllables.join("'") == expected && !path.is_complete),
                "{raw} 未覆盖混拼 {expected}: {candidates:?}"
            );
        }

        // 合法音节纠错与两字母邻键纠错均在统一 DAG 路径内。
        for (raw, expected) in [
            ("diannai", "dian'nao"),
            ("kuaisi", "kuai'di"),
            ("yingai", "ying'gai"),
            ("qinkuang", "qing'kuang"),
            ("penyou", "peng'you"),
            ("yijin", "yi'jing"),
            ("meiwentj", "mei'wen'ti"),
        ] {
            let dag = build_syllable_dag(raw, all);
            let candidates = dag_candidate_paths(&dag, 24);
            assert!(
                candidates
                    .iter()
                    .any(|path| path.syllables.join("'") == expected && path.typo_edges == 1),
                "{raw} 未生成纠错路径 {expected}: {candidates:?}"
            );
        }

        // 跨音节吞音恢复与 `*gn -> *n` 尾音校准在完整音节通道下均可稳定召回，关闭 `DELETION` 时不误触发吞音恢复。
        for (raw, expected, expected_geminates) in [
            ("chunuanhuakai", "chun'nuan'hua'kai", 1usize),
            ("changjiangungun", "chang'jiang'gun'gun", 1),
            ("zhinengongju", "zhi'neng'gong'ju", 1),
            ("fanan", "fan'nan", 1),
            ("xianan", "xian'nan", 1),
            ("kunan", "kun'nan", 1),
            ("beneng", "ben'neng", 1),
            ("mohuyign", "mo'hu'yin", 0),
        ] {
            let dag = build_syllable_dag(raw, all);
            let candidates = dag_complete_candidate_paths(&dag, 16);
            assert!(
                candidates
                    .iter()
                    .any(|path| path.syllables.join("'") == expected
                        && path.geminate_edges == expected_geminates),
                "{raw} 未生成完整纠错路径 {expected}: {candidates:?}"
            );
        }
        let no_del = autocorrect_type::NEIGHBOR | autocorrect_type::INSERTION;
        let chun_no_del =
            dag_complete_candidate_paths(&build_syllable_dag("chunuanhuakai", no_del), 16);
        assert!(
            chun_no_del.iter().all(|path| path.geminate_edges == 0),
            "未启用 DELETION 时不应生成吞音边: {chun_no_del:?}"
        );
    }

    #[test]
    fn complete_segmentations_reserve_after_the_first_path() {
        let graph = build_syllable_graph("xian");
        let paths = enumerate_complete_segmentations(&graph, 8);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths.capacity(), 8);
    }

    #[test]
    fn complete_segmentation_walk_borrows_temporary_syllables() {
        let graph = build_syllable_graph("xian");
        let (result, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            enumerate_complete_segmentations(&graph, SYLLABLE_GRAPH_PATH_LIMIT)
        });

        assert_eq!(result, [vec!["xian"], vec!["xi", "an"]]);
        assert_eq!(allocations, 7, "临时遍历路径仍复制音节: {allocations}");
    }

    #[test]
    fn dead_end_edges_are_pruned() {
        let graph = build_syllable_graph("nihz");
        assert_eq!(graph.edges.len(), 5);
        assert!(graph.edges.iter().all(Vec::is_empty));
        let graph = build_syllable_graph("zhonge");
        let from_start: Vec<_> = graph.edges[0].iter().map(|edge| edge.syllable).collect();
        // `zhong` is the only syllable at the start (`zhon`, `zho` and `zh` are prefixes), and it reaches `e`.
        assert_eq!(from_start, ["zhong"]);
    }

    #[test]
    fn edge_buckets_reserve_the_candidate_window() {
        let graph = build_syllable_graph("xianxian");

        for (start, edges) in graph.edges.iter().enumerate() {
            let expected = MAX_SYLLABLE_LENGTH.min(graph.input_length.saturating_sub(start));
            if !edges.is_empty() {
                assert!(
                    edges.capacity() >= expected,
                    "edge bucket at {start} has capacity {}, expected at least {expected}",
                    edges.capacity()
                );
            }
        }
    }

    #[test]
    fn unreadable_input_does_not_allocate_edge_buckets() {
        let graph = build_syllable_graph(&"z".repeat(256));

        assert!(graph.edges.iter().all(|edges| edges.capacity() == 0));
    }

    #[test]
    fn unreadable_graph_does_not_allocate_path_storage() {
        let graph = build_syllable_graph(&"z".repeat(256));
        let (paths, allocations) = crate::ime::personal_rerank::allocations::count(|| {
            enumerate_complete_segmentations(&graph, SYLLABLE_GRAPH_PATH_LIMIT)
        });

        assert!(paths.is_empty());
        assert_eq!(
            allocations, 0,
            "无路径时不应申请遍历工作缓冲: {allocations}"
        );
    }

    #[test]
    fn the_empty_or_mismatched_graph_has_no_paths() {
        let graph = SyllableGraph {
            input_length: 2,
            edges: Vec::new(),
        };
        let paths = enumerate_complete_segmentations(&graph, 4);
        assert!(paths.is_empty());
        assert_eq!(paths.capacity(), 0);
    }
}
