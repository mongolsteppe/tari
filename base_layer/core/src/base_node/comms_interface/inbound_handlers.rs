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
    base_node::{
        comms_interface::{
            error::CommsInterfaceError,
            local_interface::BlockEventSender,
            NodeCommsRequest,
            NodeCommsResponse,
        },
        OutboundNodeCommsInterface,
    },
    blocks::{block_header::BlockHeader, Block, NewBlock, NewBlockTemplate},
    chain_storage::{async_db::AsyncBlockchainDb, BlockAddResult, BlockchainBackend, ChainBlock},
    consensus::{ConsensusConstants, ConsensusManager},
    mempool::{async_mempool, Mempool},
    proof_of_work::{Difficulty, PowAlgorithm},
    transactions::{transaction::TransactionKernel, types::HashOutput},
};
use log::*;
use std::{
    fmt::{Display, Error, Formatter},
    sync::Arc,
};
use strum_macros::Display;
use tari_common_types::types::BlockHash;
use tari_comms::peer_manager::NodeId;
use tari_crypto::tari_utilities::{hash::Hashable, hex::Hex};
use tokio::sync::Semaphore;

const LOG_TARGET: &str = "c::bn::comms_interface::inbound_handler";
const MAX_HEADERS_PER_RESPONSE: u32 = 100;

/// Events that can be published on the Validated Block Event Stream
/// Broadcast is to notify subscribers if this is a valid propagated block event
#[derive(Debug, Clone, Display)]
pub enum BlockEvent {
    ValidBlockAdded(Arc<Block>, BlockAddResult, Broadcast),
    AddBlockFailed(Arc<Block>, Broadcast),
    BlockSyncComplete(Arc<ChainBlock>),
    BlockSyncRewind(Vec<Arc<ChainBlock>>),
}

/// Used to notify if the block event is for a propagated block.
#[derive(Debug, Clone, Copy)]
pub struct Broadcast(bool);

impl Broadcast {
    #[inline]
    pub fn is_true(&self) -> bool {
        self.0
    }
}

#[allow(clippy::identity_op)]
impl Display for Broadcast {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "Broadcast[{}]", self.0)
    }
}

impl From<Broadcast> for bool {
    fn from(v: Broadcast) -> Self {
        v.0
    }
}

impl From<bool> for Broadcast {
    fn from(v: bool) -> Self {
        Broadcast(v)
    }
}

/// The InboundNodeCommsInterface is used to handle all received inbound requests from remote nodes.
pub struct InboundNodeCommsHandlers<T> {
    block_event_sender: BlockEventSender,
    blockchain_db: AsyncBlockchainDb<T>,
    mempool: Mempool,
    consensus_manager: ConsensusManager,
    new_block_request_semaphore: Arc<Semaphore>,
    outbound_nci: OutboundNodeCommsInterface,
}

