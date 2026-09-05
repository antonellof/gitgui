//! Small syntax highlighter for the built-in editor. No grammar files, no
//! dependencies: comments, strings, numbers and keywords per language family,
//! picked from the file extension. Good enough to read code; not a linter.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

use crate::ui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Plain,
    Rust,
    C,
    Go,
    Java,
    Swift,
    Python,
    Ruby,
    Js,
    Shell,
    Toml,
    Yaml,
    Json,
    Markdown,
    Css,
    Html,
    Sql,
    Lua,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Punct,
    Heading,
    Attr,
}

/// A language description the tokenizer runs on.
struct Rules {
    line_comment: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// String delimiters; `'` is a char literal in C-like languages, treated
    /// as a string anyway.
    strings: &'static [char],
    keywords: &'static [&'static str],
    /// Words treated as types / builtins (second color).
    types: &'static [&'static str],
    /// Identifiers starting with an upper-case letter are types.
    upper_is_type: bool,
    /// `#` starts a line comment except in `#!`, `#[...]` (rust attr) and `#include`.
    hash_attr: bool,
}

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self",
    "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "yield",
];
const RUST_TY: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "bool",
    "char", "str", "String", "Vec", "Option", "Result", "Box", "Arc", "Rc", "Some", "None", "Ok", "Err",
];
const C_KW: &[&str] = &[
    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if",
    "inline", "register", "return", "sizeof", "static", "struct", "switch", "typedef", "union", "volatile", "while",
    "class", "namespace", "template", "typename", "public", "private", "protected", "virtual", "override", "new",
    "delete", "this", "true", "false", "nullptr", "using", "try", "catch", "throw", "constexpr", "operator",
];
const C_TY: &[&str] = &[
    "int", "char", "short", "long", "float", "double", "void", "unsigned", "signed", "bool", "size_t", "uint8_t",
    "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t", "NULL",
];
const GO_KW: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go",
    "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type",
    "var", "true", "false", "nil",
];
const GO_TY: &[&str] = &[
    "bool", "byte", "error", "float32", "float64", "int", "int8", "int16", "int32", "int64", "rune", "string",
    "uint", "uint8", "uint16", "uint32", "uint64", "uintptr", "any",
];
const JAVA_KW: &[&str] = &[
    "abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum",
    "extends", "final", "finally", "for", "if", "implements", "import", "instanceof", "interface", "native", "new",
    "package", "private", "protected", "public", "return", "static", "super", "switch", "synchronized", "this",
    "throw", "throws", "transient", "try", "volatile", "while", "true", "false", "null", "var", "val", "fun",
    "when", "object", "data", "sealed", "override", "is", "in", "as",
];
const JAVA_TY: &[&str] = &["int", "long", "short", "byte", "char", "float", "double", "boolean", "void"];
const SWIFT_KW: &[&str] = &[
    "associatedtype", "class", "deinit", "enum", "extension", "fileprivate", "func", "import", "init", "inout",
    "internal", "let", "open", "operator", "private", "protocol", "public", "static", "struct", "subscript",
    "typealias", "var", "break", "case", "continue", "default", "defer", "do", "else", "fallthrough", "for",
    "guard", "if", "in", "repeat", "return", "switch", "where", "while", "as", "catch", "is", "nil", "rethrows",
    "super", "self", "Self", "throw", "throws", "true", "false", "try", "some", "any", "async", "await", "actor",
];
const PY_KW: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
    "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
    "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "self", "match", "case",
];
const PY_TY: &[&str] = &[
    "int", "str", "float", "bool", "list", "dict", "set", "tuple", "bytes", "object", "print", "len", "range",
    "super", "isinstance", "type",
];
const RUBY_KW: &[&str] = &[
    "alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else", "elsif", "end", "ensure",
    "false", "for", "if", "in", "module", "next", "nil", "not", "or", "redo", "rescue", "retry", "return", "self",
    "super", "then", "true", "undef", "unless", "until", "when", "while", "yield", "require", "attr_accessor",
    "attr_reader", "private", "public", "puts",
];
const JS_KW: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete",
    "do", "else", "enum", "export", "extends", "false", "finally", "for", "function", "if", "implements", "import",
    "in", "instanceof", "interface", "let", "new", "null", "of", "package", "private", "protected", "public",
    "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void",
    "while", "with", "yield", "type", "declare", "readonly", "as", "satisfies", "keyof", "from",
];
const JS_TY: &[&str] = &[
    "string", "number", "boolean", "any", "unknown", "never", "object", "Promise", "Array", "Map", "Set",
    "console", "window", "document", "Math", "JSON",
];
const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function",
    "return", "exit", "local", "export", "set", "unset", "readonly", "shift", "source", "alias", "echo", "cd",
    "test", "true", "false", "select", "declare", "eval", "exec", "trap",
];
const SQL_KW: &[&str] = &[
    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop",
    "alter", "add", "column", "index", "primary", "key", "foreign", "references", "join", "left", "right",
    "inner", "outer", "on", "group", "by", "order", "having", "limit", "offset", "as", "and", "or", "not", "null",
    "is", "in", "like", "between", "union", "all", "distinct", "case", "when", "then", "else", "end", "begin",
    "commit", "rollback", "transaction", "view", "with", "exists", "default", "unique", "constraint", "if",
];
const SQL_TY: &[&str] = &[
    "int", "integer", "bigint", "smallint", "text", "varchar", "char", "boolean", "date", "timestamp", "float",
    "real", "double", "decimal", "numeric", "blob", "serial", "uuid", "json", "jsonb",
];
const LUA_KW: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil",
    "not", "or", "repeat", "return", "then", "true", "until", "while", "self",
];
const LUA_TY: &[&str] = &["print", "pairs", "ipairs", "type", "tostring", "tonumber", "require", "table", "string", "math"];
const CSS_KW: &[&str] = &["important", "media", "import", "keyframes", "font-face", "supports", "root"];
const TOML_KW: &[&str] = &["true", "false"];
const YAML_KW: &[&str] = &["true", "false", "null", "yes", "no", "on", "off", "~"];

