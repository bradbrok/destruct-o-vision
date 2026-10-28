//! Module files: a WGSL file whose first comment is a TOML header.
//!
//! [`parse`] reads and validates the header. [`check`] also compiles the
//! module against the glue and enforces the rules for its WGSL.

use std::fmt;

use serde::Deserialize;

use crate::glue::{
    self, MAX_HISTORY_FIELDS, MAX_INPUTS, MAX_OUTPUTS, MAX_PARAM_SLOTS, RESERVED_NAMES, RESERVED_PREFIXES,
};
use crate::interface::{Curve, Cv, History, Interface, KernelKind, Param, ParamKind, Port, PortKind};

/// The module format this crate reads.
pub const FORMAT: u32 = 1;
/// Longest default text for modules with a text field.
pub const MAX_TEXT_CHARS: usize = 256;
const HEADER_OPEN: &str = "/* dov-module";
const HEADER_CLOSE: &str = "*/";

/// Problems found in a module or package, one message per problem.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub messages: Vec<String>,
}

impl Error {
    fn one(message: impl Into<String>) -> Self {
        Self {
            messages: vec![message.into()],
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.messages.join("\n"))
    }
}

impl std::error::Error for Error {}

/// Browser sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Source,
    Process,
    Color,
    Space,
    Analog,
    Glitch,
    Control,
    Io,
}

/// A parsed, validated module file.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    /// `<namespace>.<name>`.
    pub id: String,
    pub name: String,
    pub category: Category,
    pub tags: Vec<String>,
    pub description: String,
    pub screen: bool,
    pub interface: Interface,
    /// For modules with a text field: the text a new module starts with.
    pub text_default: Option<String>,
    /// The whole file, header included. The engine compiles it as-is.
    pub source: String,
}

