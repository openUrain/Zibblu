use crate::commands::{Desktop, Result as CommandResult};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{fs, io::Write, path::Path};
use tauri::State;

const TERMS: &str = include_str!("../windows-ai-terms.txt");
const ACCEPTED_FILE: &str = "accepted-ai-terms.txt";

#[derive(Serialize)]
pub(crate) struct AiTerms {
    text: &'static str,
    accepted: bool,
}

fn is_accepted(data: &Path) -> Result<bool> {
    match fs::read_to_string(data.join(ACCEPTED_FILE)) {
        Ok(accepted) => Ok(accepted.lines().eq(TERMS.lines())),
        Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData) => Ok(false),
        Err(error) => Err(error).context("Could not read AI terms acceptance"),
    }
}

pub(crate) fn require_accepted(data: &Path) -> Result<()> {
    ensure!(is_accepted(data)?, "Review and accept the current AI terms in AI settings before using AI.");
    Ok(())
}

#[tauri::command]
pub(crate) fn ai_terms(state: State<'_, Desktop>) -> CommandResult<AiTerms> {
    Ok(AiTerms {
        text: TERMS.trim_start_matches('\u{feff}'),
        accepted: is_accepted(state.data_dir())?,
    })
}

#[tauri::command]
pub(crate) fn ai_accept_terms(state: State<'_, Desktop>, text: String) -> CommandResult<()> {
    let result = (|| -> Result<()> {
        ensure!(text == TERMS.trim_start_matches('\u{feff}'), "The AI terms changed. Review the current version.");
        let mut file = tempfile::NamedTempFile::new_in(state.data_dir())?;
        file.write_all(TERMS.as_bytes())?;
        file.persist(state.data_dir().join(ACCEPTED_FILE))?;
        Ok(())
    })();
    Ok(result.context("Could not record AI terms acceptance")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_old_terms_fail_closed() -> Result<()> {
        let profile = tempfile::tempdir()?;
        assert!(TERMS.contains("Version 2"));
        assert!(require_accepted(profile.path()).is_err());
        let accepted = profile.path().join(ACCEPTED_FILE);
        fs::write(&accepted, "Zibblu Windows AI Terms and Privacy Notice · Version 1\n")?;
        assert!(require_accepted(profile.path()).is_err());
        fs::write(accepted, TERMS)?;
        require_accepted(profile.path())
    }
}
