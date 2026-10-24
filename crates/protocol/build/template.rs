use std::{collections::HashSet, mem::take};

#[derive(Debug)]
pub struct Template {
    pub messages: Vec<Message>,
}

#[derive(Debug)]
pub struct Message {
    pub name: String,
    pub frequency: Frequency,
    pub id: u32,
    pub trust: Trust,
    pub encoding: Encoding,
    pub flags: Vec<Flag>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Frequency {
    High,
    Medium,
    Low,
    Fixed,
}

#[derive(Clone, Copy, Debug)]
pub enum Trust {
    Trusted,
    NotTrusted,
}

#[derive(Clone, Copy, Debug)]
pub enum Encoding {
    Zerocoded,
    Unencoded,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Flag {
    Deprecated,
    LLUdpBlacklisted,
    LLUdpDeprecated,
}

#[derive(Debug)]
pub struct Block {
    pub name: String,
    pub mode: BlockMode,
    pub fields: Vec<Field>,
}

#[derive(Clone, Copy, Debug)]
pub enum BlockMode {
    Single,
    Variable,
    Multiple(u32),
}

#[derive(Debug)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
}

#[derive(Clone, Copy, Debug)]
pub enum FieldKind {
    Bool,
    F32,
    F64,
    IpAddr,
    IpPort,
    Null,
    Quaternion,
    Uuid,
    Vector3,
    Vector3d,
    Vector4,
    S8,
    S16,
    S32,
    S64,
    U8,
    U16,
    U32,
    U64,
    Fixed(u32),
    Variable(u8),
}

#[derive(Clone, Debug)]
struct Token {
    text: String,
    line: usize,
    column: usize,
}

pub fn parse(path: &str, source: &str) -> Result<Template, String> {
    let tokens = tokenize(source);
    let mut parser = Parser { path, tokens, cursor: 0 };

    parser.expect_word("version")?;
    let version = parser.next_word("template version")?;
    if version.text != "2.0" {
        return Err(parser.error(&version, format!("unsupported template version {}", version.text)));
    }

    let mut messages = Vec::new();
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    let mut generated_names = HashSet::new();
    while parser.cursor < parser.tokens.len() {
        parser.expect_open()?;
        let message = parser.parse_message()?;
        if !names.insert(message.name.clone()) {
            return Err(parser.error_current(format!("duplicate message name {}", message.name)));
        }
        if !ids.insert((message.frequency, message.id)) {
            return Err(parser.error_current(format!("duplicate {:?} message ID {}", message.frequency, message.id)));
        }
        let generated_name = rust_constant_name(&message.name);
        if !generated_names.insert(generated_name.clone()) {
            return Err(parser.error_current(format!("message names collide as generated Rust constant {generated_name}")));
        }
        messages.push(message);
    }

    if messages.is_empty() {
        return Err(format!("{path}: template contains no messages"));
    }
    Ok(Template { messages })
}

fn tokenize(source: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let line = line.split_once("//").map_or(line, |(before, _)| before);
        let mut word = String::new();
        let mut word_column = 1;
        let flush = |word: &mut String, tokens: &mut Vec<Token>, line: usize, column: usize| {
            if !word.is_empty() {
                tokens.push(Token { text: take(word), line, column });
            }
        };
        for (column_index, character) in line.chars().enumerate() {
            let column = column_index + 1;
            match character {
                '{' | '}' => {
                    flush(&mut word, &mut tokens, line_index + 1, word_column);
                    tokens.push(Token { text: character.to_string(), line: line_index + 1, column });
                    word_column = column + 1;
                }
                value if value.is_whitespace() => {
                    flush(&mut word, &mut tokens, line_index + 1, word_column);
                    word_column = column + 1;
                }
                value => {
                    if word.is_empty() {
                        word_column = column;
                    }
                    word.push(value);
                }
            }
        }
        flush(&mut word, &mut tokens, line_index + 1, word_column);
    }
    tokens
}

