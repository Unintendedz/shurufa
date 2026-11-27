//! 为词网格建图补充汉语二字动宾离合词拆插边（如 `xu'ge'yuan -> 许个愿`、`shui'le'ge'jiao -> 睡了个觉`）。
//!
//! 出货词库 `msime-pinyin.db` 仅收录少量带中缀的三音节动宾短语（如 `睡个觉`、`洗个澡`、`帮个忙`），
//! 大量高频日常二字动宾离合词（如 `许愿`、`请假`、`加班`、`充电`、`排队`、`转账`、`退款`、`备份`、`鞠躬`）
//! 缺失 `V+个/了/过/着+O` 或 `V+了个+O` 词条，导致词网格只能用高频双字名词偏切（`各院`、`哥原`）拼凑，
//! 连带把正确读法挤出 `nbest = 12`。这里按双端单字证据在建图阶段合成对应词边，不发起额外查库，
//! 也不影响不含中缀或缺失对应单字的输入路径。

use super::decode::{edge_log_prob, Edge, Graph, LatticeOptions};
use crate::pinyin::syllables::canonical_lattice_syllable;

/// 一条已收录的二字动宾离合词（按 `(s0, s2, Reverse(weight), v, o)` 有序排列）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SeparableVoEntry {
    pub s0: &'static str,
    pub s2: &'static str,
    pub v: char,
    pub o: char,
    pub weight: u32,
}

const fn entry(
    s0: &'static str,
    s2: &'static str,
    v: char,
    o: char,
    weight: u32,
) -> SeparableVoEntry {
    SeparableVoEntry {
        s0,
        s2,
        v,
        o,
        weight,
    }
}

/// 同一起始列最多追加的合成离合词边数，全部暂存在栈上避免堆分配。
const MAX_PENDING_PER_COLUMN: usize = 8;