impl Lang {
    pub fn from_path(path: &str) -> Lang {
        let name = path.rsplit('/').next().unwrap_or(path);
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "makefile" | "dockerfile" | ".gitignore" | ".gitattributes" | ".env" => return Lang::Shell,
            "cargo.lock" => return Lang::Toml,
            _ => {}
        }
        let ext = lower.rsplit('.').next().unwrap_or("");
        match ext {
            "rs" => Lang::Rust,
            "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "m" | "mm" | "cs" => Lang::C,
            "go" => Lang::Go,
            "java" | "kt" | "kts" | "scala" | "groovy" => Lang::Java,
            "swift" => Lang::Swift,
            "py" | "pyi" | "pyw" => Lang::Python,
            "rb" | "rake" | "gemspec" => Lang::Ruby,
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" => Lang::Js,
            "sh" | "bash" | "zsh" | "fish" | "bashrc" | "zshrc" | "profile" | "env" => Lang::Shell,
            "toml" | "ini" | "cfg" | "conf" | "properties" | "gitconfig" => Lang::Toml,
            "yml" | "yaml" => Lang::Yaml,
            "json" | "jsonc" | "json5" | "lock" => Lang::Json,
            "md" | "markdown" | "txt" | "rst" => Lang::Markdown,
            "css" | "scss" | "sass" | "less" => Lang::Css,
            "html" | "htm" | "xml" | "svg" | "xhtml" | "plist" => Lang::Html,
            "sql" | "psql" | "mysql" => Lang::Sql,
            "lua" => Lang::Lua,
            _ => Lang::Plain,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Lang::Plain => "text",
            Lang::Rust => "rust",
            Lang::C => "c",
            Lang::Go => "go",
            Lang::Java => "java",
            Lang::Swift => "swift",
            Lang::Python => "python",
            Lang::Ruby => "ruby",
            Lang::Js => "javascript",
            Lang::Shell => "shell",
            Lang::Toml => "toml",
            Lang::Yaml => "yaml",
            Lang::Json => "json",
            Lang::Markdown => "markdown",
            Lang::Css => "css",
            Lang::Html => "html",
            Lang::Sql => "sql",
            Lang::Lua => "lua",
        }
    }

    fn rules(self) -> Rules {
        let c_like = |kw, ty, upper| Rules {
            line_comment: &["//"],
            block_comment: Some(("/*", "*/")),
            strings: &['"', '\'', '`'],
            keywords: kw,
            types: ty,
            upper_is_type: upper,
            hash_attr: false,
        };
        match self {
            Lang::Rust => Rules {
                line_comment: &["//"],
                block_comment: Some(("/*", "*/")),
                strings: &['"'],
                keywords: RUST_KW,
                types: RUST_TY,
                upper_is_type: true,
                hash_attr: true,
            },
            Lang::C => Rules {
                hash_attr: true,
                ..c_like(C_KW, C_TY, false)
            },
            Lang::Go => c_like(GO_KW, GO_TY, false),
            Lang::Java => c_like(JAVA_KW, JAVA_TY, true),
            Lang::Swift => c_like(SWIFT_KW, &[], true),
            Lang::Js => c_like(JS_KW, JS_TY, true),
            Lang::Css => Rules {
                line_comment: &[],
                block_comment: Some(("/*", "*/")),
                strings: &['"', '\''],
                keywords: CSS_KW,
                types: &[],
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Python => Rules {
                line_comment: &["#"],
                block_comment: Some(("\"\"\"", "\"\"\"")),
                strings: &['"', '\''],
                keywords: PY_KW,
                types: PY_TY,
                upper_is_type: true,
                hash_attr: false,
            },
            Lang::Ruby => Rules {
                line_comment: &["#"],
                block_comment: None,
                strings: &['"', '\'', '`'],
                keywords: RUBY_KW,
                types: &[],
                upper_is_type: true,
                hash_attr: false,
            },
            Lang::Shell => Rules {
                line_comment: &["#"],
                block_comment: None,
                strings: &['"', '\'', '`'],
                keywords: SH_KW,
                types: &[],
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Toml => Rules {
                line_comment: &["#", ";"],
                block_comment: None,
                strings: &['"', '\''],
                keywords: TOML_KW,
                types: &[],
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Yaml => Rules {
                line_comment: &["#"],
                block_comment: None,
                strings: &['"', '\''],
                keywords: YAML_KW,
                types: &[],
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Json => Rules {
                line_comment: &["//"],
                block_comment: Some(("/*", "*/")),
                strings: &['"'],
                keywords: &["true", "false", "null"],
                types: &[],
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Sql => Rules {
                line_comment: &["--"],
                block_comment: Some(("/*", "*/")),
                strings: &['\'', '"'],
                keywords: SQL_KW,
                types: SQL_TY,
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Lua => Rules {
                line_comment: &["--"],
                block_comment: Some(("--[[", "]]")),
                strings: &['"', '\''],
                keywords: LUA_KW,
                types: LUA_TY,
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Html => Rules {
                line_comment: &[],
                block_comment: Some(("<!--", "-->")),
                strings: &['"', '\''],
                keywords: &[],
                types: &[],
                upper_is_type: false,
                hash_attr: false,
            },
            Lang::Markdown | Lang::Plain => Rules {
                line_comment: &[],
                block_comment: None,
                strings: &[],
                keywords: &[],
                types: &[],
                upper_is_type: false,
                hash_attr: false,
            },
        }
    }
}

/// Colors for each token kind, derived from the theme.
pub struct Palette {
    pub plain: Color32,
    pub comment: Color32,
    pub string: Color32,
    pub number: Color32,
    pub keyword: Color32,
    pub ty: Color32,
    pub punct: Color32,
    pub heading: Color32,
    pub attr: Color32,
}

impl Palette {
    pub fn from_theme(theme: &Theme, plain: Color32) -> Self {
        if theme.dark {
            Palette {
                plain,
                comment: theme.line_no,
                string: theme.graph[1],
                number: theme.graph[6],
                keyword: theme.graph[4],
                ty: theme.graph[2],
                punct: Color32::from_rgb(0x9d, 0xa3, 0xb5),
                heading: theme.graph[0],
                attr: theme.graph[7],
            }
        } else {
            Palette {
                plain,
                comment: theme.line_no,
                string: theme.graph[1],
                number: theme.graph[6],
                keyword: theme.graph[4],
                ty: theme.graph[2],
                punct: Color32::from_rgb(0x5c, 0x5f, 0x77),
                heading: theme.graph[0],
                attr: theme.graph[7],
            }
        }
    }

    fn color(&self, kind: Kind) -> Color32 {
        match kind {
            Kind::Plain => self.plain,
            Kind::Comment => self.comment,
            Kind::String => self.string,
            Kind::Number => self.number,
            Kind::Keyword => self.keyword,
            Kind::Type => self.ty,
            Kind::Punct => self.punct,
            Kind::Heading => self.heading,
            Kind::Attr => self.attr,
        }
    }
}

/// Tokenize `text` into `(byte range end, kind)` spans, contiguous from 0.
pub fn tokenize(text: &str, lang: Lang) -> Vec<(usize, Kind)> {
    if matches!(lang, Lang::Markdown) {
        return tokenize_markdown(text);
    }
    if matches!(lang, Lang::Html) {
        return tokenize_html(text);
    }
    let rules = lang.rules();
    let bytes = text.as_bytes();
    let mut out: Vec<(usize, Kind)> = Vec::new();
    let mut i = 0;
    let push = |end: usize, kind: Kind, out: &mut Vec<(usize, Kind)>| {
        if let Some(last) = out.last_mut() {
            if last.1 == kind {
                last.0 = end;
                return;
            }
        }
        out.push((end, kind));
    };
    let is_ident_start = |b: u8| b.is_ascii_alphabetic() || b == b'_';
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let at_line_start = |i: usize| i == 0 || bytes[i - 1] == b'\n';
    while i < bytes.len() {
        let rest = &text[i..];
        // Block comment.
        if let Some((open, close)) = rules.block_comment {
            if let Some(body) = rest.strip_prefix(open) {
                let end = body
                    .find(close)
                    .map(|p| i + open.len() + p + close.len())
                    .unwrap_or(bytes.len());
                push(end, Kind::Comment, &mut out);
                i = end;
                continue;
            }
        }
        let b = bytes[i];
        // Shebang on the first line.
        if i == 0 && rest.starts_with("#!") {
            let end = rest.find('\n').unwrap_or(bytes.len());
            push(end, Kind::Comment, &mut out);
            i = end;
            continue;
        }
        // Rust attributes and C preprocessor lines.
        if rules.hash_attr && b == b'#' {
            let bracket = rest.starts_with("#[") || rest.starts_with("#![");
            let directive = at_line_start(i)
                && rest[1..]
                    .trim_start_matches([' ', '\t'])
                    .starts_with(|c: char| c.is_ascii_alphabetic());
            if bracket {
                let mut depth = 0usize;
                let mut end = bytes.len();
                for (k, c) in rest.bytes().enumerate() {
                    match c {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i + k + 1;
                                break;
                            }
                        }
                        b'\n' => {
                            end = i + k;
                            break;
                        }
                        _ => {}
                    }
                }
                push(end, Kind::Attr, &mut out);
                i = end;
                continue;
            }
            if directive {
                let end = rest.find('\n').map(|p| i + p).unwrap_or(bytes.len());
                push(end, Kind::Attr, &mut out);
                i = end;
                continue;
            }
        }
        // Line comment.
        if rules.line_comment.iter().any(|p| rest.starts_with(*p)) {
            let end = rest.find('\n').map(|p| i + p).unwrap_or(bytes.len());
            push(end, Kind::Comment, &mut out);
            i = end;
            continue;
        }
        // Strings, with backslash escapes; stop at end of line for the
        // single-quote kinds so an apostrophe in prose does not eat the file.
        if rules.strings.contains(&(b as char)) {
            let quote = b;
            let mut j = i + 1;
            let mut closed = false;
            while j < bytes.len() {
                let c = bytes[j];
                if c == b'\\' {
                    j += 2;
                    continue;
                }
                if c == quote {
                    j += 1;
                    closed = true;
                    break;
                }
                if c == b'\n' && quote != b'`' {
                    break;
                }
                j += 1;
            }
            let end = j.min(bytes.len());
            if closed || quote != b'\'' {
                push(end, Kind::String, &mut out);
                i = end;
                continue;
            }
            push(i + 1, Kind::Punct, &mut out);
            i += 1;
            continue;
        }
        // Numbers.
        if b.is_ascii_digit() && (i == 0 || !is_ident(bytes[i - 1])) {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || bytes[j] == b'_') {
                j += 1;
            }
            push(j, Kind::Number, &mut out);
            i = j;
            continue;
        }
        // Identifiers and keywords.
        if is_ident_start(b) {
            let mut j = i + 1;
            while j < bytes.len() && (is_ident(bytes[j]) || (bytes[j] == b'?' && lang == Lang::Ruby)) {
                j += 1;
            }
            // CSS: `word-with-dashes`.
            if lang == Lang::Css {
                while j < bytes.len() && (is_ident(bytes[j]) || bytes[j] == b'-') {
                    j += 1;
                }
            }
            let word = &text[i..j];
            let kind = if rules.keywords.iter().any(|k| word_eq(k, word, lang)) {
                Kind::Keyword
            } else if rules.types.contains(&word)
                || (rules.upper_is_type && word.starts_with(|c: char| c.is_ascii_uppercase()))
            {
                Kind::Type
            } else if matches!(lang, Lang::Toml | Lang::Yaml) && is_key_position(text, i, j) {
                Kind::Attr
            } else {
                Kind::Plain
            };
            push(j, kind, &mut out);
            i = j;
            continue;
        }
        // Everything else: punctuation, whitespace, non-ASCII text.
        let ch_len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        let kind = if b.is_ascii_punctuation() { Kind::Punct } else { Kind::Plain };
        push(i + ch_len, kind, &mut out);
        i += ch_len;
    }
    out
}

fn word_eq(keyword: &str, word: &str, lang: Lang) -> bool {
    if lang == Lang::Sql {
        keyword.eq_ignore_ascii_case(word)
    } else {
        keyword == word
    }
}

/// TOML / YAML: an identifier followed by `=` or `:` at the start of a line
/// is a key.
fn is_key_position(text: &str, start: usize, end: usize) -> bool {
    let line_start = text[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    if !text[line_start..start].trim().is_empty() && !text[line_start..start].trim_start().starts_with('-') {
        return false;
    }
    let after = text[end..].trim_start_matches([' ', '\t']);
    after.starts_with('=') || after.starts_with(':')
}

fn tokenize_markdown(text: &str) -> Vec<(usize, Kind)> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let end = pos + line.len();
        let trimmed = line.trim_start();
        let kind = if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            Kind::String
        } else if in_fence {
            Kind::String
        } else if trimmed.starts_with('#') {
            Kind::Heading
        } else if trimmed.starts_with('>') {
            Kind::Comment
        } else {
            Kind::Plain
        };
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            let indent = line.len() - trimmed.len();
            out.push((pos + indent + 1, Kind::Keyword));
            out.push((end, kind));
        } else if !line.is_empty() {
            out.push((end, kind));
        }
        pos = end;
    }
    out
}

