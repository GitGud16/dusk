//! `dusq`, the Dusk command line: `dusq compress` and `dusq extract-audio` (from M4).

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!(
        "dusq {}: no commands yet. `compress` and `extract-audio` arrive in milestone M4.",
        env!("CARGO_PKG_VERSION")
    );
    ExitCode::from(2)
}