/// 现代汉语常用二字动宾离合词词表（严格按 `(s0, s2, Reverse(weight), v, o)` 排序以支持零分配二分查找）。
static SEPARABLE_VO_COMPOUNDS: &[SeparableVoEntry] = &[
    entry("ao", "ye", '熬', '夜', 44460),
    entry("ba", "ya", '拔', '牙', 11550),
    entry("bai", "fo", '拜', '佛', 14200),
    entry("bai", "nian", '拜', '年', 28775),
    entry("bai", "shi", '拜', '师', 58090),
    entry("bai", "tan", '摆', '摊', 16600),
    entry("ban", "jia", '搬', '家', 100680),
    entry("ban", "jiang", '颁', '奖', 94985),
    entry("bang", "mang", '帮', '忙', 307185),
    entry("bao", "dao", '报', '到', 188080),
    entry("bao", "jia", '报', '价', 500384),
    entry("bao", "ming", '报', '名', 500552),
    entry("bao", "xiao", '报', '销', 111085),
    entry("bei", "an", '备', '案', 500411),
    entry("bei", "fen", '备', '份', 311110),
    entry("bei", "guo", '背', '锅', 32295),
    entry("bei", "ke", '备', '课', 59135),
    entry("bi", "ye", '毕', '业', 500344),
    entry("bi", "zui", '闭', '嘴', 22585),
    entry("biao", "tai", '表', '态', 69500),
    entry("bo", "hao", '拨', '号', 90835),
    entry("bu", "jiao", '补', '觉', 44444),
    entry("bu", "kao", '补', '考', 37290),
    entry("bu", "ke", '补', '课', 37525),
    entry("bu", "ya", '补', '牙', 2755),
    entry("cai", "keng", '踩', '坑', 9999),
    entry("ce", "shi", '测', '试', 500807),
    entry("cha", "dui", '插', '队', 13135),
    entry("cha", "hua", '插', '话', 13880),
    entry("cha", "zui", '插', '嘴', 16530),
    entry("chang", "ge", '唱', '歌', 179395),
    entry("chao", "cai", '炒', '菜', 30820),
    entry("chao", "che", '超', '车', 28705),
    entry("chao", "jia", '吵', '架', 83570),
    entry("chi", "fan", '吃', '饭', 422140),
    entry("chong", "dian", '充', '电', 170375),
    entry("chong", "liang", '冲', '凉', 9190),
    entry("chong", "qi", '重', '启', 133085),
    entry("chong", "zao", '冲', '澡', 1720),
    entry("chong", "zhi", '充', '值', 111790),
    entry("chou", "jiang", '抽', '奖', 57065),
    entry("chou", "kong", '抽', '空', 25145),
    entry("chou", "qian", '抽', '签', 34415),
    entry("chou", "yan", '抽', '烟', 107725),
    entry("chu", "chai", '出', '差', 141495),
    entry("chu", "cuo", '出', '错', 135235),
    entry("chu", "guo", '出', '国', 221515),
    entry("chu", "jia", '出', '价', 75325),
    entry("chu", "men", '出', '门', 200495),
    entry("chu", "qi", '出', '气', 16455),
    entry("chu", "qian", '出', '钱', 38830),
    entry("chu", "ti", '出', '题', 47110),
    entry("chu", "yuan", '出', '院', 57710),
    entry("chuang", "ye", '创', '业', 500462),
    entry("chui", "niu", '吹', '牛', 31680),
    entry("ci", "zhi", '辞', '职', 165965),
    entry("cui", "dan", '催', '单', 10000),
    entry("cun", "dang", '存', '档', 140750),
    entry("cun", "qian", '存', '钱', 17020),
    entry("da", "bao", '打', '包', 89175),
    entry("da", "che", '打', '车', 32160),
    entry("da", "du", '打', '赌', 20470),
    entry("da", "dun", '打', '盹', 8185),
    entry("da", "fen", '打', '分', 141925),
    entry("da", "ge", '打', '嗝', 7735),
    entry("da", "gong", '打', '工', 217690),
    entry("da", "jia", '打', '架', 95595),
    entry("da", "jie", '打', '结', 13920),
    entry("da", "ka", '打', '卡', 12930),
    entry("da", "kuan", '打', '款', 5555),
    entry("da", "ma", '打', '码', 21675),
    entry("da", "pai", '打', '牌', 43075),
    entry("da", "qiu", '打', '球', 68890),
    entry("da", "ti", '答', '题', 133920),
    entry("da", "zhe", '打', '折', 142740),
    entry("da", "zhen", '打', '针', 22205),
    entry("da", "zi", '打', '字', 70320),
    entry("dai", "kuan", '贷', '款', 500512),
    entry("dao", "bie", '道', '别', 23110),
    entry("dao", "che", '倒', '车', 25050),
    entry("dao", "qian", '道', '歉', 134980),
    entry("de", "jiang", '得', '奖', 35170),
    entry("deng", "ji", '登', '记', 500597),
    entry("deng", "lu", '登', '录', 501936),
    entry("di", "tou", '低', '头', 130475),
    entry("dian", "cai", '点', '菜', 31410),
    entry("dian", "can", '点', '餐', 9999),
    entry("dian", "ge", '点', '歌', 49160),
    entry("dian", "ming", '点', '名', 35520),
    entry("dian", "tou", '点', '头', 185735),
    entry("dian", "zan", '点', '赞', 9999),
    entry("diao", "tou", '掉', '头', 31415),
    entry("diao", "yu", '钓', '鱼', 127655),
    entry("ding", "can", '订', '餐', 16230),
    entry("ding", "hun", '订', '婚', 31710),
    entry("dou", "feng", '兜', '风', 9115),
    entry("du", "dang", '读', '档', 1060),
    entry("du", "jia", '度', '假', 137585),
    entry("du", "shu", '读', '书', 490950),
    entry("duan", "dian", '断', '电', 49925),
    entry("duan", "wang", '断', '网', 7715),
    entry("dui", "zhang", '对', '账', 9055),
    entry("duo", "guan", '夺', '冠', 55860),
    entry("fa", "cai", '发', '财', 64350),
    entry("fa", "kuan", '罚', '款', 500380),
    entry("fa", "shi", '发', '誓', 53670),
    entry("fa", "tie", '发', '帖', 501440),
    entry("fa", "xin", '发', '信', 63995),
    entry("fa", "yan", '发', '言', 500413),
    entry("fan", "cuo", '犯', '错', 29975),
    entry("fan", "shen", '翻', '身', 70985),
    entry("fang", "jia", '放', '假', 78710),
    entry("fang", "pi", '放', '屁', 25305),
    entry("fang", "xue", '放', '学', 48270),
    entry("fen", "shou", '分', '手', 183700),
    entry("fu", "kuan", '付', '款', 361505),
    entry("gai", "ming", '改', '名', 99025),
    entry("gai", "zhang", '盖', '章', 179620),
    entry("gan", "bei", '干', '杯', 15070),
    entry("gao", "bie", '告', '别', 144110),
    entry("geng", "xin", '更', '新', 500729),
    entry("gu", "fen", '估', '分', 8888),
    entry("gu", "zhang", '鼓', '掌', 48785),
    entry("gua", "hao", '挂', '号', 39345),
    entry("gua", "ke", '挂', '科', 9999),
    entry("guai", "wan", '拐', '弯', 20485),
    entry("guan", "deng", '关', '灯', 11870),
    entry("guan", "ji", '关', '机', 138565),
    entry("guan", "men", '关', '门', 59645),
    entry("guang", "jie", '逛', '街', 95835),
    entry("guo", "jie", '过', '节', 45225),
    entry("guo", "nian", '过', '年', 94215),
    entry("hai", "qian", '还', '钱', 10000),
    entry("hai", "yuan", '还', '愿', 10000),
    entry("he", "cha", '喝', '茶', 78790),
    entry("he", "jiu", '喝', '酒', 222580),
    entry("he", "shui", '喝', '水', 70410),
    entry("he", "ying", '合', '影', 81950),
    entry("hua", "bing", '画', '饼', 2210),
    entry("hua", "qian", '花', '钱', 142170),
    entry("hua", "xue", '滑', '雪', 128935),
    entry("hua", "zhuang", '化', '妆', 164320),
    entry("huan", "ban", '换', '班', 4750),
    entry("huan", "che", '换', '车', 11330),
    entry("huan", "qian", '还', '钱', 13835),
    entry("huan", "yuan", '还', '愿', 5140),
    entry("hui", "guo", '回', '国', 124995),
    entry("hui", "shou", '挥', '手', 61605),
    entry("hui", "tie", '回', '帖', 224040),
    entry("hui", "tou", '回', '头', 243475),
    entry("hui", "xin", '回', '信', 43860),
    entry("ji", "zhang", '记', '账', 89675),
    entry("jia", "ban", '加', '班', 131370),
    entry("jia", "cai", '加', '菜', 2155),
    entry("jia", "can", '加', '餐', 5600),
    entry("jia", "fen", '加', '分', 96790),
    entry("jia", "qun", '加', '群', 9999),
    entry("jian", "fei", '减', '肥', 360895),
    entry("jian", "mian", '见', '面', 267135),
    entry("jian", "qun", '建', '群', 4415),
    entry("jian", "shen", '健', '身', 263950),
    entry("jiang", "jia", '降', '价', 207275),
    entry("jiang", "ke", '讲', '课', 94085),
    entry("jiao", "che", '叫', '车', 3675),
    entry("jiao", "fei", '缴', '费', 238480),
    entry("jie", "dan", '接', '单', 18835),
    entry("jie", "hun", '结', '婚', 500312),
    entry("jie", "qian", '借', '钱', 47190),
    entry("jie", "tu", '截', '图', 91840),
    entry("jie", "zhang", '结', '账', 41320),
    entry("jing", "li", '敬', '礼', 33030),
    entry("ju", "can", '聚', '餐', 32235),
    entry("ju", "gong", '鞠', '躬', 32440),
    entry("ju", "hui", '聚', '会', 196320),
    entry("ju", "li", '举', '例', 83360),
    entry("kai", "che", '开', '车', 295295),
    entry("kai", "chuang", '开', '窗', 31020),
    entry("kai", "deng", '开', '灯', 20510),
    entry("kai", "dian", '开', '店', 104685),
    entry("kai", "fang", '开', '房', 9965),
    entry("kai", "hui", '开', '会', 121320),
    entry("kai", "ji", '开', '机', 161200),
    entry("kai", "jiang", '开', '奖', 77135),
    entry("kai", "kou", '开', '口', 207560),
    entry("kai", "men", '开', '门', 164335),
    entry("kai", "piao", '开', '票', 32525),
    entry("kan", "bing", '看', '病', 89635),
    entry("kan", "fang", '看', '房', 47575),
    entry("kan", "jia", '砍', '价', 18300),
    entry("kan", "shu", '看', '书', 144645),
    entry("kao", "shi", '考', '试', 501195),
    entry("ke", "tou", '磕', '头', 21675),
    entry("kou", "fen", '扣', '分', 46205),
    entry("kua", "nian", '跨', '年', 10055),
    entry("li", "fa", '理', '发', 32010),
    entry("li", "hun", '离', '婚', 273330),
    entry("li", "zhi", '离', '职', 103295),
    entry("lian", "wang", '联', '网', 164600),
    entry("liao", "tian", '聊', '天', 397750),
    entry("liu", "gou", '遛', '狗', 5110),
    entry("liu", "lei", '流', '泪', 131495),
    entry("liu", "nian", '留', '念', 23255),
    entry("liu", "shen", '留', '神', 11605),
    entry("liu", "xin", '留', '心', 45410),
    entry("liu", "yan", '留', '言', 500772),
    entry("liu", "ying", '留', '影', 18495),
    entry("lou", "lian", '露', '脸', 13280),
    entry("lou", "mian", '露', '面', 38370),
    entry("lu", "lian", '露', '脸', 10000),
    entry("lu", "mao", '撸', '猫', 9999),
    entry("lu", "mian", '露', '面', 10000),
    entry("lu", "ping", '录', '屏', 9999),
    entry("lu", "xiang", '录', '像', 223555),
    entry("lu", "yin", '录', '音', 320235),
    entry("mai", "dan", '买', '单', 46985),
    entry("mai", "fang", '买', '房', 141580),
    entry("mei", "rong", '美', '容', 438405),
    entry("mian", "dan", '免', '单', 9999),
    entry("mo", "yu", '摸', '鱼', 23480),
    entry("pa", "shan", '爬', '山', 45020),
    entry("pai", "dui", '排', '队', 115920),
    entry("pai", "shou", '拍', '手', 22345),
    entry("pai", "zhao", '拍', '照', 169180),
    entry("pao", "bu", '跑', '步', 66055),
    entry("pao", "zao", '泡', '澡', 10740),
    entry("pei", "qian", '赔', '钱', 22320),
    entry("pei", "yin", '配', '音', 80445),
    entry("pei", "zui", '赔', '罪', 5575),
    entry("peng", "bei", '碰', '杯', 4750),
    entry("peng", "chang", '捧', '场', 36120),
    entry("peng", "tou", '碰', '头', 13050),
    entry("pi", "yao", '辟', '谣', 7045),
    entry("pin", "che", '拼', '车', 15630),
    entry("pin", "dan", '拼', '单', 9999),
    entry("ping", "lun", '评', '论', 928410),
    entry("qi", "chuang", '起', '床', 114555),
    entry("qi", "ming", '起', '名', 61775),
    entry("qian", "dao", '签', '到', 32795),
    entry("qian", "ming", '签', '名', 500257),
    entry("qian", "qian", '欠', '钱', 13900),
    entry("qian", "shou", '牵', '手', 50420),
    entry("qian", "zi", '签', '字', 267055),
    entry("qiao", "ke", '翘', '课', 9999),
    entry("qing", "jia", '请', '假', 84215),
    entry("qing", "ke", '请', '客', 43355),
    entry("qiu", "hun", '求', '婚', 50035),
    entry("qu", "ming", '取', '名', 79130),
    entry("qu", "qian", '取', '钱', 12400),
    entry("ran", "fa", '染', '发', 29275),
    entry("re", "shen", '热', '身', 27515),
    entry("ren", "cuo", '认', '错', 23685),
    entry("ru", "zhi", '入', '职', 22195),
    entry("sa", "huang", '撒', '谎', 42105),
    entry("san", "bu", '散', '步', 101880),
    entry("san", "hui", '散', '会', 6670),
    entry("sao", "di", '扫', '地', 26900),
    entry("sao", "ma", '扫', '码', 9999),
    entry("sao", "mu", '扫', '墓', 9885),
    entry("shan", "tie", '删', '帖', 27920),
    entry("shang", "ban", '上', '班', 316475),
    entry("shang", "chuang", '上', '床', 68455),
    entry("shang", "ke", '上', '课', 268270),
    entry("shang", "wang", '上', '网', 500394),
    entry("shang", "xue", '上', '学', 182515),
    entry("sheng", "ji", '升', '级', 500441),
    entry("sheng", "qi", '生', '气', 237195),
    entry("sheng", "qian", '省', '钱', 84205),
    entry("shi", "li", '施', '礼', 11735),
    entry("shou", "fei", '收', '费', 500454),
    entry("shou", "kuan", '收', '款', 96540),
    entry("shu", "ye", '输', '液', 37555),
    entry("shua", "ji", '刷', '机', 16380),
    entry("shua", "ya", '刷', '牙', 43400),
    entry("shuai", "guo", '甩', '锅', 9999),
    entry("shui", "jiao", '睡', '觉', 256865),
    entry("shuo", "hua", '说', '话', 500373),
    entry("song", "li", '送', '礼', 66780),
    entry("suan", "zhang", '算', '账', 17680),
    entry("suo", "men", '锁', '门', 7865),
    entry("ta", "qing", '踏', '青', 12135),
    entry("tai", "gang", '抬', '杠', 6890),
    entry("tai", "tou", '抬', '头', 183170),
    entry("tan", "hua", '谈', '话', 235620),
    entry("tan", "qi", '叹', '气', 29750),
    entry("tang", "fa", '烫', '发', 18125),
    entry("tao", "ke", '逃', '课', 15475),
    entry("ti", "jian", '体', '检', 201755),
    entry("ti", "qiu", '踢', '球', 42270),
    entry("ti", "xian", '提', '现', 8640),
    entry("ti", "xing", '提', '醒', 500382),
    entry("tiao", "cao", '跳', '槽', 102895),
    entry("tiao", "wu", '跳', '舞', 149890),
    entry("ting", "che", '停', '车', 200370),
    entry("ting", "dian", '停', '电', 69665),
    entry("ting", "ge", '听', '歌', 34150),
    entry("ting", "ke", '听', '课', 101025),
    entry("tong", "dian", '通', '电', 41425),
    entry("tong", "hua", '通', '话', 154220),
    entry("tong", "qi", '通', '气', 35815),
    entry("tou", "lan", '偷', '懒', 34110),
    entry("tou", "piao", '投', '票', 500349),
    entry("tou", "ping", '投', '屏', 1111),
    entry("tu", "cao", '吐', '槽', 9085),
    entry("tui", "dan", '退', '单', 10000),
    entry("tui", "fang", '退', '房', 40300),
    entry("tui", "fei", '退', '费', 20585),
    entry("tui", "kuan", '退', '款', 63750),
    entry("tui", "qun", '退', '群', 9999),
    entry("tuo", "di", '拖', '地', 9490),
    entry("wa", "keng", '挖', '坑', 11990),
    entry("wan", "yao", '弯', '腰', 38230),
    entry("wo", "shou", '握', '手', 83930),
    entry("wu", "shui", '午', '睡', 19185),
    entry("xi", "che", '洗', '车', 36255),
    entry("xi", "jiao", '洗', '脚', 24115),
    entry("xi", "lian", '洗', '脸', 70480),
    entry("xi", "shou", '洗', '手', 48190),
    entry("xi", "tou", '洗', '头', 28280),
    entry("xi", "wan", '洗', '碗', 25205),
    entry("xi", "ya", '洗', '牙', 5540),
    entry("xi", "zao", '洗', '澡', 175105),
    entry("xia", "ban", '下', '班', 118710),
    entry("xia", "dan", '下', '单', 72155),
    entry("xia", "ke", '下', '课', 39115),
    entry("xiang", "qin", '相', '亲', 46420),
    entry("xie", "xin", '写', '信', 83405),
    entry("xie", "zhuang", '卸', '妆', 25450),
    entry("xie", "zi", '写', '字', 84815),
    entry("xing", "li", '行', '礼', 31575),
    entry("xiu", "jia", '休', '假', 65025),
    entry("xu", "yuan", '许', '愿', 39130),
    entry("xun", "jia", '询', '价', 145525),
    entry("yao", "tou", '摇', '头', 139030),
    entry("you", "yong", '游', '泳', 208615),
    entry("yue", "hui", '约', '会', 146930),
    entry("zao", "ju", '造', '句', 14515),
    entry("zhang", "jia", '涨', '价', 119155),
    entry("zhang", "zui", '张', '嘴', 25975),
    entry("zhao", "shou", '招', '手', 39445),
    entry("zheng", "qian", '挣', '钱', 101700),
    entry("zhi", "ban", '值', '班', 180605),
    entry("zhi", "zhao", '支', '招', 20165),
    entry("zhong", "jiang", '中', '奖', 106960),
    entry("zhu", "ce", '注', '册', 506752),
    entry("zhu", "yuan", '住', '院', 196185),
    entry("zhuan", "fa", '转', '发', 163030),
    entry("zhuan", "qian", '赚', '钱', 357380),
    entry("zhuan", "quan", '转', '圈', 11690),
    entry("zhuan", "shen", '转', '身', 254835),
    entry("zhuan", "wan", '转', '弯', 61600),
    entry("zhuan", "zhang", '转', '账', 52130),
    entry("zhuang", "xiu", '装', '修', 500409),
    entry("zu", "fang", '租', '房', 500281),
    entry("zuo", "che", '坐', '车', 47840),
    entry("zuo", "fan", '做', '饭', 103395),
    entry("zuo", "meng", '做', '梦', 91650),
];

