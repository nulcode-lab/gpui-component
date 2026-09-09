// vendor/gpui-component/crates/ui/src/input/bracket.rs

/// 括号对配置
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BracketPair {
    pub start: char,
    pub end: char,
    /// 是否自动闭合
    pub close: bool,
    /// 是否支持选中文本环绕
    pub surround: bool,
}

impl BracketPair {
    pub const fn new(start: char, end: char, close: bool, surround: bool) -> Self {
        Self { start, end, close, surround }
    }
}

/// 默认括号对配置
pub const DEFAULT_BRACKET_PAIRS: &[BracketPair] = &[
    BracketPair::new('{', '}', true, true),
    BracketPair::new('[', ']', true, true),
    BracketPair::new('(', ')', true, true),
    BracketPair::new('"', '"', true, true),
    BracketPair::new('\'', '\'', true, true),
];

/// 仅匹配高亮的括号对（不自动闭合）
/// 需要通过 is_in_template_context 判断上下文
pub const MATCH_ONLY_BRACKET_PAIRS: &[BracketPair] = &[
    BracketPair::new('<', '>', false, false),
];

/// 判断字符是否是括号的开始
pub fn is_bracket_start(ch: char) -> bool {
    DEFAULT_BRACKET_PAIRS.iter().any(|p| p.start == ch)
        || MATCH_ONLY_BRACKET_PAIRS.iter().any(|p| p.start == ch)
}

/// 判断字符是否是括号的结束
pub fn is_bracket_end(ch: char) -> bool {
    DEFAULT_BRACKET_PAIRS.iter().any(|p| p.end == ch)
        || MATCH_ONLY_BRACKET_PAIRS.iter().any(|p| p.end == ch)
}

/// 获取括号对配置
pub fn get_bracket_pair_for_start(ch: char) -> Option<&'static BracketPair> {
    DEFAULT_BRACKET_PAIRS.iter().find(|p| p.start == ch)
        .or_else(|| MATCH_ONLY_BRACKET_PAIRS.iter().find(|p| p.start == ch))
}

/// 获取括号对配置（通过结束字符）
pub fn get_bracket_pair_for_end(ch: char) -> Option<&'static BracketPair> {
    DEFAULT_BRACKET_PAIRS.iter().find(|p| p.end == ch)
        .or_else(|| MATCH_ONLY_BRACKET_PAIRS.iter().find(|p| p.end == ch))
}

/// 判断节点类型是否是注释或字符串
pub fn is_comment_or_string(kind: &str) -> bool {
    matches!(
        kind,
        "comment" |
        "line_comment" |
        "block_comment" |
        "string" |
        "string_literal" |
        "raw_string_literal" |
        "char_literal" |
        "character_literal" |
        "string_content" |
        "escape_sequence"
    )
}
