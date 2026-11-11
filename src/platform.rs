//! What Janis asks of the operating system, resolved the same way everywhere.

use std::path::PathBuf;

/// The application's identifier, which names its per-user data directory.
pub const APPLICATION_IDENTIFIER: &str = "com.olivierbazin.janis";

/// The directory `janis.db` lives in: the per-user data directory of the
/// platform, under the application's identifier.
pub fn data_directory() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|directory| directory.join(APPLICATION_IDENTIFIER))
        .ok_or_else(|| "this system reports no per-user data directory".to_owned())
}
