// Small, dependency-free JSON writer. Raw is used only for our own SDK/build JSON.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    Raw(String),
}

impl Json {
    pub fn object<const N: usize>(items: [(&str, Json); N]) -> Self {
        Self::Object(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn encode(&self) -> String {
        let mut output = String::new();
        self.append(&mut output);
        output
    }

    fn append(&self, output: &mut String) {
        match self {
            Self::Null => output.push_str("null"),
            Self::Bool(b) => output.push_str(if *b { "true" } else { "false" }),
            Self::Number(n) => output.push_str(&n.to_string()),
            Self::String(s) => quoted(s, output),
            Self::Raw(s) => output.push_str(s),
            Self::Array(values) => {
                output.push('[');
                for (i, value) in values.iter().enumerate() {
                    if i != 0 {
                        output.push(',');
                    }
                    value.append(output);
                }
                output.push(']');
            }
            Self::Object(values) => {
                output.push('{');
                for (i, (key, value)) in values.iter().enumerate() {
                    if i != 0 {
                        output.push(',');
                    }
                    quoted(key, output);
                    output.push(':');
                    value.append(output);
                }
                output.push('}');
            }
        }
    }
}

fn quoted(s: &str, output: &mut String) {
    use std::fmt::Write;
    output.push('"');
    for c in s.chars() {
        match c {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            c if c < '\u{20}' => {
                let _ = write!(output, "\\u{:04x}", c as u32);
            }
            c => output.push(c),
        }
    }
    output.push('"');
}

impl From<&str> for Json {
    fn from(v: &str) -> Self {
        Self::String(v.to_string())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}
impl From<bool> for Json {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<u32> for Json {
    fn from(v: u32) -> Self {
        Self::Number(v as i64)
    }
}
impl From<usize> for Json {
    fn from(v: usize) -> Self {
        Self::Number(v as i64)
    }
}
impl From<i32> for Json {
    fn from(v: i32) -> Self {
        Self::Number(v as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_control_characters() {
        assert_eq!(
            Json::from("a\n\t\"\\\0").encode(),
            "\"a\\n\\t\\\"\\\\\\u0000\""
        );
    }
}
