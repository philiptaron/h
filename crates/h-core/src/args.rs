//! Command-line parsing shared by the binaries.

use std::ffi::OsStr;

/// What a `*-shell-init` binary was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command<T> {
    Help,
    Version,
    Run(T),
}

/// The error message for an unrecognized command-line argument.
pub fn unknown_option(arg: &OsStr) -> String {
    format!("Unknown option: {}", arg.to_string_lossy())
}

/// Split `line` into words as a POSIX shell would, without expanding anything: words are
/// separated by blanks, `'…'` keeps everything literally, `"…"` keeps everything but `\"`, `\\`,
/// `\$` and `` \` ``, and a backslash outside quotes keeps the next character. This is how the
/// `--git-opts` string of `h-shell-init` reaches `h` through the shell function.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let unterminated = || format!("Unterminated quote in: {line}");
    let (mut words, mut word) = (Vec::new(), None::<String>);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c.is_ascii_whitespace() {
            words.extend(word.take());
            continue;
        }
        let word = word.get_or_insert_default();
        match c {
            '\'' => loop {
                match chars.next().ok_or_else(unterminated)? {
                    '\'' => break,
                    c => word.push(c),
                }
            },
            '"' => loop {
                match chars.next().ok_or_else(unterminated)? {
                    '"' => break,
                    '\\' => match chars.next().ok_or_else(unterminated)? {
                        c @ ('"' | '\\' | '$' | '`') => word.push(c),
                        c => word.extend(['\\', c]),
                    },
                    c => word.push(c),
                }
            },
            '\\' => word.extend(chars.next()),
            c => word.push(c),
        }
    }
    words.extend(word);
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_unknown_option() {
        assert_eq!(unknown_option(OsStr::new("--bogus")), "Unknown option: --bogus");
    }

    #[test]
    fn splits_words_as_the_shell_does() {
        let line =
            r#"-c user.name="Me Too" -c user.signingkey="~/.ssh/a b.pub"  -c x='it''s "q"' a\ b"#;
        let want = [
            "-c",
            "user.name=Me Too",
            "-c",
            "user.signingkey=~/.ssh/a b.pub",
            "-c",
            r#"x=its "q""#,
            "a b",
        ];
        assert_eq!(split_words(line).unwrap(), want);
        assert_eq!(split_words(r#""a\"b\\c\d" '\' """#).unwrap(), [r#"a"b\c\d"#, "\\", ""]);
        assert_eq!(split_words("  \t\n").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn rejects_unterminated_quotes() {
        for line in [r#"-c user.name="Me"#, "-c 'x", r#""a\"#] {
            assert_eq!(split_words(line), Err(format!("Unterminated quote in: {line}")), "{line}");
        }
    }
}
