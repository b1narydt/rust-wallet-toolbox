//! Transaction size estimation utilities.
//!
//! Ported from wallet-toolbox/src/storage/methods/utils.ts.
//! Computes serialized transaction sizes for fee estimation.

/// Returns the byte size required to encode a number as a Bitcoin VarUint.
pub fn var_uint_size(val: usize) -> usize {
    if val <= 0xfc {
        1
    } else if val <= 0xffff {
        3
    } else if val <= 0xffff_ffff {
        5
    } else {
        9
    }
}

/// Returns the serialized byte length of a transaction input.
///
/// Layout: txid(32) + vout(4) + script_length_varint + script + sequence(4)
pub fn transaction_input_size(script_size: usize) -> usize {
    32 + 4 + var_uint_size(script_size) + script_size + 4
}

/// Returns the serialized byte length of a transaction output.
///
/// Layout: script_length_varint + script + satoshis(8)
pub fn transaction_output_size(script_size: usize) -> usize {
    var_uint_size(script_size) + script_size + 8
}

/// Compute the serialized binary transaction size in bytes.
///
/// Layout: version(4) + input_count_varint + inputs + output_count_varint + outputs + locktime(4)
pub fn transaction_size(input_script_lengths: &[usize], output_script_lengths: &[usize]) -> usize {
    4 + var_uint_size(input_script_lengths.len())
        + input_script_lengths
            .iter()
            .map(|&s| transaction_input_size(s))
            .sum::<usize>()
        + var_uint_size(output_script_lengths.len())
        + output_script_lengths
            .iter()
            .map(|&s| transaction_output_size(s))
            .sum::<usize>()
        + 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_var_uint_size_small() {
        assert_eq!(var_uint_size(0), 1);
        assert_eq!(var_uint_size(0xfc), 1);
    }

    #[test]
    fn test_var_uint_size_medium() {
        assert_eq!(var_uint_size(0xfd), 3);
        assert_eq!(var_uint_size(0xffff), 3);
    }

    #[test]
    fn test_var_uint_size_large() {
        assert_eq!(var_uint_size(0x10000), 5);
        assert_eq!(var_uint_size(0xffff_ffff), 5);
    }

    #[test]
    fn test_transaction_input_size_p2pkh() {
        // P2PKH unlock script is 108 bytes
        // 32 + 4 + 1 + 108 + 4 = 149
        assert_eq!(transaction_input_size(108), 149);
    }

    #[test]
    fn test_transaction_output_size_p2pkh() {
        // P2PKH locking script is 25 bytes
        // 1 + 25 + 8 = 34
        assert_eq!(transaction_output_size(25), 34);
    }

    #[test]
    fn test_transaction_size_single_p2pkh() {
        // 1 P2PKH input (108) + 1 P2PKH output (25)
        // 4 (version) + 1 (input count) + 149 (input) + 1 (output count) + 34 (output) + 4 (locktime)
        // = 193
        let size = transaction_size(&[108], &[25]);
        assert_eq!(size, 193);
    }

    #[test]
    fn test_transaction_size_multiple_io() {
        // 2 inputs, 2 outputs
        let size = transaction_size(&[108, 108], &[25, 25]);
        // 4 + 1 + 149*2 + 1 + 34*2 + 4 = 4 + 1 + 298 + 1 + 68 + 4 = 376
        assert_eq!(size, 376);
    }
}
