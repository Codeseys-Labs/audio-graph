use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
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

const ADAPTERS: &str = "credentials/adapters";
const RECOVERY: &str = "credentials/adapters/linux_secret_service_recovery.rs";
const WIRE: &str = "credentials/adapters/linux_secret_service_wire.rs";
const SEALED: &str = "credentials/adapters/native_interaction.rs";

fn starts(tokens: &[Token<'_>], words: &[&str]) -> bool {
    tokens.len() >= words.len()
        && tokens
            .iter()
            .zip(words)
            .all(|(token, word)| *token == Token::Code(word))
}

fn skip_group(tokens: &[Token<'_>], open: &str, close: &str) -> usize {
    let mut depth = 0;
    for (at, token) in tokens.iter().enumerate() {
        if *token == Token::Code(open) {
            depth += 1;
        } else if *token == Token::Code(close) {
            depth -= 1;
            if depth == 0 {
                return at + 1;
            }
        }
    }
    tokens.len()
}

fn production(tokens: Vec<Token<'_>>) -> Vec<Token<'_>> {
    let mut result = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        let tail = &tokens[at..];
        if starts(tail, &["#", "[", "cfg", "(", "test", ")", "]"]) {
            let mut after = 7;
            while starts(&tail[after..], &["#", "["]) {
                after += 1 + skip_group(&tail[after + 1..], "[", "]");
            }
            if starts(&tail[after..], &["pub"]) {
                after += 1;
                if starts(&tail[after..], &["("]) {
                    after += skip_group(&tail[after..], "(", ")");
                }
            }
            if starts(&tail[after..], &["use"])
                && let Some(end) = tail[after..].iter().position(|t| *t == Token::Code(";"))
            {
                at += after + end + 1;
                continue;
            }
            if (starts(&tail[after..], &["mod"]) || starts(&tail[after..], &["struct"]))
                && let Some(body) = tail[after..]
                    .iter()
                    .position(|t| matches!(t, Token::Code("{" | ";")))
            {
                after += body;
                at += if tail[after] == Token::Code("{") {
                    after + skip_group(&tail[after..], "{", "}")
                } else {
                    after + 1
                };
                continue;
            }
        }
        result.push(tokens[at].clone());
        at += 1;
    }
    result
}

fn interactive(value: &str) -> bool {
    matches!(value, "Unlock" | "Prompt")
}

fn byte_value(token: &Token<'_>) -> Option<u8> {
    match token {
        Token::Code(number) => {
            let number = number.trim_end_matches("u8").replace('_', "");
            if let Some(hex) = number.strip_prefix("0x") {
                u8::from_str_radix(hex, 16).ok()
            } else if let Some(octal) = number.strip_prefix("0o") {
                u8::from_str_radix(octal, 8).ok()
            } else if let Some(binary) = number.strip_prefix("0b") {
                u8::from_str_radix(binary, 2).ok()
            } else {
                number.parse().ok()
            }
        }
        Token::Literal(ch) if ch.len() == 1 => Some(ch.as_bytes()[0]),
        _ => None,
    }
}

fn literal_array<T>(
    tokens: &[Token<'_>],
    parse: impl Fn(&Token<'_>) -> Option<T>,
) -> Option<(Vec<T>, usize)> {
    let mut items = Vec::new();
    let mut tail = tokens.strip_prefix(&[Token::Code("[")])?;
    loop {
        if starts(tail, &["]"]) {
            return Some((items, tokens.len() - tail.len() + 1));
        }
        items.push(parse(tail.first()?)?);
        tail = &tail[1..];
        if starts(tail, &[","]) {
            tail = &tail[1..];
        } else if !starts(tail, &["]"]) {
            return None;
        }
    }
}

fn inspect_methods(tokens: &[Token<'_>], source: &str, path: &str) -> Result<(), &'static str> {
    let recovery = path == RECOVERY;
    for method in ["Unlock", "Prompt"] {
        let count = (0..tokens.len())
            .filter(|&at| constant_string(&tokens[at..]).is_some_and(|(value, _)| value == method))
            .count();
        if count != usize::from(recovery) || (!path.ends_with(".rs") && source.trim() == method) {
            return Err("unexpected interactive method literal");
        }
    }
    let from_utf8 = tokens.iter().any(|token| {
        matches!(
            token,
            Token::Code("from_utf8" | "from_utf8_unchecked" | "from_utf8_lossy")
        )
    });
    for at in 0..tokens.len() {
        let tail = &tokens[at..];
        if starts(tail, &["stringify", "!"])
            && matches!(tail.get(2), Some(Token::Code("(" | "[" | "{")))
            && matches!(tail.get(3), Some(Token::Code(name)) if interactive(name))
        {
            return Err("interactive method from stringify");
        }
        if let Some((parts, length)) = literal_array(tail, |token| match token {
            Token::Literal(value) => Some(value.clone()),
            _ => None,
        }) && starts(&tail[length..], &[".", "concat", "("])
            && interactive(&parts.concat())
        {
            return Err("interactive method from array concat");
        }
        if from_utf8
            && let Some((bytes, _)) = literal_array(tail, byte_value)
            && std::str::from_utf8(&bytes).is_ok_and(interactive)
        {
            return Err("interactive method from byte array");
        }
    }
    Ok(())
}

fn recovery_source(path: &str) -> bool {
    path == RECOVERY || path.starts_with("credentials/adapters/linux_secret_service_recovery/")
}

fn inspect_gate(tokens: &[Token<'_>], path: &str) -> Result<(), &'static str> {
    if path == SEALED {
        return Ok(());
    }
    let aliases_gate = tokens
        .split(|token| *token == Token::Code(";"))
        .any(|clause| {
            has(clause, &["type"]) && has(clause, &["="]) && has(clause, &["NativeInteractionGate"])
        });
    if has(tokens, &["NativeInteractionGate", "as"]) || aliases_gate {
        return Err("sealed gate type alias");
    }
    if has(tokens, &["NativeInteractionGate", ">", "::"]) {
        return Err("sealed gate qualified path");
    }
    for at in 0..tokens.len() {
        if !matches!(tokens[at], Token::Code("{")) {
            continue;
        }
        let field = tokens.get(at + 1);
        if at != 0
            && matches!(tokens[at - 1], Token::Code(owner) if owner.chars().next().is_some_and(char::is_uppercase))
            && (matches!(
                tokens[at - 1],
                Token::Code("NativeInteractionGate" | "Gate" | "AllowPrompt" | "ForbidPrompt")
            ) || matches!(field, Some(Token::Code("stalled" | "queued_waiters"))))
        {
            return Err("sealed gate construction or destructuring");
        }
    }
    for member in [
        "acquire",
        "latch_stalled",
        "is_stalled",
        "latch_commit_unknown",
        "latch_forbid_uncertainty",
    ] {
        if has(tokens, &["::", member]) {
            return Err("reference to sealed gate method");
        }
    }
    // WIRE also supplies the generated interactive transport.
    if recovery_source(path) || path == WIRE {
        for field in ["access", "stalled", "lease", "queued_waiters"] {
            if has(tokens, &[".", field]) {
                return Err("sealed gate implementation detail");
            }
        }
        for member in [
            "mint",
            "acquire",
            "latch_stalled",
            "is_stalled",
            "latch_commit_unknown",
            "latch_forbid_uncertainty",
        ] {
            if has(tokens, &[".", member]) {
                return Err("sealed gate implementation detail");
            }
        }
    }
    if path == RECOVERY
        && !has(
            tokens,
            &[
                "let",
                "gate",
                "=",
                "process_native_interaction_gate",
                "(",
                ")",
                ";",
            ],
        )
    {
        return Err("recovery must use the process gate");
    }
    Ok(())
}

fn test_source(path: &str) -> bool {
    matches!(
        path,
        "credentials/adapters/linux_secret_service_recovery/tests.rs"
            | "credentials/adapters/linux_secret_service_wire/crypto/tests.rs"
            | "credentials/adapters/linux_secret_service_wire/linux/tests.rs"
            | "credentials/adapters/linux_secret_service_wire/linux/tests/mutation_tests.rs"
            | "credentials/adapters/linux_secret_service_wire/linux/tests/qualification_tests.rs"
            | "credentials/adapters/linux_secret_service_wire/linux/tests/qualification_tests/inventory.rs"
    )
}

fn inspect(source: &str, path: &str) -> Result<(), &'static str> {
    if test_source(path) {
        return Ok(());
    }
    let tokens = production(tokenize(source));
    if path != WIRE {
        if has(&tokens, &["#", "[", "zbus", "::", "proxy"]) {
            return Err("raw D-Bus proxy attribute");
        }
        for name in ["call_method", "call_with_flags", "Proxy", "ProxyBuilder"] {
            if has(&tokens, &[name]) {
                return Err("raw D-Bus call surface");
            }
        }
        for pattern in [
            &["Message", "::", "method_call"][..],
            &["Connection", "::", "session"],
        ] {
            if has(&tokens, pattern) {
                return Err("raw D-Bus call surface");
            }
        }
        if has(&tokens, &["zbus"]) || has(&tokens, &["send_flagged"]) {
            return Err("raw D-Bus crate or send macro outside wire");
        }
        if path != RECOVERY && has(&tokens, &["define_recovery_peer"]) {
            return Err("interactive peer outside recovery");
        }
    }
    if !path.starts_with("credentials/") {
        return Ok(());
    }
    for include in ["include", "include_str", "include_bytes"] {
        if has(&tokens, &[include, "!"]) {
            return Err("production credential source include");
        }
    }
    for (at, _) in tokens
        .iter()
        .enumerate()
        .filter(|(at, _)| starts(&tokens[*at..], &["#", "["]))
    {
        let end = at + 1 + skip_group(&tokens[at + 1..], "[", "]");
        let attribute = &tokens[at..end];
        if !has(attribute, &["path", "="]) {
            continue;
        }
        if path == SEALED
            && matches!(attribute, [Token::Code("#"), Token::Code("["), Token::Code("path"),
                Token::Code("="), Token::Literal(value), Token::Code("]")]
                if value == "../../linux_secret_service_recovery.rs")
            && starts(
                &tokens[end..],
                &["mod", "linux_secret_service_recovery", ";"],
            )
        {
            continue;
        }
        return Err("production credential source path override");
    }
    if matches!(
        path,
        RECOVERY | WIRE | "credentials/adapters/linux_secret_service_wire/crypto.rs"
    ) && has(&tokens, &["mod", "tests", ";"])
    {
        return Err("scripted peer must remain test-only");
    }
    // The sealed path redirects the recovery module. Rust can resolve an
    // out-of-line child beside that redirected file, outside its namesake
    // directory. Require production recovery descendants to be lexical.
    if recovery_source(path)
        && tokens
            .windows(3)
            .any(|item| matches!(item, [Token::Code("mod"), Token::Code(_), Token::Code(";")]))
    {
        return Err("out-of-line recovery child");
    }
    inspect_methods(&tokens, source, path)?;
    inspect_gate(&tokens, path)
}

fn scan(root: &Path) -> Result<(), (PathBuf, &'static str)> {
    assert!(root.join(RECOVERY).is_file());
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            let credentials = relative == "credentials" || relative.starts_with("credentials/");
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                directories.push(path);
            } else if credentials || path.extension().is_some_and(|ext| ext == "rs") {
                if kind.is_symlink() {
                    return Err((path, "source symlink is outside the inventory"));
                }
                let source = fs::read_to_string(&path)
                    .map_err(|_| (path.clone(), "source is outside the text inventory"))?;
                inspect(&source, &relative).map_err(|reason| (path, reason))?;
            }
        }
    }
    Ok(())
}

fn source_directory() -> PathBuf {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(file!())
        .canonicalize()
        .unwrap();
    source
        .ancestors()
        .find(|path| path.ends_with("src"))
        .unwrap()
        .to_path_buf()
}

#[test]
fn recovery_method_and_gate_source_inventory_is_closed() {
    scan(&source_directory()).unwrap();
}

#[test]
fn inventory_rejects_scratch_source_bypasses() {
    let scratch = super::Bus::configure();
    let root = scratch.root.join("src");
    let child = root.join(RECOVERY);
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    let trusted = fs::read_to_string(source_directory().join(RECOVERY)).unwrap();
    fs::write(&child, &trusted).unwrap();
    let source = "credentials/bypass.rs";
    let nested = "credentials/adapters/linux_secret_service_recovery/alternate.rs";
    let literal = "unexpected interactive method literal";
    let gate = "sealed gate implementation detail";
    let valid = root.join(source);
    for snippet in [
        "#[cfg(test)] use zbus::{Connection, Message};",
        "fn f() { let _ = std::str::from_utf8(&[0o104, 0o151, 0o163, 0o155, 0o151, 0o163, 0o163]); }",
    ] {
        fs::write(&valid, snippet).unwrap();
        assert_eq!(scan(&root), Ok(()), "{snippet}");
    }
    fs::remove_file(valid).unwrap();
    let cases = [
        (
            source,
            "use zbus::{Connection, Message};",
            "raw D-Bus crate or send macro outside wire",
        ),
        (
            source,
            "fn f() { let _ = std::str::from_utf8(&[0o125,0o156,0o154,0o157,0o143,0o153]); }",
            "interactive method from byte array",
        ),
        (
            source,
            "fn f() { let _ = std::str::from_utf8(&[0b01010101,0b01101110,0b01101100,0b01101111,0b01100011,0b01101011]); }",
            "interactive method from byte array",
        ),
        (source, r#"const A: &str = concat!("Un","lock");"#, literal),
        (source, r#"const A: &str = "\x50rompt";"#, literal),
        (source, r#"const A: &str = "\u{55}nlock";"#, literal),
        (
            source,
            r#"fn f() { let bytes = [85,110,108,111,99,107]; let _ = std::str::from_utf8(&bytes); }"#,
            "interactive method from byte array",
        ),
        (
            source,
            r#"fn f() { let _ = ["Pro","mpt"].concat(); }"#,
            "interactive method from array concat",
        ),
        (
            source,
            "const A: &str = stringify!(Unlock);",
            "interactive method from stringify",
        ),
        (
            source,
            r#"const A: &str = include_str!("payload");"#,
            "production credential source include",
        ),
        (
            source,
            r#"const A: &[u8] = include_bytes!("payload");"#,
            "production credential source include",
        ),
        (
            source,
            r#"include!("payload");"#,
            "production credential source include",
        ),
        (
            source,
            r#"#[path = "../x.rs"] mod x;"#,
            "production credential source path override",
        ),
        (
            source,
            r#"#[cfg_attr(unix,path = "../x.rs")] mod x;"#,
            "production credential source path override",
        ),
        (
            "other.rs",
            r#"fn f(c: &zbus::Connection, m: &str) { let _ = c.call_method(None, "/", None, m, &()); }"#,
            "raw D-Bus call surface",
        ),
        (
            "other.rs",
            r#"#[zbus::proxy(interface = "org.freedesktop.Secret.Prompt")] trait Escape { fn prompt(&self); }"#,
            "raw D-Bus proxy attribute",
        ),
        (
            "other.rs",
            "extern crate zbus as bus;",
            "raw D-Bus crate or send macro outside wire",
        ),
        (
            RECOVERY,
            "mod second_child { fn f(g: &super::NativeInteractionGate) { let _ = g.acquire(); } }",
            gate,
        ),
        (
            RECOVERY,
            "mod second_child { fn f(g: &super::NativeInteractionGate) { g.latch_stalled(); } }",
            gate,
        ),
        (RECOVERY, "mod out_of_line;", "out-of-line recovery child"),
        (
            nested,
            "fn f(g: &super::NativeInteractionGate) { let _ = g.acquire(); }",
            gate,
        ),
        (
            nested,
            "fn f(g: &super::NativeInteractionGate) { g.latch_stalled(); }",
            gate,
        ),
        (
            nested,
            "fn f(g: &super::NativeInteractionGate) { g.is_stalled(); }",
            gate,
        ),
        (
            source,
            "use super::NativeInteractionGate as Gate;",
            "sealed gate type alias",
        ),
        (
            source,
            "fn f(g: &Gate) { let f = Gate::latch_stalled; f(g); }",
            "reference to sealed gate method",
        ),
        (
            source,
            "type Gate = super::NativeInteractionGate;",
            "sealed gate type alias",
        ),
        (
            source,
            "define_recovery_peer!();",
            "interactive peer outside recovery",
        ),
        (
            source,
            "fn f(g: &Gate) { let _ = <super::NativeInteractionGate>::is_stalled(g); }",
            "sealed gate qualified path",
        ),
        (
            source,
            "fn f(g: Gate) { let Gate { stalled, .. } = g; }",
            "sealed gate construction or destructuring",
        ),
        ("credentials/payload.txt", "Unlock", literal),
    ];
    // Rust's lexical module lookup must not make an alternate recovery child
    // exempt from the same gate rules. Each case adds exactly one source.
    fs::create_dir_all(root.join(ADAPTERS).join("linux_secret_service_recovery")).unwrap();
    scan(&root).unwrap();
    for (path, snippet, reason) in cases {
        let file = root.join(path);
        if path == RECOVERY {
            fs::write(&file, format!("{trusted}\n{snippet}")).unwrap();
        } else {
            fs::write(&file, snippet).unwrap();
        }
        assert_eq!(scan(&root), Err((file.clone(), reason)), "{snippet}");
        if path == RECOVERY {
            fs::write(&file, &trusted).unwrap();
        } else {
            fs::remove_file(file).unwrap();
        }
        scan(&root).unwrap();
    }
}
