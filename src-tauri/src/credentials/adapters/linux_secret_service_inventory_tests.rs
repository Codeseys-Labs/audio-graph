use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Code(&'a str),
    Literal(String),
}

fn word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn block_comment_end(source: &str) -> &str {
    let mut depth = 1;
    let mut offset = 2;
    let bytes = source.as_bytes();
    while offset < bytes.len() {
        match &bytes[offset..] {
            [b'/', b'*', ..] => {
                depth += 1;
                offset += 2;
            }
            [b'*', b'/', ..] => {
                depth -= 1;
                offset += 2;
                if depth == 0 {
                    return &source[offset..];
                }
            }
            _ => offset += 1,
        }
    }
    ""
}

fn raw_literal(source: &str) -> Option<(&str, &str)> {
    let source = source
        .strip_prefix("br")
        .or_else(|| source.strip_prefix("cr"))
        .or_else(|| source.strip_prefix('r'))?;
    let hashes = source.len() - source.trim_start_matches('#').len();
    let content = source[hashes..].strip_prefix('"')?;
    let end = format!("\"{}", "#".repeat(hashes));
    content.split_once(&end)
}

fn escaped(source: &str) -> Option<(Option<char>, &str)> {
    let ch = source.chars().next()?;
    let rest = &source[ch.len_utf8()..];
    match ch {
        'x' => {
            let pair = rest.get(..2)?;
            Some((
                char::from_u32(u32::from_str_radix(pair, 16).ok()?),
                &rest[2..],
            ))
        }
        'u' => {
            let (digits, remainder) = rest.strip_prefix('{')?.split_once('}')?;
            let value = u32::from_str_radix(&digits.replace('_', ""), 16).ok()?;
            Some((char::from_u32(value), remainder))
        }
        '\n' | '\r' => Some((None, rest.trim_start())),
        'n' => Some((Some('\n'), rest)),
        'r' => Some((Some('\r'), rest)),
        't' => Some((Some('\t'), rest)),
        '0' => Some((Some('\0'), rest)),
        other => Some((Some(other), rest)),
    }
}

fn quoted_literal(mut source: &str, quote: char) -> Option<(String, &str)> {
    let mut value = String::new();
    while let Some(ch) = source.chars().next() {
        source = &source[ch.len_utf8()..];
        if ch == quote {
            return Some((value, source));
        }
        if ch == '\\' {
            let (decoded, rest) = escaped(source)?;
            value.extend(decoded);
            source = rest;
        } else {
            value.push(ch);
        }
        // An apostrophe followed by an identifier is often a lifetime.
        if quote == '\'' && !source.starts_with('\'') {
            return None;
        }
    }
    None
}

fn tokenize(mut source: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    while !source.is_empty() {
        let ch = source.chars().next().unwrap();
        if ch.is_whitespace() {
            source = source.trim_start();
        } else if source.starts_with("//") {
            source = source.split_once('\n').map_or("", |(_, rest)| rest);
        } else if source.starts_with("/*") {
            source = block_comment_end(source);
        } else if let Some((value, rest)) = raw_literal(source) {
            tokens.push(Token::Literal(value.to_owned()));
            source = rest;
        } else if let Some((value, rest)) = source
            .strip_prefix('"')
            .or_else(|| source.strip_prefix("b\""))
            .or_else(|| source.strip_prefix("c\""))
            .and_then(|content| quoted_literal(content, '"'))
        {
            tokens.push(Token::Literal(value));
            source = rest;
        } else if let Some((value, rest)) = source
            .strip_prefix('\'')
            .or_else(|| source.strip_prefix("b'"))
            .and_then(|content| quoted_literal(content, '\''))
        {
            tokens.push(Token::Literal(value));
            source = rest;
        } else if word(ch) {
            let ident = source
                .strip_prefix("r#")
                .filter(|rest| rest.starts_with(word))
                .unwrap_or(source);
            let end = ident.find(|next| !word(next)).unwrap_or(ident.len());
            tokens.push(Token::Code(&ident[..end]));
            source = &ident[end..];
        } else {
            let width = if source.starts_with("::") {
                2
            } else {
                ch.len_utf8()
            };
            tokens.push(Token::Code(&source[..width]));
            source = &source[width..];
        }
    }
    tokens
}

