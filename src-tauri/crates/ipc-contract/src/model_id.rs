//! What may be stored or shown for a model. Where the model ran decides, not
//! what its identifier looks like.
//!
//! - A model at a remote endpoint keeps the provider's hosted identifier.
//! - A model that ran on this machine, or at an endpoint the app classes as
//!   local, gets a display name and never its identifier. A local server
//!   reports the path it loaded as its model id, and the same path is what
//!   the user configures as the model to request. A path names a user and a
//!   directory layout.
//! - A stored record that cannot say where its model ran is read by the
//!   local rule. A hosted id that loses its namespace is cosmetic; a path
//!   that survives is a leak.
//!
//! No rule here looks at the shape of the identifier to guess whether it is a
//! path: `numbat/checkpoint` and `org/model` are the same shape.

/// What stands in for a local model whose identifier leaves no name to show:
/// a bare root, a home marker, or anything that still begins with a drive.
///
/// The last case covers what older builds stored for a Windows path. They
/// dropped its backslashes before persisting it, so `C:\Users\name\m.gguf`
/// was stored as `C:Usersnamem.gguf`, which cannot be cut back to a file name.
pub const LOCAL_MODEL_PLACEHOLDER: &str = "local-model";

/// What to store or show for a model, given where it ran.
pub fn for_place(id: &str, ran_remotely: bool) -> &str {
    if ran_remotely {
        id
    } else {
        local_display_name(id)
    }
}

/// The display name of a local model: what follows the last separator of its
/// identifier, with a `file:` scheme and trailing separators removed first.
///
/// The result is a part of `id`, never trimmed or otherwise altered, or
/// [`LOCAL_MODEL_PLACEHOLDER`]. An empty id stays empty. Applying it again
/// changes nothing.
pub fn local_display_name(id: &str) -> &str {
    if id.is_empty() {
        return id;
    }
    let mut name = id;
    loop {
        let unschemed = match name.get(..5) {
            Some(scheme) if scheme.eq_ignore_ascii_case("file:") => &name[5..],
            _ => name,
        };
        let last = unschemed
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default();
        if last.len() == name.len() {
            break;
        }
        name = last;
    }
    let drive_prefixed =
        matches!(name.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic());
    if drive_prefixed || matches!(name, "" | "~" | "." | "..") {
        LOCAL_MODEL_PLACEHOLDER
    } else {
        name
    }
}

/// [`for_place`] over an owned identifier, reusing it when nothing is cut.
pub fn owned_for_place(id: String, ran_remotely: bool) -> String {
    let kept = for_place(&id, ran_remotely);
    if kept.len() == id.len() && kept.as_ptr() == id.as_ptr() {
        id
    } else {
        kept.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{LOCAL_MODEL_PLACEHOLDER, for_place, local_display_name};

    /// (identifier, what a local model shows). A remote model keeps every one
    /// of these whole.
    const CASES: [(&str, &str); 22] = [
        // Paths a local server reports or a user configures.
        ("/home/numbat/models/private.gguf", "private.gguf"),
        ("/models/mistral-7b/", "mistral-7b"),
        ("~/models/private.gguf", "private.gguf"),
        ("./numbat/private", "private"),
        ("C:\\Users\\numbat\\models\\private.gguf", "private.gguf"),
        ("C:/Users/numbat/models/private.gguf", "private.gguf"),
        (
            "\\\\numbat-host\\share\\private.safetensors",
            "private.safetensors",
        ),
        // The four that survived a shape test.
        ("numbat/checkpoint", "checkpoint"),
        ("numbat/private.gguf/", "private.gguf"),
        ("C:numbat/checkpoint", "checkpoint"),
        ("file:///home/numbat/checkpoint", "checkpoint"),
        // A name is not trimmed.
        ("/models/ private.gguf", " private.gguf"),
        // What older builds stored for a Windows path, and other values that
        // leave no name.
        ("C:Usersnumbatmodelsprivate.gguf", LOCAL_MODEL_PLACEHOLDER),
        ("C:private.gguf", LOCAL_MODEL_PLACEHOLDER),
        ("C:\\", LOCAL_MODEL_PLACEHOLDER),
        ("/", LOCAL_MODEL_PLACEHOLDER),
        ("~", LOCAL_MODEL_PLACEHOLDER),
        // Hosted identifiers, which a local endpoint can serve too.
        ("org/model.gguf", "model.gguf"),
        ("org/repo/model.safetensors", "model.safetensors"),
        (
            "accounts/fireworks/models/llama-v3p1-8b-instruct",
            "llama-v3p1-8b-instruct",
        ),
        ("org/model@rev1", "model@rev1"),
        ("llama3.2:latest", "llama3.2:latest"),
    ];

    #[test]
    fn a_local_model_shows_a_name_and_a_remote_one_keeps_its_identifier() {
        for (id, shown) in CASES {
            assert_eq!(local_display_name(id), shown, "{id}");
            assert_eq!(for_place(id, false), shown, "{id}");
            assert_eq!(for_place(id, true), id, "{id}");
            assert_eq!(
                local_display_name(shown),
                shown,
                "{shown} is already a display name"
            );
            assert!(!shown.contains(['/', '\\']), "{shown}");
            assert!(!shown.contains("numbat"), "{shown}");
        }
    }

    #[test]
    fn a_display_name_is_a_fixed_point_even_through_a_nested_scheme() {
        for id in ["numbat/file:private.gguf", "file:file:/numbat/private.gguf"] {
            let shown = local_display_name(id);
            assert_eq!(shown, "private.gguf", "{id}");
            assert_eq!(local_display_name(shown), shown);
        }
    }
}
