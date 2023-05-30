// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
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

//! # Treasury Pallet
//!
//! The Treasury pallet provides a "pot" of funds that can be managed by stakeholders in the system
//! and a structure for making spending proposals from this pot.
//!
//! - [`Config`]
//! - [`Call`]
//!
//! ## Overview
//!
//! The Treasury Pallet itself provides the pot to store funds, and a means for stakeholders to
//! propose, approve, and deny expenditures. The chain will need to provide a method (e.g.
//! inflation, fees) for collecting funds.
//!
//! By way of example, the Council could vote to fund the Treasury with a portion of the block
//! reward and use the funds to pay developers.
//!
//!
//! ### Terminology
//!
//! - **Proposal:** A suggestion to allocate funds from the pot to a beneficiary.
//! - **Beneficiary:** An account who will receive the funds from a proposal iff the proposal is
//!   approved.
//! - **Deposit:** Funds that a proposer must lock when making a proposal. The deposit will be
//!   returned or slashed if the proposal is approved or rejected respectively.
//! - **Pot:** Unspent funds accumulated by the treasury pallet.
//!
//! ## Interface
//!
//! ### Dispatchable Functions
//!
//! General spending/proposal protocol:
//! - `propose_spend` - Make a spending proposal and stake the required deposit.
//! - `reject_proposal` - Reject a proposal, slashing the deposit.
//! - `approve_proposal` - Accept the proposal, returning the deposit.
//! - `remove_approval` - Remove an approval, the deposit will no longer be returned.
//!
//! ## GenesisConfig
//!
//! The Treasury pallet depends on the [`GenesisConfig`].

#![cfg_attr(not(feature = "std"), no_std)]

mod benchmarking;
#[cfg(test)]
mod tests;
pub mod weights;

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use sp_runtime::{
	traits::{AccountIdConversion, CheckedAdd, Saturating, StaticLookup, Zero},
	Permill, RuntimeDebug,
};
use sp_std::{collections::btree_map::BTreeMap, prelude::*};

use frame_support::{
	log, print,
	traits::{
		tokens::{AssetId, ConversionFromAssetBalance, Pay, PaymentStatus},
		Currency,
		ExistenceRequirement::KeepAlive,
		Get, Imbalance, OnUnbalanced, ReservableCurrency, WithdrawReasons,
	},
	weights::Weight,
	PalletId,
};

pub use pallet::*;
pub use weights::WeightInfo;

const LOG_TARGET: &str = "runtime::treasury";

pub type PayBalanceOf<T, I> = <<T as Config<I>>::Paymaster as Pay>::Balance;
pub type BalanceOf<T, I = ()> =
	<<T as Config<I>>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
pub type PositiveImbalanceOf<T, I = ()> = <<T as Config<I>>::Currency as Currency<
	<T as frame_system::Config>::AccountId,
>>::PositiveImbalance;
pub type NegativeImbalanceOf<T, I = ()> = <<T as Config<I>>::Currency as Currency<
	<T as frame_system::Config>::AccountId,
>>::NegativeImbalance;
type AccountIdLookupOf<T> = <<T as frame_system::Config>::Lookup as StaticLookup>::Source;

/// A trait to allow the Treasury Pallet to spend it's funds for other purposes.
/// There is an expectation that the implementer of this trait will correctly manage
/// the mutable variables passed to it:
/// * `budget_remaining`: How much available funds that can be spent by the treasury. As funds are
///   spent, you must correctly deduct from this value.
/// * `imbalance`: Any imbalances that you create should be subsumed in here to maximize efficiency
///   of updating the total issuance. (i.e. `deposit_creating`)
/// * `total_weight`: Track any weight that your `spend_fund` implementation uses by updating this
///   value.
/// * `missed_any`: If there were items that you want to spend on, but there were not enough funds,
///   mark this value as `true`. This will prevent the treasury from burning the excess funds.
#[impl_trait_for_tuples::impl_for_tuples(30)]
pub trait SpendFunds<T: Config<I>, I: 'static = ()> {
	fn spend_funds(
		budget_remaining: &mut BalanceOf<T, I>,
		imbalance: &mut PositiveImbalanceOf<T, I>,
		total_weight: &mut Weight,
		missed_any: &mut bool,
	);
}

pub trait Asset<AssetId, Fungibility> {
	fn asset_kind(&self) -> AssetId;
	fn amount(&self) -> Fungibility;
}

/// An index of a proposal. Just a `u32`.
pub type ProposalIndex = u32;

/// An index of a pending payment. Just a `u32`.
pub type PendingPaymentIndex = u32;

