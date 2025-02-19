// Copyright 2020. The Tari Project
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

use super::{error::BaseNodeServiceError, service::BaseNodeState};
use futures::{stream::Fuse, StreamExt};
use std::sync::Arc;
use tari_comms::peer_manager::Peer;

use std::time::Duration;
use tari_common_types::chain_metadata::ChainMetadata;
use tari_service_framework::reply_channel::SenderService;
use tokio::sync::broadcast;
use tower::Service;

pub type BaseNodeEventSender = broadcast::Sender<Arc<BaseNodeEvent>>;
pub type BaseNodeEventReceiver = broadcast::Receiver<Arc<BaseNodeEvent>>;
/// API Request enum
#[derive(Debug)]
pub enum BaseNodeServiceRequest {
    GetChainMetadata,
    SetBaseNodePeer(Box<Peer>),
    GetBaseNodePeer,
    GetBaseNodeLatency,
}
/// API Response enum
#[derive(Debug)]
pub enum BaseNodeServiceResponse {
    ChainMetadata(Option<ChainMetadata>),
    BaseNodePeerSet,
    BaseNodePeer(Option<Box<Peer>>),
    Latency(Option<Duration>),
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum BaseNodeEvent {
    BaseNodeStateChanged(BaseNodeState),
    BaseNodePeerSet(Box<Peer>),
}

/// The Base Node Service Handle is a struct that contains the interfaces used to communicate with a running
/// Base Node
#[derive(Clone)]
pub struct BaseNodeServiceHandle {
    handle: SenderService<BaseNodeServiceRequest, Result<BaseNodeServiceResponse, BaseNodeServiceError>>,
    event_stream_sender: BaseNodeEventSender,
}

impl BaseNodeServiceHandle {
    pub fn new(
        handle: SenderService<BaseNodeServiceRequest, Result<BaseNodeServiceResponse, BaseNodeServiceError>>,
        event_stream_sender: BaseNodeEventSender,
    ) -> Self {
        Self {
            handle,
            event_stream_sender,
        }
    }

    pub fn get_event_stream_fused(&self) -> Fuse<BaseNodeEventReceiver> {
        self.event_stream_sender.subscribe().fuse()
    }

    pub async fn get_chain_metadata(&mut self) -> Result<Option<ChainMetadata>, BaseNodeServiceError> {
        match self.handle.call(BaseNodeServiceRequest::GetChainMetadata).await?? {
            BaseNodeServiceResponse::ChainMetadata(metadata) => Ok(metadata),
            _ => Err(BaseNodeServiceError::UnexpectedApiResponse),
        }
    }

    pub async fn set_base_node_peer(&mut self, peer: Peer) -> Result<(), BaseNodeServiceError> {
        match self
            .handle
            .call(BaseNodeServiceRequest::SetBaseNodePeer(Box::new(peer)))
            .await??
        {
            BaseNodeServiceResponse::BaseNodePeerSet => Ok(()),
            _ => Err(BaseNodeServiceError::UnexpectedApiResponse),
        }
    }

    pub async fn get_base_node_peer(&mut self) -> Result<Option<Peer>, BaseNodeServiceError> {
        match self.handle.call(BaseNodeServiceRequest::GetBaseNodePeer).await?? {
            BaseNodeServiceResponse::BaseNodePeer(peer) => Ok(peer.map(|p| *p)),
            _ => Err(BaseNodeServiceError::UnexpectedApiResponse),
        }
    }

    pub async fn get_base_node_latency(&mut self) -> Result<Option<Duration>, BaseNodeServiceError> {
        match self.handle.call(BaseNodeServiceRequest::GetBaseNodeLatency).await?? {
            BaseNodeServiceResponse::Latency(latency) => Ok(latency),
            _ => Err(BaseNodeServiceError::UnexpectedApiResponse),
        }
    }
}