struct Parser<'a> {
    path: &'a str,
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser<'_> {
    fn parse_message(&mut self) -> Result<Message, String> {
        let header = self.take_words_until_brace("message declaration")?;
        if header.len() < 5 {
            return Err(self.error_tokens(&header, "message declaration needs five fields"));
        }
        let name = header[0].text.clone();
        validate_identifier(self.path, &header[0])?;
        let frequency = self.parse_frequency(&header[1])?;
        let id = parse_number(self.path, &header[2])?;
        validate_message_id(self.path, &header[2], frequency, id)?;
        let trust = self.parse_trust(&header[3])?;
        let encoding = self.parse_encoding(&header[4])?;
        let mut flags = Vec::new();
        for token in &header[5..] {
            let flag = self.parse_flag(token)?;
            if flags.contains(&flag) {
                return Err(self.error(token, "duplicate message flag"));
            }
            flags.push(flag);
        }

        let mut blocks = Vec::new();
        let mut block_names = HashSet::new();
        while self.peek_text() == Some("{") {
            self.expect_open()?;
            blocks.push(self.parse_block(&mut block_names)?);
        }
        self.expect_close()?;

        Ok(Message { name, frequency, id, trust, encoding, flags, blocks })
    }

    fn parse_block(&mut self, block_names: &mut HashSet<String>) -> Result<Block, String> {
        let header = self.take_words_until_brace("block declaration")?;
        if header.len() < 2 {
            return Err(self.error_tokens(&header, "block declaration needs a name and mode"));
        }
        validate_identifier(self.path, &header[0])?;
        if !block_names.insert(header[0].text.clone()) {
            return Err(self.error(&header[0], format!("duplicate block name {} in message", header[0].text)));
        }
        let mode = match header[1].text.as_str() {
            "Single" if header.len() == 2 => BlockMode::Single,
            "Variable" if header.len() == 2 => BlockMode::Variable,
            "Multiple" if header.len() == 3 => {
                let count = header[2].text.parse::<u32>().map_err(|_| self.error(&header[2], "invalid Multiple block count"))?;
                if count == 0 {
                    return Err(self.error(&header[2], "Multiple block count must be positive"));
                }
                BlockMode::Multiple(count)
            }
            "Single" | "Variable" | "Multiple" => {
                return Err(self.error_tokens(&header, "invalid block declaration fields"));
            }
            _ => return Err(self.error(&header[1], "unknown block mode")),
        };

        let mut fields = Vec::new();
        let mut field_names = HashSet::new();
        while self.peek_text() == Some("{") {
            self.expect_open()?;
            fields.push(self.parse_field(&mut field_names)?);
        }
        self.expect_close()?;

        Ok(Block { name: header[0].text.clone(), mode, fields })
    }

    fn parse_field(&mut self, field_names: &mut HashSet<String>) -> Result<Field, String> {
        let declaration = self.take_words_until_close("field declaration")?;
        if declaration.len() < 2 {
            return Err(self.error_tokens(&declaration, "field declaration needs a name and type"));
        }
        validate_identifier(self.path, &declaration[0])?;
        if !field_names.insert(declaration[0].text.clone()) {
            return Err(self.error(&declaration[0], format!("duplicate field name {} in block", declaration[0].text)));
        }
        let kind = match declaration[1].text.as_str() {
            "BOOL" => FieldKind::Bool,
            "F32" => FieldKind::F32,
            "F64" => FieldKind::F64,
            "IPADDR" => FieldKind::IpAddr,
            "IPPORT" => FieldKind::IpPort,
            "NULL" => FieldKind::Null,
            "LLQuaternion" => FieldKind::Quaternion,
            "LLUUID" => FieldKind::Uuid,
            "LLVector3" => FieldKind::Vector3,
            "LLVector3d" => FieldKind::Vector3d,
            "LLVector4" => FieldKind::Vector4,
            "S8" => FieldKind::S8,
            "S16" => FieldKind::S16,
            "S32" => FieldKind::S32,
            "S64" => FieldKind::S64,
            "U8" => FieldKind::U8,
            "U16" => FieldKind::U16,
            "U32" => FieldKind::U32,
            "U64" => FieldKind::U64,
            "Fixed" if declaration.len() == 3 => {
                let size = declaration[2].text.parse::<u32>().map_err(|_| self.error(&declaration[2], "invalid fixed field size"))?;
                if size == 0 {
                    return Err(self.error(&declaration[2], "fixed field size must be positive"));
                }
                FieldKind::Fixed(size)
            }
            "Variable" if declaration.len() == 3 => {
                let size = declaration[2].text.parse::<u8>().map_err(|_| self.error(&declaration[2], "invalid variable length prefix"))?;
                if !matches!(size, 1 | 2) {
                    return Err(self.error(&declaration[2], "variable length prefix must be one or two bytes"));
                }
                FieldKind::Variable(size)
            }
            "Fixed" | "Variable" => {
                return Err(self.error_tokens(&declaration, "field type has invalid arguments"));
            }
            _ => return Err(self.error(&declaration[1], "unknown field type")),
        };
        if declaration.len() != 2 && !matches!(kind, FieldKind::Fixed(_) | FieldKind::Variable(_)) {
            return Err(self.error_tokens(&declaration, "scalar field type has extra arguments"));
        }

        Ok(Field { name: declaration[0].text.clone(), kind })
    }

    fn parse_frequency(&self, token: &Token) -> Result<Frequency, String> {
        match token.text.as_str() {
            "High" => Ok(Frequency::High),
            "Medium" => Ok(Frequency::Medium),
            "Low" => Ok(Frequency::Low),
            "Fixed" => Ok(Frequency::Fixed),
            _ => Err(self.error(token, "unknown message frequency")),
        }
    }

    fn parse_trust(&self, token: &Token) -> Result<Trust, String> {
        match token.text.as_str() {
            "Trusted" => Ok(Trust::Trusted),
            "NotTrusted" => Ok(Trust::NotTrusted),
            _ => Err(self.error(token, "unknown trust value")),
        }
    }

    fn parse_encoding(&self, token: &Token) -> Result<Encoding, String> {
        match token.text.as_str() {
            "Zerocoded" => Ok(Encoding::Zerocoded),
            "Unencoded" => Ok(Encoding::Unencoded),
            _ => Err(self.error(token, "unknown message encoding")),
        }
    }

    fn parse_flag(&self, token: &Token) -> Result<Flag, String> {
        match token.text.as_str() {
            "Deprecated" => Ok(Flag::Deprecated),
            "UDPBlackListed" => Ok(Flag::LLUdpBlacklisted),
            "UDPDeprecated" => Ok(Flag::LLUdpDeprecated),
            _ => Err(self.error(token, "unknown message flag")),
        }
    }

    fn take_words_until_brace(&mut self, description: &str) -> Result<Vec<Token>, String> {
        let mut words = Vec::new();
        while let Some(token) = self.tokens.get(self.cursor) {
            if token.text == "{" || token.text == "}" {
                return Ok(words);
            }
            if token.text.is_empty() {
                return Err(self.error(token, format!("invalid token in {description}")));
            }
            words.push(token.clone());
            self.cursor += 1;
        }
        Err(self.error_current(format!("unterminated {description}")))
    }

    fn take_words_until_close(&mut self, description: &str) -> Result<Vec<Token>, String> {
        let mut words = Vec::new();
        while let Some(token) = self.tokens.get(self.cursor) {
            match token.text.as_str() {
                "}" => {
                    self.cursor += 1;
                    return Ok(words);
                }
                "{" => return Err(self.error(token, format!("unexpected '{{' in {description}"))),
                _ => {
                    words.push(token.clone());
                    self.cursor += 1;
                }
            }
        }
        Err(self.error_current(format!("unterminated {description}")))
    }

    fn expect_word(&mut self, expected: &str) -> Result<(), String> {
        let token = self.next_word(expected)?;
        if token.text == expected { Ok(()) } else { Err(self.error(&token, format!("expected {expected}"))) }
    }

    fn next_word(&mut self, description: &str) -> Result<Token, String> {
        let token =
            self.tokens.get(self.cursor).cloned().ok_or_else(|| format!("{}: expected {description}, found end of file", self.path))?;
        if token.text == "{" || token.text == "}" {
            return Err(self.error(&token, format!("expected {description}")));
        }
        self.cursor += 1;
        Ok(token)
    }

    fn expect_open(&mut self) -> Result<(), String> {
        let token = self.tokens.get(self.cursor).cloned().ok_or_else(|| format!("{}: expected '{{', found end of file", self.path))?;
        if token.text != "{" {
            return Err(self.error(&token, "expected '{'"));
        }
        self.cursor += 1;
        Ok(())
    }

    fn expect_close(&mut self) -> Result<(), String> {
        let token = self.tokens.get(self.cursor).cloned().ok_or_else(|| format!("{}: expected '}}', found end of file", self.path))?;
        if token.text != "}" {
            return Err(self.error(&token, "expected '}'"));
        }
        self.cursor += 1;
        Ok(())
    }

    fn peek_text(&self) -> Option<&str> {
        self.tokens.get(self.cursor).map(|token| token.text.as_str())
    }

    fn error(&self, token: &Token, message: impl AsRef<str>) -> String {
        format!("{}:{}:{}: {}", self.path, token.line, token.column, message.as_ref())
    }

    fn error_current(&self, message: impl AsRef<str>) -> String {
        match self.tokens.get(self.cursor) {
            Some(token) => self.error(token, message),
            None => format!("{}: {}", self.path, message.as_ref()),
        }
    }

    fn error_tokens(&self, tokens: &[Token], message: impl AsRef<str>) -> String {
        match tokens.first() {
            Some(token) => self.error(token, message),
            None => self.error_current(message),
        }
    }
}

