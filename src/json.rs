use std::collections::HashMap;
use std::fmt;

fn write_string(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    f.write_str("\"")?;
    for character in value.chars() {
        match character {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\u{08}' => f.write_str("\\b")?,
            '\u{0c}' => f.write_str("\\f")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            character if character <= '\u{1f}' => write!(f, "\\u{:04x}", character as u32)?,
            character => write!(f, "{character}")?,
        }
    }
    f.write_str("\"")
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(HashMap<String, JsonValue>),
}

impl fmt::Display for JsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsonValue::Null => write!(f, "null"),
            JsonValue::Bool(b) => write!(f, "{b}"),
            JsonValue::Number(n) => write!(f, "{n}"),
            JsonValue::String(s) => write_string(f, s),
            JsonValue::Array(arr) => {
                write!(f, "[")?;
                for (i, v) in arr.iter().enumerate() {
                    if i > 0 { write!(f, ",")?; }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            JsonValue::Object(obj) => {
                write!(f, "{{")?;
                let mut first = true;
                for (k, v) in obj {
                    if !first { write!(f, ",")?; }
                    first = false;
                    write_string(f, k)?;
                    write!(f, ":{v}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

impl JsonValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn _as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            JsonValue::Number(n) => {
                if *n >= 0.0 && *n == n.floor() {
                    Some(*n as u64)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        match self {
            JsonValue::Array(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&HashMap<String, JsonValue>> {
        match self {
            JsonValue::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        match self {
            JsonValue::Object(m) => m.get(key),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct JsonError {
    pub message: String,
    pub pos: usize,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON error at position {}: {}", self.pos, self.message)
    }
}

pub struct Parser {
    input: Vec<u8>,
    pos: usize,
}

impl Parser {
    pub fn new(input: &str) -> Parser {
        Parser {
            input: input.as_bytes().to_vec(),
            pos: 0,
        }
    }

    pub fn parse(&mut self) -> Result<JsonValue, JsonError> {
        self.skip_whitespace();
        if self.pos >= self.input.len() {
            return Err(self.err("unexpected end of input"));
        }
        let val = self.parse_value()?;
        self.skip_whitespace();
        Ok(val)
    }

    fn parse_value(&mut self) -> Result<JsonValue, JsonError> {
        self.skip_whitespace();
        if self.pos >= self.input.len() {
            return Err(self.err("unexpected end of input"));
        }
        match self.input[self.pos] {
            b'"' => self.parse_string().map(JsonValue::String),
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b't' | b'f' => self.parse_bool(),
            b'n' => self.parse_null(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            c => Err(self.err(&format!("unexpected character: '{}'", c as char))),
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        if self.pos >= self.input.len() || self.input[self.pos] != b'"' {
            return Err(self.err("expected '\"'"));
        }
        self.pos += 1;
        let mut s = String::new();
        loop {
            if self.pos >= self.input.len() {
                return Err(self.err("unterminated string"));
            }
            let c = self.input[self.pos];
            self.pos += 1;
            match c {
                b'"' => return Ok(s),
                b'\\' => {
                    if self.pos >= self.input.len() {
                        return Err(self.err("unterminated escape in string"));
                    }
                    match self.input[self.pos] {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{08}'),
                        b'f' => s.push('\u{0c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            if self.pos + 4 >= self.input.len() {
                                return Err(self.err("unterminated \\u escape"));
                            }
                            let hex_str = &self.input[self.pos + 1..self.pos + 5];
                            let hex_str = std::str::from_utf8(hex_str).map_err(|_| self.err("invalid \\u escape"))?;
                            let code = u32::from_str_radix(hex_str, 16).map_err(|_| self.err("invalid \\u hex"))?;
                            if let Some(ch) = char::from_u32(code) {
                                s.push(ch);
                            }
                            self.pos += 4;
                        }
                        _ => return Err(self.err("invalid escape character")),
                    }
                    self.pos += 1;
                }
                _ if c & 0x80 == 0 => s.push(c as char),
                _ => {
                    let n = if c & 0xE0 == 0xC0 { 2 }
                        else if c & 0xF0 == 0xE0 { 3 }
                        else if c & 0xF8 == 0xF0 { 4 }
                        else { return Err(self.err("invalid UTF-8 start byte")); };
                    if self.pos + n > self.input.len() + 1 {
                        return Err(self.err("truncated UTF-8 sequence"));
                    }
                    let mut code = (c & (0x7F >> n)) as u32;
                    for _ in 1..n {
                        let b = self.input[self.pos];
                        if b & 0xC0 != 0x80 {
                            return Err(self.err("invalid continuation byte"));
                        }
                        code = (code << 6) | (b & 0x3F) as u32;
                        self.pos += 1;
                    }
                    if let Some(ch) = char::from_u32(code) {
                        s.push(ch);
                    } else {
                        return Err(self.err("invalid Unicode code point"));
                    }
                }
            }
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, JsonError> {
        let start = self.pos;
        if self.pos < self.input.len() && self.input[self.pos] == b'-' {
            self.pos += 1;
        }
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos < self.input.len() && self.input[self.pos] == b'.' {
            self.pos += 1;
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.input.len() && (self.input[self.pos] == b'e' || self.input[self.pos] == b'E') {
            self.pos += 1;
            if self.pos < self.input.len() && (self.input[self.pos] == b'+' || self.input[self.pos] == b'-') {
                self.pos += 1;
            }
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let num_str = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| self.err("invalid number"))?;
        let n: f64 = num_str.parse().map_err(|_| self.err("invalid number"))?;
        Ok(JsonValue::Number(n))
    }

    fn parse_bool(&mut self) -> Result<JsonValue, JsonError> {
        if self.pos + 4 <= self.input.len() && &self.input[self.pos..self.pos + 4] == b"true" {
            self.pos += 4;
            Ok(JsonValue::Bool(true))
        } else if self.pos + 5 <= self.input.len() && &self.input[self.pos..self.pos + 5] == b"false" {
            self.pos += 5;
            Ok(JsonValue::Bool(false))
        } else {
            Err(self.err("expected 'true' or 'false'"))
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, JsonError> {
        if self.pos + 4 <= self.input.len() && &self.input[self.pos..self.pos + 4] == b"null" {
            self.pos += 4;
            Ok(JsonValue::Null)
        } else {
            Err(self.err("expected 'null'"))
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, JsonError> {
        self.pos += 1;
        let mut obj = HashMap::new();
        self.skip_whitespace();
        if self.pos < self.input.len() && self.input[self.pos] == b'}' {
            self.pos += 1;
            return Ok(JsonValue::Object(obj));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.pos >= self.input.len() || self.input[self.pos] != b':' {
                return Err(self.err("expected ':'"));
            }
            self.pos += 1;
            let val = self.parse_value()?;
            obj.insert(key, val);
            self.skip_whitespace();
            if self.pos >= self.input.len() {
                return Err(self.err("unterminated object"));
            }
            if self.input[self.pos] == b'}' {
                self.pos += 1;
                return Ok(JsonValue::Object(obj));
            }
            if self.input[self.pos] != b',' {
                return Err(self.err("expected ',' or '}'"));
            }
            self.pos += 1;
        }
    }

    fn parse_array(&mut self) -> Result<JsonValue, JsonError> {
        self.pos += 1;
        let mut arr = Vec::new();
        self.skip_whitespace();
        if self.pos < self.input.len() && self.input[self.pos] == b']' {
            self.pos += 1;
            return Ok(JsonValue::Array(arr));
        }
        loop {
            let val = self.parse_value()?;
            arr.push(val);
            self.skip_whitespace();
            if self.pos >= self.input.len() {
                return Err(self.err("unterminated array"));
            }
            if self.input[self.pos] == b']' {
                self.pos += 1;
                return Ok(JsonValue::Array(arr));
            }
            if self.input[self.pos] != b',' {
                return Err(self.err("expected ',' or ']'"));
            }
            self.pos += 1;
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn err(&self, msg: &str) -> JsonError {
        JsonError {
            message: msg.to_string(),
            pos: self.pos,
        }
    }
}

pub fn parse(input: &str) -> Result<JsonValue, JsonError> {
    Parser::new(input).parse()
}

pub fn serialize(val: &JsonValue) -> String {
    val.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_null() {
        let v = parse("null").unwrap();
        assert_eq!(v, JsonValue::Null);
    }

    #[test]
    fn test_parse_bool() {
        assert_eq!(parse("true").unwrap(), JsonValue::Bool(true));
        assert_eq!(parse("false").unwrap(), JsonValue::Bool(false));
    }

    #[test]
    fn test_parse_number() {
        assert_eq!(parse("42").unwrap(), JsonValue::Number(42.0));
        assert_eq!(parse("3.14").unwrap(), JsonValue::Number(3.14));
        assert_eq!(parse("-1").unwrap(), JsonValue::Number(-1.0));
    }

    #[test]
    fn test_parse_string() {
        assert_eq!(parse("\"hello\"").unwrap(), JsonValue::String("hello".into()));
        assert_eq!(parse("\"\"").unwrap(), JsonValue::String("".into()));
    }

    #[test]
    fn test_parse_escaped_string() {
        let v = parse("\"hello\\nworld\"").unwrap();
        assert_eq!(v, JsonValue::String("hello\nworld".into()));
    }

    #[test]
    fn test_parse_array() {
        let v = parse("[1,2,3]").unwrap();
        assert_eq!(v, JsonValue::Array(vec![
            JsonValue::Number(1.0),
            JsonValue::Number(2.0),
            JsonValue::Number(3.0),
        ]));
    }

    #[test]
    fn test_parse_empty_array() {
        assert_eq!(parse("[]").unwrap(), JsonValue::Array(vec![]));
    }

    #[test]
    fn test_parse_nested_array() {
        let v = parse("[[1,2],[3,4]]").unwrap();
        assert_eq!(v, JsonValue::Array(vec![
            JsonValue::Array(vec![JsonValue::Number(1.0), JsonValue::Number(2.0)]),
            JsonValue::Array(vec![JsonValue::Number(3.0), JsonValue::Number(4.0)]),
        ]));
    }

    #[test]
    fn test_parse_object() {
        let v = parse("{\"a\":1,\"b\":\"two\"}").unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("a"), Some(&JsonValue::Number(1.0)));
        assert_eq!(obj.get("b"), Some(&JsonValue::String("two".into())));
    }

    #[test]
    fn test_parse_empty_object() {
        assert_eq!(parse("{}").unwrap(), JsonValue::Object(HashMap::new()));
    }

    #[test]
    fn test_parse_nested_object() {
        let v = parse("{\"outer\":{\"inner\":42}}").unwrap();
        let outer = v.get("outer").and_then(|v| v.as_object()).unwrap();
        assert_eq!(outer.get("inner"), Some(&JsonValue::Number(42.0)));
    }

    #[test]
    fn test_whitespace_tolerance() {
        let v = parse("  {  \"a\"  :  1  }  ").unwrap();
        assert_eq!(v.get("a"), Some(&JsonValue::Number(1.0)));
    }

    #[test]
    fn test_unicode_string() {
        let v = parse("\"héllo 🎉\"").unwrap();
        assert_eq!(v, JsonValue::String("héllo 🎉".into()));
    }

    #[test]
    fn test_parse_error_unclosed() {
        assert!(parse("{").is_err());
    }

    #[test]
    fn test_parse_error_trailing_comma() {
        assert!(parse("[1,]").is_err());
    }

    #[test]
    fn test_roundtrip_null() {
        let v = JsonValue::Null;
        let s = serialize(&v);
        let back = parse(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_roundtrip_object() {
        let mut obj = HashMap::new();
        obj.insert("name".into(), JsonValue::String("test".into()));
        obj.insert("count".into(), JsonValue::Number(42.0));
        obj.insert("active".into(), JsonValue::Bool(true));
        obj.insert("data".into(), JsonValue::Null);
        let v = JsonValue::Object(obj);
        let s = serialize(&v);
        let back = parse(&s).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_roundtrip_escaped_object_keys_and_values() {
        let mut obj = HashMap::new();
        obj.insert(
            "key\"\\\n\t\u{0001}".into(),
            JsonValue::String("value\"\\\n\r\t\u{0008}\u{000c}\u{001f}".into()),
        );
        let value = JsonValue::Object(obj);

        let serialized = serialize(&value);
        let parsed = parse(&serialized).unwrap();

        assert_eq!(parsed, value);
    }

    #[test]
    fn test_as_str() {
        let v = JsonValue::String("hello".into());
        assert_eq!(v.as_str(), Some("hello"));
        assert_eq!(JsonValue::Null.as_str(), None);
    }

    #[test]
    fn test_as_u64() {
        assert_eq!(JsonValue::Number(42.0).as_u64(), Some(42));
        assert_eq!(JsonValue::Number(3.14).as_u64(), None);
    }

    #[test]
    fn test_as_array() {
        let v = JsonValue::Array(vec![JsonValue::Number(1.0)]);
        assert!(v.as_array().is_some());
        assert!(JsonValue::Null.as_array().is_none());
    }

    #[test]
    fn test_as_object() {
        let v = JsonValue::Object(HashMap::new());
        assert!(v.as_object().is_some());
        assert!(JsonValue::Null.as_object().is_none());
    }

    #[test]
    fn test_get_on_object() {
        let mut obj = HashMap::new();
        obj.insert("key".into(), JsonValue::String("val".into()));
        let v = JsonValue::Object(obj);
        assert_eq!(v.get("key").and_then(|v| v.as_str()), Some("val"));
        assert_eq!(v.get("missing"), None);
    }

    #[test]
    fn test_get_on_non_object_returns_none() {
        assert_eq!(JsonValue::Null.get("key"), None);
    }

    #[test]
    fn test_deeply_nested() {
        let input = r#"{"a":{"b":{"c":{"d":[1,2,3]}}}}"#;
        let v = parse(input).unwrap();
        let d = v.get("a").and_then(|v| v.get("b")).and_then(|v| v.get("c")).and_then(|v| v.get("d"));
        assert!(d.is_some());
        let arr = d.unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn test_todo_tool_schema() {
        let schema = r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"content":{"type":"string"},"status":{"type":"string"},"priority":{"type":"string"}}}}},"required":["todos"]}"#;
        match parse(schema) {
            Ok(v) => {
                let obj = v.as_object().unwrap();
                assert!(obj.get("type").is_some());
                assert!(obj.get("properties").is_some());
                assert!(obj.get("required").is_some());
            }
            Err(e) => panic!("Todo schema invalid: {e}"),
        }
    }

    #[test]
    fn test_todo_tool_in_object() {
        let schema = r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"content":{"type":"string"},"status":{"type":"string"},"priority":{"type":"string"}}}}},"required":["todos"]}"#;
        let wrapped = format!(
            r#"{{"type":"function","function":{{"name":"todo_write","description":"Manage a task/todo list","parameters":{schema}}}}}"#
        );
        match parse(&wrapped) {
            Ok(v) => {
                let obj = v.as_object().unwrap();
                assert_eq!(obj.get("type").and_then(|v| v.as_str()), Some("function"));
            }
            Err(e) => panic!("Wrapped todo tool invalid: {e}"),
        }
    }
}
