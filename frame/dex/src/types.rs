// This file is part of Substrate.

// Copyright (C) 2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

#[derive(Encode, Decode, Default, PartialEq, Eq, MaxEncodedLen, TypeInfo)]
pub struct PoolInfo<AccountId, AssetId, Balance> {
	/// Owner of the pool
	pub owner: AccountId,
	/// LP asset
	pub lp_token: AssetId,
	/// The first asset supported by the pool
	pub asset1: AssetId,
	/// The second asset supported by the pool
	pub asset2: AssetId,
	/// Pool balance1
	pub balance1: Balance,
	/// Pool balance2
	pub balance2: Balance,
}
