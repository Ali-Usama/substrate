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

#![cfg_attr(not(feature = "std"), no_std)]

mod types;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mock;

pub use pallet::*;
pub use types::*;

// TODO: make it configurable
pub const MIN_LIQUIDITY: u64 = 1;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	use frame_support::{
		traits::{
			fungibles::{
				metadata::Mutate as MutateMetadata, Create, Inspect, InspectEnumerable, Mutate,
				Transfer,
			},
			Currency, ExistenceRequirement, ReservableCurrency,
		},
		transactional,
		PalletId,
	};
	use sp_runtime::traits::{
		AccountIdConversion, AtLeast32BitUnsigned, CheckedAdd, CheckedDiv, CheckedMul, CheckedSub,
		IntegerSquareRoot, One, Zero,
	};

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		type Currency: ReservableCurrency<Self::AccountId, Balance = Self::AssetBalance>;

		type AssetBalance: AtLeast32BitUnsigned
			+ codec::FullCodec
			+ Copy
			+ MaybeSerializeDeserialize
			+ sp_std::fmt::Debug
			+ Default
			+ From<u64>
			+ IntegerSquareRoot
			+ Zero
			+ TypeInfo
			+ MaxEncodedLen;

		type AssetId: Member
			+ Parameter
			+ Default
			+ Copy
			+ codec::HasCompact
			+ From<u32>
			+ MaybeSerializeDeserialize
			+ MaxEncodedLen
			+ PartialOrd
			+ TypeInfo;

		type Assets: Inspect<Self::AccountId, AssetId = Self::AssetId, Balance = Self::AssetBalance>
			+ Create<Self::AccountId>
			+ InspectEnumerable<Self::AccountId>
			+ Mutate<Self::AccountId>
			+ MutateMetadata<Self::AccountId>
			+ Transfer<Self::AccountId>;

		/// The dex's pallet id, used for deriving its sovereign account ID.
		#[pallet::constant]
		type PalletId: Get<PalletId>;
	}

	pub type BalanceOf<T> =
		<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

	pub type AssetIdOf<T> =
		<<T as Config>::Assets as Inspect<<T as frame_system::Config>::AccountId>>::AssetId;
	pub type AssetBalanceOf<T> =
		<<T as Config>::Assets as Inspect<<T as frame_system::Config>::AccountId>>::Balance;

	pub type PoolIdOf<T> = (AssetIdOf<T>, AssetIdOf<T>);

	#[pallet::storage]
	pub type Pools<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		PoolIdOf<T>,
		PoolInfo<T::AccountId, AssetIdOf<T>, AssetBalanceOf<T>>,
		OptionQuery,
	>;

	// Pallet's events.
	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		PoolCreated {
			creator: T::AccountId,
			pool_id: PoolIdOf<T>,
			lp_token: AssetIdOf<T>,
		},
		LiquidityAdded {
			who: T::AccountId,
			mint_to: T::AccountId,
			pool_id: PoolIdOf<T>,
			amount1_provided: AssetBalanceOf<T>,
			amount2_provided: AssetBalanceOf<T>,
			lp_token: AssetIdOf<T>,
			liquidity: AssetBalanceOf<T>,
		},
		LiquidityRemoved {
			who: T::AccountId,
			withdraw_to: T::AccountId,
			pool_id: PoolIdOf<T>,
			amount1: AssetBalanceOf<T>,
			amount2: AssetBalanceOf<T>,
			lp_token: AssetIdOf<T>,
			liquidity: AssetBalanceOf<T>,
		},
	}

	// Your Pallet's error messages.
	#[pallet::error]
	pub enum Error<T> {
		/// Provided assets are equal.
		EqualAssets,
		/// Pool already exists.
		PoolExists,
		/// Desired amount can't be zero.
		WrongDesiredAmount,
		/// The deadline has already passed.
		DeadlinePassed,
		/// The pool doesn't exist.
		PoolNotFound,
		/// An overflow happened.
		Overflow,
		/// Insufficient amount provided.
		InsufficientAmount,
		/// Optimal calculated amount is less than desired.
		OptimalAmountLessThanDesired,
		/// Insufficient liquidity minted.
		InsufficientLiquidityMinted,
		/// Asked liquidity can't be zero.
		ZeroLiquidity,
		/// Amount can't be zero.
		ZeroAmount,
		/// Calculated amount out is less than min desired.
		InsufficientOutputAmount,
		/// Insufficient liquidity in the pool.
		InsufficientLiquidity,
		/// Excessive input amount.
		ExcessiveInputAmount,
	}

	// Pallet's callable functions.
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(0)]
		#[transactional]
		pub fn setup(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let pallet_account = Self::account_id();
			T::Currency::transfer(
				&sender,
				&pallet_account,
				amount,
				ExistenceRequirement::KeepAlive,
			)?;

			T::Assets::create(1.into(), sender.clone(), true, 1u64.into())?;
			T::Assets::set(1.into(), &sender, "DOT".into(), "DOT".into(), 0)?;

			T::Assets::create(2.into(), sender.clone(), true, 1u64.into())?;
			T::Assets::set(2.into(), &sender, "USDC".into(), "USDC".into(), 0)?;

			T::Assets::mint_into(1.into(), &sender, 10000000000000000000u64.into())?;
			T::Assets::mint_into(2.into(), &sender, 10000000000000000000u64.into())?;

			Ok(())
		}

		#[pallet::weight(0)]
		#[transactional]
		pub fn create_pool(
			origin: OriginFor<T>,
			asset1: AssetIdOf<T>, // TODO: convert into MultiToken
			asset2: AssetIdOf<T>,
			lp_token: AssetIdOf<T>,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			ensure!(asset1 != asset2, Error::<T>::EqualAssets);

			let pool_id = Self::get_pool_id(asset1, asset2);
			let (asset1, asset2) = pool_id;
			ensure!(!Pools::<T>::contains_key(&pool_id), Error::<T>::PoolExists);

			let pallet_account = Self::account_id();
			T::Assets::create(lp_token, pallet_account.clone(), true, MIN_LIQUIDITY.into())?;
			T::Assets::set(lp_token, &pallet_account, "LP".into(), "LP".into(), 0)?;

			let pool_info = PoolInfo {
				owner: sender.clone(),
				lp_token,
				asset1,
				asset2,
				balance1: Zero::zero(),
				balance2: Zero::zero(),
			};

			Pools::<T>::insert(pool_id, pool_info);

			Self::deposit_event(Event::PoolCreated { creator: sender, pool_id, lp_token });

			Ok(())
		}

		#[pallet::weight(0)]
		#[transactional]
		pub fn add_liquidity(
			origin: OriginFor<T>,
			asset1: AssetIdOf<T>,
			asset2: AssetIdOf<T>,
			amount1_desired: AssetBalanceOf<T>,
			amount2_desired: AssetBalanceOf<T>,
			amount1_min: AssetBalanceOf<T>,
			amount2_min: AssetBalanceOf<T>,
			mint_to: T::AccountId,
			deadline: T::BlockNumber,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			let pool_id = Self::get_pool_id(asset1, asset2);
			let (asset1, asset2) = pool_id;

			ensure!(
				amount1_desired > Zero::zero() && amount2_desired > Zero::zero(),
				Error::<T>::WrongDesiredAmount
			);

			let now = frame_system::Pallet::<T>::block_number();
			ensure!(deadline >= now, Error::<T>::DeadlinePassed);

			Pools::<T>::try_mutate(&pool_id, |maybe_pool| {
				let pool = maybe_pool.as_mut().ok_or(Error::<T>::PoolNotFound)?;

				let amount1: AssetBalanceOf<T>;
				let amount2: AssetBalanceOf<T>;

				let reserve1 = pool.balance1;
				let reserve2 = pool.balance2;

				if reserve1.is_zero() && reserve2.is_zero() {
					amount1 = amount1_desired;
					amount2 = amount2_desired;
				} else {
					let amount2_optimal = Self::quote(&amount1_desired, &reserve1, &reserve2)?;

					if amount2_optimal <= amount2_desired {
						ensure!(amount2_optimal >= amount2_min, Error::<T>::InsufficientAmount);
						amount1 = amount1_desired;
						amount2 = amount2_optimal;
					} else {
						let amount1_optimal = Self::quote(&amount2_desired, &reserve2, &reserve1)?;
						ensure!(
							amount1_optimal <= amount1_desired,
							Error::<T>::OptimalAmountLessThanDesired
						);
						ensure!(amount1_optimal >= amount1_min, Error::<T>::InsufficientAmount);
						amount1 = amount1_optimal;
						amount2 = amount2_desired;
					}
				}

				let pallet_account = Self::account_id();
				T::Assets::transfer(asset1, &sender, &pallet_account, amount1, false)?;
				T::Assets::transfer(asset2, &sender, &pallet_account, amount2, false)?;

				let total_supply = T::Assets::total_issuance(pool.lp_token);

				let liquidity: AssetBalanceOf<T>;
				if total_supply.is_zero() {
					liquidity = amount1
						.checked_mul(&amount2)
						.ok_or(Error::<T>::Overflow)?
						.integer_sqrt()
						.checked_sub(&MIN_LIQUIDITY.into())
						.ok_or(Error::<T>::Overflow)?;
					T::Assets::mint_into(pool.lp_token, &pallet_account, MIN_LIQUIDITY.into())?;
				} else {
					let side1 = amount1
						.checked_mul(&total_supply)
						.ok_or(Error::<T>::Overflow)?
						.checked_div(&reserve1)
						.ok_or(Error::<T>::Overflow)?;

					let side2 = amount2
						.checked_mul(&total_supply)
						.ok_or(Error::<T>::Overflow)?
						.checked_div(&reserve2)
						.ok_or(Error::<T>::Overflow)?;

					liquidity = side1.min(side2);
				}

				ensure!(liquidity > MIN_LIQUIDITY.into(), Error::<T>::InsufficientLiquidityMinted);

				T::Assets::mint_into(pool.lp_token, &mint_to, liquidity)?;

				pool.balance1 = reserve1 + amount1;
				pool.balance2 = reserve2 + amount2;

				Self::deposit_event(Event::LiquidityAdded {
					who: sender,
					mint_to,
					pool_id,
					amount1_provided: amount1,
					amount2_provided: amount2,
					lp_token: pool.lp_token,
					liquidity,
				});

				Ok(())
			})
		}

		#[pallet::weight(0)]
		#[transactional]
		pub fn remove_liquidity(
			origin: OriginFor<T>,
			asset1: AssetIdOf<T>,
			asset2: AssetIdOf<T>,
			liquidity: AssetBalanceOf<T>,
			amount1_min: AssetBalanceOf<T>,
			amount2_min: AssetBalanceOf<T>,
			withdraw_to: T::AccountId,
			deadline: T::BlockNumber,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			let pool_id = Self::get_pool_id(asset1, asset2);
			let (asset1, asset2) = pool_id;

			ensure!(liquidity > Zero::zero(), Error::<T>::ZeroLiquidity);

			let now = frame_system::Pallet::<T>::block_number();
			ensure!(deadline >= now, Error::<T>::DeadlinePassed);

			Pools::<T>::try_mutate(&pool_id, |maybe_pool| {
				let pool = maybe_pool.as_mut().ok_or(Error::<T>::PoolNotFound)?;

				let pallet_account = Self::account_id();
				T::Assets::transfer(pool.lp_token, &sender, &pallet_account, liquidity, false)?;

				let reserve1 = pool.balance1;
				let reserve2 = pool.balance2;

				let total_supply = T::Assets::total_issuance(pool.lp_token);

				let amount1 = liquidity
					.checked_mul(&reserve1)
					.ok_or(Error::<T>::Overflow)?
					.checked_div(&total_supply)
					.ok_or(Error::<T>::Overflow)?;

				let amount2 = liquidity
					.checked_mul(&reserve2)
					.ok_or(Error::<T>::Overflow)?
					.checked_div(&total_supply)
					.ok_or(Error::<T>::Overflow)?;

				ensure!(
					!amount1.is_zero() && amount1 >= amount1_min,
					Error::<T>::InsufficientAmount
				);
				ensure!(
					!amount2.is_zero() && amount2 >= amount2_min,
					Error::<T>::InsufficientAmount
				);

				T::Assets::burn_from(pool.lp_token, &pallet_account, liquidity)?;

				T::Assets::transfer(asset1, &pallet_account, &withdraw_to, amount1, false)?;
				T::Assets::transfer(asset2, &pallet_account, &withdraw_to, amount2, false)?;

				pool.balance1 = reserve1 - amount1;
				pool.balance2 = reserve2 - amount2;

				Self::deposit_event(Event::LiquidityRemoved {
					who: sender,
					withdraw_to,
					pool_id,
					amount1,
					amount2,
					lp_token: pool.lp_token,
					liquidity,
				});

				Ok(())
			})
		}

		#[pallet::weight(0)]
		#[transactional]
		pub fn swap_exact_tokens_for_tokens(
			origin: OriginFor<T>,
			asset1: AssetIdOf<T>,
			asset2: AssetIdOf<T>,
			amount_in: AssetBalanceOf<T>,
			amount_out_min: AssetBalanceOf<T>,
			send_to: T::AccountId,
			deadline: T::BlockNumber,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			let pool_id = Self::get_pool_id(asset1, asset2);

			ensure!(
				amount_in > Zero::zero() && amount_out_min > Zero::zero(),
				Error::<T>::ZeroAmount
			);

			let now = frame_system::Pallet::<T>::block_number();
			ensure!(deadline >= now, Error::<T>::DeadlinePassed);

			Pools::<T>::try_mutate(&pool_id, |maybe_pool| {
				let pool = maybe_pool.as_mut().ok_or(Error::<T>::PoolNotFound)?;

				let reserve_in = if asset1 == pool.asset1 { pool.balance1 } else { pool.balance2 };
				let reserve_out = if asset2 == pool.asset2 { pool.balance2 } else { pool.balance1 };

				let amount1 = amount_in;
				let amount2 = Self::get_amount_out(&amount1, &reserve_in, &reserve_out)?;

				ensure!(amount2 >= amount_out_min, Error::<T>::InsufficientOutputAmount);

				let pallet_account = Self::account_id();
				T::Assets::transfer(asset1, &sender, &pallet_account, amount1, false)?;

				let (send_asset, send_amount) =
					Self::validate_swap(asset1, amount2, pool_id, pool.balance1, pool.balance2)?;

				T::Assets::transfer(send_asset, &pallet_account, &send_to, send_amount, false)?;

				if send_asset == pool.asset1 {
					pool.balance1 -= send_amount;
					pool.balance2 += amount1;
				} else {
					pool.balance2 -= send_amount;
					pool.balance1 += amount1;
				}

				Ok(())
			})
		}

		#[pallet::weight(0)]
		#[transactional]
		pub fn swap_tokens_for_exact_tokens(
			origin: OriginFor<T>,
			asset1: AssetIdOf<T>,
			asset2: AssetIdOf<T>,
			amount_out: AssetBalanceOf<T>,
			amount_in_max: AssetBalanceOf<T>,
			send_to: T::AccountId,
			deadline: T::BlockNumber,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			let pool_id = Self::get_pool_id(asset1, asset2);

			ensure!(
				amount_out > Zero::zero() && amount_in_max > Zero::zero(),
				Error::<T>::ZeroAmount
			);

			let now = frame_system::Pallet::<T>::block_number();
			ensure!(deadline >= now, Error::<T>::DeadlinePassed);

			Pools::<T>::try_mutate(&pool_id, |maybe_pool| {
				let pool = maybe_pool.as_mut().ok_or(Error::<T>::PoolNotFound)?;

				let reserve_in = if asset1 == pool.asset1 { pool.balance1 } else { pool.balance2 };
				let reserve_out = if asset2 == pool.asset2 { pool.balance2 } else { pool.balance1 };

				let amount2 = amount_out;
				let amount1 = Self::get_amount_in(&amount2, &reserve_in, &reserve_out)?;
				ensure!(amount1 <= amount_in_max, Error::<T>::ExcessiveInputAmount);

				let pallet_account = Self::account_id();
				T::Assets::transfer(asset1, &sender, &pallet_account, amount1, false)?;

				let (send_asset, send_amount) =
					Self::validate_swap(asset1, amount2, pool_id, pool.balance1, pool.balance2)?;

				T::Assets::transfer(send_asset, &pallet_account, &send_to, send_amount, false)?;

				if send_asset == pool.asset1 {
					pool.balance1 -= send_amount;
					pool.balance2 += amount1;
				} else {
					pool.balance2 -= send_amount;
					pool.balance1 += amount1;
				}

				Ok(())
			})
		}
	}

	// Your Pallet's internal functions.
	impl<T: Config> Pallet<T> {
		/// The account ID of the dex pallet.
		///
		/// This actually does computation. If you need to keep using it, then make sure you cache
		/// the value and only call this once.
		pub fn account_id() -> T::AccountId {
			T::PalletId::get().into_account_truncating()
		}

		/// Returns a pool id constructed from 2 sorted assets.
		pub fn get_pool_id(asset1: AssetIdOf<T>, asset2: AssetIdOf<T>) -> PoolIdOf<T> {
			if asset1 <= asset2 {
				(asset1, asset2)
			} else {
				(asset2, asset1)
			}
		}

		pub fn quote_price(asset1: u32, asset2: u32) -> Option<AssetBalanceOf<T>> {
			let asset1 = asset1.into();
			let asset2 = asset2.into();
			let pool_id = Self::get_pool_id(asset1, asset2);
			let (pool_asset1, _) = pool_id;

			if let Some(pool) = Pools::<T>::get(pool_id) {
				let (reserve1, reserve2) = if asset1 == pool_asset1 {
					(pool.balance1, pool.balance2)
				} else {
					(pool.balance2, pool.balance1)
				};
				Self::quote(&One::one(), &reserve1, &reserve2).ok()
			} else {
				None
			}
		}

		// TODO: we should probably use u128 for calculations
		/// Calculates the optimal amount from the reserves.
		pub fn quote(
			amount: &AssetBalanceOf<T>,
			reserve1: &AssetBalanceOf<T>,
			reserve2: &AssetBalanceOf<T>,
		) -> Result<AssetBalanceOf<T>, Error<T>> {
			// amount * reserve2 / reserve1
			amount
				.checked_mul(&reserve2)
				.ok_or(Error::<T>::Overflow)?
				.checked_div(&reserve1)
				.ok_or(Error::<T>::Overflow)

			/*let amount2 = u128::from(amount)
				.checked_mul(u128::from(reserve2))
				.ok_or(Error::<T>::Overflow)?
				.checked_div(u128::from(reserve1))
				.ok_or(Error::<T>::Overflow)?;

			u64::try_from(amount2).map(|x| x.into()).map_err(|_| Error::<T>::Overflow)*/
		}

		/// Calculates amount out
		///
		/// Given an input amount of an asset and pair reserves, returns the maximum output amount
		/// of the other asset
		pub fn get_amount_out(
			amount_in: &AssetBalanceOf<T>,
			reserve_in: &AssetBalanceOf<T>,
			reserve_out: &AssetBalanceOf<T>,
		) -> Result<AssetBalanceOf<T>, Error<T>> {
			if reserve_in.is_zero() || reserve_out.is_zero() {
				return Err(Error::<T>::InsufficientLiquidity.into())
			}

			let amount_in_with_fee =
				amount_in.checked_mul(&997u64.into()).ok_or(Error::<T>::Overflow)?;
			let numerator =
				amount_in_with_fee.checked_mul(reserve_out).ok_or(Error::<T>::Overflow)?;
			let denominator = reserve_in
				.checked_mul(&1000u64.into())
				.ok_or(Error::<T>::Overflow)?
				.checked_add(&amount_in_with_fee)
				.ok_or(Error::<T>::Overflow)?;

			numerator.checked_div(&denominator).ok_or(Error::<T>::Overflow)
		}

		/// Calculates amount in
		///
		/// Given an output amount of an asset and pair reserves, returns a required input amount
		/// of the other asset
		pub fn get_amount_in(
			amount_out: &AssetBalanceOf<T>,
			reserve_in: &AssetBalanceOf<T>,
			reserve_out: &AssetBalanceOf<T>,
		) -> Result<AssetBalanceOf<T>, Error<T>> {
			if reserve_in.is_zero() || reserve_out.is_zero() {
				return Err(Error::<T>::InsufficientLiquidity.into())
			}

			// uint numerator = reserveIn.mul(amountOut).mul(1000);
			// uint denominator = reserveOut.sub(amountOut).mul(997);
			// amountIn = (numerator / denominator).add(1);

			let numerator = reserve_in
				.checked_mul(amount_out)
				.ok_or(Error::<T>::Overflow)?
				.checked_mul(&1000u64.into())
				.ok_or(Error::<T>::Overflow)?;

			let denominator = reserve_out
				.checked_sub(amount_out)
				.ok_or(Error::<T>::Overflow)?
				.checked_mul(&997u64.into())
				.ok_or(Error::<T>::Overflow)?;

			numerator
				.checked_div(&denominator)
				.ok_or(Error::<T>::Overflow)?
				.checked_add(&One::one())
				.ok_or(Error::<T>::Overflow)
		}

		pub fn validate_swap(
			asset_from: AssetIdOf<T>,
			amount_out: AssetBalanceOf<T>,
			pool_id: PoolIdOf<T>,
			reserve1: AssetBalanceOf<T>,
			reserve2: AssetBalanceOf<T>,
		) -> Result<(AssetIdOf<T>, AssetBalanceOf<T>), Error<T>> {
			let (pool_asset1, pool_asset2) = pool_id;

			if asset_from == pool_asset1 {
				ensure!(amount_out < reserve2, Error::<T>::InsufficientLiquidity);
				Ok((pool_asset2, amount_out))
			} else {
				ensure!(amount_out < reserve1, Error::<T>::InsufficientLiquidity);
				Ok((pool_asset1, amount_out))
			}
		}
	}
}
