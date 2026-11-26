use turso_parser::lexer::Lexer;
use turso_parser::token::TokenType;

pub struct Word<'a> {
    pub kind: TokenType,
    pub text: &'a [u8],
}

pub fn first_statement(sql: &str) -> Vec<Word<'_>> {
    let mut words = Vec::new();
    for token in Lexer::new(sql.as_bytes()) {
        let Ok(token) = token else { break };
        match token.token_type {
            TokenType::TK_NONE => {}
            TokenType::TK_SEMI => break,
            kind => words.push(Word { kind, text: token.value }),
        }
    }
    words
}

fn is_identifier(kind: TokenType) -> bool {
    !matches!(
        kind,
        TokenType::TK_LP | TokenType::TK_RP | TokenType::TK_SEMI | TokenType::TK_COMMA | TokenType::TK_DOT | TokenType::TK_EOF
    )
}

pub fn omits(words: &[Word<'_>]) -> bool {
    if words.first().is_some_and(|word| matches!(word.kind, TokenType::TK_EXPLAIN | TokenType::TK_ANALYZE | TokenType::TK_REINDEX)) {
        return true;
    }
    for (index, word) in words.iter().enumerate() {
        let next = words.get(index + 1).map(|word| word.kind);
        let omitted = match word.kind {
            TokenType::TK_DO => true,
            TokenType::TK_OVER => next.is_some_and(|kind| kind == TokenType::TK_LP || is_identifier(kind)),
            TokenType::TK_FILTER => next == Some(TokenType::TK_LP),
            TokenType::TK_WINDOW => words.get(index + 2).is_some_and(|word| word.kind == TokenType::TK_AS),
            TokenType::TK_ID => next == Some(TokenType::TK_LP) && word.text.len() > 7 && word.text[..7].eq_ignore_ascii_case(b"pragma_"),
            _ => false,
        };
        if omitted {
            return true;
        }
    }
    false
}

pub fn names_view(words: &[Word<'_>], views: &[Vec<u8>]) -> bool {
    if views.is_empty() || words.iter().any(|word| word.kind == TokenType::TK_VIEW) {
        return false;
    }
    words.iter().any(|word| {
        let text = word.text.strip_prefix(b"\"").and_then(|inner| inner.strip_suffix(b"\"")).unwrap_or(word.text);
        is_identifier(word.kind) && views.iter().any(|view| view.eq_ignore_ascii_case(text))
    })
}
