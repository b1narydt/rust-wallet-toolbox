use bsv_wallet_toolbox::storage::action_types::*;

#[test]
fn test_create_action_args_wire_format_matches_ts() {
    let args = StorageCreateActionArgs {
        description: "test payment".to_string(),
        inputs: vec![StorageCreateActionInput {
            outpoint: StorageOutPoint {
                txid: "a".repeat(64),
                vout: 0,
            },
            input_description: "spend utxo".to_string(),
            unlocking_script_length: 107,
            sequence_number: 0xffffffff,
        }],
        outputs: vec![StorageCreateActionOutput {
            locking_script: "76a914aabb88ac".to_string(),
            satoshis: 1000,
            output_description: "payment".to_string(),
            basket: None,
            custom_instructions: None,
            tags: vec![],
        }],
        lock_time: 0,
        version: 1,
        labels: vec![],
        options: StorageCreateActionOptions::default(),
        input_beef: Some(vec![0, 1, 2]),
        is_new_tx: true,
        is_sign_action: false,
        is_no_send: false,
        is_delayed: true,
        is_send_with: false,
        is_remix_change: false,
        is_test_werr_review_actions: None,
        include_all_source_transactions: false,
        random_vals: None,
    };
    let json = serde_json::to_value(&args).unwrap();

    // Verify nested outpoint object
    let input = &json["inputs"][0];
    assert!(
        input.get("outpoint").is_some(),
        "must have nested outpoint object"
    );
    assert_eq!(input["outpoint"]["txid"].as_str().unwrap().len(), 64);
    assert_eq!(input["outpoint"]["vout"].as_u64().unwrap(), 0);
    assert!(
        input.get("outpointTxid").is_none(),
        "flat outpointTxid must NOT appear"
    );

    // Verify options present
    assert!(json.get("options").is_some(), "must have options object");
    let opts = &json["options"];
    assert!(opts.get("signAndProcess").is_some());
    assert!(opts.get("acceptDelayedBroadcast").is_some());
    assert!(opts.get("randomizeOutputs").is_some());

    // Verify boolean flags
    assert!(json.get("isNewTx").is_some());
    assert!(json.get("isSendWith").is_some());
    assert!(json.get("isDelayed").is_some());
    assert!(json.get("isRemixChange").is_some());
    assert!(json.get("includeAllSourceTransactions").is_some());

    // Verify inputBEEF (uppercase BEEF)
    assert!(
        json.get("inputBEEF").is_some(),
        "must use inputBEEF not inputBeef"
    );

    // Verify camelCase
    assert!(json.get("lockTime").is_some());
    assert!(json.get("isSignAction").is_some());
    assert!(json.get("lock_time").is_none());

    // Negative assertion: camelCase "inputBeef" must NOT appear (we use "inputBEEF")
    assert!(json.get("inputBeef").is_none(), "'inputBeef' (camelCase) must not appear");
}
