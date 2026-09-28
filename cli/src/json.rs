//! A tiny, dependency-free JSON value + parser/serializer, just enough for the
//! Language Server Protocol subset `pwe lsp` speaks. Strings are fully unescaped
//! (PWE source lives inside a JSON string), which a string-scanning shortcut
//! could not do safely.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 9.0e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Json::Str(s) => {
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
            Json::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    Json::Str(k.clone()).write(out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
}

pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = Parser {
        b: s.as_bytes(),
        pos: 0,
    };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.pos != p.b.len() {
        return Err("trailing data".to_string());
    }
    Ok(v)
}

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.b.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }
    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.b.get(self.pos) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(_) => self.number(),
            None => Err("unexpected end".to_string()),
        }
    }
    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.b[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(v)
        } else {
            Err(format!("bad literal at {}", self.pos))
        }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        while matches!(
            self.b.get(self.pos),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.pos += 1;
        }
        std::str::from_utf8(&self.b[start..self.pos])
            .ok()
            .and_then(|t| t.parse::<f64>().ok())
            .map(Json::Num)
            .ok_or_else(|| format!("bad number at {start}"))
    }
    fn string(&mut self) -> Result<String, String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.b.get(self.pos) {
                None => return Err("unterminated string".to_string()),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.b.get(self.pos) {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => {
                            let hex = self
                                .b
                                .get(self.pos + 1..self.pos + 5)
                                .and_then(|h| std::str::from_utf8(h).ok())
                                .and_then(|h| u32::from_str_radix(h, 16).ok())
                                .ok_or("bad \\u escape")?;
                            out.push(char::from_u32(hex).unwrap_or('\u{fffd}'));
                            self.pos += 4;
                        }
                        _ => return Err("bad escape".to_string()),
                    }
                    self.pos += 1;
                }
                Some(_) => {
                    let rest =
                        std::str::from_utf8(&self.b[self.pos..]).map_err(|e| e.to_string())?;
                    let ch = rest.chars().next().ok_or("eof")?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }
    fn array(&mut self) -> Result<Json, String> {
        self.pos += 1;
        let mut a = Vec::new();
        self.ws();
        if self.b.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(Json::Arr(a));
        }
        loop {
            a.push(self.value()?);
            self.ws();
            match self.b.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(a));
                }
                _ => return Err("bad array".to_string()),
            }
        }
    }
    fn object(&mut self) -> Result<Json, String> {
        self.pos += 1;
        let mut m = Vec::new();
        self.ws();
        if self.b.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(Json::Obj(m));
        }
        loop {
            self.ws();
            if self.b.get(self.pos) != Some(&b'"') {
                return Err("expected key".to_string());
            }
            let k = self.string()?;
            self.ws();
            if self.b.get(self.pos) != Some(&b':') {
                return Err("expected :".to_string());
            }
            self.pos += 1;
            let v = self.value()?;
            m.push((k, v));
            self.ws();
            match self.b.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(m));
                }
                _ => return Err("bad object".to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_source_with_quotes_and_braces() {
        let src = "world { title = \"a{b\\\"c\" entity e { state=(x=1.0) } }";
        let msg = Json::Obj(vec![("text".into(), Json::Str(src.into()))]).to_json();
        let back = parse(&msg).unwrap();
        assert_eq!(back.get("text").unwrap().as_str().unwrap(), src);
    }

    #[test]
    fn parses_nested_and_rejects_trailing() {
        let j = parse(r#"{"id":1,"params":{"x":[true,null,2.5]}}"#).unwrap();
        assert_eq!(j.get("id").unwrap(), &Json::Num(1.0));
        assert_eq!(
            j.get("params")
                .unwrap()
                .get("x")
                .unwrap()
                .as_arr()
                .unwrap()
                .len(),
            3
        );
        assert!(parse("1 2").is_err());
    }
}