/// An index for the number tor times a payment is retried. Just a u32
pub type RetryIndex = u32;

/// A spending proposal.
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
#[derive(Encode, Decode, Clone, PartialEq, Eq, MaxEncodedLen, RuntimeDebug, TypeInfo)]
pub struct Proposal<AccountId, Balance> {
	/// The account proposing it.
	proposer: AccountId,
	/// The (total) amount that should be paid if the proposal is accepted.
	value: Balance,
	/// The account to whom the payment should be made if the proposal is accepted.
	beneficiary: AccountId,
	/// The amount held on deposit (reserved) for making this proposal.
	bond: Balance,
}

/// PendingPayment represents treasury spend payment which has not yet succeeded.
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
#[derive(Encode, Decode, Clone, PartialEq, Eq, MaxEncodedLen, RuntimeDebug, TypeInfo)]
pub struct PendingPayment<AccountId, Balance, AssetKind, PaymentId> {
	/// The account to whom the payment should be made if the proposal is accepted.
	beneficiary: AccountId,
	/// The asset_kind of the amount to be paid
	asset_kind: AssetKind,
	/// The (total) amount that should be paid.
	// value: AssetBalance,
	/// The amount to be paid, but normalized to the native asset class
	normalized_value: Balance,
	/// The identifier for tracking the status of a payment which is in flight.
	payment_id: Option<PaymentId>,
	/// The number of times this payment has been attempted
	tries: RetryIndex,
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{dispatch_context::with_context, pallet_prelude::*};
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(PhantomData<(T, I)>);

	#[cfg(feature = "runtime-benchmarks")]
	pub trait BenchmarkHelper<AssetKind> {
		fn create_asset_kind(id: u32) -> AssetKind;
	}
	#[cfg(feature = "runtime-benchmarks")]
	impl<AssetKind: From<u32>> BenchmarkHelper<AssetKind> for () {
		fn create_asset_kind(_id: u32) -> AssetKind {
			0u32.into()
		}
	}
	#[cfg(feature = "runtime-benchmarks")]
	pub struct NilBenchmarkHelper;
	#[cfg(feature = "runtime-benchmarks")]
	impl<AssetKind: From<()>> BenchmarkHelper<AssetKind> for NilBenchmarkHelper {
		fn create_asset_kind(_id: u32) -> AssetKind {
			().into()
		}
	}

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config {
		/// The staking balance.
		type Currency: Currency<Self::AccountId> + ReservableCurrency<Self::AccountId>;

		/// Origin from which approvals must come.
		type ApproveOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// Origin from which rejections must come.
		type RejectOrigin: EnsureOrigin<Self::RuntimeOrigin>;

		/// The identifier for what asset should be spent.
		type AssetId: AssetId;

		// TODO: replace with individual types
		type AssetKind: Asset<Self::AssetId, PayBalanceOf<Self, I>> + AssetId;

		/// The means by which we can make payments to beneficiaries.
		/// This can be implmented over fungibles or some other means.
		type Paymaster: Pay<Beneficiary = Self::AccountId, AssetKind = Self::AssetId>;

		type MaxPaymentRetries: Get<RetryIndex>;

		// The means of knowing what is the equivalent native Balance of a given asset id Balance.
		type BalanceConverter: ConversionFromAssetBalance<
			PayBalanceOf<Self, I>,
			Self::AssetKind,
			BalanceOf<Self, I>,
		>;

		/// The overarching event type.
		type RuntimeEvent: From<Event<Self, I>>
			+ IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Handler for the unbalanced decrease when slashing for a rejected proposal or bounty.
		type OnSlash: OnUnbalanced<NegativeImbalanceOf<Self, I>>;

		/// Fraction of a proposal's value that should be bonded in order to place the proposal.
		/// An accepted proposal gets these back. A rejected proposal does not.
		#[pallet::constant]
		type ProposalBond: Get<Permill>;

		/// Minimum amount of funds that should be placed in a deposit for making a proposal.
		#[pallet::constant]
		type ProposalBondMinimum: Get<BalanceOf<Self, I>>;

		/// Maximum amount of funds that should be placed in a deposit for making a proposal.
		#[pallet::constant]
		type ProposalBondMaximum: Get<Option<BalanceOf<Self, I>>>;

		/// Period between successive spends.
		#[pallet::constant]
		type SpendPeriod: Get<Self::BlockNumber>;

		/// Percentage of spare funds (if any) that are burnt per spend period.
		#[pallet::constant]
		type Burn: Get<Permill>;

		/// The treasury's pallet id, used for deriving its sovereign account ID.
		#[pallet::constant]
		type PalletId: Get<PalletId>;

		/// Handler for the unbalanced decrease when treasury funds are burned.
		type BurnDestination: OnUnbalanced<NegativeImbalanceOf<Self, I>>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;

		/// Runtime hooks to external pallet using treasury to compute spend funds.
		type SpendFunds: SpendFunds<Self, I>;

		/// The maximum number of approvals that can wait in the spending queue.
		///
		/// NOTE: This parameter is also used within the Bounties Pallet extension if enabled.
		#[pallet::constant]
		type MaxApprovals: Get<u32>;

		/// The origin required for approving spends from the treasury outside of the proposal
		/// process. The `Success` value is the maximum amount that this origin is allowed to
		/// spend at a time.
		type SpendOrigin: EnsureOrigin<Self::RuntimeOrigin, Success = BalanceOf<Self, I>>;

		/// Helper trait for benchmarks.
		#[cfg(feature = "runtime-benchmarks")]
		type BenchmarkHelper: BenchmarkHelper<Self::AssetKind>;
	}