fn tokenize_html(text: &str) -> Vec<(usize, Kind)> {
    let mut out: Vec<(usize, Kind)> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let push = |end: usize, kind: Kind, out: &mut Vec<(usize, Kind)>| {
        if let Some(last) = out.last_mut() {
            if last.1 == kind {
                last.0 = end;
                return;
            }
        }
        out.push((end, kind));
    };
    while i < bytes.len() {
        let rest = &text[i..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map(|p| i + p + 3).unwrap_or(bytes.len());
            push(end, Kind::Comment, &mut out);
            i = end;
            continue;
        }
        if bytes[i] == b'<' {
            // Tag: name is a keyword, attributes are attr, values are strings.
            let end = rest.find('>').map(|p| i + p + 1).unwrap_or(bytes.len());
            let tag = &text[i..end];
            let mut j = 0;
            let name_end = tag[1..]
                .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .map(|p| p + 1)
                .unwrap_or(tag.len());
            push(i + name_end, Kind::Keyword, &mut out);
            j = j.max(name_end);
            let tb = tag.as_bytes();
            while j < tag.len() {
                let c = tb[j];
                if c == b'"' || c == b'\'' {
                    let close = tag[j + 1..].find(c as char).map(|p| j + p + 2).unwrap_or(tag.len());
                    push(i + close, Kind::String, &mut out);
                    j = close;
                } else if c.is_ascii_alphabetic() {
                    let mut k = j;
                    while k < tag.len() && (tb[k].is_ascii_alphanumeric() || tb[k] == b'-' || tb[k] == b':') {
                        k += 1;
                    }
                    push(i + k, Kind::Attr, &mut out);
                    j = k;
                } else {
                    push(i + j + 1, Kind::Punct, &mut out);
                    j += 1;
                }
            }
            i = end;
            continue;
        }
        let ch_len = rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        push(i + ch_len, Kind::Plain, &mut out);
        i += ch_len;
    }
    out
}

