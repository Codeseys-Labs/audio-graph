//! Accept a piped credential only. Use a hidden prompt in the caller's terminal.
use audio_graph::credentials::save_typesafe_credential_from_reader;
use std::io::IsTerminal;

fn main() {
    let stdin = std::io::stdin();
    let saved = std::env::args_os().len() == 1
        && !stdin.is_terminal()
        && save_typesafe_credential_from_reader(stdin.lock());
    println!("{}", if saved { "saved" } else { "missing" });
    if !saved {
        std::process::exit(1);
    }
}