/// 二分查找匹配 `(s0, s2)` 的动宾离合词切片；零堆分配。
fn lookup_vo_entries(s0: &str, s2: &str) -> &'static [SeparableVoEntry] {
    let start = SEPARABLE_VO_COMPOUNDS.partition_point(|item| (item.s0, item.s2) < (s0, s2));
    let rest = &SEPARABLE_VO_COMPOUNDS[start..];
    let len = rest.partition_point(|item| item.s0 == s0 && item.s2 == s2);
    &rest[..len]
}

/// 将单音节中缀映射为对应汉字及是否为「个」中缀（影响有效词频换算档位）。
fn infix_for_syllable(syllable: &str) -> Option<(char, bool)> {
    match syllable {
        "ge" => Some(('个', true)),
        "le" => Some(('了', false)),
        "guo" => Some(('过', false)),
        "zhe" => Some(('着', false)),
        _ => None,
    }
}

/// 检查列 `column` 中是否存在终点为 `end`、词面恰好为单字 `ch` 的边。
fn has_single_char_edge(column: &[Edge], end: usize, ch: char) -> bool {
    let mut buf = [0u8; 4];
    let target: &str = ch.encode_utf8(&mut buf);
    column
        .iter()
        .any(|edge| edge.end == end && edge.word == target)
}

