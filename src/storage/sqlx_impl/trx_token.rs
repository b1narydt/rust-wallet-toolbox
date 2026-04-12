//! Type-erased transaction token for object-safe trait dispatch.
//!
//! TrxToken wraps a `Box<dyn Any + Send + Sync>` so that the StorageProvider
//! trait hierarchy can remain object-safe (no associated types). Concrete
//! implementations (e.g., `StorageSqlx<Sqlite>`) downcast internally to their
//! specific transaction handle type.

use std::any::Any;

/// A type-erased transaction token that can be passed through
/// `dyn StorageProvider` method signatures.
///
/// The inner type for SQLite is
/// `Arc<tokio::sync::Mutex<Option<sqlx::Transaction<'static, Sqlite>>>>`,
/// but trait consumers only see the opaque TrxToken.
pub struct TrxToken {
    inner: Box<dyn Any + Send + Sync>,
}

impl TrxToken {
    /// Create a new TrxToken wrapping any Send + Sync type.
    pub fn new<T: Any + Send + Sync>(inner: T) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// Downcast to a reference of the concrete inner type.
    pub fn downcast_ref<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.inner.downcast_ref::<T>()
    }

    /// Consume this token and attempt to downcast to the concrete inner type.
    pub fn downcast<T: Any + Send + Sync>(self) -> Result<T, Self> {
        match self.inner.downcast::<T>() {
            Ok(val) => Ok(*val),
            Err(inner) => Err(Self { inner }),
        }
    }
}

impl std::fmt::Debug for TrxToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deref through the `Box` so `Any::type_id` reports the erased inner
        // type, not `Box<dyn Any + Send + Sync>` itself.
        f.debug_struct("TrxToken")
            .field("inner_type_id", &(*self.inner).type_id())
            .finish()
    }
}
