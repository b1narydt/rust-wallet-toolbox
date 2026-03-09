//! BRC-114 action time label parsing.
//!
//! Translates the TypeScript `parseBrc114ActionTimeLabels` utility for
//! extracting time range filters from action labels used in spending
//! authorization time-window checks.

use crate::WalletError;

/// Result of parsing BRC-114 action time labels.
#[derive(Debug, Clone)]
pub struct ParsedBrc114ActionTimeLabels {
    /// Start of the time range (UNIX milliseconds), if specified.
    pub from: Option<u64>,
    /// End of the time range (UNIX milliseconds), if specified.
    pub to: Option<u64>,
    /// Whether any time filter label was found.
    pub time_filter_requested: bool,
    /// Labels that were not time-related.
    pub remaining_labels: Vec<String>,
}

/// Parse action labels to extract BRC-114 time range filters.
///
/// Looks for labels matching `"action time from <timestamp>"` and
/// `"action time to <timestamp>"` patterns. Returns the parsed time range
/// along with remaining non-time labels.
///
/// Returns an error for duplicate time labels, invalid timestamps, or
/// if `from >= to`.
pub fn parse_brc114_action_time_labels(
    labels: &[String],
) -> Result<ParsedBrc114ActionTimeLabels, WalletError> {
    let mut from: Option<u64> = None;
    let mut to: Option<u64> = None;
    let mut remaining_labels = Vec::new();
    let mut time_filter_requested = false;

    for label in labels {
        if let Some(rest) = label.strip_prefix("action time from ") {
            time_filter_requested = true;
            if from.is_some() {
                return Err(WalletError::InvalidParameter {
                    parameter: "labels".to_string(),
                    must_be: "valid. Duplicate action time from label.".to_string(),
                });
            }
            let n: u64 = rest.parse().map_err(|_| WalletError::InvalidParameter {
                parameter: "labels".to_string(),
                must_be: "valid. Invalid action time from timestamp value.".to_string(),
            })?;
            from = Some(n);
            continue;
        }

        if let Some(rest) = label.strip_prefix("action time to ") {
            time_filter_requested = true;
            if to.is_some() {
                return Err(WalletError::InvalidParameter {
                    parameter: "labels".to_string(),
                    must_be: "valid. Duplicate action time to label.".to_string(),
                });
            }
            let n: u64 = rest.parse().map_err(|_| WalletError::InvalidParameter {
                parameter: "labels".to_string(),
                must_be: "valid. Invalid action time to timestamp value.".to_string(),
            })?;
            to = Some(n);
            continue;
        }

        remaining_labels.push(label.clone());
    }

    if let (Some(f), Some(t)) = (from, to) {
        if f >= t {
            return Err(WalletError::InvalidParameter {
                parameter: "labels".to_string(),
                must_be: "valid. action time from must be less than action time to.".to_string(),
            });
        }
    }

    Ok(ParsedBrc114ActionTimeLabels {
        from,
        to,
        time_filter_requested,
        remaining_labels,
    })
}

/// Create a BRC-114 action time label from a UNIX millisecond timestamp.
pub fn make_brc114_action_time_label(unix_millis: u64) -> String {
    format!("action time {}", unix_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_labels() {
        let result = parse_brc114_action_time_labels(&[]).unwrap();
        assert_eq!(result.from, None);
        assert_eq!(result.to, None);
        assert!(!result.time_filter_requested);
        assert!(result.remaining_labels.is_empty());
    }

    #[test]
    fn test_parse_from_and_to() {
        let labels = vec![
            "action time from 1000".to_string(),
            "action time to 2000".to_string(),
            "other-label".to_string(),
        ];
        let result = parse_brc114_action_time_labels(&labels).unwrap();
        assert_eq!(result.from, Some(1000));
        assert_eq!(result.to, Some(2000));
        assert!(result.time_filter_requested);
        assert_eq!(result.remaining_labels, vec!["other-label"]);
    }

    #[test]
    fn test_parse_duplicate_from_error() {
        let labels = vec![
            "action time from 1000".to_string(),
            "action time from 2000".to_string(),
        ];
        assert!(parse_brc114_action_time_labels(&labels).is_err());
    }

    #[test]
    fn test_parse_from_gte_to_error() {
        let labels = vec![
            "action time from 2000".to_string(),
            "action time to 1000".to_string(),
        ];
        assert!(parse_brc114_action_time_labels(&labels).is_err());

        let labels_eq = vec![
            "action time from 1000".to_string(),
            "action time to 1000".to_string(),
        ];
        assert!(parse_brc114_action_time_labels(&labels_eq).is_err());
    }

    #[test]
    fn test_parse_invalid_timestamp() {
        let labels = vec!["action time from abc".to_string()];
        assert!(parse_brc114_action_time_labels(&labels).is_err());
    }

    #[test]
    fn test_make_brc114_action_time_label() {
        assert_eq!(
            make_brc114_action_time_label(1234567890),
            "action time 1234567890"
        );
    }
}
