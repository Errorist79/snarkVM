// Copyright (C) 2019-2021 Aleo Systems Inc.
// This file is part of the snarkVM library.

// The snarkVM library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The snarkVM library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the snarkVM library. If not, see <https://www.gnu.org/licenses/>.

use crate::{AleoAmount, Network, Transaction, TransactionError, TransactionScheme};
use snarkvm_algorithms::merkle_tree::MerkleTree;
use snarkvm_utilities::{
    has_duplicates,
    to_bytes_le,
    variable_length_integer::{read_variable_length_integer, variable_length_integer},
    FromBytes,
    ToBytes,
};

use anyhow::{anyhow, Result};
use std::{
    io::{Read, Result as IoResult, Write},
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BlockTransactions<N: Network>(pub Vec<Transaction<N>>);

impl<N: Network> BlockTransactions<N> {
    /// Initializes an empty list of transactions.
    pub fn new() -> Self {
        Self(vec![])
    }

    /// Initializes from a given list of transactions.
    pub fn from(transactions: &[Transaction<N>]) -> Self {
        Self(transactions.to_vec())
    }

    /// Adds the given transaction to the list of transactions, if it is valid.
    pub fn push(&mut self, transaction: Transaction<N>) -> Result<()> {
        match transaction.is_valid() {
            true => Ok(self.0.push(transaction)),
            false => Err(anyhow!("Failed to push due to an invalid transaction")),
        }
    }

    /// Returns `true` if the transactions are well-formed.
    pub fn is_valid(&self) -> bool {
        // TODO (howardwu): This check can be parallelized for performance improvement.
        // Ensure each transaction is well-formed.
        for transaction in &self.0 {
            if !transaction.is_valid() {
                eprintln!("Invalid transaction found in the transactions list");
                return false;
            }
        }

        // Ensure there are no duplicate serial numbers.
        match self.to_serial_numbers() {
            Ok(serial_numbers) => {
                if has_duplicates(serial_numbers) {
                    eprintln!("Found duplicate serial numbers in the transactions");
                    return false;
                }
            }
            Err(error) => {
                eprintln!("Failed to retrieve serial numbers from the transactions: {}", error);
                return false;
            }
        };

        // Ensure there are no duplicate commitments.
        match self.to_commitments() {
            Ok(commitments) => {
                if has_duplicates(commitments) {
                    eprintln!("Found duplicate commitments in the transactions");
                    return false;
                }
            }
            Err(error) => {
                eprintln!("Failed to retrieve commitments from the transactions: {}", error);
                return false;
            }
        };

        // Ensure there is exactly one coinbase transaction.
        let num_coinbase = self.to_coinbase_transaction_count();
        if num_coinbase != N::BLOCK_COINBASE_TX_COUNT {
            eprintln!(
                "Block must have exactly {} coinbase transaction(s), found {}",
                N::BLOCK_COINBASE_TX_COUNT,
                num_coinbase
            );
            return false;
        }

        true
    }

    /// Returns the transactions root, by computing the root for a Merkle tree of the transactions.
    pub fn to_transactions_root(&self) -> Result<N::TransactionsRoot> {
        assert!(!self.0.is_empty(), "Cannot process an empty list of transactions");
        let transaction_ids = (*self)
            .iter()
            .map(|tx| {
                let id_bytes = tx.to_transaction_id()?.to_bytes_le()?;
                assert_eq!(id_bytes.len(), 32);

                let mut transaction_id = [0u8; 32];
                transaction_id.copy_from_slice(&id_bytes);
                Ok(transaction_id)
            })
            .collect::<Result<Vec<[u8; 32]>>>()?;

        Ok(*MerkleTree::<N::TransactionsTreeParameters>::new(
            Arc::new(N::transactions_tree_parameters().clone()),
            &transaction_ids,
        )?
        .root())
    }

    /// Returns the commitments, by constructing a flattened list of commitments from all transactions.
    pub fn to_commitments(&self) -> Result<Vec<<N as Network>::Commitment>> {
        assert!(!self.0.is_empty(), "Cannot process an empty list of transactions");
        Ok(self.0.iter().map(|tx| tx.commitments()).flatten().cloned().collect())
    }

    /// Returns the serial numbers, by constructing a flattened list of serial numbers from all transactions.
    pub fn to_serial_numbers(&self) -> Result<Vec<<N as Network>::SerialNumber>> {
        assert!(!self.0.is_empty(), "Cannot process an empty list of transactions");
        Ok(self.0.iter().map(|tx| tx.serial_numbers()).flatten().cloned().collect())
    }

    /// Returns `true` if the transactions contains exactly one coinbase transaction.
    pub fn to_coinbase_transaction_count(&self) -> usize {
        // Filter out all transactions with a positive value balance.
        self.iter().filter(|t| t.value_balance().is_negative()).count()
    }

    /// Returns the total transaction fees, by summing the value balance from all positive transactions.
    /// Note - this amount does *not* include the block reward.
    pub fn to_transaction_fees(&self) -> Result<AleoAmount> {
        self.0
            .iter()
            .filter_map(|t| match t.value_balance().is_negative() {
                true => None,
                false => Some(*t.value_balance()),
            })
            .reduce(|a, b| a.add(b))
            .ok_or(anyhow!("Failed to compute the transaction fees for block"))
    }

    /// Returns the net value balance, by summing the value balance from all transactions.
    pub fn to_net_value_balance(&self) -> Result<AleoAmount> {
        assert!(!self.0.is_empty(), "Cannot process an empty list of transactions");
        self.0
            .iter()
            .map(|transaction| *transaction.value_balance())
            .reduce(|a, b| a.add(b))
            .ok_or(anyhow!("Failed to compute net value balance for block"))
    }

    /// Serializes the transactions into strings.
    pub fn serialize_as_str(&self) -> Result<Vec<String>, TransactionError> {
        self.0
            .iter()
            .map(|transaction| -> Result<String, TransactionError> { Ok(hex::encode(to_bytes_le![transaction]?)) })
            .collect::<Result<Vec<String>, TransactionError>>()
    }
}

impl<N: Network> FromBytes for BlockTransactions<N> {
    #[inline]
    fn read_le<R: Read>(mut reader: R) -> IoResult<Self> {
        let num_transactions = read_variable_length_integer(&mut reader)?;
        let mut transactions = Vec::with_capacity(num_transactions);
        for _ in 0..num_transactions {
            transactions.push(FromBytes::read_le(&mut reader)?);
        }
        Ok(Self(transactions))
    }
}

impl<N: Network> ToBytes for BlockTransactions<N> {
    #[inline]
    fn write_le<W: Write>(&self, mut writer: W) -> IoResult<()> {
        variable_length_integer(self.0.len() as u64).write_le(&mut writer)?;
        for transaction in &self.0 {
            transaction.write_le(&mut writer)?;
        }
        Ok(())
    }
}

impl<N: Network> Default for BlockTransactions<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<N: Network> Deref for BlockTransactions<N> {
    type Target = Vec<Transaction<N>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<N: Network> DerefMut for BlockTransactions<N> {
    fn deref_mut(&mut self) -> &mut Vec<Transaction<N>> {
        &mut self.0
    }
}
