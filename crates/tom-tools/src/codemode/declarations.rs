//! TypeScript-style declarations for the script-visible tool API.
//!
//! The model writes JavaScript, but a *declaration* is the cheapest way to say
//! what a call takes and returns: `tools.read({ path: string }): Promise<string>`
//! carries more than a sentence of prose and costs fewer tokens. Port of
//! `packages/codemode/src/declarations.ts`, cut down to what the tool actually
//! emits: a tool member's signature and its doc comment, with JSON Schema types
//! rendered as TypeScript.
//!
//! Schemas only shape the declarations; arguments are not validated against them
//! here (the nested call goes through the normal tool pipeline, which validates).

use serde_json::Value;

/// Largest rendered input type, in characters, before it becomes `unknown`.
pub const DEFAULT_INPUT_SCHEMA_MAX_CHARS: usize = 16_000;

/// Local `$ref` expansions per rendered schema, so shared definitions cannot
/// blow up the output.
const MAX_REF_EXPANSIONS: usize = 32;

/// JSON Schema keywords with a number or string value shown as `@keyword value`.
const TAG_KEYWORDS: &[&str] = &[
    "minimum",
    "exclusiveMinimum",
    "maximum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "format",
    "minItems",
    "maxItems",
];

/// Longest property tag; longer ones (large defaults, long patterns) are left out.
const MAX_TAG_CHARS: usize = 80;

const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
    "implements",
    "package",
    "protected",
    "interface",
    "private",
    "public",
];

/// The JavaScript identifier a tool name becomes: characters outside
/// `[A-Za-z0-9_$]` become `_`, a leading digit gets a `_` prefix, and a reserved
/// word gets a `_` suffix.
pub fn to_codemode_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 1);
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || character == '_' || character == '$' {
            out.push(character);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        return "_".to_string();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if RESERVED.contains(&out.as_str()) {
        out.push('_');
    }
    // `tools.then` would make the proxy awaitable and hang every `await tools`.
    if out == "then" || out == "toJSON" || out == "constructor" || out == "prototype" {
        out.push('_');
    }
    out
}

/// One tool as the declaration renderer sees it.
#[derive(Debug, Clone)]
pub struct CodemodeDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// The tool's structured output schema, when it declared one. Tools without
    /// one resolve to their text output, a string.
    pub output_schema: Option<Value>,
}

/// `name(args: Input): Promise<Output>;` with the doc comment above it.
pub fn render_tool_signature(declaration: &CodemodeDeclaration, indent: &str) -> String {
    let input = render_type(&declaration.input_schema, "", 0, &mut 0);
    let output = match &declaration.output_schema {
        Some(schema) => render_type(schema, "", 0, &mut 0),
        None => "string".to_string(),
    };
    let name = to_codemode_identifier(&declaration.name);
    let mut rendered = format!(
        "{}// {}\n{indent}{name}(args: {}): Promise<{output}>;\n",
        indent,
        one_line(&declaration.description),
        input.trim_end_matches(';'),
    );
    // Multi-line input types read badly on one line; keep the shape readable.
    if input.contains('\n') {
        rendered = format!(
            "{}// {}\n{indent}{name}(args: {}): Promise<{output}>;\n",
            indent,
            one_line(&declaration.description),
            input.trim_end_matches(';').trim_end(),
        );
    }
    rendered
}

/// `### `name`` followed by the tool's description and declaration, which is what
/// a script author needs in order to call it.
pub fn render_tool_sample(declaration: &CodemodeDeclaration) -> String {
    let id = to_codemode_identifier(&declaration.name);
    let heading = if id == declaration.name {
        format!("### `{id}`")
    } else {
        format!("### `{id}` (`{}`)", declaration.name)
    };
    let mut out = format!("{heading}\n{}", declaration.description.trim());
    let input = render_type(&declaration.input_schema, "", 0, &mut 0);
    let output = match &declaration.output_schema {
        Some(schema) => render_type(schema, "", 0, &mut 0),
        None => "string".to_string(),
    };
    out.push_str(&format!(
        "\n\n`tools.{id}(args: {}): Promise<{}>`",
        input.trim_end_matches(';').trim(),
        output.trim_end_matches(';').trim(),
    ));
    out
}

fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Render a JSON Schema as a TypeScript type. `$ref`s resolve against `root`
/// while they fit [`MAX_REF_EXPANSIONS`]; anything unrenderable is `unknown`.
pub fn render_type(schema: &Value, indent: &str, depth: usize, expansions: &mut usize) -> String {
    let _ = schema; // re-bound below; kept for signature symmetry
    render(schema, indent, depth, expansions, None)
        .trim_end_matches(';')
        .to_string()
}

fn render(
    schema: &Value,
    indent: &str,
    depth: usize,
    expansions: &mut usize,
    root: Option<&Value>,
) -> String {
    if depth > 8 {
        return "unknown".to_string();
    }
    let Some(object) = schema.as_object() else {
        return match schema {
            Value::Bool(true) => "unknown".to_string(),
            Value::Bool(false) => "never".to_string(),
            _ => "unknown".to_string(),
        };
    };
    if object.is_empty() {
        return "unknown".to_string();
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        if *expansions >= MAX_REF_EXPANSIONS {
            return "unknown".to_string();
        }
        if let Some(target) = resolve_ref(root.unwrap_or(schema), reference) {
            *expansions += 1;
            return render(target, indent, depth + 1, expansions, root.or(Some(schema)));
        }
        return "unknown".to_string();
    }
    if let Some(Value::Array(variants)) = object.get("anyOf").or_else(|| object.get("oneOf")) {
        let parts: Vec<String> = variants
            .iter()
            .map(|variant| render(variant, indent, depth + 1, expansions, root))
            .collect();
        let mut unique: Vec<String> = Vec::new();
        for part in parts {
            if !unique.contains(&part) {
                unique.push(part);
            }
        }
        return unique.join(" | ");
    }
    if let Some(Value::Array(all)) = object.get("allOf") {
        let parts: Vec<String> = all
            .iter()
            .map(|part| render(part, indent, depth + 1, expansions, root))
            .collect();
        return parts.join(" & ");
    }
    if let Some(choices) = object.get("enum").and_then(Value::as_array) {
        if choices.is_empty() {
            return "never".to_string();
        }
        return choices
            .iter()
            .map(|choice| match choice {
                Value::String(text) => format!("{:?}", text),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" | ");
    }
    if let Some(Value::String(kind)) = object.get("type") {
        return match kind.as_str() {
            "string" => "string".to_string(),
            "number" | "integer" => "number".to_string(),
            "boolean" => "boolean".to_string(),
            "null" => "null".to_string(),
            "array" => {
                let item = object
                    .get("items")
                    .map(|items| render(items, indent, depth + 1, expansions, root))
                    .unwrap_or_else(|| "unknown".to_string());
                if item.contains(' ') && !item.contains('|') {
                    format!("Array<{item}>")
                } else {
                    format!("{item}[]")
                }
            }
            "object" => render_object(object, indent, depth, expansions, root),
            _ => "unknown".to_string(),
        };
    }
    if let Some(Value::Array(kinds)) = object.get("type") {
        let parts: Vec<String> = kinds
            .iter()
            .filter_map(Value::as_str)
            .map(|kind| {
                render(
                    &Value::String(kind.to_string()),
                    indent,
                    depth + 1,
                    expansions,
                    root,
                )
            })
            .collect();
        return parts.join(" | ");
    }
    if object.contains_key("properties") || object.contains_key("required") {
        return render_object(object, indent, depth, expansions, root);
    }
    "unknown".to_string()
}

fn render_object(
    object: &serde_json::Map<String, Value>,
    indent: &str,
    depth: usize,
    expansions: &mut usize,
    root: Option<&Value>,
) -> String {
    let Some(properties) = object.get("properties").and_then(Value::as_object) else {
        if let Some(additional) = object.get("additionalProperties") {
            let value = render(additional, indent, depth + 1, expansions, root);
            return format!("Record<string, {value}>");
        }
        return "Record<string, unknown>".to_string();
    };
    let required: Vec<&str> = object
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let inner = format!("{indent}  ");
    let mut lines = Vec::new();
    for (name, property) in properties {
        let doc = property
            .get("description")
            .and_then(Value::as_str)
            .map(|text| format!("{inner}// {}\n", one_line(text)))
            .unwrap_or_default();
        let tags = property_tags(property);
        let kind = render(property, &inner, depth + 1, expansions, root);
        let optional = if required.contains(&name.as_str()) {
            ""
        } else {
            "?"
        };
        lines.push(format!("{doc}{inner}{name}{optional}: {kind};{tags}"));
    }
    if lines.is_empty() {
        return "Record<string, unknown>".to_string();
    }
    format!("{{\n{}\n{indent}}}", lines.join("\n"))
}

/// ` // @integer @minimum 1` for constraints a TypeScript type cannot express.
fn property_tags(property: &Value) -> String {
    let Some(object) = property.as_object() else {
        return String::new();
    };
    let mut tags = Vec::new();
    if object.get("type").and_then(Value::as_str) == Some("integer") {
        tags.push("@integer".to_string());
    }
    for keyword in TAG_KEYWORDS {
        let Some(value) = object.get(*keyword) else {
            continue;
        };
        let rendered = match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        if rendered.chars().count() > MAX_TAG_CHARS {
            continue;
        }
        tags.push(format!("@{keyword} {rendered}"));
    }
    if let Some(default) = object.get("default") {
        let rendered = match default {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        if rendered.chars().count() <= MAX_TAG_CHARS {
            tags.push(format!("@default {rendered}"));
        }
    }
    if tags.is_empty() {
        String::new()
    } else {
        format!(" // {}", tags.join(" "))
    }
}

/// Resolve a local `#/$defs/...` or `#/definitions/...` reference.
fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        return Some(root);
    }
    if !pointer.starts_with("/$defs/") && !pointer.starts_with("/definitions/") {
        return None;
    }
    root.pointer(pointer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identifiers_are_valid_javascript_names() {
        assert_eq!(to_codemode_identifier("read"), "read");
        assert_eq!(to_codemode_identifier("my-tool"), "my_tool");
        assert_eq!(
            to_codemode_identifier("mcp__fs__read_file"),
            "mcp__fs__read_file"
        );
        assert_eq!(to_codemode_identifier("delete"), "delete_");
        assert_eq!(to_codemode_identifier("2fast"), "_2fast");
        // `tools.then` would make the proxy awaitable and hang `await tools`.
        assert_eq!(to_codemode_identifier("then"), "then_");
    }

    #[test]
    fn object_schemas_render_as_typescript() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File to read"},
                "limit": {"type": "integer", "minimum": 1}
            },
            "required": ["path"]
        });
        let rendered = render_type(&schema, "", 0, &mut 0);
        assert!(rendered.contains("path: string;"), "{rendered}");
        assert!(rendered.contains("limit?: number;"), "{rendered}");
        assert!(rendered.contains("@integer @minimum 1"), "{rendered}");
        assert!(rendered.contains("// File to read"), "{rendered}");
    }

    #[test]
    fn text_output_is_the_default_return_type() {
        let declaration = CodemodeDeclaration {
            name: "read".into(),
            description: "Read file contents".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            output_schema: None,
        };
        let sample = render_tool_sample(&declaration);
        assert!(sample.contains("### `read`"), "{sample}");
        assert!(sample.contains("Promise<string>"), "{sample}");
    }
}
