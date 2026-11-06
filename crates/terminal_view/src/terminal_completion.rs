use fs::Fs;
use futures::future::join_all;
use std::path::{Path, PathBuf};
use task::ShellKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CompletionQuote {
    None,
    Single,
    Double,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompletionToken {
    pub directory_prefix: String,
    pub file_prefix: String,
    pub quote: CompletionQuote,
    pub word_index: usize,
    pub path_like: bool,
}

impl CompletionToken {
    pub fn is_eligible(&self) -> bool {
        let looks_like_option =
            self.directory_prefix.is_empty() && self.file_prefix.starts_with('-');
        let contains_expansion = self.directory_prefix.contains('$')
            || self.directory_prefix.contains('`')
            || self.file_prefix.contains('$')
            || self.file_prefix.contains('`');

        !looks_like_option
            && !contains_expansion
            && (!self.directory_prefix.is_empty() || !self.file_prefix.is_empty())
            && (self.word_index > 0 || self.path_like)
    }

    pub fn resolve_directory(&self, cwd: &Path) -> PathBuf {
        if self.directory_prefix.is_empty() {
            return cwd.to_path_buf();
        }

        let directory = if self.directory_prefix.starts_with('~') {
            shellexpand::tilde(&self.directory_prefix).into_owned()
        } else {
            self.directory_prefix.clone()
        };
        let directory = PathBuf::from(directory);
        if directory.is_absolute() {
            directory
        } else {
            cwd.join(directory)
        }
    }

    pub fn remaining_text(
        &self,
        target: &str,
        current_prefix: &str,
        shell_kind: ShellKind,
    ) -> String {
        let suffix = target.strip_prefix(current_prefix).unwrap_or(target);
        escape_suffix(suffix, self.quote, shell_kind)
    }

    pub fn candidate_text(
        &self,
        candidate: &CompletionCandidate,
        current_prefix: &str,
        shell_kind: ShellKind,
    ) -> String {
        let mut text = self.remaining_text(&candidate.name, current_prefix, shell_kind);
        if candidate.is_dir {
            text.push(path_separator(shell_kind));
        } else {
            match self.quote {
                CompletionQuote::None => text.push(' '),
                CompletionQuote::Single => {
                    text.push('\'');
                    text.push(' ');
                }
                CompletionQuote::Double => {
                    text.push('"');
                    text.push(' ');
                }
            }
        }
        text
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompletionCandidate {
    pub name: String,
    pub is_dir: bool,
}

impl CompletionCandidate {
    pub fn display_name(&self, shell_kind: ShellKind) -> String {
        if self.is_dir {
            format!("{}{}", self.name, path_separator(shell_kind))
        } else {
            self.name.clone()
        }
    }
}

pub(crate) fn parse_last_word(input: &str) -> Option<CompletionToken> {
    parse_last_word_for_platform(input, cfg!(windows))
}

fn parse_last_word_for_platform(input: &str, windows_paths: bool) -> Option<CompletionToken> {
    let chars: Vec<char> = input.chars().collect();
    let mut quote = CompletionQuote::None;
    let mut escaped = false;
    let mut current_start = None;
    let mut word_index = 0;

    for (ix, character) in chars.iter().copied().enumerate() {
        match quote {
            CompletionQuote::None => {
                if escaped {
                    escaped = false;
                    continue;
                }
                if character == '\\' && !windows_paths {
                    current_start.get_or_insert(ix);
                    escaped = true;
                    continue;
                }
                if character.is_whitespace() {
                    if current_start.take().is_some() {
                        word_index += 1;
                    }
                    continue;
                }
                current_start.get_or_insert(ix);
                if character == '\'' {
                    quote = CompletionQuote::Single;
                } else if character == '"' {
                    quote = CompletionQuote::Double;
                }
            }
            CompletionQuote::Single => {
                if character == '\'' {
                    quote = CompletionQuote::None;
                }
            }
            CompletionQuote::Double => {
                if escaped {
                    escaped = false;
                } else if character == '\\' && !windows_paths {
                    escaped = true;
                } else if character == '"' {
                    quote = CompletionQuote::None;
                }
            }
        }
    }

    let start = current_start?;
    let end = chars.len();
    let raw = chars[start..end].iter().collect::<String>();
    let value = unescape_word(&raw, windows_paths);
    let (directory_prefix, file_prefix) = split_path_prefix(&value, windows_paths);
    let path_like = !directory_prefix.is_empty() || value.starts_with('~');

    Some(CompletionToken {
        directory_prefix,
        file_prefix,
        quote,
        word_index,
        path_like,
    })
}

fn unescape_word(raw: &str, windows_paths: bool) -> String {
    let mut value = String::with_capacity(raw.len());
    let mut quote = CompletionQuote::None;
    let mut escaped = false;

    for character in raw.chars() {
        match quote {
            CompletionQuote::None => {
                if escaped {
                    value.push(character);
                    escaped = false;
                } else if character == '\\' && !windows_paths {
                    escaped = true;
                } else if character == '\'' {
                    quote = CompletionQuote::Single;
                } else if character == '"' {
                    quote = CompletionQuote::Double;
                } else {
                    value.push(character);
                }
            }
            CompletionQuote::Single => {
                if character == '\'' {
                    quote = CompletionQuote::None;
                } else {
                    value.push(character);
                }
            }
            CompletionQuote::Double => {
                if escaped {
                    if character == '\n' {
                        // Backslash-newline is a line continuation inside
                        // double quotes and contributes no character.
                    } else if is_double_quote_escape(character) {
                        value.push(character);
                    } else {
                        // Unlike unquoted strings, a backslash only escapes a
                        // small set of characters inside double quotes.
                        value.push('\\');
                        value.push(character);
                    }
                    escaped = false;
                } else if character == '\\' && !windows_paths {
                    escaped = true;
                } else if character == '"' {
                    quote = CompletionQuote::None;
                } else {
                    value.push(character);
                }
            }
        }
    }

    if escaped {
        value.push('\\');
    }
    value
}

fn is_double_quote_escape(character: char) -> bool {
    matches!(character, '\\' | '"' | '$' | '`' | '\n')
}

fn split_path_prefix(value: &str, windows_paths: bool) -> (String, String) {
    match value.rfind(|character| character == '/' || windows_paths && character == '\\') {
        Some(index) => (
            value[..=index].to_string(),
            value[index + character_len(value, index)..].to_string(),
        ),
        None => (String::new(), value.to_string()),
    }
}

fn character_len(value: &str, byte_index: usize) -> usize {
    value[byte_index..].chars().next().map_or(0, char::len_utf8)
}

pub(crate) async fn find_candidates(
    fs: &dyn Fs,
    cwd: &Path,
    token: &CompletionToken,
) -> anyhow::Result<Vec<CompletionCandidate>> {
    let directory = token.resolve_directory(cwd);
    let mut entries = fs.read_dir(&directory).await?;
    let include_hidden = token.file_prefix.starts_with('.');
    let mut matching_paths = Vec::new();

    while let Some(entry) = futures::StreamExt::next(&mut entries).await {
        let path = entry?;
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(&token.file_prefix) {
            continue;
        }
        if !include_hidden && name.starts_with('.') {
            continue;
        }
        matching_paths.push((name.to_string(), path));
    }

    let mut candidates = join_all(matching_paths.into_iter().map(|(name, path)| async move {
        CompletionCandidate {
            name,
            is_dir: fs.is_dir(&path).await,
        }
    }))
    .await;

    candidates.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(candidates)
}

pub(crate) fn common_name_prefix(candidates: &[CompletionCandidate]) -> String {
    let Some(first) = candidates.first() else {
        return String::new();
    };
    let mut prefix = first.name.clone();

    for candidate in &candidates[1..] {
        let common_len = prefix
            .chars()
            .zip(candidate.name.chars())
            .take_while(|(left, right)| left == right)
            .map(|(character, _)| character.len_utf8())
            .sum();
        prefix.truncate(common_len);
        if prefix.is_empty() {
            break;
        }
    }

    prefix
}

fn escape_suffix(suffix: &str, quote: CompletionQuote, shell_kind: ShellKind) -> String {
    let mut escaped = String::with_capacity(suffix.len());
    for character in suffix.chars() {
        if quote == CompletionQuote::Single && character == '\'' {
            // A single quote cannot be escaped while single-quoted. Close the
            // quote, insert an escaped quote, then reopen it.
            escaped.push_str("'\\''");
            continue;
        }

        let needs_escape = match quote {
            CompletionQuote::Single => false,
            CompletionQuote::Double => matches!(character, '\\' | '"' | '$' | '`'),
            CompletionQuote::None => match shell_kind {
                ShellKind::PowerShell | ShellKind::Pwsh => {
                    character.is_whitespace()
                        || matches!(
                            character,
                            '`' | '$'
                                | '\''
                                | '"'
                                | '&'
                                | ';'
                                | '|'
                                | '<'
                                | '>'
                                | '('
                                | ')'
                                | '['
                                | ']'
                                | '{'
                                | '}'
                                | '@'
                                | '#'
                        )
                }
                ShellKind::Cmd => {
                    character.is_whitespace()
                        || matches!(
                            character,
                            '^' | '&' | '|' | '<' | '>' | '(' | ')' | '%' | '!'
                        )
                }
                _ => {
                    character.is_whitespace()
                        || matches!(
                            character,
                            '\\' | '\''
                                | '"'
                                | '`'
                                | '$'
                                | '&'
                                | ';'
                                | '|'
                                | '<'
                                | '>'
                                | '('
                                | ')'
                                | '['
                                | ']'
                                | '{'
                                | '}'
                                | '*'
                                | '?'
                                | '!'
                                | '#'
                        )
                }
            },
        };

        if needs_escape {
            match (quote, shell_kind) {
                (CompletionQuote::None, ShellKind::PowerShell | ShellKind::Pwsh) => {
                    escaped.push('`');
                }
                (CompletionQuote::None, ShellKind::Cmd) => escaped.push('^'),
                _ => escaped.push('\\'),
            }
        }
        escaped.push(character);
    }
    escaped
}

fn path_separator(shell_kind: ShellKind) -> char {
    match shell_kind {
        ShellKind::Cmd => '\\',
        _ => '/',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_path_argument() {
        let token = parse_last_word("ls src/ter").unwrap();
        assert_eq!(token.directory_prefix, "src/");
        assert_eq!(token.file_prefix, "ter");
        assert_eq!(token.quote, CompletionQuote::None);
        assert_eq!(token.word_index, 1);
        assert!(token.is_eligible());
    }

    #[test]
    fn parses_quoted_path_with_spaces() {
        let token = parse_last_word("cat \"my dir/fo").unwrap();
        assert_eq!(token.directory_prefix, "my dir/");
        assert_eq!(token.file_prefix, "fo");
        assert_eq!(token.quote, CompletionQuote::Double);
    }

    #[test]
    fn parses_escaped_space() {
        let token = parse_last_word_for_platform("cat my\\ dir/fo", false).unwrap();
        assert_eq!(token.directory_prefix, "my dir/");
        assert_eq!(token.file_prefix, "fo");
        assert_eq!(token.quote, CompletionQuote::None);
    }

    #[test]
    fn ignores_trailing_whitespace_and_options() {
        assert!(parse_last_word("ls ").is_none());
        assert!(!parse_last_word("ls --col").unwrap().is_eligible());
    }

    #[test]
    fn allows_path_like_first_word() {
        assert!(parse_last_word("./scr").unwrap().is_eligible());
        assert!(!parse_last_word("cargo").unwrap().is_eligible());
    }

    #[test]
    fn computes_unicode_safe_common_prefix() {
        let candidates = vec![
            CompletionCandidate {
                name: "测试-one".into(),
                is_dir: false,
            },
            CompletionCandidate {
                name: "测试-two".into(),
                is_dir: true,
            },
        ];
        assert_eq!(common_name_prefix(&candidates), "测试-");
    }

    #[test]
    fn escapes_unquoted_spaces() {
        let token = parse_last_word("cat my").unwrap();
        assert_eq!(
            token.remaining_text("my file", "my", ShellKind::Posix),
            "\\ file"
        );
    }

    #[test]
    fn quoted_candidates_do_not_escape_spaces() {
        let token = parse_last_word("cat \"my").unwrap();
        assert_eq!(
            token.remaining_text("my file", "my", ShellKind::Posix),
            " file"
        );
        assert_eq!(
            token.candidate_text(
                &CompletionCandidate {
                    name: "my file".into(),
                    is_dir: false,
                },
                "my",
                ShellKind::Posix,
            ),
            " file\" "
        );
    }

    #[test]
    fn preserves_ordinary_backslashes_in_double_quotes() {
        let token = parse_last_word_for_platform("cat \"C:\\Users\\fo", false).unwrap();
        assert_eq!(token.directory_prefix, "");
        assert_eq!(token.file_prefix, "C:\\Users\\fo");
        assert_eq!(token.quote, CompletionQuote::Double);
    }

    #[test]
    fn parses_unquoted_windows_paths() {
        let token = parse_last_word_for_platform("cd C:\\Users\\fo", true).unwrap();
        assert_eq!(token.directory_prefix, "C:\\Users\\");
        assert_eq!(token.file_prefix, "fo");
        assert_eq!(token.quote, CompletionQuote::None);
    }

    #[test]
    fn posix_backslashes_are_not_path_separators() {
        let token = parse_last_word_for_platform("cat file\\ name", false).unwrap();
        assert_eq!(token.directory_prefix, "");
        assert_eq!(token.file_prefix, "file name");
    }

    #[test]
    fn single_quoted_candidates_escape_embedded_quotes() {
        let token = parse_last_word("cat 'my").unwrap();
        assert_eq!(
            token.candidate_text(
                &CompletionCandidate {
                    name: "my file's.txt".into(),
                    is_dir: false,
                },
                "my",
                ShellKind::Posix,
            ),
            " file'\\''s.txt' "
        );
    }
}
