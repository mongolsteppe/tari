// Copyright 2019. The Tari Project
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
// disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
// following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
// products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
// USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::{
    chain_storage::{BlockchainBackend, BlockchainDatabase, MmrTree},
    crypto::tari_utilities::Hashable,
    transactions::{transaction::Transaction, types::CryptoFactories},
    validation::{MempoolTransactionValidation, ValidationError},
};
use log::*;

pub const LOG_TARGET: &str = "c::val::transaction_validators";

/// This validator will check the internal consistency of the transaction.
///
/// 1. The sum of inputs, outputs and fees equal the (public excess value + offset)
/// 1. The signature signs the canonical message with the private excess
/// 1. Range proofs of the outputs are valid
///
/// This function does NOT check that inputs come from the UTXO set
pub struct TxInternalConsistencyValidator {
    factories: CryptoFactories,
}

impl TxInternalConsistencyValidator {
    pub fn new(factories: CryptoFactories) -> Self {
        Self { factories }
    }
}

impl MempoolTransactionValidation for TxInternalConsistencyValidator {
    fn validate(&self, tx: &Transaction) -> Result<(), ValidationError> {
        tx.validate_internal_consistency(&self.factories, None)
            .map_err(ValidationError::TransactionError)?;
        Ok(())
    }
}

/// This validator will check the transaction against the current consensus rules.
///
/// 1. The transaction weight should not exceed the maximum weight for 1 block
#[derive(Clone)]
pub struct TxConsensusValidator<B> {
    db: BlockchainDatabase<B>,
}

impl<B: BlockchainBackend> TxConsensusValidator<B> {
    pub fn new(db: BlockchainDatabase<B>) -> Self {
        Self { db }
    }
}

impl<B: BlockchainBackend> MempoolTransactionValidation for TxConsensusValidator<B> {
    fn validate(&self, tx: &Transaction) -> Result<(), ValidationError> {
        let consensus_constants = self.db.consensus_constants()?;
        // validate maximum tx weight
        if tx.calculate_weight() > consensus_constants.get_max_block_weight_excluding_coinbase() {
            return Err(ValidationError::MaxTransactionWeightExceeded);
        }

        Ok(())
    }
}

/// This validator assumes that the transaction was already validated and it will skip this step. It will only check, in
/// order,: All inputs exist in the backend, All timelocks (kernel lock heights and output maturities) have passed
#[derive(Clone)]
pub struct TxInputAndMaturityValidator<B> {
    db: BlockchainDatabase<B>,
}

impl<B: BlockchainBackend> TxInputAndMaturityValidator<B> {
    pub fn new(db: BlockchainDatabase<B>) -> Self {
        Self { db }
    }
}

impl<B: BlockchainBackend> MempoolTransactionValidation for TxInputAndMaturityValidator<B> {
    fn validate(&self, tx: &Transaction) -> Result<(), ValidationError> {
        let db = self.db.db_read_access()?;
        verify_not_stxos(tx, &*db)?;
        check_not_duplicate_txos(tx, &*db)?;

        let tip_height = db.fetch_chain_metadata()?.height_of_longest_chain();
        verify_timelocks(tx, tip_height)?;
        verify_no_duplicated_inputs_outputs(tx)?;
        Ok(())
    }
}

// This function checks that all the timelocks in the provided transaction pass. It checks kernel lock heights and
// input maturities
fn verify_timelocks(tx: &Transaction, current_height: u64) -> Result<(), ValidationError> {
    if tx.min_spendable_height() > current_height + 1 {
        return Err(ValidationError::MaturityError);
    }
    Ok(())
}

// This function checks that the inputs exists in the UTXO set but do not exist in the STXO set.
fn verify_not_stxos<B: BlockchainBackend>(tx: &Transaction, db: &B) -> Result<(), ValidationError> {
    let deleted = db.fetch_deleted_bitmap()?;
    let mut not_found_input = Vec::new();
    for input in tx.body.inputs() {
        if let Some((_, index, _height)) = db.fetch_output(&input.output_hash())? {
            if deleted.bitmap().contains(index) {
                warn!(
                    target: LOG_TARGET,
                    "Transaction validation failed due to already spent input: {}", input
                );
                return Err(ValidationError::ContainsSTxO);
            }
        } else if !tx
            .body
            .outputs()
            .iter()
            .any(|output| output.hash() == input.output_hash())
        {
            warn!(
                target: LOG_TARGET,
                "Transaction uses input: {} which does not exist yet", input
            );
            not_found_input.push(input.output_hash());
        }
    }
    if !not_found_input.is_empty() {
        return Err(ValidationError::UnknownInputs(not_found_input));
    }

    Ok(())
}

// This function checks that the inputs and outputs do not exist in the STxO set.
fn check_not_duplicate_txos<B: BlockchainBackend>(transaction: &Transaction, db: &B) -> Result<(), ValidationError> {
    for output in transaction.body.outputs() {
        if db.fetch_mmr_leaf_index(MmrTree::Utxo, &output.hash())?.is_some() {
            warn!(
                target: LOG_TARGET,
                "Transaction validation failed due to previously spent output: {}", output
            );
            return Err(ValidationError::ContainsTxO);
        }
    }
    Ok(())
}

/// This function checks the at the tx contains no duplicated inputs or outputs.
fn verify_no_duplicated_inputs_outputs(tx: &Transaction) -> Result<(), ValidationError> {
    if tx.body.contains_duplicated_inputs() {
        warn!(target: LOG_TARGET, "Transaction validation failed due to double input");
        return Err(ValidationError::UnsortedOrDuplicateInput);
    }
    if tx.body.contains_duplicated_outputs() {
        warn!(target: LOG_TARGET, "Transaction validation failed due to double output");
        return Err(ValidationError::UnsortedOrDuplicateOutput);
    }
    Ok(())
}

pub struct MempoolValidator {
    validators: Vec<Box<dyn MempoolTransactionValidation>>,
}

impl MempoolValidator {
    pub fn new(validators: Vec<Box<dyn MempoolTransactionValidation>>) -> Self {
        Self { validators }
    }
}

impl MempoolTransactionValidation for MempoolValidator {
    fn validate(&self, transaction: &Transaction) -> Result<(), ValidationError> {
        for v in &self.validators {
            v.validate(transaction)?;
        }
        Ok(())
    }
}
