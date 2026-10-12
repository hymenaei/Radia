use std::{collections::HashSet, fmt::Write as _, mem::take};

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
    UUID,
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

pub fn generate(template: &Template) -> String {
    let mut output = String::new();
    output.push_str("use crate::types::{Frequency, MessageId};\n");
    output.push_str("use crate::types::Frequency::{Fixed, High, Low, Medium};\n\n");
    output.push_str("/// Trust classification declared by an LLUDP message template.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum MessageTrust {\n    /// The message requires a trusted circuit.\n    Trusted,\n    /// The message does not require a trusted circuit.\n    NotTrusted,\n}\n\n",
    );
    output.push_str("/// Payload encoding declared by an LLUDP message template.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum MessageEncoding {\n    /// The datagram's message bytes use LLUDP zero coding.\n    Zerocoded,\n    /// The datagram's message bytes are sent without zero coding.\n    Unencoded,\n}\n\n",
    );
    output.push_str("/// Flags describing template status or LLUDP policy for a message.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum MessageFlag {\n    /// The template marks the message as deprecated.\n    Deprecated,\n    /// The template blacklists the message for LLUDP.\n    LLUdpBlacklisted,\n    /// The template marks LLUDP use of the message as deprecated.\n    LLUdpDeprecated,\n}\n\n",
    );
    output.push_str("/// Repetition mode of a message block in the wire payload.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub enum BlockMode {\n    /// Exactly one block instance, with no count prefix.\n    Single,\n    /// A block count followed by that many instances; the count is one byte.\n    Variable,\n    /// A fixed number of instances, with the count stored in the schema rather than the payload.\n    Multiple(\n        /// Number of block instances.\n        u32,\n    ),\n}\n\n",
    );
    output.push_str(
        "/// Wire encoding of one field in an LLUDP message template. `S` and `U` variants are signed and unsigned integers named by bit width; multibyte numeric values use little-endian order.\n",
    );
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str("pub enum FieldKind {\n");
    output.push_str(
        "    /// Boolean encoded as one byte; any nonzero byte decodes as true.\n    Bool,\n    /// 32-bit IEEE-754 float in little-endian order.\n    F32,\n    /// 64-bit IEEE-754 float in little-endian order.\n    F64,\n    /// Four IPv4 address octets in wire order.\n    IpAddr,\n    /// Two port bytes in wire order.\n    IpPort,\n    /// Field with no payload bytes.\n    Null,\n    /// Three little-endian `f32` quaternion components.\n    Quaternion,\n    /// 16-byte UUID in the wire byte order.\n    UUID,\n",
    );
    output.push_str(
        "    /// Three little-endian `f32` components.\n    Vector3,\n    /// Three little-endian `f64` components.\n    Vector3d,\n    /// Four little-endian `f32` components.\n    Vector4,\n    S8,\n    S16,\n    S32,\n    S64,\n    U8,\n    U16,\n    U32,\n    U64,\n",
    );
    output.push_str(
        "    /// Raw bytes whose size is fixed by the template.\n    Fixed(\n        /// Number of field bytes.\n        u32,\n    ),\n    /// Raw bytes with a length prefix.\n    Variable(\n        /// Prefix width in bytes; only 1 and 2 are supported.\n        u8,\n    ),\n}\n\n",
    );
    output.push_str("/// Name and wire encoding of one field in a message block.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub struct FieldSchema {\n    /// Field name from the message template.\n    pub name: &'static str,\n    /// Wire encoding for this field.\n    pub kind: FieldKind,\n}\n\n",
    );
    output.push_str("/// Field definitions and repetition mode for one message block.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str(
        "pub struct BlockSchema {\n    /// Block name from the message template.\n    pub name: &'static str,\n    /// Repetition count for this block.\n    pub mode: BlockMode,\n    /// Fields in wire order.\n    pub fields: &'static [FieldSchema],\n}\n\n",
    );
    output.push_str("/// Wire ID, policy, encoding, flags, and ordered blocks for one template message.\n");
    output.push_str("#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n");
    output.push_str("pub struct MessageSchema {\n");
    output.push_str(
        "    /// Message name from the template.\n    pub name: &'static str,\n    /// Frequency and number used by the wire protocol.\n    pub id: MessageId,\n    /// Trust classification from the template.\n    pub trust: MessageTrust,\n",
    );
    output.push_str(
        "    /// Default zero-coding mode from the template.\n    pub encoding: MessageEncoding,\n    /// Status and LLUDP-policy flags from the template.\n    pub flags: &'static [MessageFlag],\n",
    );
    output.push_str("    /// Blocks in wire order.\n    pub blocks: &'static [BlockSchema],\n}\n\n");
    output.push_str("/// Number of messages in the compiled template.\n");
    writeln!(&mut output, "pub const MESSAGE_COUNT: usize = {};\n", template.messages.len()).expect("write generated source");
    output.push_str("/// Messages in the order declared by the template.\npub const MESSAGES: &[MessageSchema] = &[\n");

    for message in &template.messages {
        output.push_str("    MessageSchema {\n");
        writeln!(&mut output, "        name: {:?},", message.name).expect("write generated source");
        writeln!(&mut output, "        id: MessageId::{}({}),", frequency_constructor(message.frequency), message.id)
            .expect("write generated source");
        writeln!(&mut output, "        trust: MessageTrust::{:?},", message.trust).expect("write generated source");
        writeln!(&mut output, "        encoding: MessageEncoding::{:?},", message.encoding).expect("write generated source");
        if message.flags.is_empty() {
            output.push_str("        flags: &[],\n");
        } else {
            output.push_str("        flags: &[\n");
            for flag in &message.flags {
                writeln!(&mut output, "            MessageFlag::{flag:?},").expect("write generated source");
            }
            output.push_str("        ],\n");
        }
        if message.blocks.is_empty() {
            output.push_str("        blocks: &[],\n");
        } else {
            output.push_str("        blocks: &[\n");
            for block in &message.blocks {
                output.push_str("            BlockSchema {\n");
                writeln!(&mut output, "                name: {:?},", block.name).expect("write generated source");
                writeln!(&mut output, "                mode: {},", block_mode(block.mode)).expect("write generated source");
                if block.fields.is_empty() {
                    output.push_str("                fields: &[],\n");
                } else {
                    output.push_str("                fields: &[\n");
                    for field in &block.fields {
                        output.push_str("                    FieldSchema {\n");
                        writeln!(&mut output, "                        name: {:?},", field.name).expect("write generated source");
                        writeln!(&mut output, "                        kind: {},", field_kind(field.kind)).expect("write generated source");
                        output.push_str("                    },\n");
                    }
                    output.push_str("                ],\n");
                }
                output.push_str("            },\n");
            }
            output.push_str("        ],\n");
        }
        output.push_str("    },\n");
    }
    let mut id_index = (0..template.messages.len()).collect::<Vec<_>>();
    id_index.sort_by_key(|index| {
        let message = &template.messages[*index];
        (frequency_rank(message.frequency), message.id)
    });
    output.push_str("];\n\n");
    output.push_str("// This sorted indirection keeps ID lookup logarithmic without reordering the readable schema list.\n");
    output.push_str("const MESSAGE_ID_INDEX: &[usize] = &[\n");
    for index in id_index {
        writeln!(&mut output, "    {index},").expect("write generated source");
    }
    output.push_str("];\n\n");
    output.push_str("fn frequency_rank(frequency: Frequency) -> u8 {\n");
    output.push_str("    match frequency {\n");
    output.push_str("        High => 0,\n");
    output.push_str("        Medium => 1,\n");
    output.push_str("        Low => 2,\n");
    output.push_str("        Fixed => 3,\n");
    output.push_str("    }\n}\n\n");
    output.push_str(
        "/// Find the schema for an exact message frequency and number.\n///\n/// Returns `None` when the template has no matching ID.\npub fn find_by_id(id: MessageId) -> Option<&'static MessageSchema> {\n",
    );
    output.push_str("    let key = (frequency_rank(id.frequency), id.number);\n");
    output.push_str("    let index = MESSAGE_ID_INDEX.binary_search_by_key(&key, |index| {\n");
    output.push_str("        let message = &MESSAGES[*index];\n");
    output.push_str("        (frequency_rank(message.id.frequency), message.id.number)\n");
    output.push_str("    }).ok()?;\n");
    output.push_str("    Some(&MESSAGES[MESSAGE_ID_INDEX[index]])\n}\n\n");
    output.push_str(
        "/// Find a schema by its exact, case-sensitive template message name.\n///\n/// Returns `None` when the template has no matching name.\npub fn find_by_name(name: &str) -> Option<&'static MessageSchema> {\n",
    );
    output.push_str("    MESSAGES.iter().find(|message| message.name == name)\n}\n\n");
    output.push_str(
        "/// Named wire identifiers for messages in the checked-in template.\n///\n/// Each value uses the message's template frequency and number.\npub mod ids {\n    use crate::types::MessageId;\n",
    );
    for message in &template.messages {
        writeln!(
            &mut output,
            "    pub const {}: MessageId = MessageId::{}({});",
            rust_constant_name(&message.name),
            frequency_constructor(message.frequency),
            message.id
        )
        .expect("write generated source");
    }
    output.push_str("}\n");
    output
}

