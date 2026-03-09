//! Wallet-toolbox-specific validation helpers.
//!
//! Most argument validation uses `bsv::wallet::validation::validate_*` functions
//! directly from the bsv-sdk. This file only adds validation helpers that are
//! specific to the wallet-toolbox layer and not present in the SDK.

use crate::error::WalletError;

/// Validates the `originator` parameter for wallet interface methods.
///
/// The originator must be non-empty and no more than 250 characters if provided.
/// Returns `Ok(())` if `originator` is `None` (optional parameter).
///
/// # Errors
///
/// Returns `WalletError::InvalidParameter` if the originator string is empty
/// or exceeds 250 characters.
pub fn validate_originator(originator: Option<&str>) -> Result<(), WalletError> {
    if let Some(orig) = originator {
        if orig.is_empty() {
            return Err(WalletError::InvalidParameter {
                parameter: "originator".to_string(),
                must_be: "a non-empty string".to_string(),
            });
        }
        if orig.len() > 250 {
            return Err(WalletError::InvalidParameter {
                parameter: "originator".to_string(),
                must_be: "no more than 250 characters".to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_originator_none_is_ok() {
        assert!(validate_originator(None).is_ok());
    }

    #[test]
    fn test_validate_originator_valid() {
        assert!(validate_originator(Some("my-app")).is_ok());
    }

    #[test]
    fn test_validate_originator_empty_rejected() {
        let result = validate_originator(Some(""));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_originator_too_long_rejected() {
        let long = "x".repeat(251);
        let result = validate_originator(Some(&long));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_originator_250_chars_ok() {
        let exact = "x".repeat(250);
        assert!(validate_originator(Some(&exact)).is_ok());
    }
}
