//! The name the program was called by: `flottille`, or `flot` through a link to it. Its help,
//! its errors and the commands it tells the user to run say that name (`docs/cli.md`).

use std::path::Path;
use std::sync::OnceLock;

const OWN_NAME: &str = "flottille";

/// The file name of `argv[0]`; `flottille` when the system gave none.
pub fn name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::args_os()
            .next()
            .as_deref()
            .and_then(|called| Path::new(called).file_name())
            .and_then(|file| file.to_str())
            .filter(|file| !file.is_empty())
            .map_or(OWN_NAME, |file| Box::leak(file.to_owned().into_boxed_str()))
    })
}
