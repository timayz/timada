use std::io::{BufRead, Write};

use anyhow::{Context, Result};

/// Reads one line, the password, on the standard input and writes its hash
/// on the standard output: the value of `auth.root.password_hash`. Read
/// rather than taken as an argument, which the shell's history and the
/// process list would keep.
pub async fn hash_password() -> Result<()> {
    let mut password = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut password)
        .context("cannot read the password on the standard input")?;
    let password = password.trim_end_matches(['\n', '\r']);
    let hash = timada_identity::hash_password(password).await?;
    writeln!(std::io::stdout(), "{hash}").context("cannot write the hash")?;
    Ok(())
}
