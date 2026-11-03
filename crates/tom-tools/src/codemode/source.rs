//! The codemode source format: JavaScript, optionally preceded by one options
//! line.
//!
//! ```js
//! // @options: {"max_output_tokens": 2000, "timeout_ms": 30000}
//! const text = await tools.read({ path: "package.json" });
//! return JSON.parse(text).name;
//! ```
//!
//! Port of `packages/codemode/src/source.ts`. The options line is replaced by an
//! empty line rather than removed, so a stack-trace line number still matches
//! what the model wrote. A misplaced options line is an error instead of being
//! silently ignored: it is the one mistake whose symptom (the budget is ignored)
//! is otherwise invisible.

/// Prefix of the options line.
pub const CODEMODE_OPTIONS_PREFIX: &str = "// @options:";

/// Largest delay a timer supports; bounds `timeout_ms`.
const MAX_TIMEOUT_MS: u64 = 2_147_483_647;

/// The fields a script may set in its `// @options:` line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodemodeSourceOptions {
    /// Token budget for the script's output.
    pub max_output_tokens: Option<u64>,
    /// Hard deadline for the whole script in milliseconds, including tool calls.
    pub timeout_ms: Option<u64>,
}

/// A parsed script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCodemodeSource {
    /// The script with the options line replaced by an empty line, so line
    /// numbers are unchanged.
    pub code: String,
    pub options: CodemodeSourceOptions,
}

/// Why a script could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CodemodeSourceError(pub String);

const SUPPORTED_FIELDS_TEXT: &str = "`max_output_tokens` and `timeout_ms`";

fn parse_options(directive: &str) -> Result<CodemodeSourceOptions, CodemodeSourceError> {
    if directive.is_empty() {
        return Err(CodemodeSourceError(format!(
            "@options must be a JSON object with supported fields {SUPPORTED_FIELDS_TEXT}"
        )));
    }
    let value: serde_json::Value = serde_json::from_str(directive).map_err(|error| {
        CodemodeSourceError(format!(
            "@options must be valid JSON with supported fields {SUPPORTED_FIELDS_TEXT}: {error}"
        ))
    })?;
    let Some(fields) = value.as_object() else {
        return Err(CodemodeSourceError(format!(
            "@options must be a JSON object with supported fields {SUPPORTED_FIELDS_TEXT}"
        )));
    };
    for key in fields.keys() {
        if key != "max_output_tokens" && key != "timeout_ms" {
            return Err(CodemodeSourceError(format!(
                "@options only supports {SUPPORTED_FIELDS_TEXT}; got `{key}`"
            )));
        }
    }
    let mut options = CodemodeSourceOptions::default();
    if let Some(value) = fields.get("max_output_tokens") {
        let budget = value.as_u64().ok_or_else(|| {
            CodemodeSourceError(
                "@options field `max_output_tokens` must be a non-negative safe integer".into(),
            )
        })?;
        options.max_output_tokens = Some(budget);
    }
    if let Some(value) = fields.get("timeout_ms") {
        let timeout = value
            .as_u64()
            .filter(|value| *value != 0 && *value <= MAX_TIMEOUT_MS);
        let Some(timeout) = timeout else {
            return Err(CodemodeSourceError(format!(
                "@options field `timeout_ms` must be a positive integer up to {MAX_TIMEOUT_MS}"
            )));
        };
        options.timeout_ms = Some(timeout);
    }
    Ok(options)
}

