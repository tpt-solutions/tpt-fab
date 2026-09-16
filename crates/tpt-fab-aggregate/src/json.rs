//! Minimal JSON support, in-crate and dependency-free.
//!
//! This is the entire JSON stack the outcome-file exchange needs. Objects preserve insertion
//! order so serialization is deterministic — the property the signature scheme relies on for
//! canonical bytes. Zero external dependencies is a hard requirement for this crate (see the
//! crate docs): no serde, no network-capable anything.

use crate::error::AggregateError;
use std::fmt;

/// A JSON value. Objects keep insertion order (deterministic serialization).
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// A number (stored as f64; integers round-trip when integral).
    Num(f64),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object with insertion-ordered entries.
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Object entry lookup.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Convenience: string value.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Convenience: numeric value.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Convenience: array elements.
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }

    /// Serializes to a compact JSON string (deterministic: objects in insertion order).
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Json::Str(s) => write_escaped(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_escaped(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// Parses a JSON document.
    pub fn parse(text: &str) -> Result<Json, AggregateError> {
        let bytes: Vec<char> = text.chars().collect();
        let mut p = Parser { src: &bytes, pos: 0 };
        p.skip_ws();
        let v = p.value()?;
        p.skip_ws();
        if p.pos != p.src.len() {
            return Err(AggregateError::Json("trailing characters after value".into()));
        }
        Ok(v)
    }
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.serialize())
    }
}

fn write_escaped(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    src: &'a [char],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t') | Some('\n') | Some('\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, c: char) -> Result<(), AggregateError> {
        if self.bump() == Some(c) {
            Ok(())
        } else {
            Err(AggregateError::Json(format!("expected '{c}' at char {}", self.pos)))
        }
    }

    fn value(&mut self) -> Result<Json, AggregateError> {
        self.skip_ws();
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('t') => self.literal("true", Json::Bool(true)),
            Some('f') => self.literal("false", Json::Bool(false)),
            Some('n') => self.literal("null", Json::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            other => {
                Err(AggregateError::Json(format!("unexpected '{other:?}' at char {}", self.pos)))
            }
        }
    }

    fn literal(&mut self, text: &str, v: Json) -> Result<Json, AggregateError> {
        for expected in text.chars() {
            if self.bump() != Some(expected) {
                return Err(AggregateError::Json(format!("bad literal, expected {text}")));
            }
        }
        Ok(v)
    }

    fn number(&mut self) -> Result<Json, AggregateError> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.bump();
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        {
            self.bump();
        }
        let text: String = self.src[start..self.pos].iter().collect();
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| AggregateError::Json(format!("bad number '{text}'")))
    }

    fn string(&mut self) -> Result<String, AggregateError> {
        self.expect('"')?;
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err(AggregateError::Json("unterminated string".into())),
                Some('"') => return Ok(s),
                Some('\\') => match self.bump() {
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some('/') => s.push('/'),
                    Some('b') => s.push('\u{8}'),
                    Some('f') => s.push('\u{c}'),
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('t') => s.push('\t'),
                    Some('u') => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            let c = self
                                .bump()
                                .ok_or_else(|| AggregateError::Json("bad \\u".into()))?;
                            code = code * 16
                                + c.to_digit(16)
                                    .ok_or_else(|| AggregateError::Json("bad \\u digit".into()))?;
                        }
                        s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    other => return Err(AggregateError::Json(format!("bad escape {other:?}"))),
                },
                Some(c) => s.push(c),
            }
        }
    }

    fn array(&mut self) -> Result<Json, AggregateError> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some(']') => return Ok(Json::Arr(items)),
                _ => return Err(AggregateError::Json("expected ',' or ']'".into())),
            }
        }
    }

    fn object(&mut self) -> Result<Json, AggregateError> {
        self.expect('{')?;
        let mut entries: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Json::Obj(entries));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(':')?;
            let value = self.value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => return Ok(Json::Obj(entries)),
                _ => return Err(AggregateError::Json("expected ',' or '}'".into())),
            }
        }
    }
}

/// Builds a [`Json::Obj`] programmatically.
#[macro_export]
macro_rules! jobj {
    ($($k:expr => $v:expr),* $(,)?) => {
        $crate::json::Json::Obj(vec![$(($k.to_string(), $v)),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_nested() {
        let src = r#"{"a":1,"b":[true,false,null],"c":{"d":"x\ny"},"e":-2.5,"f":3}"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("a").unwrap().as_f64(), Some(1.0));
        assert_eq!(v.get("f").unwrap().as_f64(), Some(3.0));
        let out = v.serialize();
        let v2 = Json::parse(&out).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    fn serialization_is_deterministic_insertion_ordered() {
        let a = jobj! {"x" => Json::Num(1.0), "y" => Json::Num(2.0)};
        let b = jobj! {"y" => Json::Num(2.0), "x" => Json::Num(1.0)};
        assert_ne!(a.serialize(), b.serialize(), "insertion order must be preserved");
        assert_eq!(a.serialize(), a.serialize());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Json::parse("{").is_err());
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("hello").is_err());
        assert!(Json::parse("{}extra").is_err());
    }

    #[test]
    fn unicode_escape() {
        let v = Json::parse(r#""A""#).unwrap();
        assert_eq!(v, Json::Str("A".into()));
    }
}