impl Module {
    pub fn namespace(&self) -> &str {
        self.id.split('.').next().unwrap_or_default()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: u32,
    id: String,
    name: String,
    category: String,
    #[serde(default)]
    tags: Vec<String>,
    description: String,
    kernel: String,
    #[serde(default)]
    screen: bool,
    #[serde(default, rename = "input")]
    inputs: Vec<PortHeader>,
    #[serde(default, rename = "output")]
    outputs: Vec<PortHeader>,
    #[serde(default, rename = "param")]
    params: Vec<ParamHeader>,
    history: Option<HistoryHeader>,
    text: Option<TextHeader>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortHeader {
    id: String,
    name: Option<String>,
    kind: String,
    default: Option<f32>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DefaultValue {
    Number(f64),
    Name(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamHeader {
    id: String,
    name: Option<String>,
    range: Option<[f32; 2]>,
    choices: Option<Vec<String>>,
    default: Option<DefaultValue>,
    curve: Option<String>,
    unit: Option<String>,
    cv: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryHeader {
    output: String,
    freeze: Option<String>,
    fields: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextHeader {
    default: String,
}

/// Where the header sits in a file.
struct Located<'a> {
    toml: &'a str,
    /// 1-based file line of the header's first TOML line.
    first_line: usize,
}

fn locate_header(source: &str) -> Result<Located<'_>, Error> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut offset = 0;
    let mut open_line = None;
    let mut start = 0;
    for (n, line) in source.split_inclusive('\n').enumerate() {
        let trimmed = line.trim();
        match open_line {
            None if trimmed.is_empty() => {}
            None if trimmed == HEADER_OPEN => {
                open_line = Some(n);
                start = offset + line.len();
            }
            None => {
                return Err(Error::one(format!(
                    "line {}: a module file must start with a `{HEADER_OPEN}` comment",
                    n + 1
                )));
            }
            Some(open) if trimmed == HEADER_CLOSE => {
                let toml = &source[start..offset];
                if toml.contains("*/") || toml.contains("/*") {
                    return Err(Error::one(
                        "the header can't contain `/*` or `*/`: WGSL would end or nest the comment there",
                    ));
                }
                return Ok(Located {
                    toml,
                    first_line: open + 2,
                });
            }
            Some(_) => {}
        }
        offset += line.len();
    }
    Err(Error::one(match open_line {
        None => format!("a module file must start with a `{HEADER_OPEN}` comment"),
        Some(n) => format!(
            "line {}: the header comment is never closed with `*/` on its own line",
            n + 1
        ),
    }))
}

/// Reads and validates a module file's header.
pub fn parse(source: &str) -> Result<Module, Error> {
    let located = locate_header(source)?;
    let header: Header = toml::from_str(located.toml).map_err(|e| {
        let line = e
            .span()
            .map(|span| located.first_line + located.toml[..span.start].matches('\n').count());
        let message = e.message().trim().to_string();
        Error::one(match line {
            Some(line) => format!("line {line}: {message}"),
            None => message,
        })
    })?;
    let mut errors = Vec::new();
    let module = validate(header, source, &mut errors);
    match module {
        Some(m) if errors.is_empty() => Ok(m),
        _ => Err(Error { messages: errors }),
    }
}

fn validate(h: Header, source: &str, errors: &mut Vec<String>) -> Option<Module> {
    let mut err = |m: String| errors.push(m);
    if h.format != FORMAT {
        err(format!(
            "format {} isn't supported (this version reads format {FORMAT})",
            h.format
        ));
    }
    if !is_module_id(&h.id) {
        err(format!(
            "id `{}` must be `<namespace>.<name>`: lowercase letters, digits and dashes, each starting with a letter",
            h.id
        ));
    }
    check_label(&h.name, "name", 32, &mut err);
    if h.description.trim().is_empty() || h.description.chars().count() > 600 {
        err("description must be 1 to 600 characters".into());
    }
    if h.tags.len() > 8 || h.tags.iter().any(|t| !is_slug(t) || t.len() > 24) {
        err("tags: at most 8, each lowercase letters, digits and dashes".into());
    }
    let category = match h.category.as_str() {
        "source" => Some(Category::Source),
        "process" => Some(Category::Process),
        "color" => Some(Category::Color),
        "space" => Some(Category::Space),
        "analog" => Some(Category::Analog),
        "glitch" => Some(Category::Glitch),
        "control" => Some(Category::Control),
        "io" => Some(Category::Io),
        other => {
            err(format!(
                "category `{other}` must be source, process, color, space, analog, glitch, control or io"
            ));
            None
        }
    };
    let kernel = match h.kernel.as_str() {
        "pointwise" => Some(KernelKind::Pointwise),
        "linescan" => Some(KernelKind::LineScan),
        other => {
            err(format!("kernel `{other}` must be pointwise or linescan"));
            None
        }
    };

    let inputs: Vec<Port> = h
        .inputs
        .into_iter()
        .filter_map(|p| port(p, true, &mut err))
        .collect();
    let outputs: Vec<Port> = h
        .outputs
        .into_iter()
        .filter_map(|p| port(p, false, &mut err))
        .collect();
    let params: Vec<Param> = h.params.into_iter().filter_map(|p| param(p, &mut err)).collect();

    let history = h.history.and_then(|hh| {
        let output = outputs.iter().position(|p| p.id == hh.output);
        if output.is_none() {
            err(format!("history output `{}` isn't one of the outputs", hh.output));
        }
        let freeze = match &hh.freeze {
            Some(id) => match params.iter().position(|p| &p.id == id) {
                Some(i) => Some(Some(i)),
                None => {
                    err(format!("history freeze `{id}` isn't one of the params"));
                    None
                }
            },
            None => Some(None),
        };
        let fields = hh.fields.unwrap_or(MAX_HISTORY_FIELDS);
        if !(1..=MAX_HISTORY_FIELDS).contains(&fields) {
            err(format!("history fields must be 1 to {MAX_HISTORY_FIELDS}"));
        }
        Some(History {
            output: output?,
            freeze: freeze?,
            fields,
        })
    });
    let text_default = h.text.map(|t| {
        if t.default.chars().count() > MAX_TEXT_CHARS {
            err(format!(
                "text default must be at most {MAX_TEXT_CHARS} characters"
            ));
        }
        t.default
    });

    let interface = Interface {
        inputs,
        outputs,
        params,
        kernel: kernel?,
        history,
        text: text_default.is_some(),
    };

    // Ids must be unique on each side, generated CV jacks included.
    let mut seen = std::collections::HashSet::new();
    for slot in interface.inputs_full() {
        if !seen.insert(slot.id.clone()) {
            err(format!(
                "input `{}` is declared twice (CV jacks are named `<param>_cv`)",
                slot.id
            ));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for p in &interface.outputs {
        if !seen.insert(&p.id) {
            err(format!("output `{}` is declared twice", p.id));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for p in &interface.params {
        if !seen.insert(&p.id) {
            err(format!("param `{}` is declared twice", p.id));
        }
    }
    if interface.outputs.is_empty() {
        err("a module needs at least one output".into());
    }
    if interface.inputs_full().len() > MAX_INPUTS {
        err(format!(
            "too many inputs: at most {MAX_INPUTS}, counting CV jacks"
        ));
    }
    if interface.outputs.len() > MAX_OUTPUTS {
        err(format!("too many outputs: at most {MAX_OUTPUTS}"));
    }
    if interface.param_slots() > MAX_PARAM_SLOTS {
        err(format!(
            "too many params: at most {MAX_PARAM_SLOTS} slots, counting one per CV jack"
        ));
    }

    Some(Module {
        id: h.id,
        name: h.name,
        category: category?,
        tags: h.tags,
        description: h.description,
        screen: h.screen,
        interface,
        text_default,
        source: source.to_string(),
    })
}

fn port(p: PortHeader, input: bool, err: &mut impl FnMut(String)) -> Option<Port> {
    let side = if input { "input" } else { "output" };
    if !is_field_id(&p.id) {
        err(format!("{side} id `{}`: {}", p.id, FIELD_ID_RULE));
    }
    let name = p.name.unwrap_or_else(|| p.id.clone());
    check_label(&name, &format!("{side} `{}` name", p.id), 24, err);
    if input && p.hidden {
        err(format!("input `{}`: only outputs can be hidden", p.id));
    }
    if !input && p.default.is_some() {
        err(format!("output `{}`: only inputs have a default", p.id));
    }
    let kind = match p.kind.as_str() {
        "mono" => PortKind::Mono,
        "rgb" => PortKind::Rgb,
        "control" => PortKind::Control,
        other => {
            err(format!(
                "{side} `{}`: kind `{other}` must be mono, rgb or control",
                p.id
            ));
            return None;
        }
    };
    let default = p.default.unwrap_or(0.0);
    if !default.is_finite() {
        err(format!("input `{}`: default must be a number", p.id));
    }
    Some(Port {
        id: p.id,
        name,
        kind,
        default,
        hidden: p.hidden,
    })
}

fn param(p: ParamHeader, err: &mut impl FnMut(String)) -> Option<Param> {
    let id = p.id;
    if !is_field_id(&id) {
        err(format!("param id `{id}`: {FIELD_ID_RULE}"));
    }
    let name = p.name.unwrap_or_else(|| id.clone());
    check_label(&name, &format!("param `{id}` name"), 24, err);
    let cv = match p.cv.as_deref() {
        None => None,
        Some("video") => Some(Cv::Video),
        Some("control") => Some(Cv::Control),
        Some(other) => {
            err(format!("param `{id}`: cv `{other}` must be video or control"));
            None
        }
    };
    let kind = match (p.range, p.choices) {
        (Some([min, max]), None) => {
            if !(min.is_finite() && max.is_finite() && min < max) {
                err(format!("param `{id}`: range must be [min, max] with min < max"));
                return None;
            }
            let curve = match p.curve.as_deref() {
                None | Some("linear") => Curve::Linear,
                Some("exp") if min > 0.0 => Curve::Exp,
                Some("exp") => {
                    err(format!("param `{id}`: an exp curve needs min > 0"));
                    Curve::Linear
                }
                Some(other) => {
                    err(format!("param `{id}`: curve `{other}` must be linear or exp"));
                    Curve::Linear
                }
            };
            let default = match p.default {
                None => min,
                Some(DefaultValue::Number(v)) if (min..=max).contains(&(v as f32)) => v as f32,
                Some(_) => {
                    err(format!("param `{id}`: default must be a number within the range"));
                    min
                }
            };
            let unit = p.unit.unwrap_or_default();
            if unit.chars().count() > 12 {
                err(format!("param `{id}`: unit must be at most 12 characters"));
            }
            ParamKind::Knob {
                min,
                max,
                default,
                curve,
                unit,
            }
        }
        (None, Some(options)) => {
            if options.is_empty() || options.len() > 32 {
                err(format!("param `{id}`: choices must list 1 to 32 options"));
            }
            for o in &options {
                check_label(o, &format!("param `{id}` choice"), 16, err);
            }
            if p.curve.is_some() || p.unit.is_some() {
                err(format!("param `{id}`: curve and unit are for knobs, not choices"));
            }
            let default = match p.default {
                None => 0,
                Some(DefaultValue::Name(name)) => match options.iter().position(|o| *o == name) {
                    Some(i) => i as u32,
                    None => {
                        err(format!("param `{id}`: default `{name}` isn't one of the choices"));
                        0
                    }
                },
                Some(DefaultValue::Number(v))
                    if v >= 0.0 && v.fract() == 0.0 && (v as usize) < options.len() =>
                {
                    v as u32
                }
                Some(DefaultValue::Number(_)) => {
                    err(format!("param `{id}`: default must be a choice name or index"));
                    0
                }
            };
            ParamKind::Choice { options, default }
        }
        _ => {
            err(format!(
                "param `{id}`: give either a range (a knob) or choices (a switch)"
            ));
            return None;
        }
    };
    Some(Param { id, name, kind, cv })
}

fn check_label(label: &str, what: &str, max: usize, err: &mut impl FnMut(String)) {
    if label.trim().is_empty() || label.chars().count() > max {
        err(format!("{what} must be 1 to {max} characters"));
    }
}

const FIELD_ID_RULE: &str = "must be lowercase letters, digits and underscores, start with a letter, \
                             be at most 32 characters, and not be a WGSL keyword";

/// Port and param ids become WGSL struct fields.
pub fn is_field_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && id.len() <= 32
        && !id.contains("__")
        && !WGSL_RESERVED.contains(&id)
}

fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.ends_with('-')
        && s.len() <= 40
}

/// `<namespace>.<name>`.
pub fn is_module_id(id: &str) -> bool {
    matches!(id.split_once('.'), Some((ns, name)) if is_slug(ns) && is_slug(name))
}

/// WGSL keywords and reserved words (WGSL spec, sections 15.1 and 15.2).
const WGSL_RESERVED: &[&str] = &[
    "alias",
    "break",
    "case",
    "const",
    "const_assert",
    "continue",
    "continuing",
    "default",
    "diagnostic",
    "discard",
    "else",
    "enable",
    "false",
    "fn",
    "for",
    "if",
    "let",
    "loop",
    "override",
    "requires",
    "return",
    "struct",
    "switch",
    "true",
    "var",
    "while",
    "abstract",
    "active",
    "alignas",
    "alignof",
    "as",
    "asm",
    "asm_fragment",
    "async",
    "attribute",
    "auto",
    "await",
    "become",
    "cast",
    "catch",
    "class",
    "co_await",
    "co_return",
    "co_yield",
    "coherent",
    "column_major",
    "common",
    "compile",
    "compile_fragment",
    "concept",
    "const_cast",
    "consteval",
    "constexpr",
    "constinit",
    "crate",
    "debugger",
    "decltype",
    "delete",
    "demote",
    "demote_to_helper",
    "do",
    "dynamic_cast",
    "enum",
    "explicit",
    "export",
    "extends",
    "extern",
    "external",
    "fallthrough",
    "filter",
    "final",
    "finally",
    "friend",
    "from",
    "fxgroup",
    "get",
    "goto",
    "groupshared",
    "highp",
    "impl",
    "implements",
    "import",
    "inline",
    "instanceof",
    "interface",
    "layout",
    "lowp",
    "macro",
    "macro_rules",
    "match",
    "mediump",
    "meta",
    "mod",
    "module",
    "move",
    "mut",
    "mutable",
    "namespace",
    "new",
    "nil",
    "noexcept",
    "noinline",
    "nointerpolation",
    "noperspective",
    "null",
    "nullptr",
    "of",
    "operator",
    "package",
    "packoffset",
    "partition",
    "pass",
    "patch",
    "pixelfragment",
    "precise",
    "precision",
    "premerge",
    "priv",
    "protected",
    "pub",
    "public",
    "readonly",
    "ref",
    "regardless",
    "register",
    "reinterpret_cast",
    "require",
    "resource",
    "restrict",
    "self",
    "set",
    "shared",
    "sizeof",
    "smooth",
    "snorm",
    "static",
    "static_assert",
    "static_cast",
    "std",
    "subroutine",
    "super",
    "target",
    "template",
    "this",
    "thread_local",
    "throw",
    "trait",
    "try",
    "type",
    "typedef",
    "typeid",
    "typename",
    "typeof",
    "union",
    "unless",
    "unorm",
    "unsafe",
    "unsized",
    "use",
    "using",
    "varying",
    "virtual",
    "volatile",
    "wgsl",
    "where",
    "with",
    "writeonly",
    "yield",
];

/// The WGSL with comments blanked out (newlines kept, so lines still line up).
fn strip_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let (mut i, mut depth) = (0, 0usize);
    while i < bytes.len() {
        let two = &bytes[i..(i + 2).min(bytes.len())];
        if depth == 0 && two == b"//" {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
        } else if two == b"/*" {
            depth += 1;
            out.extend_from_slice(b"  ");
            i += 2;
        } else if depth > 0 && two == b"*/" {
            depth -= 1;
            out.extend_from_slice(b"  ");
            i += 2;
        } else {
            out.push(if depth > 0 && bytes[i] != b'\n' {
                b' '
            } else {
                bytes[i]
            });
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Rules for the module's own WGSL that the compiler can't express.
fn check_body(module: &Module, errors: &mut Vec<String>) {
    let code = strip_comments(&module.source);
    let mut declared = Vec::new();
    for (n, line) in code.lines().enumerate() {
        let at = |what: &str| format!("line {}: {what}", n + 1);
        let trimmed = line.trim_start();
        for (pattern, why) in [
            (
                "@group",
                "modules can't declare resources; the engine binds everything",
            ),
            (
                "@binding",
                "modules can't declare resources; the engine binds everything",
            ),
            (
                "@compute",
                "modules can't declare entry points; write `fn process`",
            ),
            (
                "@vertex",
                "modules can't declare entry points; write `fn process`",
            ),
            (
                "@fragment",
                "modules can't declare entry points; write `fn process`",
            ),
            (
                "@workgroup_size",
                "modules can't declare entry points; write `fn process`",
            ),
        ] {
            if line.contains(pattern) {
                errors.push(at(why));
            }
        }
        for (keyword, why) in [
            ("enable ", "`enable` directives aren't allowed in format 1"),
            ("requires ", "`requires` directives aren't allowed in format 1"),
            ("override ", "`override` constants aren't allowed in format 1"),
            (
                "var",
                "module-scope variables aren't allowed; use `const`, or carry state with `linescan`",
            ),
        ] {
            let is_module_scope = !line.starts_with(char::is_whitespace);
            if is_module_scope && trimmed.starts_with(keyword) {
                let next = trimmed[keyword.len()..].chars().next();
                if keyword != "var" || matches!(next, Some(' ' | '<')) {
                    errors.push(at(why));
                }
            }
        }
        for keyword in ["fn ", "struct ", "const ", "alias "] {
            if let Some(rest) = trimmed.strip_prefix(keyword) {
                let name: String = rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !line.starts_with(char::is_whitespace) || keyword == "fn " {
                    declared.push((n + 1, name));
                }
            }
        }
    }
    for (line, name) in &declared {
        if RESERVED_NAMES.contains(&name.as_str()) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p)) {
            errors.push(format!("line {line}: `{name}` is a reserved name"));
        }
    }
    let has = |name: &str| declared.iter().any(|(_, n)| n == name);
    if !has("process") {
        errors.push("missing `fn process`".into());
    }
    if module.interface.kernel == KernelKind::LineScan && !(has("State") && has("init_state")) {
        errors.push("a linescan kernel needs `struct State` and `fn init_state() -> State`".into());
    }
}

/// Parses a module file and checks everything: the header, the rules for
/// its WGSL, and that it compiles against the glue. `path` labels errors.
pub fn check(source: &str, path: &str) -> Result<Module, Error> {
    let module = parse(source)?;
    let mut errors = Vec::new();
    check_body(&module, &mut errors);
    if !errors.is_empty() {
        return Err(Error { messages: errors });
    }
    let shader = glue::shader(&module.interface, &module.source);
    let author_lines = module.source.lines().count();
    let in_glue = |e: &str| {
        // naga reports `path:line:col`; lines past the module are glue.
        let line = e
            .split(&format!("{path}:"))
            .nth(1)
            .and_then(|r| r.split(':').next())
            .and_then(|n| n.parse::<usize>().ok());
        match line {
            Some(n) if n > author_lines => {
                format!("{e}\n(in the generated glue; `destructovision module glue {path}` shows it)")
            }
            _ => e.to_string(),
        }
    };
    let parsed = naga::front::wgsl::parse_str(&shader)
        .map_err(|e| Error::one(in_glue(e.emit_to_string_with_path(&shader, path).trim())))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&parsed)
    .map_err(|e| Error::one(in_glue(e.emit_to_string_with_path(&shader, path).trim())))?;

    // Belt and braces: whatever got through, only the glue's resources and
    // entry point may exist.
    let inputs = module.interface.inputs_full().len();
    let allowed = |name: &str| {
        name == "u"
            || (module.interface.history.is_some() && name == "ring")
            || (module.interface.text && (name == "text_tex" || name == "text_sampler"))
            || name
                .strip_prefix("in_")
                .and_then(|i| i.parse::<usize>().ok())
                .is_some_and(|i| i < inputs)
            || name
                .strip_prefix("out_")
                .and_then(|k| k.parse::<usize>().ok())
                .is_some_and(|k| k < module.interface.outputs.len())
    };
    for (_, var) in parsed.global_variables.iter() {
        let name = var.name.as_deref().unwrap_or("");
        if !allowed(name) {
            errors.push(format!("`{name}`: modules can't declare module-scope variables"));
        }
    }
    if parsed.entry_points.len() != 1 || !parsed.overrides.is_empty() {
        errors.push("modules can't declare entry points or override constants".into());
    }
    if errors.is_empty() {
        Ok(module)
    } else {
        Err(Error { messages: errors })
    }
}