fn constant_string<'a>(tokens: &'a [Token<'a>]) -> Option<(String, &'a [Token<'a>])> {
    match tokens {
        [Token::Literal(value), rest @ ..] => Some((value.clone(), rest)),
        [
            Token::Code("concat"),
            Token::Code("!"),
            Token::Code(open @ ("(" | "[" | "{")),
            tail @ ..,
        ] => {
            let close = match *open {
                "(" => ")",
                "[" => "]",
                _ => "}",
            };
            let mut rest = tail;
            let mut value = String::new();
            loop {
                if let [Token::Code(end), after @ ..] = rest
                    && *end == close
                {
                    return Some((value, after));
                }
                let (piece, after) = constant_string(rest)?;
                value.push_str(&piece);
                rest = match after {
                    [Token::Code(","), next @ ..] => next,
                    [Token::Code(end), next @ ..] if *end == close => return Some((value, next)),
                    _ => return None,
                };
            }
        }
        _ => None,
    }
}

fn has(tokens: &[Token<'_>], pattern: &[&str]) -> bool {
    tokens.windows(pattern.len()).any(|window| {
        window
            .iter()
            .zip(pattern)
            .all(|(token, wanted)| token == &Token::Code(wanted))
    })
}

const RECOVERY: &str = "linux_secret_service_recovery.rs";

fn inspect(source: &str, recovery: bool) -> Result<(), &'static str> {
    let tokens = tokenize(source);
    let names = [["Un", "lock"].concat(), ["Pro", "mpt"].concat()];
    for name in names {
        let count = (0..tokens.len())
            .filter(|&at| constant_string(&tokens[at..]).is_some_and(|(value, _)| value == name))
            .count();
        if count != usize::from(recovery) {
            return Err("unexpected interactive method literal");
        }
    }
    if recovery {
        for pattern in [
            &["NativeInteractionGate", "{"][..],
            &["NativeInteractionGate", "::"],
            &["AllowPrompt", "{"],
            &["ForbidPrompt", "{"],
            &["::", "mint", "("],
            &[".", "access"],
            &[".", "stalled"],
            &[".", "lease"],
            &[".", "queued_waiters"],
            &[".", "acquire", "("],
            &["latch_stalled", "("],
            &["is_stalled", "("],
            &["latch_commit_unknown", "("],
            &["latch_forbid_uncertainty", "("],
        ] {
            if has(&tokens, pattern) {
                return Err("sealed gate implementation detail");
            }
        }
        if !has(
            &tokens,
            &[
                "let",
                "gate",
                "=",
                "process_native_interaction_gate",
                "(",
                ")",
                ";",
            ],
        ) {
            return Err("recovery must use the process gate");
        }
    }
    Ok(())
}

fn scan(root: &Path) -> Result<(), (PathBuf, &'static str)> {
    let recovery = root.join(RECOVERY);
    assert!(recovery.is_file());
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                inspect(&fs::read_to_string(&path).unwrap(), path == recovery)
                    .map_err(|reason| (path, reason))?;
            }
        }
    }
    Ok(())
}

fn adapter_directory() -> PathBuf {
    let source = Path::new(file!());
    let source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(source)
    };
    source.parent().unwrap().to_path_buf()
}

#[test]
fn recovery_method_and_gate_source_inventory_is_closed() {
    scan(&adapter_directory()).unwrap();
}

#[test]
fn inventory_rejects_scratch_source_bypasses() {
    let scratch = super::Bus::configure();
    let root = &scratch.root;
    let child = root.join(RECOVERY);
    let trusted = fs::read_to_string(adapter_directory().join(RECOVERY)).unwrap();
    fs::write(&child, &trusted).unwrap();
    let nested = root.join("other_adapter");
    fs::create_dir(&nested).unwrap();
    let outsider = nested.join("unrelated.rs");
    fs::write(&outsider, "// ordinary module").unwrap();
    scan(root).unwrap();
    // Build these source strings without planting the bypass in our own AST.
    let concatenation = format!("concat!({}, {})", "\"Un\"", "\"lock\"");
    let escaped = format!("\"{}{}\"", "\\x50", "rompt");
    let unicode = format!("\"{}{}\"", "\\u{55}", "nlock");
    for snippet in [concatenation, escaped, unicode] {
        fs::write(&outsider, format!("fn bypass() {{ let _ = {snippet}; }}")).unwrap();
        assert_eq!(
            scan(root),
            Err((outsider.clone(), "unexpected interactive method literal"))
        );
    }
    fs::write(&outsider, "// ordinary module").unwrap();
    for member in ["acquire", "latch_stalled", "is_stalled"] {
        fs::write(
            &child,
            format!("{trusted}\nfn bypass() {{ gate./* gap */r#{member}(); }}"),
        )
        .unwrap();
        assert_eq!(
            scan(root),
            Err((child.clone(), "sealed gate implementation detail"))
        );
    }
}