/// Lay out `text` as a single monospace job with per-token colors.
pub fn layout_job(text: &str, lang: Lang, font: FontId, palette: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut start = 0;
    for (end, kind) in tokenize(text, lang) {
        if end <= start {
            continue;
        }
        job.append(
            &text[start..end],
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: palette.color(kind),
                ..Default::default()
            },
        );
        start = end;
    }
    if start < text.len() {
        job.append(
            &text[start..],
            0.0,
            TextFormat {
                font_id: font,
                color: palette.plain,
                ..Default::default()
            },
        );
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, lang: Lang) -> Vec<(&str, Kind)> {
        let mut out = Vec::new();
        let mut start = 0;
        for (end, kind) in tokenize(text, lang) {
            out.push((&text[start..end], kind));
            start = end;
        }
        out
    }

    #[test]
    fn rust_tokens() {
        let t = kinds("fn main() { let x = \"hi\"; // c\n}", Lang::Rust);
        assert_eq!(t[0], ("fn", Kind::Keyword));
        assert!(t.contains(&("\"hi\"", Kind::String)));
        assert!(t.contains(&("// c", Kind::Comment)));
        assert!(t.contains(&("let", Kind::Keyword)));
    }

    #[test]
    fn rust_attribute_and_lifetime_quote() {
        let t = kinds("#[derive(Debug)]\nfn f<'a>(x: &'a str) {}", Lang::Rust);
        assert_eq!(t[0], ("#[derive(Debug)]", Kind::Attr));
        // The `'a` quote never opens a string, so `str` is still a type.
        assert!(t.contains(&("str", Kind::Type)));
    }

    #[test]
    fn block_comment_spans_lines() {
        let t = kinds("a /* x\ny */ b", Lang::C);
        assert!(t.contains(&("/* x\ny */", Kind::Comment)));
    }

    #[test]
    fn python_hash_comment_and_numbers() {
        let t = kinds("x = 42 # note\n", Lang::Python);
        assert!(t.contains(&("42", Kind::Number)));
        assert!(t.contains(&("# note", Kind::Comment)));
    }

    #[test]
    fn toml_keys() {
        let t = kinds("[package]\nname = \"gitgui\"\n", Lang::Toml);
        assert!(t.contains(&("name", Kind::Attr)));
        assert!(t.contains(&("\"gitgui\"", Kind::String)));
    }

    #[test]
    fn markdown_headings_and_fences() {
        let t = kinds("# Title\ntext\n```\ncode\n```\n", Lang::Markdown);
        assert_eq!(t[0], ("# Title\n", Kind::Heading));
        assert!(t.iter().any(|(s, k)| *s == "code\n" && *k == Kind::String));
    }

    #[test]
    fn html_tags() {
        let t = kinds("<a href=\"x\">hi</a>", Lang::Html);
        assert_eq!(t[0], ("<a", Kind::Keyword));
        assert!(t.contains(&("href", Kind::Attr)));
        assert!(t.contains(&("\"x\"", Kind::String)));
    }

    #[test]
    fn spans_cover_whole_text() {
        for (text, lang) in [
            ("héllo 'wörld' # ü\n", Lang::Python),
            ("<p>é</p>", Lang::Html),
            ("# é\n- x\n", Lang::Markdown),
            ("", Lang::Rust),
        ] {
            let spans = tokenize(text, lang);
            let end = spans.last().map(|s| s.0).unwrap_or(0);
            assert_eq!(end, text.len(), "{text:?}");
        }
    }

    #[test]
    fn lang_from_path() {
        assert_eq!(Lang::from_path("src/main.rs"), Lang::Rust);
        assert_eq!(Lang::from_path("Makefile"), Lang::Shell);
        assert_eq!(Lang::from_path("Cargo.lock"), Lang::Toml);
        assert_eq!(Lang::from_path("a/b.unknown"), Lang::Plain);
    }
}