fn tokenize(source: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let line = line.split_once("//").map_or(line, |(before, _)| before);
        let mut word = String::new();
        let mut word_start = line_index + 1;
        let flush = |word: &mut String, tokens: &mut Vec<Token>, line: usize| {
            if !word.is_empty() {
                tokens.push(Token { text: take(word), line });
            }
        };
        for character in line.chars() {
            match character {
                '{' | '}' => {
                    flush(&mut word, &mut tokens, word_start);
                    tokens.push(Token { text: character.to_string(), line: line_index + 1 });
                    word_start = line_index + 1;
                }
                value if value.is_whitespace() => {
                    flush(&mut word, &mut tokens, word_start);
                    word_start = line_index + 1;
                }
                value => {
                    if word.is_empty() {
                        word_start = line_index + 1;
                    }
                    word.push(value);
                }
            }
        }
        flush(&mut word, &mut tokens, word_start);
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
        while self.peek_text() == Some("{") {
            self.expect_open()?;
            blocks.push(self.parse_block()?);
        }
        self.expect_close()?;

        Ok(Message { name, frequency, id, trust, encoding, flags, blocks })
    }

    fn parse_block(&mut self) -> Result<Block, String> {
        let header = self.take_words_until_brace("block declaration")?;
        if header.len() < 2 {
            return Err(self.error_tokens(&header, "block declaration needs a name and mode"));
        }
        validate_identifier(self.path, &header[0])?;
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
        while self.peek_text() == Some("{") {
            self.expect_open()?;
            fields.push(self.parse_field()?);
        }
        self.expect_close()?;

        Ok(Block { name: header[0].text.clone(), mode, fields })
    }

    fn parse_field(&mut self) -> Result<Field, String> {
        let declaration = self.take_words_until_close("field declaration")?;
        if declaration.len() < 2 {
            return Err(self.error_tokens(&declaration, "field declaration needs a name and type"));
        }
        validate_identifier(self.path, &declaration[0])?;
        let kind = match declaration[1].text.as_str() {
            "BOOL" => FieldKind::Bool,
            "F32" => FieldKind::F32,
            "F64" => FieldKind::F64,
            "IPADDR" => FieldKind::IpAddr,
            "IPPORT" => FieldKind::IpPort,
            "NULL" => FieldKind::Null,
            "LLQuaternion" => FieldKind::Quaternion,
            "LLUUID" => FieldKind::UUID,
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
        format!("{}:{}: {}", self.path, token.line, message.as_ref())
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
        Err(format!("{path}:{}: invalid identifier {}", token.line, token.text))
    }
}