fn validate_identifier(path: &str, token: &Token) -> Result<(), String> {
    let mut characters = token.text.chars();
    let valid_start = characters.next().is_some_and(|value| value.is_ascii_alphabetic() || value == '_');
    if valid_start && characters.all(|value| value.is_ascii_alphanumeric() || value == '_') {
        Ok(())
    } else {
        Err(format!("{path}:{}:{}: invalid identifier {}", token.line, token.column, token.text))
    }
}

fn parse_number(path: &str, token: &Token) -> Result<u32, String> {
    let parsed = token
        .text
        .strip_prefix("0x")
        .or_else(|| token.text.strip_prefix("0X"))
        .map_or_else(|| token.text.parse::<u32>(), |value| u32::from_str_radix(value, 16));
    parsed.map_err(|_| format!("{path}:{}:{}: invalid message ID {}", token.line, token.column, token.text))
}

fn validate_message_id(path: &str, token: &Token, frequency: Frequency, id: u32) -> Result<(), String> {
    let valid = match frequency {
        Frequency::High | Frequency::Medium => (1..=0xfe).contains(&id),
        Frequency::Low => (1..=0xfff9).contains(&id),
        Frequency::Fixed => (0xffff_fffa..=u32::MAX).contains(&id),
    };
    if valid {
        Ok(())
    } else {
        Err(format!("{path}:{}:{}: ID {} is outside the {:?} message range", token.line, token.column, token.text, frequency))
    }
}

