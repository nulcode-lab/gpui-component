//! Language editing configuration. Grammar resources live in `highlighter`.

pub use gpui_base::input::language_config::LanguageConfig;

use gpui_base::input::{AutoClosingPair, BracketPair, IndentationRules, SyntaxContext};
use regex::Regex;
use std::sync::LazyLock;

struct ComponentLanguages {
    defaults: std::collections::HashMap<&'static str, std::rc::Rc<LanguageConfig>>,
    fallback: std::rc::Rc<LanguageConfig>,
}

impl gpui_base::input::LanguageProvider for ComponentLanguages {
    fn language_name(&self, name: &str) -> gpui::SharedString {
        crate::highlighter::LanguageRegistry::singleton().editing_language_name(name)
    }

    fn config(&self, name: &str) -> std::rc::Rc<LanguageConfig> {
        self.defaults.get(name).unwrap_or(&self.fallback).clone()
    }

    fn syntax_context_provider(
        &self,
        name: &str,
    ) -> Option<std::rc::Rc<dyn gpui_base::input::SyntaxContextProvider>> {
        super::syntax_context::syntax_context_provider(name)
    }
}

pub(crate) fn init(cx: &mut gpui::App) {
    gpui_base::input::set_language_provider(
        std::rc::Rc::new(ComponentLanguages {
            defaults: ["text", "json", "python", "cpp"]
                .into_iter()
                .map(|name| (name, std::rc::Rc::new(default_language_config(name))))
                .collect(),
            fallback: std::rc::Rc::new(LanguageConfig::default()),
        }),
        cx,
    );
}

/// Language editing defaults. Editor preferences are stored separately on EditorState.
/// Unknown languages use structural bracket indentation; only Python treats a colon
/// as a block opener. Plain text has no structural or automatic pairs.
fn default_language_config(language: &str) -> LanguageConfig {
    static PYTHON_INDENT: LazyLock<IndentationRules> = LazyLock::new(|| {
        IndentationRules::new(
            Regex::new(r"[\{\(\[:]\s*$").unwrap(),
            Regex::new(r"^\s*[\}\)\]]").unwrap(),
        )
    });
    static CPP_INDENT: LazyLock<IndentationRules> = LazyLock::new(|| {
        // Increase after a line that opens a block/paren (`{`, `(`, `[` at the
        // end), a control statement whose condition closes on the same line
        // (`if (x == a)`), a dangling `else`/`do`/`try`, a `case ...:` label,
        // or an access specifier.
        IndentationRules::new(
            Regex::new(
                r"([{(\[]\s*$)|(^\s*\}?\s*(if|for|while|switch|catch)\s*\(.*\)\s*$)|(^\s*\}?\s*(else|do|try)(\s+if\s*\(.*\))?\s*$)|(^\s*(case\b.*|default|public|private|protected)\s*:\s*$)",
            )
            .unwrap(),
            Regex::new(r"^\s*[\}\)\]]").unwrap(),
        )
    });
    match language.to_lowercase().as_str() {
        "text" => LanguageConfig::default()
            .brackets([])
            .auto_closing_pairs([]),
        "json" => LanguageConfig::default()
            .brackets([BracketPair::new("{", "}"), BracketPair::new("[", "]")])
            .auto_closing_pairs(
                [
                    AutoClosingPair::new("{", "}"),
                    AutoClosingPair::new("[", "]"),
                    AutoClosingPair::new("\"", "\""),
                ]
                .into_iter()
                .map(|p| p.not_in([SyntaxContext::String, SyntaxContext::Comment])),
            ),
        "python" => LanguageConfig::default().indentation_rules(PYTHON_INDENT.clone()),
        // C-family editing defaults so the fork's C++ editor keeps
        // brace/paren indentation without per-language tree-sitter queries.
        "cpp" | "c" => LanguageConfig::default().indentation_rules(CPP_INDENT.clone()),
        _ => LanguageConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_has_no_editing_rules() {
        let rules = default_language_config("text");
        assert!(rules.brackets.is_empty());
        assert!(rules.auto_closing_pairs.unwrap().is_empty());
        assert!(rules.indentation_rules.is_none());
    }

    #[test]
    fn json_only_closes_json_delimiters() {
        let rules = default_language_config("json");
        let pairs = rules.auto_closing_pairs.unwrap();
        assert_eq!(pairs.len(), 3);
        assert!(
            pairs
                .iter()
                .all(|p| p.open.as_ref() != "'" && p.open.as_ref() != "(")
        );
    }

    #[test]
    fn cpp_indent_follows_control_statements_and_blocks() {
        let rules = default_language_config("cpp");
        let rules = rules.indentation_rules.unwrap();
        let inc = rules.increase_indent_pattern.as_ref().unwrap();
        let dec = rules.decrease_indent_pattern.as_ref().unwrap();

        // Control statements whose condition closes on the same line.
        for line in ["if(x == a)", "  if (x == a)", "for(int i = 0; i < n; i++)",
                     "while (x--)", "switch (v)", "} else", "else if (y)"] {
            assert!(inc.is_match(line), "increase must match {line:?}");
        }
        // Blocks and dangling keywords.
        for line in ["int main(){", "if (x) {", "    do", "else", "case 1:", "default:",
                     "public:", "struct S {"] {
            assert!(inc.is_match(line), "increase must match {line:?}");
        }
        // Ordinary lines must not increase.
        for line in ["return 0;", "int x = f(a, b);", "x++;", "int main()"] {
            assert!(!inc.is_match(line), "increase must not match {line:?}");
        }
        // A closing delimiter starts a dedented line.
        for line in ["}", "  }", ")", "]"] {
            assert!(dec.is_match(line), "decrease must match {line:?}");
        }
    }

    #[test]
    fn colon_indentation_is_language_specific() {
        assert!(
            default_language_config("python")
                .indentation_rules
                .unwrap()
                .increase_indent_pattern
                .unwrap()
                .is_match("if enabled:")
        );
        for language in ["rust", "javascript", "json", "unknown"] {
            assert!(
                default_language_config(language)
                    .indentation_rules
                    .is_none()
            );
        }
    }
}