	/// Number of proposals that have been made.
	#[pallet::storage]
	#[pallet::getter(fn proposal_count)]
	pub(crate) type ProposalCount<T, I = ()> = StorageValue<_, ProposalIndex, ValueQuery>;

	/// Proposals that have been made.
	#[pallet::storage]
	#[pallet::getter(fn proposals)]
	pub type Proposals<T: Config<I>, I: 'static = ()> = StorageMap<
		_,
		Twox64Concat,
		ProposalIndex,
		Proposal<T::AccountId, BalanceOf<T, I>>,
		OptionQuery,
	>;

	/// PendingPaymentsInbox that have not yet processed or are not yet successful.
	/// When a `PendingPayment` is processed or paid out, it is moved to the PendingPayments
	/// storage where it is monitored until it is successful.
	#[pallet::storage]
	pub type PendingPaymentsInbox<T: Config<I>, I: 'static = ()> = CountedStorageMap<
		_,
		Twox64Concat,
		PendingPaymentIndex,
		PendingPayment<T::AccountId, BalanceOf<T, I>, T::AssetKind, <T::Paymaster as Pay>::Id>,
		OptionQuery,
	>;

	/// PendingPayments that have are not yet successful. When a `PendingPayment` is verified to be
	/// successful, it is deleted from storage.
	#[pallet::storage]
	pub type PendingPayments<T: Config<I>, I: 'static = ()> = CountedStorageMap<
		_,
		Twox64Concat,
		PendingPaymentIndex,
		PendingPayment<T::AccountId, BalanceOf<T, I>, T::AssetKind, <T::Paymaster as Pay>::Id>,
		OptionQuery,
	>;

	/// The amount which has been reported as inactive to Currency.
	#[pallet::storage]
	pub type Deactivated<T: Config<I>, I: 'static = ()> =
		StorageValue<_, BalanceOf<T, I>, ValueQuery>;

	/// Proposal indices that have been approved but not yet awarded.
	#[pallet::storage]
	#[pallet::getter(fn approvals)]
	pub type Approvals<T: Config<I>, I: 'static = ()> =
		StorageValue<_, BoundedVec<ProposalIndex, T::MaxApprovals>, ValueQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig;

	#[cfg(feature = "std")]
	impl Default for GenesisConfig {
		fn default() -> Self {
			Self
		}
	}

