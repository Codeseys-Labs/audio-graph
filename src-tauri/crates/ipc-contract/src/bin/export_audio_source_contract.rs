use std::{env, error::Error, ffi::OsStr, fs, path::PathBuf};

fn default_output_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../src/generated/audioSource.ts")
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    let check = first.as_deref() == Some(OsStr::new("--check"));
    let output_path = if check {
        args.next()
            .map(PathBuf::from)
            .unwrap_or_else(default_output_path)
    } else {
        first.map(PathBuf::from).unwrap_or_else(default_output_path)
    };

    if let Some(extra) = args.next() {
        return Err(format!(
            "unexpected extra argument: {}",
            PathBuf::from(extra).display()
        )
        .into());
    }

    let expected = audio_graph_ipc_contract::audio_source_contract_typescript_module();

    if check {
        let actual = fs::read_to_string(&output_path)?;
        if actual != expected {
            return Err(format!(
                "audio source contract drift detected in {}; run `bun run generate:audio-source-contract`",
                output_path.display()
            )
            .into());
        }
        println!("audio source contract is current: {}", output_path.display());
        return Ok(());
    }

    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    fs::write(&output_path, expected)?;
    println!("wrote {}", output_path.display());

    Ok(())
}