fn parse_number(path: &str, token: &Token) -> Result<u32, String> {
    let parsed = token
        .text
        .strip_prefix("0x")
        .or_else(|| token.text.strip_prefix("0X"))
        .map_or_else(|| token.text.parse::<u32>(), |value| u32::from_str_radix(value, 16));
    parsed.map_err(|_| format!("{path}:{}: invalid message ID {}", token.line, token.text))
}

fn validate_message_id(path: &str, token: &Token, frequency: Frequency, id: u32) -> Result<(), String> {
    let valid = match frequency {
        Frequency::High | Frequency::Medium => (1..=0xfe).contains(&id),
        Frequency::Low => (1..=0xfff9).contains(&id),
        Frequency::Fixed => (0xffff_fffa..=u32::MAX).contains(&id),
    };
    if valid { Ok(()) } else { Err(format!("{path}:{}: ID {} is outside the {:?} message range", token.line, token.text, frequency)) }
}

fn rust_constant_name(name: &str) -> String {
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

fn frequency_constructor(frequency: Frequency) -> &'static str {
    match frequency {
        Frequency::High => "high",
        Frequency::Medium => "medium",
        Frequency::Low => "low",
        Frequency::Fixed => "fixed",
    }
}

fn frequency_rank(frequency: Frequency) -> u8 {
    match frequency {
        Frequency::High => 0,
        Frequency::Medium => 1,
        Frequency::Low => 2,
        Frequency::Fixed => 3,
    }
}

fn block_mode(mode: BlockMode) -> String {
    match mode {
        BlockMode::Single => "BlockMode::Single".into(),
        BlockMode::Variable => "BlockMode::Variable".into(),
        BlockMode::Multiple(count) => format!("BlockMode::Multiple({count})"),
    }
}

fn field_kind(kind: FieldKind) -> String {
    match kind {
        FieldKind::Bool => "FieldKind::Bool".into(),
        FieldKind::F32 => "FieldKind::F32".into(),
        FieldKind::F64 => "FieldKind::F64".into(),
        FieldKind::IpAddr => "FieldKind::IpAddr".into(),
        FieldKind::IpPort => "FieldKind::IpPort".into(),
        FieldKind::Null => "FieldKind::Null".into(),
        FieldKind::Quaternion => "FieldKind::Quaternion".into(),
        FieldKind::UUID => "FieldKind::UUID".into(),
        FieldKind::Vector3 => "FieldKind::Vector3".into(),
        FieldKind::Vector3d => "FieldKind::Vector3d".into(),
        FieldKind::Vector4 => "FieldKind::Vector4".into(),
        FieldKind::S8 => "FieldKind::S8".into(),
        FieldKind::S16 => "FieldKind::S16".into(),
        FieldKind::S32 => "FieldKind::S32".into(),
        FieldKind::S64 => "FieldKind::S64".into(),
        FieldKind::U8 => "FieldKind::U8".into(),
        FieldKind::U16 => "FieldKind::U16".into(),
        FieldKind::U32 => "FieldKind::U32".into(),
        FieldKind::U64 => "FieldKind::U64".into(),
        FieldKind::Fixed(size) => format!("FieldKind::Fixed({size})"),
        FieldKind::Variable(size) => format!("FieldKind::Variable({size})"),
    }
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
        let generated = generate(&template);
        assert!(generated.contains("FieldKind::Null"));
        assert!(generated.contains("FieldKind::S64"));
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
}
