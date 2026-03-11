//! Macro to implement sqlx Type/Encode/Decode for enums as VARCHAR/TEXT strings.
//!
//! sqlx 0.8's `#[derive(Type)]` on Rust enums maps to MySQL ENUM columns,
//! which has strict compatibility checks that fail even when variant names match.
//! This macro bypasses that by treating enums as plain string columns, using
//! strum's `Display` (for encoding) and `FromStr` (for decoding) implementations.

/// Implement sqlx `Type`, `Encode`, and `Decode` for an enum, treating it as a
/// string column (VARCHAR/TEXT) across all database backends.
///
/// The enum must derive `strum::Display` and `strum::EnumString` so that
/// `to_string()` and `FromStr` are available.
macro_rules! impl_sqlx_string_enum {
    ($enum_type:ty) => {
        #[cfg(feature = "mysql")]
        impl sqlx::Type<sqlx::MySql> for $enum_type {
            fn type_info() -> sqlx::mysql::MySqlTypeInfo {
                <str as sqlx::Type<sqlx::MySql>>::type_info()
            }
            fn compatible(ty: &sqlx::mysql::MySqlTypeInfo) -> bool {
                <str as sqlx::Type<sqlx::MySql>>::compatible(ty)
            }
        }

        #[cfg(feature = "mysql")]
        impl<'r> sqlx::Decode<'r, sqlx::MySql> for $enum_type {
            fn decode(
                value: sqlx::mysql::MySqlValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                let s = <&str as sqlx::Decode<sqlx::MySql>>::decode(value)?;
                <$enum_type as std::str::FromStr>::from_str(s).map_err(Into::into)
            }
        }

        #[cfg(feature = "mysql")]
        impl<'q> sqlx::Encode<'q, sqlx::MySql> for $enum_type {
            fn encode_by_ref(
                &self,
                buf: &mut <sqlx::MySql as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                let s = self.to_string();
                <String as sqlx::Encode<sqlx::MySql>>::encode(s, buf)
            }
        }

        #[cfg(feature = "sqlite")]
        impl sqlx::Type<sqlx::Sqlite> for $enum_type {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <str as sqlx::Type<sqlx::Sqlite>>::type_info()
            }
            fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
                <str as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            }
        }

        #[cfg(feature = "sqlite")]
        impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for $enum_type {
            fn decode(
                value: sqlx::sqlite::SqliteValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                let s = <&str as sqlx::Decode<sqlx::Sqlite>>::decode(value)?;
                <$enum_type as std::str::FromStr>::from_str(s).map_err(Into::into)
            }
        }

        #[cfg(feature = "sqlite")]
        impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for $enum_type {
            fn encode_by_ref(
                &self,
                buf: &mut <sqlx::Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                let s = self.to_string();
                <String as sqlx::Encode<sqlx::Sqlite>>::encode(s, buf)
            }
        }

        #[cfg(feature = "postgres")]
        impl sqlx::Type<sqlx::Postgres> for $enum_type {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <str as sqlx::Type<sqlx::Postgres>>::type_info()
            }
            fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
                <str as sqlx::Type<sqlx::Postgres>>::compatible(ty)
            }
        }

        #[cfg(feature = "postgres")]
        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $enum_type {
            fn decode(
                value: sqlx::postgres::PgValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                let s = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
                <$enum_type as std::str::FromStr>::from_str(s).map_err(Into::into)
            }
        }

        #[cfg(feature = "postgres")]
        impl<'q> sqlx::Encode<'q, sqlx::Postgres> for $enum_type {
            fn encode_by_ref(
                &self,
                buf: &mut <sqlx::Postgres as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                let s = self.to_string();
                <String as sqlx::Encode<sqlx::Postgres>>::encode(s, buf)
            }
        }
    };
}
