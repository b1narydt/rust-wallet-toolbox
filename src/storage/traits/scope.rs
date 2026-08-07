//! Tenant scoping for the authenticated storage surface.
//!
//! The `*_auth` methods of [`WalletStorageProvider`] are the trust boundary
//! between an authenticated caller and a database that may hold rows for many
//! users. Every one of them must translate "who the transport authenticated"
//! into "which rows this query may touch". This module is the single place
//! where that translation happens:
//!
//! - [`resolve_user_scope`] is the only way to obtain a [`UserScope`]. It
//!   resolves `auth.identity_key` (the credential the transport verified)
//!   against the users table and rejects a mismatched `auth.user_id` claim
//!   with `WERR_UNAUTHORIZED`.
//! - [`UserScope::apply`] forces the tenant predicate onto a query filter,
//!   rejecting a caller-supplied foreign `user_id` rather than ignoring it.
//! - [`UserScope::check_owner`] does the same for insert paths where the row
//!   type carries a concrete `user_id` column.
//!
//! Holding a `UserScope` is proof both checks ran: the field is private and
//! the type has no other constructor. Lower layers (`StorageReader` and
//! friends) stay tenant-unscoped on purpose — the monitor sweeps every user's
//! transactions and `proven_txs` is deduplicated chain data shared across
//! users — so the boundary lives here, at the surface a remote caller reaches.
//!
//! TS parity: `StorageKnex.findCertificatesAuth` / `findOutputBasketsAuth` /
//! `findOutputsAuth` (StorageKnex.ts:714-730) force `partial.userId =
//! auth.userId` and throw `WERR_UNAUTHORIZED` on mismatch; the TS
//! `StorageServer.validateParam0` resolves `auth.userId` from the
//! authenticated identity key before dispatch. `resolve_user_scope` performs
//! both steps.
//!
//! [`WalletStorageProvider`]: crate::storage::traits::wallet_provider::WalletStorageProvider

use crate::error::{WalletError, WalletResult};
use crate::storage::find_args::{CertificatePartial, OutputBasketPartial, OutputPartial};
use crate::storage::traits::reader_writer::StorageReaderWriter;
use crate::wallet::types::AuthId;

/// Proof that an `AuthId` has been resolved to a storage user row.
///
/// Constructed only by [`resolve_user_scope`]; holding one means the
/// identity-key lookup and the `user_id` claim check both ran.
#[derive(Debug, Clone, Copy)]
pub struct UserScope {
    user_id: i64,
}

impl UserScope {
    /// The resolved tenant user id.
    pub fn user_id(&self) -> i64 {
        self.user_id
    }

    /// Force the tenant predicate onto a query filter.
    ///
    /// A caller-supplied `user_id` that is set, non-zero, and not the
    /// caller's own is rejected with `WERR_UNAUTHORIZED` (zero means "unset",
    /// matching TS StorageKnex.ts:715). Otherwise the filter's `user_id`
    /// becomes the caller's, so the SQL that runs always carries
    /// `user_id = <caller>`.
    pub fn apply<P: UserScopedPartial>(&self, partial: &mut P) -> WalletResult<()> {
        match *partial.user_id_mut() {
            Some(claimed) if claimed != 0 && claimed != self.user_id => {
                Err(WalletError::Unauthorized(format!(
                    "query user_id {claimed} does not match authenticated user {}",
                    self.user_id
                )))
            }
            _ => {
                *partial.user_id_mut() = Some(self.user_id);
                Ok(())
            }
        }
    }

    /// Validate an owner column for insert paths and return the value to
    /// store. Zero means "unset" and resolves to the caller; any other
    /// foreign value is `WERR_UNAUTHORIZED` (TS StorageKnex.ts:271).
    pub fn check_owner(&self, user_id: i64) -> WalletResult<i64> {
        if user_id != 0 && user_id != self.user_id {
            return Err(WalletError::Unauthorized(format!(
                "record user_id {user_id} does not match authenticated user {}",
                self.user_id
            )));
        }
        Ok(self.user_id)
    }
}

/// Query filters that carry the tenant column.
///
/// Implemented for the partials reachable from the `*_auth` wire methods so
/// [`UserScope::apply`] is the one path from an authenticated identity to a
/// tenant predicate.
pub trait UserScopedPartial {
    /// The `user_id` filter slot the tenant predicate is forced through.
    fn user_id_mut(&mut self) -> &mut Option<i64>;
}

impl UserScopedPartial for OutputPartial {
    fn user_id_mut(&mut self) -> &mut Option<i64> {
        &mut self.user_id
    }
}

impl UserScopedPartial for OutputBasketPartial {
    fn user_id_mut(&mut self) -> &mut Option<i64> {
        &mut self.user_id
    }
}

impl UserScopedPartial for CertificatePartial {
    fn user_id_mut(&mut self) -> &mut Option<i64> {
        &mut self.user_id
    }
}

/// Resolve an `AuthId` to a [`UserScope`].
///
/// The tenant is whoever holds `auth.identity_key` — the credential the
/// transport layer authenticated. `auth.user_id` is a claim, not a
/// credential: when present, non-zero, and different from the row resolved
/// for the identity key it is rejected with `WERR_UNAUTHORIZED`.
///
/// Uses find-or-insert semantics: an identity key never seen before becomes
/// a fresh user that owns nothing, so unknown callers see empty result sets
/// rather than an error (same behavior the list_* methods already have).
pub async fn resolve_user_scope<T: StorageReaderWriter + ?Sized>(
    storage: &T,
    auth: &AuthId,
) -> WalletResult<UserScope> {
    let (user, _) = storage.find_or_insert_user(&auth.identity_key, None).await?;
    if let Some(claimed) = auth.user_id {
        if claimed != 0 && claimed != user.user_id {
            return Err(WalletError::Unauthorized(format!(
                "auth user_id {claimed} does not match identity key owner {}",
                user.user_id
            )));
        }
    }
    Ok(UserScope {
        user_id: user.user_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(user_id: i64) -> UserScope {
        UserScope { user_id }
    }

    #[test]
    fn apply_forces_user_id_when_unset() {
        let mut partial = OutputPartial::default();
        scope(7).apply(&mut partial).unwrap();
        assert_eq!(partial.user_id, Some(7));
    }

    #[test]
    fn apply_treats_zero_as_unset() {
        let mut partial = OutputPartial {
            user_id: Some(0),
            ..Default::default()
        };
        scope(7).apply(&mut partial).unwrap();
        assert_eq!(partial.user_id, Some(7));
    }

    #[test]
    fn apply_keeps_own_user_id() {
        let mut partial = CertificatePartial {
            user_id: Some(7),
            ..Default::default()
        };
        scope(7).apply(&mut partial).unwrap();
        assert_eq!(partial.user_id, Some(7));
    }

    #[test]
    fn apply_rejects_foreign_user_id() {
        let mut partial = OutputBasketPartial {
            user_id: Some(8),
            ..Default::default()
        };
        let err = scope(7).apply(&mut partial).unwrap_err();
        assert!(matches!(err, WalletError::Unauthorized(_)), "got {err:?}");
    }

    #[test]
    fn check_owner_resolves_zero_to_caller() {
        assert_eq!(scope(7).check_owner(0).unwrap(), 7);
        assert_eq!(scope(7).check_owner(7).unwrap(), 7);
    }

    #[test]
    fn check_owner_rejects_foreign_owner() {
        let err = scope(7).check_owner(8).unwrap_err();
        assert!(matches!(err, WalletError::Unauthorized(_)), "got {err:?}");
    }
}
