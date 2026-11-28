//! The one definition of which part of a model id may be stored or shown.

/// Weight-file extensions: what a local server's model id ends in when the id
/// is the path of the file it loaded.
const MODEL_FILE_EXTENSIONS: [&str; 8] = [
    "gguf",
    "ggml",
    "bin",
    "safetensors",
    "onnx",
    "pt",
    "pth",
    "ckpt",
];

/// The part of a model id that may be persisted: never a location on someone's
/// machine.
///
/// A local or self-hosted server (llama.cpp server, vLLM) reports the path it
/// loaded as its model id, and that same path is what the user configures as
/// the model to request. A path names a user and a directory layout; a record
/// needs the model only. The test is on shape, never on meaning: a leading
/// `/`, `~` or `.` segment, a drive prefix, a backslash, or a directory in
/// front of a weight file. Such a value is cut down to its final component. An
/// `org/model` id has none of those marks and comes back whole.
pub fn without_path(raw: &str) -> &str {
    let id = raw.trim();
    if !is_path_shaped(id) {
        return id;
    }
    let name = id
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    // A home marker, a dot segment or a bare drive names no model.
    let bare_drive = name.len() == 2 && name.ends_with(':');
    if bare_drive || matches!(name, "~" | "." | "..") {
        ""
    } else {
        name
    }
}

fn is_path_shaped(id: &str) -> bool {
    let drive_prefix = matches!(
        id.as_bytes(),
        [drive, b':', b'/' | b'\\', ..] if drive.is_ascii_alphabetic()
    );
    let names_a_weight_file = id.rsplit_once('.').is_some_and(|(_, extension)| {
        MODEL_FILE_EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    });
    id.starts_with(['/', '~'])
        || id.starts_with("./")
        || id.starts_with("../")
        || id.contains('\\')
        || drive_prefix
        || (id.contains('/') && names_a_weight_file)
}

#[cfg(test)]
mod tests {
    use super::without_path;

    #[test]
    fn a_model_id_keeps_no_path() {
        for (raw, kept) in [
            // Ordinary ids come back whole.
            ("anthropic/claude-sonnet-4.5", "anthropic/claude-sonnet-4.5"),
            (
                "accounts/fireworks/models/llama-v3p1-8b-instruct",
                "accounts/fireworks/models/llama-v3p1-8b-instruct",
            ),
            ("llama3.2:latest", "llama3.2:latest"),
            (
                "hf.co/bartowski/Llama-3.2-1B-Instruct-GGUF:Q4_K_M",
                "hf.co/bartowski/Llama-3.2-1B-Instruct-GGUF:Q4_K_M",
            ),
            ("private.gguf", "private.gguf"),
            // POSIX paths.
            ("/home/numbat-user/models/private.gguf", "private.gguf"),
            ("/models/mistral-7b/", "mistral-7b"),
            ("~/models/private.gguf", "private.gguf"),
            ("./numbat-user/private", "private"),
            ("../numbat-user/private", "private"),
            ("numbat-user/private.GGUF", "private.GGUF"),
            // Windows paths.
            (
                "C:\\Users\\numbat-user\\models\\private.gguf",
                "private.gguf",
            ),
            ("C:/Users/numbat-user/models/private.gguf", "private.gguf"),
            (
                "\\\\numbat-host\\share\\private.safetensors",
                "private.safetensors",
            ),
            ("numbat-user\\private", "private"),
            // Nothing that names a model is left.
            ("/", ""),
            ("~", ""),
            ("C:\\", ""),
            ("../", ""),
        ] {
            assert_eq!(without_path(raw), kept, "{raw}");
            assert_eq!(without_path(kept), kept, "{kept} is already cut");
        }
    }
}
