//! Focused regression test for the fork's matched-brace query: typing inside
//! `if()` in a C++ buffer must find the innermost paren pair via cpp/brackets.scm.
#[cfg(all(test, feature = "tree-sitter", feature = "tree-sitter-cpp"))]
mod tests {
    use gpui_base::input::RopeExt as _;
    use ropey::Rope;
    use std::time::Duration;

    use crate::highlighter::highlighter::{SyntaxHighlighter, innermost_bracket_pair_merged};

    fn parse(buffer: &str) -> (SyntaxHighlighter, Rope) {
        let mut highlighter = SyntaxHighlighter::new("cpp");
        let text = Rope::from(buffer);
        highlighter.update(None, &text, Some(Duration::from_secs(1)));
        (highlighter, text)
    }

    #[test]
    fn cpp_brackets_query_is_registered_and_compiles() {
        let config = crate::highlighter::registry::LanguageRegistry::singleton()
            .language("cpp")
            .expect("cpp should be registered");
        assert!(
            !config.brackets.is_empty(),
            "cpp config.brackets is EMPTY - brackets.scm was not wired"
        );
        let lang = config.language.clone().expect("cpp grammar");
        assert!(
            tree_sitter::Query::new(&lang, &config.brackets).is_ok(),
            "cpp brackets.scm failed to compile"
        );
    }

    #[test]
    fn bracket_pairs_finds_parens_in_sample() {
        let (highlighter, text) = parse("int main() { return 0; }");
        let pairs = highlighter.bracket_pairs(0..text.len());
        eprintln!("bracket_pairs found: {:?}", pairs);
        assert!(
            !pairs.is_empty(),
            "bracket_pairs() found 0 matches on `int main() {{ return 0; }}`"
        );
    }

    #[test]
    fn dump_tree_for_empty_if_parens() {
        let (highlighter, _text) = parse("for(int i = 1; i <= 5; i++){\n    if()\n}");
        let tree = highlighter.tree().expect("tree");
        eprintln!("ROOT SEXP: {}", tree.root_node().to_sexp());
        fn walk(node: tree_sitter::Node<'_>, depth: usize) {
            if node.child_count() == 0 {
                eprintln!(
                    "{}LEAF {} {}..{}",
                    "  ".repeat(depth),
                    node.kind(),
                    node.start_byte(),
                    node.end_byte()
                );
                return;
            }
            eprintln!("{}NODE {}", "  ".repeat(depth), node.kind());
            for i in 0..node.child_count() {
                walk(node.child(i.try_into().unwrap()).unwrap(), depth + 1);
            }
        }
        walk(tree.root_node(), 0);

        // Manually run the brackets query and dump every match.
        let config = crate::highlighter::registry::LanguageRegistry::singleton()
            .language("cpp")
            .expect("cpp config");
        let lang = config.language.clone().expect("grammar");
        let query = tree_sitter::Query::new(&lang, &config.brackets).expect("query compiles");
        let open_ix = query
            .capture_names()
            .iter()
            .position(|n| *n == "open")
            .expect("open capture") as u32;
        let close_ix = query
            .capture_names()
            .iter()
            .position(|n| *n == "close")
            .expect("close capture") as u32;
        eprintln!("capture_names: {:?}", query.capture_names());
        let text: Rope = Rope::from("for(int i = 1; i <= 5; i++){\n    if()\n}");
        let mut qc = tree_sitter::QueryCursor::new();
        use tree_sitter::StreamingIterator as _;
        let mut matches_iter = qc.matches(
            &query,
            tree.root_node(),
            crate::highlighter::highlighter::TextProvider(&text),
        );
        let mut total = 0usize;
        while let Some(m) = matches_iter.next() {
            total += 1;
            let mut open = None;
            let mut close = None;
            for c in m.captures {
                if c.index == open_ix {
                    open = Some(c.node.byte_range());
                } else if c.index == close_ix {
                    close = Some(c.node.byte_range());
                }
            }
            eprintln!("pattern#{} open={:?} close={:?}", m.pattern_index, open, close);
        }
        eprintln!("total matches: {}", total);
    }

    #[test]
    fn innermost_bracket_pair_finds_if_parens_in_cpp() {
        let (highlighter, text) = parse("for(int i = 1; i <= 5; i++){\n    if()\n}");
        let tree = highlighter
            .tree()
            .cloned()
            .expect("cpp buffer should have a syntax tree");

        let language = gpui::SharedString::from("cpp");
        // Cursor inside the if() parens: right after the `(`.
        let cursor = text.line_start_offset(1) + "    if(".len();
        let pair = innermost_bracket_pair_merged(&tree, &text, &language, cursor);
        eprintln!("if() pair at cursor: {:?}", pair);
        assert!(pair.is_some(), "if() paren pair should match at cursor");

        let (open, close) = pair.unwrap();
        assert_eq!(open.start, text.line_start_offset(1) + "    if(".len() - 1);
        assert_eq!(close.start, text.line_start_offset(1) + "    if()".len() - 1);
    }

    #[test]
    fn innermost_bracket_pair_prefers_innermost_pair() {
        let (highlighter, text) = parse("foo(bar(baz))");
        let tree = highlighter.tree().cloned().expect("tree");

        let language = gpui::SharedString::from("cpp");
        let cursor = "foo(bar(".len();
        let pair = innermost_bracket_pair_merged(&tree, &text, &language, cursor);
        eprintln!("nested pair: {:?}", pair);
        let (open, _close) = pair.expect("innermost pair");
        // The pair right at the cursor (bar's parens), not foo's outer parens.
        assert_eq!(open.start, "foo(bar(".len() - 1);
    }

    #[test]
    fn dump_user_repro_incomplete_if_indented_close() {
        // User's live repro: incomplete `if` (no parens) inside a for-body,
        // close brace on its own indented line. The close-brace highlight
        // box paints one cell LEFT of the `}`.
        let buffer = "int main(){
    for(int i = 0; i < 5; i++){
        if
    }
    return 0;
}
";
        let (highlighter, text) = parse(buffer);
        let tree = highlighter.tree().cloned().expect("tree");
        eprintln!("ROOT SEXP: {}", tree.root_node().to_sexp());

        // Expected `}` on its own line (line index 3).
        let line3 = text.line_start_offset(3);
        let close_expected = line3 + 4; // 4-space indent, `}` at col 4
        assert_eq!(&buffer[close_expected..close_expected + 1], "}");

        let language = gpui::SharedString::from("cpp");
        let cursor = text.line_start_offset(2) + "        if".len(); // end of `if`
        let merged = innermost_bracket_pair_merged(&tree, &text, &language, cursor);
        let query = crate::highlighter::highlighter::innermost_bracket_pair_from_tree(
            &tree,
            &text,
            &language,
            cursor,
        );
        // Both engines must agree on the for-pair: open `{` at 42, close `}`
        // at 59 (its real byte position on the indented line) — the paint
        // geometry relies on byte-exact offsets.
        assert_eq!(merged, Some((42..43, 59..60)), "merged pair");
        assert_eq!(query, Some((42..43, 59..60)), "query pair");
        assert_eq!(merged.unwrap().1.start, close_expected);
    }
}
