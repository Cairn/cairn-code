use std::collections::HashMap;
use std::fmt;

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
            JsonValue::String(s) => write!(f, "\"{s}\""),
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
                    write!(f, "\"{k}\":{v}")?;
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
                _ => s.push(c as char),
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
