//! Application BA scope, independent of frame sampling and training recipes.
//! Missing versions retain the old per-route scope, not today's defaults.
use crate::error::{Result, SplatError};

pub const VERSION: u32 = 1;

pub fn shared_scope(version: Option<u32>) -> Result<bool> {
    match version {
        None => Ok(false),
        Some(VERSION) => Ok(true),
        Some(_) => Err(SplatError::UnsupportedEngine(
            "Unsupported saved bundle-adjustment policy".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_is_opt_in_for_saved_work_and_never_guesses_future_versions() {
        assert!(!shared_scope(None).unwrap());
        assert!(shared_scope(Some(VERSION)).unwrap());
        assert!(shared_scope(Some(0)).is_err());
        assert!(shared_scope(Some(VERSION + 1)).is_err());
    }
}
