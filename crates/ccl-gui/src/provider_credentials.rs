//! Shared resolution for optional CurseForge provider credentials.

use crate::{GuiError, GuiResult};
use ccl_core::accounts::{AccountError, OsProviderKeyStore};
use std::path::Path;

/// Resolve the key used by provider operations. Offline mode always disables it;
/// otherwise a non-empty environment override wins, followed by the OS vault.
pub(crate) fn curseforge_key(config_path: &Path, offline: bool) -> GuiResult<Option<String>> {
    if offline {
        return Ok(None);
    }
    if let Some(value) = select_override(std::env::var_os("CCL_CURSEFORGE_API_KEY")) {
        return Ok(Some(value));
    }
    let store = OsProviderKeyStore::for_config_file(config_path).map_err(account_error)?;
    match store.get_curseforge() {
        Err(AccountError::SecureStorage(_)) => Ok(None),
        Ok(value) => Ok(value.filter(|value| !value.is_empty())),
        Err(error) => Err(account_error(error)),
    }
}

fn select_override(value: Option<std::ffi::OsString>) -> Option<String> {
    value
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
}

fn account_error(error: AccountError) -> GuiError {
    GuiError::new("account_error", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::select_override;

    #[test]
    fn empty_environment_override_is_absent() {
        assert_eq!(select_override(None), None);
        assert_eq!(select_override(Some("".into())), None);
        assert_eq!(
            select_override(Some("fixture-key".into())),
            Some("fixture-key".into())
        );
    }
}
