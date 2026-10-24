use std::{borrow::Cow, str};

use quick_xml::{
    Reader, XmlVersion,
    escape::resolve_predefined_entity,
    events::{BytesStart, Event},
};

use crate::{Error, Limits, Result};

#[derive(Debug)]
pub(super) enum Token<'a> {
    Start(&'static str),
    Empty(&'static str),
    End(&'static str),
    Text(Cow<'a, str>),
    CharacterReference(Cow<'a, str>),
}

impl Token<'_> {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) | Self::CharacterReference(text) => Some(text),
            _ => None,
        }
    }
}

pub(super) struct Tokens<'input> {
    reader: Reader<&'input [u8]>,
    pending: Option<Token<'input>>,
    preserve_text: Vec<bool>,
    limits: Limits,
    count: usize,
    text_run: bool,
    ended: bool,
}

impl<'input> Tokens<'input> {
    pub(super) fn new(input: &'input [u8], limits: Limits) -> Result<Self> {
        let input = str::from_utf8(input).map_err(|_| Error::InvalidUtf8)?;
        let mut reader = Reader::from_str(input);
        reader.config_mut().trim_text(false);
        Ok(Self { reader, pending: None, preserve_text: Vec::new(), limits, count: 0, text_run: false, ended: false })
    }

    pub(super) fn peek(&mut self) -> Result<Option<&Token<'input>>> {
        if self.pending.is_none() && !self.ended {
            self.pending = self.read()?;
        }
        Ok(self.pending.as_ref())
    }

    pub(super) fn take(&mut self) -> Result<Option<Token<'input>>> {
        self.peek()?;
        Ok(self.pending.take())
    }

    fn check_next_name(&self) -> Result<()> {
        let remaining = self.reader.get_ref().strip_prefix(b"\xef\xbb\xbf").unwrap_or(self.reader.get_ref());
        let Some(markup) = remaining.strip_prefix(b"<") else {
            return Ok(());
        };
        if markup.starts_with(b"!") || markup.starts_with(b"?") {
            return Ok(());
        }
        let name = markup.strip_prefix(b"/").unwrap_or(markup);
        let len = name.iter().position(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>')).unwrap_or(name.len());
        tag_name(str::from_utf8(&name[..len]).map_err(|_| Error::InvalidUtf8)?)?;
        Ok(())
    }

    fn read(&mut self) -> Result<Option<Token<'input>>> {
        loop {
            self.check_next_name()?;
            let token = match self.reader.read_event() {
                Ok(Event::Start(element)) => {
                    let name = tag_name(element.name().as_ref())?;
                    if name == "binary" {
                        validate_binary_encoding(&element)?;
                    }
                    self.preserve_text.push(!matches!(name, "llsd" | "array" | "map"));
                    Token::Start(name)
                }
                Ok(Event::Empty(element)) => {
                    let name = tag_name(element.name().as_ref())?;
                    if name == "binary" {
                        validate_binary_encoding(&element)?;
                    }
                    Token::Empty(name)
                }
                Ok(Event::End(element)) => {
                    self.preserve_text.pop();
                    Token::End(tag_name(element.name().as_ref())?)
                }
                Ok(Event::Text(text)) => Token::Text(text.into_inner()),
                Ok(Event::GeneralRef(reference)) => {
                    let text =
                        match reference.resolve_char_ref().map_err(|_| Error::InvalidValue("invalid XML character reference".into()))? {
                            Some(ch) => Cow::Owned(ch.to_string()),
                            None => Cow::Borrowed(
                                resolve_predefined_entity(reference.as_ref())
                                    .ok_or_else(|| Error::InvalidValue("unknown XML entity".into()))?,
                            ),
                        };
                    Token::CharacterReference(text)
                }
                Ok(Event::CData(text)) => Token::Text(text.into_inner()),
                Ok(Event::DocType(_)) => continue,
                Ok(Event::Eof) => {
                    self.ended = true;
                    return Ok(None);
                }
                Ok(Event::Decl(_) | Event::PI(_) | Event::Comment(_)) => continue,
                Err(error) => return Err(Error::InvalidValue(error.to_string())),
            };
            if let Some(value) = token.text() {
                let preserve_text = self.preserve_text.last().copied().unwrap_or_default();
                if !preserve_text && value.bytes().all(|byte| byte.is_ascii_whitespace()) {
                    continue;
                }
            }
            let text = token.text().is_some();
            if !text || !self.text_run {
                if self.count >= self.limits.max_values.saturating_mul(8).saturating_add(16) {
                    return Err(Error::LimitExceeded("XML token count"));
                }
                self.count += 1;
            }
            self.text_run = text;
            return Ok(Some(token));
        }
    }
}

fn tag_name(name: &str) -> Result<&'static str> {
    match name {
        "llsd" => Ok("llsd"),
        "array" => Ok("array"),
        "map" => Ok("map"),
        "key" => Ok("key"),
        "undef" => Ok("undef"),
        "boolean" => Ok("boolean"),
        "integer" => Ok("integer"),
        "real" => Ok("real"),
        "uuid" => Ok("uuid"),
        "string" => Ok("string"),
        "uri" => Ok("uri"),
        "date" => Ok("date"),
        "binary" => Ok("binary"),
        _ => Err(Error::InvalidValue("unknown LLSD XML element".into())),
    }
}

fn validate_binary_encoding(element: &BytesStart<'_>) -> Result<()> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| Error::InvalidValue(error.to_string()))?;
        if attribute.key.as_ref() != "encoding" {
            continue;
        }

        let encoding = attribute.normalized_value(XmlVersion::Implicit1_0).map_err(|error| Error::InvalidValue(error.to_string()))?;
        if encoding != "base64" {
            return Err(Error::InvalidValue(format!("unsupported LLSD binary encoding {encoding:?}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::borrow::Cow;

    use super::{Token, Tokens};
    use crate::Limits;

    #[test]
    fn unescaped_text_tokens_borrow_input() {
        let input = b"<llsd><string>ordinary text</string></llsd>";
        let mut stream = Tokens::new(input, Limits::default()).expect("read XML");
        let mut tokens = Vec::new();
        while let Some(token) = stream.take().expect("read token") {
            tokens.push(token);
        }
        let text = tokens.iter().find_map(|token| match token {
            Token::Text(text) => Some(text),
            _ => None,
        });

        assert_matches!(text, Some(Cow::Borrowed("ordinary text")));
    }
}
