// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Substrate RPC implementation.
//!
//! A core implementation of Substrate RPC interfaces.

#![warn(missing_docs)]

pub use jsonrpsee::core::{
	id_providers::{
		RandomIntegerIdProvider as RandomIntegerSubscriptionId,
		RandomStringIdProvider as RandomStringSubscriptionId,
	},
	traits::IdProvider as RpcSubscriptionIdProvider,
};
pub use sc_rpc_api::DenyUnsafe;

pub mod author;
pub mod chain;
pub mod dev;
pub mod offchain;
pub mod state;
pub mod system;

#[cfg(any(test, feature = "test-helpers"))]
pub mod testing;

/// Task executor that is being used by RPC subscriptions.
pub type SubscriptionTaskExecutor = std::sync::Arc<dyn sp_core::traits::SpawnNamed>;

/// JSON-RPC helpers.
pub mod utils {
	use futures::{Stream, StreamExt};
	use jsonrpsee::{
		IntoSubscriptionCloseResponse, PendingSubscriptionSink, SendTimeoutError,
		SubscriptionCloseResponse, SubscriptionMessage, SubscriptionSink,
	};
	use sp_runtime::Serialize;

	/// Similar to [`pipe_from_stream`] but also attempts to accept the subscription.
	pub async fn accept_and_pipe_from_stream<S, T, R>(
		pending: PendingSubscriptionSink,
		stream: S,
	) -> SubscriptionResponse<R>
	where
		S: Stream<Item = T> + Unpin,
		T: Serialize,
		R: Serialize,
	{
		let Ok(sink )= pending.accept().await else {
			return SubscriptionResponse::Closed
		};
		pipe_from_stream(sink, stream).await
	}

	/// Feed items to the subscription from the underlying stream
	/// if the subscription can't keep up with the underlying stream
	/// it's dropped
	///
	/// This is simply a way to keep previous behaviour with unbounded streams
	/// and should be replaced by specific RPC endpoint behaviour.
	pub async fn pipe_from_stream<S, T, R>(
		sink: SubscriptionSink,
		mut stream: S,
	) -> SubscriptionResponse<R>
	where
		S: Stream<Item = T> + Unpin,
		T: Serialize,
		R: Serialize,
	{
		loop {
			tokio::select! {
				biased;
				_ = sink.closed() => break SubscriptionResponse::Closed,

				maybe_item = stream.next() => {
					let item = match maybe_item {
						Some(item) => item,
						None => break SubscriptionResponse::Closed,
					};

					match sink.send_timeout(crate::utils::to_sub_message(&item), std::time::Duration::from_secs(60)).await {
						Ok(_) => (),
						Err(SendTimeoutError::Closed(_)) | Err(SendTimeoutError::Timeout(_)) => break SubscriptionResponse::Closed,
					}
				}
			}
		}
	}

	/// Subscription response type for substrate.
	pub enum SubscriptionResponse<T> {
		/// The subscription was closed, no further message is sent.
		Closed,
		/// Send out a notification.
		Event(T),
	}

	impl<T: Serialize> IntoSubscriptionCloseResponse for SubscriptionResponse<T> {
		fn into_response(self) -> SubscriptionCloseResponse {
			match self {
				Self::Closed => SubscriptionCloseResponse::None,
				Self::Event(ev) => SubscriptionCloseResponse::Notif(to_sub_message(&ev)),
			}
		}
	}

	/// Build a subscription message.
	///
	/// # Panics
	///
	/// This function panics if the `Serialize` fails and is treated a bug.
	pub fn to_sub_message(val: &impl Serialize) -> SubscriptionMessage {
		SubscriptionMessage::from_json(val).expect("JSON serialization infallible; qed")
	}
}
