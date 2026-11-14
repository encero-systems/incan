//! Native COMIC tokenization with Unicode-scalar source positions, mirroring `text_tokens` and `text_names`.

use super::super::scalar_text;

/// One refusal: its category and one-based scalar position.
pub(super) type Failure = (&'static str, i64, i64);

/// A tokenizer or reader result.
pub(super) type Read<T> = Result<T, Failure>;

const PUNCTUATION: [char; 12] = ['{', '}', '[', ']', '(', ')', ',', ':', '=', '?', '|', '>'];
const DELIMITERS: [char; 19] = [
    ' ', '\t', '\r', '\n', '{', '}', '[', ']', '(', ')', ',', ':', '=', '?', '|', '<', '>', '#', '"',
];

/// Token classes of the inert COMIC grammar.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Word,
    Number,
    Text,
    Binary,
    Marker,
    Punctuation,
    End,
}

/// One scanned span with its decoded payload and starting position.
#[derive(Clone)]
pub(super) struct Token {
    pub(super) kind: Kind,
    pub(super) text: String,
    pub(super) data: Vec<u8>,
    pub(super) line: i64,
    pub(super) column: i64,
}

/// A scalar-indexed scanner; line and column always name the next character.
struct Scanner<'a> {
    characters: &'a [char],
    index: usize,
    line: i64,
    column: i64,
}

impl Scanner<'_> {
    fn at_end(&self) -> bool {
        self.index >= self.characters.len()
    }

    fn current(&self) -> char {
        self.characters[self.index]
    }

    fn following(&self, distance: usize) -> Option<char> {
        self.characters.get(self.index + distance).copied()
    }

    fn advance(&mut self) -> char {
        let character = self.characters[self.index];
        self.index += 1;
        if character == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        character
    }

    fn token(&self, kind: Kind, text: String, data: Vec<u8>, start: (i64, i64)) -> Token {
        Token {
            kind,
            text,
            data,
            line: start.0,
            column: start.1,
        }
    }
}

/// Scan every token, including the terminal end token.
pub(super) fn scan(characters: &[char]) -> Read<Vec<Token>> {
    let mut scanner = Scanner {
        characters,
        index: 0,
        line: 1,
        column: 1,
    };
    let mut tokens: Vec<Token> = Vec::new();
    loop {
        skip_layout(&mut scanner);
        let start = (scanner.line, scanner.column);
        if scanner.at_end() {
            tokens.push(scanner.token(Kind::End, String::new(), Vec::new(), start));
            return Ok(tokens);
        }
        let token = next_token(&mut scanner, tokens.last(), start)?;
        tokens.push(token);
    }
}

/// Discard whitespace and comments while retaining exact positions.
fn skip_layout(scanner: &mut Scanner) {
    let mut again = true;
    while again && !scanner.at_end() {
        again = false;
        while !scanner.at_end() && matches!(scanner.current(), ' ' | '\t' | '\r' | '\n') {
            scanner.advance();
            again = true;
        }
        if !scanner.at_end() && scanner.current() == '#' {
            while !scanner.at_end() && scanner.current() != '\n' {
                scanner.advance();
            }
            again = true;
        }
    }
}

/// Scan one span according to its leading scalar.
fn next_token(scanner: &mut Scanner, previous: Option<&Token>, start: (i64, i64)) -> Read<Token> {
    let character = scanner.current();
    if character == '"' {
        return string_token(scanner, start);
    }
    if character == 'b' && scanner.following(1) == Some('"') {
        return binary_token(scanner, start);
    }
    if character == '<' {
        if scanner.following(1) == Some('<') {
            scanner.advance();
            scanner.advance();
            return Ok(scanner.token(Kind::Punctuation, "<<".to_string(), Vec::new(), start));
        }
        let constructor = previous.is_some_and(|token| {
            token.kind != Kind::Text
                && matches!(
                    token.text.as_str(),
                    "decimal" | "ref" | "list" | "set" | "option" | "map"
                )
        });
        scanner.advance();
        if constructor {
            return Ok(scanner.token(Kind::Punctuation, "<".to_string(), Vec::new(), start));
        }
        let mut text = String::from('<');
        while !scanner.at_end() && scanner.current() != '>' {
            text.push(scanner.advance());
        }
        if scanner.at_end() {
            return Err(("unexpected_end", scanner.line, scanner.column));
        }
        text.push(scanner.advance());
        return Ok(scanner.token(Kind::Marker, text, Vec::new(), start));
    }
    if PUNCTUATION.contains(&character) {
        scanner.advance();
        return Ok(scanner.token(Kind::Punctuation, character.to_string(), Vec::new(), start));
    }
    let mut text = String::new();
    while !scanner.at_end() && !DELIMITERS.contains(&scanner.current()) {
        text.push(scanner.advance());
    }
    if text.is_empty() {
        text.push(scanner.advance());
    }
    let first = text.chars().next().unwrap_or(' ');
    let numeric =
        matches!(text.as_str(), "inf" | "-inf" | "nan") || first.is_ascii_digit() || first == '+' || first == '-';
    let kind = if numeric { Kind::Number } else { Kind::Word };
    Ok(scanner.token(kind, text, Vec::new(), start))
}