	#[cfg(feature = "std")]
	impl GenesisConfig {
		/// Direct implementation of `GenesisBuild::assimilate_storage`.
		#[deprecated(
			note = "use `<GensisConfig<T, I> as GenesisBuild<T, I>>::assimilate_storage` instead"
		)]
		pub fn assimilate_storage<T: Config<I>, I: 'static>(
			&self,
			storage: &mut sp_runtime::Storage,
		) -> Result<(), String> {
			<Self as GenesisBuild<T, I>>::assimilate_storage(self, storage)
		}
	}

	#[pallet::genesis_build]
	impl<T: Config<I>, I: 'static> GenesisBuild<T, I> for GenesisConfig {
		fn build(&self) {
			// Create Treasury account
			let account_id = <Pallet<T, I>>::account_id();
			let min = T::Currency::minimum_balance();
			if T::Currency::free_balance(&account_id) < min {
				let _ = T::Currency::make_free_balance_be(&account_id, min);
			}
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config<I>, I: 'static = ()> {
		/// New proposal.
		Proposed { proposal_index: ProposalIndex },
		/// We have ended a spend period and will now allocate funds.
		Spending { budget_remaining: BalanceOf<T, I> },
		/// Some funds have been allocated.
		Awarded { proposal_index: ProposalIndex, award: BalanceOf<T, I>, account: T::AccountId },
		/// A proposal was rejected; funds were slashed.
		Rejected { proposal_index: ProposalIndex, slashed: BalanceOf<T, I> },
		/// Some of our funds have been burnt.
		Burnt { burnt_funds: BalanceOf<T, I> },
		/// Spending has finished; this is the amount that rolls over until next spend.
		Rollover { rollover_balance: BalanceOf<T, I> },
		/// Some funds have been deposited.
		Deposit { value: BalanceOf<T, I> },
		/// We have ended a spend period and will now allocate funds.
		ProcessingProposals { waiting_proposals: ProposalIndex },
		/// Spending has finished; this is the number of proposals rolled over till next
		/// T::SpendPeriod.
		RolloverPayments {
			rollover_payments: ProposalIndex,
			allocated_payments: PendingPaymentIndex,
		},
		/// A new spend proposal has been approved.
		SpendApproved {
			proposal_index: ProposalIndex,
			amount: BalanceOf<T, I>,
			beneficiary: T::AccountId,
		},
		/// The inactive funds of the pallet have been updated.
		UpdatedInactive { reactivated: BalanceOf<T, I>, deactivated: BalanceOf<T, I> },
		/// The payment has been queued to be paid out at the next Spend Period
		PaymentQueued {
			pending_payment_index: PendingPaymentIndex,
			asset_kind: T::AssetKind,
			beneficiary: T::AccountId,
		},
		/// The payment has been processed but awaiting payment status.
		PaymentTriggered {
			pending_payment_index: PendingPaymentIndex,
			asset_kind: T::AssetKind,
			payment_id: <T::Paymaster as Pay>::Id,
			tries: RetryIndex,
		},
		/// The proposal was paid successfully
		PaymentSuccess {
			pending_payment_index: PendingPaymentIndex,
			asset_kind: T::AssetKind,
			payment_id: <T::Paymaster as Pay>::Id,
			tries: RetryIndex,
		},
		/// The proposal payment failed. Payment will be retried in next spend period.
		PaymentFailure {
			pending_payment_index: PendingPaymentIndex,
			asset_kind: T::AssetKind,
			payment_id: Option<<T::Paymaster as Pay>::Id>,
			tries: RetryIndex,
		},
	}

	/// Error for the treasury pallet.
	#[pallet::error]
	pub enum Error<T, I = ()> {
		/// Proposer's balance is too low.
		InsufficientProposersBalance,
		/// No proposal or bounty at that index.
		InvalidIndex,
		/// Too many approvals in the queue.
		TooManyApprovals,
		/// The spend origin is valid but the amount it is allowed to spend is lower than the
		/// amount to be spent.
		InsufficientPermission,
		/// Proposal has not been approved.
		ProposalNotApproved,
		/// Unable to convert asset to native balance
		BalanceConversionFailed,
		/// Invalid Spend Request
		InvalidSpendRequest,
	}

	#[pallet::hooks]
	impl<T: Config<I>, I: 'static> Hooks<BlockNumberFor<T>> for Pallet<T, I> {
		/// ## Complexity
		/// - `O(A)` where `A` is the number of approvals
		fn on_initialize(n: T::BlockNumber) -> Weight {
			let pot = Self::pot();
			let deactivated = Deactivated::<T, I>::get();
			if pot != deactivated {
				T::Currency::reactivate(deactivated);
				T::Currency::deactivate(pot);
				Deactivated::<T, I>::put(&pot);
				Self::deposit_event(Event::<T, I>::UpdatedInactive {
					reactivated: deactivated,
					deactivated: pot,
				});
			}
			// Check to see if we should spend some funds!
			if (n % T::SpendPeriod::get()).is_zero() {
				Self::spend_funds().saturating_add(Self::spend_funds_local())
			} else {
				Weight::zero()
			}
		}
	}

	#[derive(Default)]
	struct SpendContext<Balance> {
		spend_in_context: BTreeMap<Balance, Balance>,
	}

	#[pallet::call]
	impl<T: Config<I>, I: 'static> Pallet<T, I> {
		/// Put forward a suggestion for spending. A deposit proportional to the value
		/// is reserved and slashed if the proposal is rejected. It is returned once the
		/// proposal is awarded.
		///
		/// ## Complexity
		/// - O(1)
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::propose_spend())]
		pub fn propose_spend(
			origin: OriginFor<T>,
			#[pallet::compact] value: BalanceOf<T, I>,
			beneficiary: AccountIdLookupOf<T>,
		) -> DispatchResult {
			let proposer = ensure_signed(origin)?;
			let beneficiary = T::Lookup::lookup(beneficiary)?;

			let bond = Self::calculate_bond(value);
			T::Currency::reserve(&proposer, bond)
				.map_err(|_| Error::<T, I>::InsufficientProposersBalance)?;

			let c = Self::proposal_count();
			<ProposalCount<T, I>>::put(c + 1);
			<Proposals<T, I>>::insert(c, Proposal { proposer, value, beneficiary, bond });

			Self::deposit_event(Event::Proposed { proposal_index: c });
			Ok(())
		}

		/// Reject a proposed spend. The original deposit will be slashed.
		///
		/// May only be called from `T::RejectOrigin`.
		///
		/// ## Complexity
		/// - O(1)
		#[pallet::call_index(1)]
		#[pallet::weight((T::WeightInfo::reject_proposal(), DispatchClass::Operational))]
		pub fn reject_proposal(
			origin: OriginFor<T>,
			#[pallet::compact] proposal_id: ProposalIndex,
		) -> DispatchResult {
			T::RejectOrigin::ensure_origin(origin)?;

			let proposal =
				<Proposals<T, I>>::take(&proposal_id).ok_or(Error::<T, I>::InvalidIndex)?;
			let value = proposal.bond;
			let imbalance = T::Currency::slash_reserved(&proposal.proposer, value).0;
			T::OnSlash::on_unbalanced(imbalance);

			Self::deposit_event(Event::<T, I>::Rejected {
				proposal_index: proposal_id,
				slashed: value,
			});
			Ok(())
		}

		/// Approve a proposal. At a later time, the proposal will be allocated to the beneficiary
		/// and the original deposit will be returned.
		///
		/// May only be called from `T::ApproveOrigin`.
		///
		/// ## Complexity
		///  - O(1).
		#[pallet::call_index(2)]
		#[pallet::weight((T::WeightInfo::approve_proposal(T::MaxApprovals::get()), DispatchClass::Operational))]
		pub fn approve_proposal(
			origin: OriginFor<T>,
			#[pallet::compact] proposal_id: ProposalIndex,
		) -> DispatchResult {
			T::ApproveOrigin::ensure_origin(origin)?;

			ensure!(<Proposals<T, I>>::contains_key(proposal_id), Error::<T, I>::InvalidIndex);
			Approvals::<T, I>::try_append(proposal_id)
				.map_err(|_| Error::<T, I>::TooManyApprovals)?;
			Ok(())
		}

		/// Propose and approve a spend of treasury funds. This is a legacy extrinsic which might be
		/// removed in the future.
		///
		/// - `origin`: Must be `SpendOrigin` with the `Success` value being at least `amount`.
		/// - `amount`: The amount to be transferred from the treasury to the `beneficiary`.
		/// - `beneficiary`: The destination account for the transfer.
		///
		/// NOTE: For record-keeping purposes, the proposer is deemed to be equivalent to the
		/// beneficiary.
		#[pallet::call_index(3)]
		#[pallet::weight(T::WeightInfo::spend_local())]
		pub fn spend_local(
			origin: OriginFor<T>,
			#[pallet::compact] amount: BalanceOf<T, I>,
			beneficiary: AccountIdLookupOf<T>,
		) -> DispatchResult {
			let max_amount = T::SpendOrigin::ensure_origin(origin)?;
			ensure!(amount <= max_amount, Error::<T, I>::InsufficientPermission);
			with_context::<SpendContext<BalanceOf<T, I>>, _>(|v| {
				let context = v.or_default();
				// We group based on `max_amount`, to dinstinguish between different kind of
				// origins. (assumes that all origins have different `max_amount`)
				//
				// Worst case is that we reject some "valid" request.
				let spend = context.spend_in_context.entry(max_amount).or_default();
				// Ensure that we don't overflow nor use more than `max_amount`
				if spend.checked_add(&amount).map(|s| s > max_amount).unwrap_or(true) {
					Err(Error::<T, I>::InsufficientPermission)
				} else {
					*spend = spend.saturating_add(amount);
					Ok(())
				}
			})
			.unwrap_or(Ok(()))?;

			let beneficiary = T::Lookup::lookup(beneficiary)?;
			let proposal_index = Self::proposal_count();
			Approvals::<T, I>::try_append(proposal_index)
				.map_err(|_| Error::<T, I>::TooManyApprovals)?;
			let proposal = Proposal {
				proposer: beneficiary.clone(),
				value: amount,
				beneficiary: beneficiary.clone(),
				bond: Default::default(),
			};
			Proposals::<T, I>::insert(proposal_index, proposal);
			ProposalCount::<T, I>::put(proposal_index.saturating_add(1));

			Self::deposit_event(Event::SpendApproved { proposal_index, amount, beneficiary });
			Ok(())
		}

		/// Propose and approve a spend of treasury funds.
		///
		/// - `origin`: Must be `T::SpendOrigin` with the `Success` value being at least `amount`.
		/// - `asset_kind`: An indicator of the specific asset class which should be spent
		/// - `amount`: The amount to be transferred from the treasury to the `beneficiary`.
		/// - `beneficiary`: The destination account for the transfer.
		#[pallet::call_index(5)]
		#[pallet::weight(T::WeightInfo::spend())]
		pub fn spend(
			origin: OriginFor<T>,
			assets: Vec<T::AssetKind>,
			beneficiary: AccountIdLookupOf<T>,
		) -> DispatchResult {
			let max_amount = T::SpendOrigin::ensure_origin(origin)?;
			let beneficiary = T::Lookup::lookup(beneficiary)?;

			for asset in assets {
				let normalized_amount =
					T::BalanceConverter::from_asset_balance(asset.amount(), asset)
						.map_err(|_| Error::<T, I>::BalanceConversionFailed)?;
				ensure!(normalized_amount <= max_amount, Error::<T, I>::InsufficientPermission);

				with_context::<SpendContext<BalanceOf<T, I>>, _>(|v| {
					let context = v.or_default();

					// We group based on `max_amount`, to dinstinguish between different kind of
					// origins. (assumes that all origins have different `max_amount`)
					//
					// Worst case is that we reject some "valid" request.
					let spend = context.spend_in_context.entry(max_amount).or_default();

					// Ensure that we don't overflow nor use more than `max_amount`
					if spend.checked_add(&normalized_amount).map(|s| s > max_amount).unwrap_or(true)
					{
						Err(Error::<T, I>::InsufficientPermission)
					} else {
						*spend = spend.saturating_add(normalized_amount);

						Ok(())
					}
				})
				.unwrap_or(Ok(()))?;

				let pending_payment = PendingPayment {
					asset_kind: asset,
					beneficiary: beneficiary.clone(),
					normalized_value: normalized_amount,
					payment_id: None,
					tries: 0,
				};

				let next_index = PendingPaymentsInbox::<T, I>::count();
				PendingPaymentsInbox::<T, I>::insert(next_index, pending_payment);

				Self::deposit_event(Event::PaymentQueued {
					pending_payment_index: next_index,
					asset_kind: asset,
					beneficiary: beneficiary.clone(),
				});
			}
			Ok(())
		}

		/// Force a previously approved proposal to be removed from the approval queue.
		/// The original deposit will no longer be returned.
		///
		/// May only be called from `T::RejectOrigin`.
		/// - `proposal_id`: The index of a proposal
		///
		/// ## Complexity
		/// - O(A) where `A` is the number of approvals
		///
		/// Errors:
		/// - `ProposalNotApproved`: The `proposal_id` supplied was not found in the approval queue,
		/// i.e., the proposal has not been approved. This could also mean the proposal does not
		/// exist altogether, thus there is no way it would have been approved in the first place.
		#[pallet::call_index(4)]
		#[pallet::weight((T::WeightInfo::remove_approval(), DispatchClass::Operational))]
		pub fn remove_approval(
			origin: OriginFor<T>,
			#[pallet::compact] proposal_id: ProposalIndex,
		) -> DispatchResult {
			T::RejectOrigin::ensure_origin(origin)?;

			Approvals::<T, I>::try_mutate(|v| -> DispatchResult {
				if let Some(index) = v.iter().position(|x| x == &proposal_id) {
					v.remove(index);
					Ok(())
				} else {
					Err(Error::<T, I>::ProposalNotApproved.into())
				}
			})?;

			Ok(())
		}
	}
}