impl<T> InboundNodeCommsHandlers<T>
where T: BlockchainBackend + 'static
{
    /// Construct a new InboundNodeCommsInterface.
    pub fn new(
        block_event_sender: BlockEventSender,
        blockchain_db: AsyncBlockchainDb<T>,
        mempool: Mempool,
        consensus_manager: ConsensusManager,
        outbound_nci: OutboundNodeCommsInterface,
    ) -> Self {
        Self {
            block_event_sender,
            blockchain_db,
            mempool,
            consensus_manager,
            new_block_request_semaphore: Arc::new(Semaphore::new(1)),
            outbound_nci,
        }
    }

    /// Handle inbound node comms requests from remote nodes and local services.
    pub async fn handle_request(&self, request: NodeCommsRequest) -> Result<NodeCommsResponse, CommsInterfaceError> {
        debug!(target: LOG_TARGET, "Handling remote request {}", request);
        match request {
            NodeCommsRequest::GetChainMetadata => Ok(NodeCommsResponse::ChainMetadata(
                self.blockchain_db.get_chain_metadata().await?,
            )),
            NodeCommsRequest::FetchHeaders(block_nums) => {
                let mut block_headers = Vec::<BlockHeader>::new();
                for block_num in block_nums {
                    match self.blockchain_db.fetch_header(block_num).await {
                        Ok(Some(block_header)) => {
                            block_headers.push(block_header);
                        },
                        Ok(None) => return Err(CommsInterfaceError::BlockHeaderNotFound(block_num)),
                        Err(err) => {
                            error!(target: LOG_TARGET, "Could not fetch headers: {}", err.to_string());
                            return Err(err.into());
                        },
                    }
                }
                Ok(NodeCommsResponse::BlockHeaders(block_headers))
            },
            NodeCommsRequest::FetchHeadersWithHashes(block_hashes) => {
                let mut block_headers = Vec::<BlockHeader>::new();
                for block_hash in block_hashes {
                    let block_hex = block_hash.to_hex();
                    match self.blockchain_db.fetch_header_by_block_hash(block_hash).await? {
                        Some(block_header) => {
                            block_headers.push(block_header);
                        },
                        None => {
                            error!(target: LOG_TARGET, "Could not fetch headers with hashes:{}", block_hex);
                            return Err(CommsInterfaceError::InternalError(format!(
                                "Could not fetch headers with hashes:{}",
                                block_hex
                            )));
                        },
                    }
                }
                Ok(NodeCommsResponse::BlockHeaders(block_headers))
            },
            NodeCommsRequest::FetchHeadersAfter(header_hashes, stopping_hash) => {
                let mut starting_block = None;
                // Find first header that matches
                for header_hash in header_hashes {
                    match self
                        .blockchain_db
                        .fetch_header_by_block_hash(header_hash.clone())
                        .await?
                    {
                        Some(from_block) => {
                            starting_block = Some(from_block);
                            break;
                        },
                        None => {
                            // Not an error. The header requested is simply not in our chain.
                            // Logging it as debug because it may not just be not found.
                            debug!(
                                target: LOG_TARGET,
                                "Skipping header {} when searching for matching headers in our chain.",
                                header_hash.to_hex(),
                            );
                        },
                    }
                }
                let starting_block = match starting_block {
                    Some(b) => b,
                    // Send from genesis block if no hashes match
                    None => self
                        .blockchain_db
                        .fetch_header(0)
                        .await?
                        .ok_or(CommsInterfaceError::BlockHeaderNotFound(0))?,
                };
                let mut headers = vec![];
                for i in 1..MAX_HEADERS_PER_RESPONSE {
                    match self.blockchain_db.fetch_header(starting_block.height + i as u64).await {
                        Ok(header) => {
                            if let Some(header) = header {
                                let hash = header.hash();
                                headers.push(header);
                                if hash == stopping_hash {
                                    break;
                                }
                            }
                        },
                        Err(err) => {
                            error!(
                                target: LOG_TARGET,
                                "Could not fetch header at {}:{}",
                                starting_block.height + i as u64,
                                err.to_string()
                            );
                            return Err(err.into());
                        },
                    }
                }

                Ok(NodeCommsResponse::FetchHeadersAfterResponse(headers))
            },
            NodeCommsRequest::FetchMatchingUtxos(utxo_hashes) => {
                let mut res = Vec::with_capacity(utxo_hashes.len());
                for (output, spent) in (self.blockchain_db.fetch_utxos(utxo_hashes).await?)
                    .into_iter()
                    .flatten()
                {
                    if !spent {
                        res.push(output);
                    }
                }
                Ok(NodeCommsResponse::TransactionOutputs(res))
            },
            NodeCommsRequest::FetchMatchingTxos(hashes) => {
                let res = self
                    .blockchain_db
                    .fetch_utxos(hashes)
                    .await?
                    .into_iter()
                    .filter_map(|opt| opt.map(|(output, _)| output))
                    .collect();
                Ok(NodeCommsResponse::TransactionOutputs(res))
            },
            NodeCommsRequest::FetchMatchingBlocks(block_nums) => {
                let mut blocks = Vec::with_capacity(block_nums.len());
                for block_num in block_nums {
                    debug!(target: LOG_TARGET, "A peer has requested block {}", block_num);
                    match self.blockchain_db.fetch_block(block_num).await {
                        Ok(block) => blocks.push(block),
                        // We need to suppress the error as another node might ask for a block we dont have, so we
                        // return ok([])
                        Err(e) => debug!(
                            target: LOG_TARGET,
                            "Could not provide requested block {} to peer because: {}", block_num, e
                        ),
                    }
                }
                Ok(NodeCommsResponse::HistoricalBlocks(blocks))
            },
            NodeCommsRequest::FetchBlocksWithHashes(block_hashes) => {
                let mut blocks = Vec::with_capacity(block_hashes.len());
                for block_hash in block_hashes {
                    let block_hex = block_hash.to_hex();
                    debug!(
                        target: LOG_TARGET,
                        "A peer has requested a block with hash {}", block_hex
                    );

                    match self.blockchain_db.fetch_block_by_hash(block_hash).await {
                        Ok(Some(block)) => blocks.push(block),
                        Ok(None) => warn!(
                            target: LOG_TARGET,
                            "Could not provide requested block {} to peer because not stored", block_hex,
                        ),
                        Err(e) => warn!(
                            target: LOG_TARGET,
                            "Could not provide requested block {} to peer because: {}",
                            block_hex,
                            e.to_string()
                        ),
                    }
                }
                Ok(NodeCommsResponse::HistoricalBlocks(blocks))
            },
            NodeCommsRequest::FetchBlocksWithKernels(excess_sigs) => {
                let mut blocks = Vec::with_capacity(excess_sigs.len());
                for sig in excess_sigs {
                    let sig_hex = sig.get_signature().to_hex();
                    debug!(
                        target: LOG_TARGET,
                        "A peer has requested a block with kernel with sig {}", sig_hex
                    );
                    match self.blockchain_db.fetch_block_with_kernel(sig).await {
                        Ok(Some(block)) => blocks.push(block),
                        Ok(None) => warn!(
                            target: LOG_TARGET,
                            "Could not provide requested block containing kernel with sig {} to peer because not \
                             stored",
                            sig_hex
                        ),
                        Err(e) => warn!(
                            target: LOG_TARGET,
                            "Could not provide requested block containing kernel with sig {} to peer because: {}",
                            sig_hex,
                            e.to_string()
                        ),
                    }
                }
                Ok(NodeCommsResponse::HistoricalBlocks(blocks))
            },
            NodeCommsRequest::FetchBlocksWithUtxos(hashes) => {
                let mut blocks = Vec::with_capacity(hashes.len());
                for hash in hashes {
                    let hash_hex = hash.to_hex();
                    debug!(
                        target: LOG_TARGET,
                        "A peer has requested a block with hash {}", hash_hex,
                    );
                    match self.blockchain_db.fetch_block_with_utxo(hash).await {
                        Ok(Some(block)) => blocks.push(block),
                        Ok(None) => warn!(
                            target: LOG_TARGET,
                            "Could not provide requested block {} to peer because not stored", hash_hex,
                        ),
                        Err(e) => warn!(
                            target: LOG_TARGET,
                            "Could not provide requested block {} to peer because: {}",
                            hash_hex,
                            e.to_string()
                        ),
                    }
                }
                Ok(NodeCommsResponse::HistoricalBlocks(blocks))
            },
            NodeCommsRequest::GetHeaderByHash(hash) => {
                let header = self.blockchain_db.fetch_header_by_block_hash(hash).await?;
                Ok(NodeCommsResponse::BlockHeader(header))
            },
            NodeCommsRequest::GetBlockByHash(hash) => {
                let block = self.blockchain_db.fetch_block_by_hash(hash).await?;
                Ok(NodeCommsResponse::HistoricalBlock(Box::new(block)))
            },
            NodeCommsRequest::GetNewBlockTemplate(request) => {
                let best_block_header = self.blockchain_db.fetch_tip_header().await?;

                let mut header = BlockHeader::from_previous(best_block_header.header());
                let constants = self.consensus_manager.consensus_constants(header.height);
                header.version = constants.blockchain_version();
                header.pow.pow_algo = request.algo;

                let constants_weight = constants.get_max_block_weight_excluding_coinbase();
                let asking_weight = if request.max_weight > constants_weight || request.max_weight == 0 {
                    constants_weight
                } else {
                    request.max_weight
                };

                let transactions = async_mempool::retrieve(self.mempool.clone(), asking_weight)
                    .await?
                    .into_iter()
                    .map(|tx| Arc::try_unwrap(tx).unwrap_or_else(|tx| (*tx).clone()))
                    .collect::<Vec<_>>();

                debug!(
                    target: LOG_TARGET,
                    "Adding {} transaction(s) to new block template",
                    transactions.len()
                );

                let prev_hash = header.prev_hash.clone();
                let height = header.height;

                let block_template = NewBlockTemplate::from_block(
                    header.into_builder().with_transactions(transactions).build(),
                    self.get_target_difficulty_for_next_block(request.algo, constants, prev_hash)
                        .await?,
                    self.consensus_manager.get_block_reward_at(height),
                );
                debug!(
                    target: LOG_TARGET,
                    "New block template requested at height {}", block_template.header.height,
                );
                trace!(target: LOG_TARGET, "{}", block_template);
                Ok(NodeCommsResponse::NewBlockTemplate(block_template))
            },
            NodeCommsRequest::GetNewBlock(block_template) => {
                let block = self.blockchain_db.prepare_block_merkle_roots(block_template).await?;
                Ok(NodeCommsResponse::NewBlock {
                    success: true,
                    error: None,
                    block: Some(block),
                })
            },
            NodeCommsRequest::FetchKernelByExcessSig(signature) => {
                let mut kernels = Vec::<TransactionKernel>::new();

                match self.blockchain_db.fetch_kernel_by_excess_sig(signature).await {
                    Ok(kernel) => match kernel {
                        None => (),
                        Some((kernel, _kernel_hash)) => {
                            kernels.push(kernel);
                        },
                    },
                    Err(err) => {
                        error!(target: LOG_TARGET, "Could not fetch kernel {}", err.to_string());
                        return Err(err.into());
                    },
                }

                Ok(NodeCommsResponse::TransactionKernels(kernels))
            },
        }
    }

    /// Handles a `NewBlock` message. Only a single `NewBlock` message can be handled at once to prevent extraneous
    /// requests for the full block.
    /// This may (asynchronously) block until the other request(s) complete or time out and so should typically be
    /// executed in a dedicated task.
    pub async fn handle_new_block_message(
        &mut self,
        new_block: NewBlock,
        source_peer: NodeId,
    ) -> Result<(), CommsInterfaceError> {
        let NewBlock { block_hash } = new_block;

        // Only a single block request can complete at a time.
        // As multiple NewBlock requests arrive from propagation, this semaphore prevents multiple requests to nodes for
        // the same full block. The first request that succeeds will stop the node from requesting the block from any
        // other node (block_exists is true).
        let _permit = self.new_block_request_semaphore.acquire().await;

        if self.blockchain_db.block_exists(block_hash.clone()).await? {
            debug!(
                target: LOG_TARGET,
                "Block with hash `{}` already stored",
                block_hash.to_hex()
            );
            return Ok(());
        }

        debug!(
            target: LOG_TARGET,
            "Block with hash `{}` is unknown. Requesting it from peer `{}`.",
            block_hash.to_hex(),
            source_peer.short_str()
        );
        let mut block = self
            .outbound_nci
            .request_blocks_with_hashes_from_peer(vec![block_hash], Some(source_peer.clone()))
            .await?;

        match block.pop() {
            Some(block) => {
                self.handle_block(Arc::new(block.try_into_block()?), true.into(), Some(source_peer))
                    .await?;
                Ok(())
            },
            None => {
                // TODO: #banheuristic - peer propagated block hash for which it could not return the full block
                debug!(
                    target: LOG_TARGET,
                    "Peer `{}` failed to return the block that was requested.",
                    source_peer.short_str()
                );
                Err(CommsInterfaceError::InvalidPeerResponse(format!(
                    "Invalid response from peer `{}`: Peer failed to provide the block that was propagated",
                    source_peer.short_str()
                )))
            },
        }
    }

    /// Handle inbound blocks from remote nodes and local services.
    pub async fn handle_block(
        &self,
        block: Arc<Block>,
        broadcast: Broadcast,
        source_peer: Option<NodeId>,
    ) -> Result<BlockHash, CommsInterfaceError> {
        let block_hash = block.hash();
        let block_height = block.header.height;
        info!(
            target: LOG_TARGET,
            "Block #{} ({}) received from {}",
            block_height,
            block_hash.to_hex(),
            source_peer
                .as_ref()
                .map(|p| format!("remote peer: {}", p))
                .unwrap_or_else(|| "local services".to_string())
        );
        trace!(target: LOG_TARGET, "Block: {}", block);
        let add_block_result = self.blockchain_db.add_block(block.clone()).await;
        // Create block event on block event stream
        match add_block_result {
            Ok(block_add_result) => {
                trace!(target: LOG_TARGET, "Block event created: {}", block_add_result);

                let should_propagate = match &block_add_result {
                    BlockAddResult::Ok(_) => true,
                    BlockAddResult::BlockExists => false,
                    BlockAddResult::OrphanBlock => false,
                    BlockAddResult::ChainReorg { .. } => true,
                };

                self.blockchain_db.cleanup_orphans().await?;

                self.publish_block_event(BlockEvent::ValidBlockAdded(block, block_add_result, broadcast));

                if should_propagate && broadcast.is_true() {
                    info!(
                        target: LOG_TARGET,
                        "Propagate block ({}) to network.",
                        block_hash.to_hex()
                    );
                    let exclude_peers = source_peer.into_iter().collect();
                    let new_block = NewBlock::new(block_hash.clone());
                    self.outbound_nci.propagate_block(new_block, exclude_peers).await?;
                }
                Ok(block_hash)
            },
            Err(e) => {
                warn!(
                    target: LOG_TARGET,
                    "Block #{} ({}) validation failed: {:?}",
                    block_height,
                    block_hash.to_hex(),
                    e
                );
                self.publish_block_event(BlockEvent::AddBlockFailed(block, broadcast));
                Err(CommsInterfaceError::ChainStorageError(e))
            },
        }
    }

    fn publish_block_event(&self, event: BlockEvent) {
        if let Err(event) = self.block_event_sender.send(Arc::new(event)) {
            debug!(target: LOG_TARGET, "No event subscribers. Event {} dropped.", event.0)
        }
    }

    async fn get_target_difficulty_for_next_block(
        &self,
        pow_algo: PowAlgorithm,
        constants: &ConsensusConstants,
        current_block_hash: HashOutput,
    ) -> Result<Difficulty, CommsInterfaceError> {
        let target_difficulty = self
            .blockchain_db
            .fetch_target_difficulty_for_next_block(pow_algo, current_block_hash)
            .await?;

        let target = target_difficulty.calculate(
            constants.min_pow_difficulty(pow_algo),
            constants.max_pow_difficulty(pow_algo),
        );
        debug!(target: LOG_TARGET, "Target difficulty {} for PoW {}", target, pow_algo);
        Ok(target)
    }
}

impl<T> Clone for InboundNodeCommsHandlers<T> {
    fn clone(&self) -> Self {
        Self {
            block_event_sender: self.block_event_sender.clone(),
            blockchain_db: self.blockchain_db.clone(),
            mempool: self.mempool.clone(),
            consensus_manager: self.consensus_manager.clone(),
            new_block_request_semaphore: self.new_block_request_semaphore.clone(),
            outbound_nci: self.outbound_nci.clone(),
        }
    }
}