/// Decode a quoted or triple-quoted string.
fn string_token(scanner: &mut Scanner, start: (i64, i64)) -> Read<Token> {
    if scanner.following(1) == Some('"') && scanner.following(2) == Some('"') {
        for _ in 0..3 {
            scanner.advance();
        }
        let mut value = String::new();
        while !scanner.at_end() {
            if scanner.current() == '"' && scanner.following(1) == Some('"') && scanner.following(2) == Some('"') {
                for _ in 0..3 {
                    scanner.advance();
                }
                return Ok(scanner.token(Kind::Text, value, Vec::new(), start));
            }
            value.push(scanner.advance());
        }
        return Err(("unexpected_end", scanner.line, scanner.column));
    }
    scanner.advance();
    let mut value = String::new();
    while !scanner.at_end() {
        let character = scanner.advance();
        if character == '"' {
            return Ok(scanner.token(Kind::Text, value, Vec::new(), start));
        }
        if character == '\\' {
            let position = (scanner.line, scanner.column - 1);
            value.push_str(&escaped(scanner, position)?);
        } else {
            value.push(character);
        }
    }
    Err(("unexpected_end", scanner.line, scanner.column))
}

/// Decode the closed escape set and lowercase variable-width Unicode escapes.
fn escaped(scanner: &mut Scanner, position: (i64, i64)) -> Read<String> {
    let invalid = ("invalid_escape", position.0, position.1);
    if scanner.at_end() {
        return Err(("unexpected_end", scanner.line, scanner.column));
    }
    match scanner.advance() {
        '\\' => Ok("\\".to_string()),
        '"' => Ok("\"".to_string()),
        'n' => Ok("\n".to_string()),
        't' => Ok("\t".to_string()),
        'r' => Ok("\r".to_string()),
        'u' => {
            if scanner.at_end() || scanner.advance() != '{' {
                return Err(invalid);
            }
            let mut digits = String::new();
            while !scanner.at_end() && scanner.current() != '}' {
                let digit = scanner.advance();
                if !digit.is_ascii_digit() && !('a'..='f').contains(&digit) {
                    return Err(invalid);
                }
                digits.push(digit);
            }
            if scanner.at_end() || digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
                return Err(invalid);
            }
            scanner.advance();
            let value = i64::from_str_radix(&digits, 16).map_err(|_| invalid)?;
            scalar_text(value).ok_or(invalid)
        }
        _ => Err(invalid),
    }
}

/// Decode printable ASCII and exact two-digit lowercase hexadecimal byte escapes.
fn binary_token(scanner: &mut Scanner, start: (i64, i64)) -> Read<Token> {
    let invalid = |kind: &'static str| (kind, start.0, start.1);
    scanner.advance();
    scanner.advance();
    let mut data = Vec::new();
    while !scanner.at_end() {
        let character = scanner.advance();
        if character == '"' {
            return Ok(scanner.token(Kind::Binary, String::new(), data, start));
        }
        if character != '\\' {
            data.push(ascii_byte(character).ok_or(invalid("invalid_binary"))?);
            continue;
        }
        if scanner.at_end() {
            return Err(("unexpected_end", scanner.line, scanner.column));
        }
        match scanner.advance() {
            escape @ ('\\' | '"') => data.push(escape as u8),
            'x' => {
                let mut digits = String::new();
                for _ in 0..2 {
                    if scanner.at_end() {
                        return Err(("unexpected_end", scanner.line, scanner.column));
                    }
                    digits.push(scanner.advance());
                }
                if !digits
                    .chars()
                    .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit))
                {
                    return Err(invalid("invalid_escape"));
                }
                data.push(u8::from_str_radix(&digits, 16).map_err(|_| invalid("invalid_escape"))?);
            }
            _ => return Err(invalid("invalid_escape")),
        }
    }
    Err(("unexpected_end", scanner.line, scanner.column))
}

/// One printable ASCII byte.
fn ascii_byte(character: char) -> Option<u8> {
    (character.is_ascii() && (' '..='~').contains(&character)).then_some(character as u8)
}

/// Admit one core name: an identifier that is already NFC.
pub(super) fn require_name(value: &str, line: i64, column: i64, invalid: &'static str) -> Read<String> {
    let difference = crate::unicode_names::first_nfc_difference(value);
    if difference >= 0 {
        return Err(("unnormalized_name", line, column + difference));
    }
    if !crate::unicode_names::is_xid_name(value) {
        return Err((invalid, line, column));
    }
    Ok(value.to_string())
}

/// Admit every nonempty dotted path segment under the same name rule.
pub(super) fn require_path(value: &str, line: i64, column: i64, invalid: &'static str) -> Read<String> {
    if value.is_empty() {
        return Err((invalid, line, column));
    }
    let mut offset = 0;
    for segment in value.split('.') {
        require_name(segment, line, column + offset, invalid)?;
        offset += segment.chars().count() as i64 + 1;
    }
    Ok(value.to_string())
}