/// Split an optional first-line `// @options: {...}` from the script.
///
/// Errors for empty input, invalid options, and an options line that is not the
/// first line — the last one because it would otherwise be ignored without
/// notice.
pub fn parse_codemode_source(input: &str) -> Result<ParsedCodemodeSource, CodemodeSourceError> {
    if input.trim().is_empty() {
        return Err(CodemodeSourceError(
            "Expected JavaScript source text (non-empty). Provide JS only, optionally with a first \
             line `// @options: {\"max_output_tokens\": 1000}`."
                .into(),
        ));
    }
    let lines: Vec<&str> = input.split('\n').collect();
    if let Some(index) = lines
        .iter()
        .enumerate()
        .find(|(index, line)| *index > 0 && line.trim_start().starts_with(CODEMODE_OPTIONS_PREFIX))
        .map(|(index, _)| index)
    {
        return Err(CodemodeSourceError(format!(
            "The {CODEMODE_OPTIONS_PREFIX} line must be the first line of the input; found one on \
             line {}",
            index + 1
        )));
    }
    let first_line = lines[0].strip_suffix('\r').unwrap_or(lines[0]);
    let trimmed = first_line.trim_start();
    if !trimmed.starts_with(CODEMODE_OPTIONS_PREFIX) {
        return Ok(ParsedCodemodeSource {
            code: input.to_string(),
            options: CodemodeSourceOptions::default(),
        });
    }
    // Keep the newline: the code keeps the line numbering of the input.
    let code = input.find('\n').map(|index| &input[index..]).unwrap_or("");
    if code.trim().is_empty() {
        return Err(CodemodeSourceError(
            "The @options line must be followed by JavaScript source on subsequent lines".into(),
        ));
    }
    let directive = trimmed[CODEMODE_OPTIONS_PREFIX.len()..].trim();
    Ok(ParsedCodemodeSource {
        code: code.to_string(),
        options: parse_options(directive)?,
    })
}

/// Lark grammar for providers that support grammar-constrained tool input. It
/// only fixes the shape of the options line; the options JSON and the code are
/// checked by [`parse_codemode_source`].
pub const CODEMODE_SOURCE_GRAMMAR: &str = r#"
start: options_source | plain_source
options_source: OPTIONS_LINE NEWLINE SOURCE
plain_source: SOURCE

OPTIONS_LINE: /[ \t]*\/\/ @options:[^\r\n]*/
NEWLINE: /\r?\n/
SOURCE: /[\s\S]+/
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_source_is_unchanged() {
        let parsed = parse_codemode_source("return 1").unwrap();
        assert_eq!(parsed.code, "return 1");
        assert_eq!(parsed.options, CodemodeSourceOptions::default());
    }

    #[test]
    fn options_line_is_replaced_by_an_empty_line() {
        let parsed =
            parse_codemode_source("// @options: {\"max_output_tokens\": 20}\nreturn 1").unwrap();
        assert_eq!(parsed.code, "\nreturn 1");
        assert_eq!(parsed.options.max_output_tokens, Some(20));
        assert_eq!(parsed.options.timeout_ms, None);
    }

    #[test]
    fn a_misplaced_options_line_is_an_error() {
        let error = parse_codemode_source("return 1\n// @options: {}\n").unwrap_err();
        assert!(error.0.contains("must be the first line"), "{error}");
    }

    #[test]
    fn unknown_option_fields_are_rejected() {
        let error = parse_codemode_source("// @options: {\"nope\": 1}\n1").unwrap_err();
        assert!(error.0.contains("got `nope`"), "{error}");
    }

    #[test]
    fn a_zero_timeout_is_rejected() {
        let error = parse_codemode_source("// @options: {\"timeout_ms\": 0}\n1").unwrap_err();
        assert!(error.0.contains("positive integer"), "{error}");
    }

    #[test]
    fn an_options_line_without_code_is_rejected() {
        let error = parse_codemode_source("// @options: {}").unwrap_err();
        assert!(error.0.contains("subsequent lines"), "{error}");
    }

    #[test]
    fn empty_source_is_rejected() {
        assert!(parse_codemode_source("   \n\t").is_err());
    }

    #[test]
    fn crlf_input_keeps_line_numbers() {
        // The options line is replaced by an empty line, not removed, so the
        // script's own line numbers still match the input.
        let parsed = parse_codemode_source("// @options: {}\r\nreturn 1").unwrap();
        assert_eq!(parsed.code, "\nreturn 1");
        assert_eq!(parsed.code.lines().count(), 2);
    }
}
