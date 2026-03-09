//! Error helper functions for checking action results.
//!
//! Provides helpers that inspect CreateActionResult, SignActionResult, and
//! InternalizeActionResult for unsuccessful outcomes and return descriptive
//! errors. Ports the TS `throwIfAnyUnsuccessful*` pattern.

use bsv::wallet::error::WalletError;
use bsv::wallet::interfaces::{
    ActionResultStatus, CreateActionResult, InternalizeActionResult, SignActionResult,
};

/// Check all `send_with_results` in a `CreateActionResult`.
///
/// If any result has a status other than `Unproven`, returns a `WalletError::Internal`
/// with details about the unsuccessful results.
pub fn throw_if_any_unsuccessful_create_actions(
    r: &CreateActionResult,
) -> Result<(), WalletError> {
    let failures: Vec<String> = r
        .send_with_results
        .iter()
        .filter(|swr| !matches!(swr.status, ActionResultStatus::Unproven))
        .map(|swr| format!("txid={} status={}", swr.txid, swr.status.as_str()))
        .collect();

    if !failures.is_empty() {
        return Err(WalletError::Internal(format!(
            "WERR_REVIEW_ACTIONS: createAction had unsuccessful results: {}",
            failures.join(", ")
        )));
    }
    Ok(())
}

/// Check all `send_with_results` in a `SignActionResult`.
///
/// If any result has a status other than `Unproven`, returns a `WalletError::Internal`
/// with details about the unsuccessful results.
pub fn throw_if_any_unsuccessful_sign_actions(r: &SignActionResult) -> Result<(), WalletError> {
    let failures: Vec<String> = r
        .send_with_results
        .iter()
        .filter(|swr| !matches!(swr.status, ActionResultStatus::Unproven))
        .map(|swr| format!("txid={} status={}", swr.txid, swr.status.as_str()))
        .collect();

    if !failures.is_empty() {
        return Err(WalletError::Internal(format!(
            "WERR_REVIEW_ACTIONS: signAction had unsuccessful results: {}",
            failures.join(", ")
        )));
    }
    Ok(())
}

/// Check an `InternalizeActionResult` for success.
///
/// If `accepted` is false, returns a `WalletError::Internal`.
pub fn throw_if_unsuccessful_internalize_action(
    r: &InternalizeActionResult,
) -> Result<(), WalletError> {
    if !r.accepted {
        return Err(WalletError::Internal(
            "WERR_REVIEW_ACTIONS: internalizeAction was not accepted".to_string(),
        ));
    }
    Ok(())
}