pub(super) fn rust_constant_name(name: &str) -> String {
    let characters: Vec<char> = name.chars().collect();
    let mut output = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_ascii_alphanumeric() {
            if !output.ends_with('_') {
                output.push('_');
            }
            continue;
        }
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index + 1);
        let starts_word = character.is_ascii_uppercase()
            && previous.is_some_and(|previous| {
                previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase() && next.is_some_and(char::is_ascii_lowercase))
            });
        if starts_word && !output.ends_with('_') {
            output.push('_');
        }
        output.push(character.to_ascii_uppercase());
    }
    output.trim_matches('_').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_messages_and_fields() {
        let source = include_str!("message_template.msg");
        let template = parse("build/message_template.msg", source).expect("official template parses");

        assert_eq!(template.messages.len(), 483);
        let packet_ack = template.messages.iter().find(|message| message.name == "PacketAck").expect("PacketAck is defined");
        assert_eq!(packet_ack.frequency, Frequency::Fixed);
        assert_eq!(packet_ack.id, 0xffff_fffb);
        assert!(matches!(packet_ack.blocks[0].fields[0].kind, FieldKind::U32));
    }

    #[test]
    fn parses_comments_and_splits() {
        let source =
            "version 2.0\n{\n\tExample Low\n  3 NotTrusted Zerocoded // header\n  { Data Single\n    { Value Variable\n      2 }\n  }\n}\n";
        let template = parse("fixture.msg", source).expect("multiline declarations parse");
        assert!(matches!(template.messages[0].blocks[0].fields[0].kind, FieldKind::Variable(2)));
    }

    #[test]
    fn parses_null_and_i64_fields() {
        let source = "version 2.0\n{ Example Low 3 NotTrusted Unencoded { Data Single { Marker NULL } { Value S64 } } }";
        let template = parse("fixture.msg", source).expect("supported field kinds parse");
        let fields = &template.messages[0].blocks[0].fields;
        assert!(matches!(fields[0].kind, FieldKind::Null));
        assert!(matches!(fields[1].kind, FieldKind::S64));
    }

    #[test]
    fn rejects_bad_fields() {
        let unknown_type = "version 2.0\n{ Example High 1 NotTrusted Unencoded { Data Single { Value UNKNOWN } } }";
        assert!(parse("bad.msg", unknown_type).expect_err("unknown field type fails").contains("unknown field type"));

        let bad_id = "version 2.0\n{ Example High 255 NotTrusted Unencoded }";
        assert!(parse("bad.msg", bad_id).expect_err("reserved high ID fails").contains("outside the High message range"));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let source = "version 2.0\n{ One Low 1 NotTrusted Unencoded }\n{ Two Low 1 NotTrusted Unencoded }";
        assert!(parse("bad.msg", source).expect_err("duplicate IDs fail").contains("duplicate Low message ID 1"));
    }

    #[test]
    fn accepts_fixed_id_range_endpoints() {
        let source = "version 2.0\n{ First Fixed 0xFFFFFFFA NotTrusted Unencoded }\n{ Last Fixed 0xFFFFFFFF NotTrusted Unencoded }";
        let template = parse("fixed.msg", source).expect("fixed ID endpoints are valid");
        assert_eq!(template.messages[0].id, 0xffff_fffa);
        assert_eq!(template.messages[1].id, u32::MAX);

        let below_range = "version 2.0\n{ Below Fixed 0xFFFFFFF9 NotTrusted Unencoded }";
        assert!(parse("fixed.msg", below_range).expect_err("fixed ID below range fails").contains("outside the Fixed message range"));
    }

    #[test]
    fn rejects_duplicate_blocks_with_source_location() {
        let source =
            "version 2.0\n{\nExample Low 1 NotTrusted Unencoded\n{\nData Single\n{ One U8 }\n}\n{\nData Variable\n{ Two U8 }\n}\n}\n";
        assert_eq!(parse("bad.msg", source).expect_err("duplicate block names fail"), "bad.msg:9:1: duplicate block name Data in message");
    }

    #[test]
    fn rejects_duplicate_fields_with_source_location() {
        let source = "version 2.0\n{\nExample Low 1 NotTrusted Unencoded\n{\nData Single\n{ Value U8 }\n{ Value U16 }\n}\n}\n";
        assert_eq!(parse("bad.msg", source).expect_err("duplicate field names fail"), "bad.msg:7:3: duplicate field name Value in block");
    }

    #[test]
    fn duplicate_names_are_scoped_to_their_parent() {
        let source = "version 2.0\n{ Example Low 1 NotTrusted Unencoded { First Single { Value U8 } } { Second Single { Value U16 } } }\n{ Other Low 2 NotTrusted Unencoded { First Single { Value U8 } } }";
        assert_eq!(parse("valid.msg", source).expect("names in separate scopes are valid").messages.len(), 2);
    }

    #[test]
    fn invalid_identifiers_report_scope_and_location() {
        let cases = [
            ("version 2.0\n{ Bad-Message Low 1 NotTrusted Unencoded }", "bad.msg:2:3: invalid identifier Bad-Message"),
            (
                "version 2.0\n{\nExample Low 1 NotTrusted Unencoded\n{\nBad-Block Single\n}\n}\n",
                "bad.msg:5:1: invalid identifier Bad-Block",
            ),
            (
                "version 2.0\n{\nExample Low 1 NotTrusted Unencoded\n{\nData Single\n{ Bad-Field U8 }\n}\n}\n",
                "bad.msg:6:3: invalid identifier Bad-Field",
            ),
        ];

        for (source, expected) in cases {
            assert_eq!(parse("bad.msg", source).expect_err("invalid identifiers fail"), expected);
        }
    }
}