impl<T: Config<I>, I: 'static> Pallet<T, I> {
	// Add public immutables and private mutables.

	/// The account ID of the treasury pot.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn account_id() -> T::AccountId {
		T::PalletId::get().into_account_truncating()
	}

	/// The needed bond for a proposal whose spend is `value`.
	fn calculate_bond(value: BalanceOf<T, I>) -> BalanceOf<T, I> {
		let mut r = T::ProposalBondMinimum::get().max(T::ProposalBond::get() * value);
		if let Some(m) = T::ProposalBondMaximum::get() {
			r = r.min(m);
		}
		r
	}

	/// Spend_funds is triggered periodically and uses the `T::Paymaster` to payout all spend
	/// requests in the `PendingPayments` storage map.
	pub fn check_and_retry_payments() -> Weight {
		let mut total_weight = Weight::zero();
		let pending_payments_len = PendingPayments::<T, I>::count();

		Self::deposit_event(Event::ProcessingProposals { waiting_proposals: pending_payments_len });

		for key in PendingPayments::<T, I>::iter_keys() {
			if let Some(mut p) = PendingPayments::<T, I>::get(key) {
				match p.payment_id {
					None => match T::Paymaster::pay(
						&p.beneficiary,
						p.asset_kind.asset_kind(),
						p.asset_kind.amount(),
					) {
						Ok(id) => {
							total_spent = total_spent.saturating_add(p.normalized_value);
							p.tries = p.tries.saturating_add(1);
							p.payment_id = Some(id);
							Self::deposit_event(Event::PaymentTriggered {
								pending_payment_index: key,
								asset_kind: p.asset_kind,
								payment_id: id,
								tries: p.tries,
							});
							PendingPayments::<T, I>::set(key, Some(p));
						},
						Err(err) => {
							log::debug!(target: LOG_TARGET, "Paymaster::pay failed for PendingPayment with index: {:?} and error: {:?}", key, err);
							missed_payments = missed_payments.saturating_add(1);
							Self::deposit_event(Event::PaymentFailure {
								pending_payment_index: key,
								asset_kind: p.asset_kind,
								payment_id: None,
								tries: p.tries,
							});
							p.tries = p.tries.saturating_add(1);
							PendingPayments::<T, I>::set(key, Some(p));
						},
					},
					Some(payment_id) => match T::Paymaster::check_payment(payment_id) {
						PaymentStatus::Failure | PaymentStatus::Unknown => {
							log::debug!(
								target: LOG_TARGET,
								"Paymaster::pay failed for PendingPayment with index: {:?}",
								key
							);
							// try again in the next `T::SpendPeriod`.
							missed_payments = missed_payments.saturating_add(1);
							// Force the payment to none, so a fresh payment is sent during the next
							// T::SpendPeriod.
							p.payment_id = None;
							p.tries = p.tries.saturating_add(1);

							Self::deposit_event(Event::PaymentFailure {
								pending_payment_index: key,
								asset_kind: p.asset_kind,
								payment_id: Some(payment_id),
								tries: p.tries,
							});
							PendingPayments::<T, I>::set(key, Some(p));
						},
						PaymentStatus::Success => {
							PendingPayments::<T, I>::remove(key);
							Self::deposit_event(Event::PaymentSuccess {
								pending_payment_index: key,
								asset_kind: p.asset_kind,
								payment_id,
								tries: p.tries,
							});
						},
						// PaymentStatus::InProgress and PaymentStatus::Unknown indicate that the
						// proposal status is inconclusive, and might still be successful or failed
						// in the future.
						PaymentStatus::InProgress => {},
					},
				}
			}
		}

		total_weight = total_weight
			.saturating_add(T::WeightInfo::on_initialize_pending_payments(pending_payments_len));

		Self::deposit_event(Event::RolloverPayments {
			rollover_payments: missed_payments,
			allocated_payments: pending_payments_len.saturating_sub(missed_payments),
		});

		total_weight
	}

	/// Spend_funds is triggered periodically and uses the `T::Paymaster` to payout all spend
	/// requests in the `PendingPayments` storage map.
	pub fn spend_funds() -> Weight {
		let mut total_weight = Weight::zero();
		let mut total_spent = BalanceOf::<T, I>::zero();
		let mut missed_payments: u32 = 0;
		let pending_payments_len = PendingPayments::<T, I>::count();

		Self::deposit_event(Event::ProcessingProposals { waiting_proposals: pending_payments_len });

		for key in PendingPaymentsInbox::<T, I>::iter_keys() {
			if let Some(mut p) = PendingPaymentsInbox::<T, I>::get(key) {
				match p.payment_id {
					None => match T::Paymaster::pay(
						&p.beneficiary,
						p.asset_kind.asset_kind(),
						p.asset_kind.amount(),
					) {
						Ok(id) => {
							total_spent = total_spent.saturating_add(p.normalized_value);
							Self::deposit_event(Event::PaymentTriggered {
								pending_payment_index: key,
								asset_kind: p.asset_kind,
								payment_id: Some(id),
								tries: p.tries.saturating_add(1),
							});
							PendingPayments::<T, I>::insert(key, Some(p));
							PendingPaymentsInbox::<T, I>::remove(key);
						},
						Err(err) => {
							log::debug!(target: LOG_TARGET, "Paymaster::pay failed for PendingPayment with index: {:?} and error: {:?}", key, err);
							missed_payments = missed_payments.saturating_add(1);
							Self::deposit_event(Event::PaymentFailure {
								pending_payment_index: key,
								asset_kind: p.asset_kind,
								payment_id: None,
								tries: p.tries,
							});
							p.tries = p.tries.saturating_add(1);
							// Insert it into `T::PendingPayments` to be retried.
							PendingPayments::<T, I>::insert(key, Some(p));
							PendingPaymentsInbox::<T, I>::remove(key);
						},
					},
					Some(_payment_id) => unreachable!(),
				}
			}
		}

		total_weight = total_weight
			.saturating_add(T::WeightInfo::on_initialize_pending_payments(pending_payments_len));

		Self::deposit_event(Event::RolloverPayments {
			rollover_payments: missed_payments,
			allocated_payments: pending_payments_len.saturating_sub(missed_payments),
		});

		total_weight
	}

	/// Spend some money! returns number of approvals before spend.
	pub fn spend_funds_local() -> Weight {
		let mut total_weight = Weight::zero();

		let mut budget_remaining = Self::pot();
		Self::deposit_event(Event::Spending { budget_remaining });
		let account_id = Self::account_id();

		let mut missed_any = false;
		let mut imbalance = <PositiveImbalanceOf<T, I>>::zero();
		let proposals_len = Approvals::<T, I>::mutate(|v| {
			let proposals_approvals_len = v.len() as u32;
			v.retain(|&index| {
				// Should always be true, but shouldn't panic if false or we're screwed.
				if let Some(p) = Self::proposals(index) {
					if p.value <= budget_remaining {
						budget_remaining -= p.value;
						<Proposals<T, I>>::remove(index);

						// return their deposit.
						let err_amount = T::Currency::unreserve(&p.proposer, p.bond);
						debug_assert!(err_amount.is_zero());

						// provide the allocation.
						imbalance.subsume(T::Currency::deposit_creating(&p.beneficiary, p.value));

						Self::deposit_event(Event::Awarded {
							proposal_index: index,
							award: p.value,
							account: p.beneficiary,
						});
						false
					} else {
						missed_any = true;
						true
					}
				} else {
					false
				}
			});
			proposals_approvals_len
		});

		total_weight =
			total_weight.saturating_add(T::WeightInfo::on_initialize_proposals(proposals_len));

		// Call Runtime hooks to external pallet using treasury to compute spend funds.
		T::SpendFunds::spend_funds(
			&mut budget_remaining,
			&mut imbalance,
			&mut total_weight,
			&mut missed_any,
		);

		if !missed_any {
			// burn some proportion of the remaining budget if we run a surplus.
			let burn = (T::Burn::get() * budget_remaining).min(budget_remaining);
			budget_remaining -= burn;

			let (debit, credit) = T::Currency::pair(burn);
			imbalance.subsume(debit);
			T::BurnDestination::on_unbalanced(credit);
			Self::deposit_event(Event::Burnt { burnt_funds: burn })
		}

		// Must never be an error, but better to be safe.
		// proof: budget_remaining is account free balance minus ED;
		// Thus we can't spend more than account free balance minus ED;
		// Thus account is kept alive; qed;
		if let Err(problem) =
			T::Currency::settle(&account_id, imbalance, WithdrawReasons::TRANSFER, KeepAlive)
		{
			print("Inconsistent state - couldn't settle imbalance for funds spent by treasury");
			// Nothing else to do here.
			drop(problem);
		}

		Self::deposit_event(Event::Rollover { rollover_balance: budget_remaining });

		total_weight
	}

	/// Return the amount of money in the pot.
	// The existential deposit is not part of the pot so treasury account never gets deleted.
	pub fn pot() -> BalanceOf<T, I> {
		T::Currency::free_balance(&Self::account_id())
			// Must never be less than 0 but better be safe.
			.saturating_sub(T::Currency::minimum_balance())
	}
}

impl<T: Config<I>, I: 'static> OnUnbalanced<NegativeImbalanceOf<T, I>> for Pallet<T, I> {
	fn on_nonzero_unbalanced(amount: NegativeImbalanceOf<T, I>) {
		let numeric_amount = amount.peek();

		// Must resolve into existing but better to be safe.
		let _ = T::Currency::resolve_creating(&Self::account_id(), amount);

		Self::deposit_event(Event::Deposit { value: numeric_amount });
	}
}