/// 在栈缓冲中拼接若干汉字，返回合法 UTF-8 切片。
fn encode_chars<const N: usize>(chars: [char; N], buf: &mut [u8; 16]) -> &str {
    let mut len = 0;
    for ch in chars {
        let encoded = ch.encode_utf8(&mut buf[len..]);
        len += encoded.len();
    }
    std::str::from_utf8(&buf[..len]).unwrap_or("")
}

/// 将二字动宾词频折算为 Shurufa v2 统一对数概率尺度（`p99_ref(3)=85k`、`p99_ref(4)=45k`）下的拆插边有效权重：
/// 既稳压零散 `[1,1,1]` 单字与偏切组合（如 `转个长`/`睡了个叫`/`北各分`），又不越级压过真实高频三四字词典整词。
fn effective_vo_weight(entry_weight: u32, is_ge_infix: bool) -> i64 {
    if is_ge_infix {
        (i64::from(entry_weight) / 25).clamp(800, 18_000)
    } else {
        (i64::from(entry_weight) / 120).clamp(150, 4_500)
    }
}

/// 若去重与 `span_limit` 允许，构造离合词边并暂存到当前列的栈缓冲。
#[allow(clippy::too_many_arguments)]
fn try_queue_vo_edge(
    existing_column: &[Edge],
    pending: &mut [Option<Edge>; MAX_PENDING_PER_COLUMN],
    pending_len: &mut usize,
    end: usize,
    word: &str,
    key: String,
    log_prob: f64,
    span_limit: usize,
) {
    if *pending_len >= MAX_PENDING_PER_COLUMN || word.is_empty() {
        return;
    }
    let mut count_for_end = 0usize;
    for edge in existing_column {
        if edge.end == end {
            if edge.word == word {
                return;
            }
            count_for_end += 1;
        }
    }
    for queued in pending[..*pending_len].iter().flatten() {
        if queued.end == end {
            if queued.word == word {
                return;
            }
            count_for_end += 1;
        }
    }
    if count_for_end >= span_limit {
        return;
    }
    pending[*pending_len] = Some(Edge {
        end,
        word: word.to_owned(),
        key,
        log_prob,
        typo: false,
    });
    *pending_len += 1;
}

/// 遍历已建好的词网格，在 `V + 个/了/过/着 + O` 与 `V + 了个 + O` 跨度上按双端单字证据补齐动宾离合词边。
///
/// 不包含中缀音节或缺少词表/单字证据时不分配任何堆内存，也不改变未命中列的容量。
pub(super) fn append_separable_vo_edges(
    syllables: &[String],
    graph: &mut Graph,
    options: &LatticeOptions<'_>,
) {
    let n = syllables.len();
    let max_len = options.max_phrase_syllables.max(1);
    if n < 3 || max_len < 3 || options.span_limit == 0 || graph.len() != n {
        return;
    }
    let mut phrase_buf = [0u8; 16];
    for start in 0..n {
        let mut pending: [Option<Edge>; MAX_PENDING_PER_COLUMN] =
            [const { None }; MAX_PENDING_PER_COLUMN];
        let mut pending_len = 0usize;

        // 1. 三音节单字中缀：`s0 + (ge|le|guo|zhe) + s2`
        if start + 3 <= n {
            let s1 = canonical_lattice_syllable(&syllables[start + 1]);
            if let Some((infix_ch, is_ge)) = infix_for_syllable(s1) {
                let s0 = canonical_lattice_syllable(&syllables[start]);
                let s2 = canonical_lattice_syllable(&syllables[start + 2]);
                let entries = lookup_vo_entries(s0, s2);
                if !entries.is_empty()
                    && has_single_char_edge(&graph[start + 1], start + 2, infix_ch)
                {
                    let end = start + 3;
                    for item in entries {
                        if !has_single_char_edge(&graph[start], start + 1, item.v)
                            || !has_single_char_edge(&graph[start + 2], end, item.o)
                        {
                            continue;
                        }
                        let word = encode_chars([item.v, infix_ch, item.o], &mut phrase_buf);
                        let effective = effective_vo_weight(item.weight, is_ge);
                        let log_prob = edge_log_prob(effective, 3, options);
                        let key = format!("{s0}'{s1}'{s2}");
                        try_queue_vo_edge(
                            &graph[start],
                            &mut pending,
                            &mut pending_len,
                            end,
                            word,
                            key,
                            log_prob,
                            options.span_limit,
                        );
                    }
                }
            }
        }

        // 2. 四音节双字中缀：`s0 + le + ge + s3`（如「许了个愿」「睡了个觉」）
        if start + 4 <= n && max_len >= 4 {
            let s1 = canonical_lattice_syllable(&syllables[start + 1]);
            let s2 = canonical_lattice_syllable(&syllables[start + 2]);
            if s1 == "le" && s2 == "ge" {
                let s0 = canonical_lattice_syllable(&syllables[start]);
                let s3 = canonical_lattice_syllable(&syllables[start + 3]);
                let entries = lookup_vo_entries(s0, s3);
                if !entries.is_empty()
                    && has_single_char_edge(&graph[start + 1], start + 2, '了')
                    && has_single_char_edge(&graph[start + 2], start + 3, '个')
                {
                    let end = start + 4;
                    for item in entries {
                        if !has_single_char_edge(&graph[start], start + 1, item.v)
                            || !has_single_char_edge(&graph[start + 3], end, item.o)
                        {
                            continue;
                        }
                        let word = encode_chars([item.v, '了', '个', item.o], &mut phrase_buf);
                        let effective = effective_vo_weight(item.weight, true);
                        let log_prob = edge_log_prob(effective, 4, options);
                        let key = format!("{s0}'le'ge'{s3}");
                        try_queue_vo_edge(
                            &graph[start],
                            &mut pending,
                            &mut pending_len,
                            end,
                            word,
                            key,
                            log_prob,
                            options.span_limit,
                        );
                    }
                }
            }
        }

        if pending_len > 0 {
            for edge in pending[..pending_len].iter_mut().filter_map(Option::take) {
                graph[start].push(edge);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Reverse;

    use super::*;
    use crate::pinyin::syllables::is_intact;

    #[test]
    fn separable_vo_table_is_sorted_unique_and_valid() {
        assert!(!SEPARABLE_VO_COMPOUNDS.is_empty());
        for (index, item) in SEPARABLE_VO_COMPOUNDS.iter().enumerate() {
            assert!(is_intact(item.s0), "非法首音节: {:?}", item);
            assert!(is_intact(item.s2), "非法尾音节: {:?}", item);
            assert!(item.weight > 0, "权重必须为正: {:?}", item);
            assert_ne!(item.v, item.o, "不应收入叠字词: {:?}", item);
            if index > 0 {
                let prev = SEPARABLE_VO_COMPOUNDS[index - 1];
                let prev_key = (prev.s0, prev.s2, Reverse(prev.weight), prev.v, prev.o);
                let cur_key = (item.s0, item.s2, Reverse(item.weight), item.v, item.o);
                assert!(
                    prev_key < cur_key,
                    "词表排序或去重异常 at {index}: {prev:?} vs {item:?}"
                );
            }
        }
        assert!(lookup_vo_entries("xu", "yuan")
            .iter()
            .any(|item| item.v == '许' && item.o == '愿'));
        assert!(lookup_vo_entries("zzz", "zzz").is_empty());
    }
}
