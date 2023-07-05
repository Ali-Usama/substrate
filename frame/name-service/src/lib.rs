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

//! ## Name Service: Register Friendly Account Aliases and Metadata
//!
//! # Index
//!
//! * [Key Terms](#key-terms)
//! * [Goals](#goals)
//! * [Limitations](#limitations)
//! * [Usage](#usage)
//!
//! The name service pallet provides a means to register names and subdomains, also termed nodes,
//! via a commit reveal scheme. Names can act as aliases to addresses for a particular para ID, and
//! can be used by UIs to transfer tokens between accounts in a more user-friendly manner.
//!
//! ## Key Terms
//!
//! * commitment: A hash that represents a commitment to purchase a name registration. Any account
//!   //! can register a commitment by providing an owner address and a commitment hash - a
//!   bake2_256 hash of the desired name and a secret.
//! * node: Either a to-level name hash or a subnode record that exists in the service registry.
//! * name hash: A blake2_256 hash representation of a registered name.
//! * subnode: A subdomain of a registered name hash. Subnodes of a name can be registered
//!   recursively, so the depth a subnode can be registered is unbounded.
//! * registrar: Handles registration and deregistration of top-level names. It also allows the
//!   transfer of ownership of top-level names.
//! * resolver: Handles the mapping of a name registration to the metadata that can be assigned to
//!   it. An address (alongside the Para ID), text and unhashed name of the node.
//!
//! ## Goals
//!
//! The name service pallet is designed to allow account interactions to go through registered,
//! human-readable names. The targeted usage of the name service is to allow transferring of funds
//! to accounts using registered names as the recipient of the transfer, instead of their public
//! key.
//!
//!  The pallet aims to be para-agnostic; any Para ID can be registered with the name service, and
//! provided alongside an address that is being set in the resolver. To register an address with a
//! corresponding para, that para ID must be registered with the name service.
//!
//! The name service in its current form aims to provide critical chain data to allow the usage of
//! human-readable names as address aliases, and assumes that UIs will handle the routing of
//! transfers using these names.
//!
//! ## Limitations
//!
//! The name service does not handle routing transfers between paras, and assumes the UI will handle
//! the resolution of public keys from name service nodes, and handle any teleporting required to
//! transfer funds using the name service.. A future version of the name service could explore such
//! functionality.
//!
//! ## Usage
//!
//! ### Registering a top-level name
//!
//! Using a commit and reveal scheme, names can be registered on the name service, and de-registered
//! provided that no derived subnodes exist in the registry.
//!
//! ### Registering a subnode
//!
//! Subnodes can be recursively registered on the system. Ownership can be transferred between these
//! subnodes, so they do not necessarily need to be tied with the owner of parent nodes.
//!
//! ### Renewing
//!
//! Node ownership can be extended by providing a new expiry block. The fee corresponding to the new
//! expiry will be deducted from a provided fee-payer account.
//!
//! ### Transferring Ownership
//!
//! Nodes can be transferred to a new owner. Transferring a node to a new owner will also transfer
//! the node deposit to the new owner.
//!
//! ### Using Resolvers
//!
//! An address, human-readable name and text can be registered under a node. The address will be the
//! underlying account that the node is aliasing when used for transferring tokens.

#![cfg_attr(not(feature = "std"), no_std)]
use frame_support::{ensure, pallet_prelude::*, traits::Get, DefaultNoBound};
use sp_std::vec::Vec;

pub use crate::types::*;
pub use pallet::*;
pub use weights::WeightInfo;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
mod commit_reveal;
mod misc;
mod registrar;
mod resolver;
mod subnodes;
mod types;
mod weights;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::traits::{OnUnbalanced, ReservableCurrency, StorageVersion};
	use frame_system::{ensure_signed, pallet_prelude::*};
	use sp_runtime::traits::{Convert, Zero};
	use sp_std::vec::Vec;

	/// The current storage version.
	const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

	// The struct on which we build all of our Pallet logic.
	#[pallet::pallet]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: weights::WeightInfo;

		/// The Currency handler for this pallet.
		type Currency: ReservableCurrency<Self::AccountId>;

		/// Convert the block number into a balance.
		type BlockNumberToBalance: Convert<Self::BlockNumber, BalanceOf<Self>>;

		/// The account where registration fees are paid to.
		type RegistrationFeeHandler: OnUnbalanced<NegativeImbalanceOf<Self>>;

		/// The amount of blocks a user needs to wait after a Commitment before revealing.
		#[pallet::constant]
		type MinCommitmentAge: Get<Self::BlockNumber>;

		/// The amount of blocks after a commitment is created for before it expires.
		#[pallet::constant]
		type MaxCommitmentAge: Get<Self::BlockNumber>;

		/// Maximum length of a name.
		#[pallet::constant]
		type MaxNameLength: Get<u32>;

		/// Maximum length for metadata text.
		#[pallet::constant]
		type MaxTextLength: Get<u32>;

		// Maximum length for a para registration suffix.
		#[pallet::constant]
		type MaxSuffixLength: Get<u32>;

		/// An interface to access the name service resolver.
		type NameServiceResolver: NameServiceResolver<Self>;
	}

	/// Para ID Registrations.
	///
	/// A Para ID needs to be provided alongside a suffix to represent their address domain.
	#[pallet::storage]
	pub(super) type ParaRegistrations<T: Config> =
		CountedStorageMap<_, Twox64Concat, u32, BoundedSuffixOf<T>>;

	/// The deposit a user needs to make in order to commit to a name registration. A value of
	/// A value of `None` will disable commitments and therefore the registration of new names.
	#[pallet::storage]
	pub type CommitmentDeposit<T: Config> = StorageValue<_, BalanceOf<T>, OptionQuery>;

	/// The deposit a user needs to place to keep their subnodes in storage.
	/// A value of `None` will disable subnode registrations.
	#[pallet::storage]
	pub type SubNodeDeposit<T: Config> = StorageValue<_, BalanceOf<T>, OptionQuery>;

	/// Registration fee for registering a 3-letter name.
	#[pallet::storage]
	pub type TierThreeLetters<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Registration fee for registering a 4-letter name.
	#[pallet::storage]
	pub type TierFourLetters<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Default registration fee for 5+ letter names.
	#[pallet::storage]
	pub type TierDefault<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Registration fee per block.
	#[pallet::storage]
	pub type RegistrationFeePerBlock<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// The deposit taken per byte of storage used.
	#[pallet::storage]
	pub type PerByteFee<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Name Commitments
	#[pallet::storage]
	pub(super) type Commitments<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		CommitmentHash,
		Commitment<T::AccountId, BalanceOf<T>, T::BlockNumber>,
	>;

	/// Name Registrations
	#[pallet::storage]
	pub(super) type Registrations<T: Config> = CountedStorageMap<
		_,
		Twox64Concat,
		NameHash,
		Registration<T::AccountId, BalanceOf<T>, T::BlockNumber>,
	>;

	/// This resolver maps name hashes to a tuple of the account and `para_id` associated with the
	/// account.
	#[pallet::storage]
	pub(super) type AddressResolver<T: Config> =
		StorageMap<_, Blake2_128Concat, NameHash, (T::AccountId, u32)>;

	/// This resolver maps name hashes to an account
	#[pallet::storage]
	pub(super) type NameResolver<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		NameHash,
		BytesStorage<T::AccountId, BalanceOf<T>, BoundedNameOf<T>>,
	>;

	/// This resolver maps name hashes to an account
	#[pallet::storage]
	pub(super) type TextResolver<T: Config> = StorageMap<
		_,
		Blake2_128Concat,
		NameHash,
		BytesStorage<T::AccountId, BalanceOf<T>, BoundedTextOf<T>>,
	>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub commitment_deposit: Option<BalanceOf<T>>,
		pub subnode_deposit: Option<BalanceOf<T>>,
		pub tier_three_letters: BalanceOf<T>,
		pub tier_four_letters: BalanceOf<T>,
		pub tier_default: BalanceOf<T>,
		pub registration_fee_per_block: BalanceOf<T>,
		pub per_byte_fee: BalanceOf<T>,
	}

	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			Self {
				commitment_deposit: None,
				subnode_deposit: None,
				tier_three_letters: Zero::zero(),
				tier_four_letters: Zero::zero(),
				tier_default: Zero::zero(),
				registration_fee_per_block: <BalanceOf<T>>::from(1u32),
				per_byte_fee: <BalanceOf<T>>::from(1u32),
			}
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			if let Some(commitment_deposit) = self.commitment_deposit {
				CommitmentDeposit::<T>::put(commitment_deposit);
			}
			if let Some(subnode_deposit) = self.subnode_deposit {
				SubNodeDeposit::<T>::put(subnode_deposit);
			}
			TierThreeLetters::<T>::put(self.tier_three_letters);
			TierFourLetters::<T>::put(self.tier_four_letters);
			TierDefault::<T>::put(self.tier_default);
			RegistrationFeePerBlock::<T>::put(self.registration_fee_per_block);
			PerByteFee::<T>::put(self.per_byte_fee);
		}
	}

	// Your Pallet's events.
	#[pallet::event]
	#[pallet::generate_deposit(pub(crate) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A new `Commitment` has taken place.
		Committed { depositor: T::AccountId, owner: T::AccountId, hash: CommitmentHash },
		/// A new `Registration` has taken added.
		NameRegistered { name_hash: NameHash, owner: T::AccountId },
		/// A `Registration` has been transferred to a new owner.
		NewOwner { from: T::AccountId, to: T::AccountId },
		/// A `Registration` has been renewed.
		NameRenewed { name_hash: NameHash, expires: T::BlockNumber },
		/// An address has been set for a name hash to resolve to.
		AddressSet { name_hash: NameHash, address: T::AccountId },
		/// An name has been set as a reverse lookup for a name hash. You can query storage to see
		/// what the name is.
		NameSet { name_hash: NameHash },
		/// An address has been set for a name hash to resolve to. You can query storage to see
		/// what text was set.
		TextSet { name_hash: NameHash },
		/// An address was deregistered.
		AddressDeregistered { name_hash: NameHash },
	}

	// Your Pallet's error messages.
	#[pallet::error]
	#[cfg_attr(test, derive(PartialEq))]
	pub enum Error<T> {
		/// It has not passed the minimum waiting period to reveal a commitment.
		TooEarlyToReveal,
		/// Commitment deposits have been disabled and commitments cannot be registered.
		CommitmentsDisabled,
		/// Subnode deposits have been disabled and subnodes cannot be registered.
		SubNodesDisabled,
		/// This commitment hash already exists in storage.
		CommitmentExists,
		/// The commitment cannot yet be removed. Has not expired.
		CommitmentNotExpired,
		/// This commitment does not exist.
		CommitmentNotFound,
		/// A `Registration` of this name already exists.
		RegistrationExists,
		/// This registration has not yet expired.
		RegistrationNotExpired,
		/// This registration does not exist.
		RegistrationNotFound,
		/// Name is too short to be registered.
		NameTooShort,
		/// The name was longer than the configured limit.
		NameTooLong,
		/// The text was longer than the configured limit.
		TextTooLong,
		/// The account is not the name controller.
		NotController,
		/// The account is not the name owner.
		NotOwner,
		/// Cannot renew this registration.
		RegistrationHasNoExpiry,
		/// Renew expiry time is not in the future
		ExpiryInvalid,
		/// The name provided does not match the expected hash.
		BadName,
		/// The para ID was not found.
		ParaRegistrationNotFound,
	}

	// Your Pallet's callable functions.
	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Force the registration of a name hash. It will overwrite any existing name registration,
		/// returning the deposit to the original owner.
		///
		/// Can only be called by the `root` origin.
		#[pallet::call_index(0)]
		#[pallet::weight(0)]
		pub fn force_register(
			origin: OriginFor<T>,
			name_hash: NameHash,
			who: T::AccountId,
			maybe_expiry: Option<T::BlockNumber>,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::do_register(name_hash, who.clone(), who, maybe_expiry, None)?;
			Ok(())
		}

		/// Force the de-registration of a name hash. It will delete any existing name registration,
		/// returning the deposit to the original owner.
		///
		/// Can only be called by the `root` origin.
		#[pallet::call_index(1)]
		#[pallet::weight(0)]
		pub fn force_deregister(origin: OriginFor<T>, name_hash: NameHash) -> DispatchResult {
			ensure_root(origin)?;
			Self::do_deregister(name_hash);
			Ok(())
		}

		/// Allow a sender to commit to a new name registration on behalf of the `owner`. By making
		/// a commitment, the sender will reserve a deposit until the name is revealed or the
		/// commitment is removed.
		///
		/// The commitment hash should be the `bake2_256(name: <u8, MaxNameLength>, secret: u64)`,
		/// which allows the sender to keep name being registered secret until it is revealed.
		///
		/// The `name` must be at least 3 characters long.
		///
		/// When `MinCommitmentAge` blocks have passed, any user can submit `reveal` with the
		/// `name` and `secret` parameters, and the registration will be completed.
		///
		/// See `fn reveal`.
		#[pallet::call_index(2)]
		#[pallet::weight(0)]
		pub fn commit(
			origin: OriginFor<T>,
			owner: T::AccountId,
			commitment_hash: CommitmentHash,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			Self::do_commit(sender, owner, commitment_hash)?;
			Ok(())
		}

		/// Allow a sender to reveal a previously committed name registration on behalf of the
		/// committed `owner`. By revealing the name, the sender will pay a non-refundable
		/// registration fee.
		///
		/// The registration fee is calculated using the length of the name and the length of the
		/// registration.
		#[pallet::call_index(3)]
		#[pallet::weight(0)]
		pub fn reveal(
			origin: OriginFor<T>,
			name: Vec<u8>,
			secret: u64,
			length: T::BlockNumber,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let name_bounded: BoundedVec<u8, T::MaxNameLength> =
				BoundedVec::try_from(name).map_err(|_| Error::<T>::NameTooLong)?;
			Self::do_reveal(sender, name_bounded.to_vec(), secret, length)?;
			Ok(())
		}

		/// Allows anyone to remove a commitment that has expired the reveal period.
		///
		/// By doing so, the commitment deposit is returned to the original depositor.
		#[pallet::call_index(4)]
		#[pallet::weight(0)]
		pub fn remove_commitment(
			origin: OriginFor<T>,
			commitment_hash: CommitmentHash,
		) -> DispatchResult {
			ensure_signed_or_root(origin)?;
			let commitment = Self::get_commitment(commitment_hash)?;
			let block_number = frame_system::Pallet::<T>::block_number();
			ensure!(
				Self::is_commitment_expired(&commitment, &block_number),
				Error::<T>::CommitmentNotExpired
			);
			Self::do_remove_commitment(&commitment_hash, &commitment);
			Ok(())
		}

		/// Transfers the ownership and deposits of a name registration to a new owner.
		///
		/// Can only be called by the existing owner of the name registration.
		#[pallet::call_index(5)]
		#[pallet::weight(0)]
		pub fn transfer(
			origin: OriginFor<T>,
			new_owner: T::AccountId,
			name_hash: NameHash,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let registration = Self::get_registration(name_hash)?;
			ensure!(Self::is_owner(&registration, &sender), Error::<T>::NotOwner);
			Self::do_transfer_ownership(name_hash, new_owner)?;
			Ok(())
		}

		/// Set the controller for a name registration.
		///
		/// Can only be called by the existing controller or owner.
		#[pallet::call_index(6)]
		#[pallet::weight(0)]
		pub fn set_controller(
			origin: OriginFor<T>,
			name_hash: NameHash,
			to: T::AccountId,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;

			Registrations::<T>::try_mutate(name_hash, |maybe_registration| {
				let r = maybe_registration.as_mut().ok_or(Error::<T>::RegistrationNotFound)?;
				ensure!(Self::is_controller(&r, &sender), Error::<T>::NotController);
				r.controller = to.clone();
				Self::deposit_event(Event::<T>::NewOwner { from: sender, to });
				Ok(())
			})
		}

		/// Allows any sender to extend the registration of an existing name.
		///
		/// By doing so, the sender will pay the non-refundable registration extension fee.
		#[pallet::call_index(7)]
		#[pallet::weight(0)]
		pub fn renew(
			origin: OriginFor<T>,
			name_hash: NameHash,
			expiry: T::BlockNumber,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			Self::do_renew(sender, name_hash, expiry)?;
			Ok(())
		}

		/// Deregister a registered name.
		///
		/// If the registration is still valid, only the owner of the name can make this call.
		///
		/// If the registration is expired, then anyone can call this function to make the name
		/// available.
		#[pallet::call_index(8)]
		#[pallet::weight(0)]
		pub fn deregister(origin: OriginFor<T>, name_hash: NameHash) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let registration =
				Registrations::<T>::get(name_hash).ok_or(Error::<T>::RegistrationNotFound)?;
			// If the registration is expired, anyone can trigger deregister.
			if !Self::is_expired(&registration) {
				ensure!(Self::is_owner(&registration, &sender), Error::<T>::NotOwner);
			}
			Self::do_deregister(name_hash);
			Ok(())
		}

		#[pallet::call_index(9)]
		#[pallet::weight(0)]
		pub fn set_subnode_record(
			origin: OriginFor<T>,
			parent_hash: NameHash,
			label: Vec<u8>,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let label_bounded: BoundedVec<u8, T::MaxNameLength> =
				BoundedVec::try_from(label).map_err(|_| Error::<T>::NameTooLong)?;
			Self::do_set_subnode_record(sender, parent_hash, &label_bounded)?;
			Ok(())
		}

		#[pallet::call_index(10)]
		#[pallet::weight(0)]
		pub fn deregister_subnode(
			origin: OriginFor<T>,
			parent_hash: NameHash,
			label_hash: NameHash,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let subnode_hash = Self::subnode_hash(parent_hash, label_hash);
			let subnode_registration = Self::get_registration(subnode_hash)?;
			// The owner isn't calling, we check that the parent registration doesn't exist, which
			// mean this subnode is still valid.
			if !Self::is_owner(&subnode_registration, &sender) {
				ensure!(
					Self::get_registration(parent_hash).is_err(),
					Error::<T>::RegistrationNotExpired
				);
			}
			Self::do_deregister(subnode_hash);
			Ok(())
		}

		#[pallet::call_index(11)]
		#[pallet::weight(0)]
		pub fn set_subnode_owner(
			origin: OriginFor<T>,
			parent_hash: NameHash,
			label_hash: NameHash,
			new_owner: T::AccountId,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			Self::do_set_subnode_owner(sender, parent_hash, label_hash, new_owner)?;
			Ok(())
		}

		#[pallet::call_index(12)]
		#[pallet::weight(0)]
		pub fn set_address(
			origin: OriginFor<T>,
			name_hash: NameHash,
			address: T::AccountId,
			para_id: u32,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			ensure!(
				ParaRegistrations::<T>::contains_key(para_id),
				Error::<T>::ParaRegistrationNotFound
			);
			let registration =
				Registrations::<T>::get(name_hash).ok_or(Error::<T>::RegistrationNotFound)?;
			ensure!(Self::is_controller(&registration, &sender), Error::<T>::NotController);
			T::NameServiceResolver::set_address(name_hash, address, para_id, sender)?;
			Ok(())
		}

		/// Register the raw name for a given name hash. This can be used as a reverse lookup for
		/// front-ends.
		///
		/// This is a permissionless function that anyone can call who is willing to place a deposit
		/// to store this data on chain.

		#[pallet::call_index(13)]
		#[pallet::weight(0)]
		pub fn set_name(
			origin: OriginFor<T>,
			name_hash: NameHash,
			name: Vec<u8>,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let name_bounded: BoundedVec<u8, T::MaxNameLength> =
				BoundedVec::try_from(name).map_err(|_| Error::<T>::NameTooLong)?;
			ensure!(Registrations::<T>::contains_key(name_hash), Error::<T>::RegistrationNotFound);
			T::NameServiceResolver::set_name(name_hash, name_bounded, sender)?;
			Ok(())
		}

		#[pallet::call_index(14)]
		#[pallet::weight(0)]
		pub fn set_text(
			origin: OriginFor<T>,
			name_hash: NameHash,
			text: Vec<u8>,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			let text_bounded: BoundedVec<u8, T::MaxTextLength> =
				BoundedVec::try_from(text).map_err(|_| Error::<T>::TextTooLong)?;

			let registration =
				Registrations::<T>::get(name_hash).ok_or(Error::<T>::RegistrationNotFound)?;
			ensure!(Self::is_controller(&registration, &sender), Error::<T>::NotController);
			T::NameServiceResolver::set_text(name_hash, text_bounded, sender)?;
			Ok(())
		}

		/// Inserts a suffix for a para ID.
		///
		/// Overwrites existing values if already present.
		/// Can only be called by the `root` origin.
		/// TODO: explore the possibility of bounding this call to XCM calls in addition to root.
		#[pallet::call_index(15)]
		#[pallet::weight(0)]
		pub fn register_para(origin: OriginFor<T>, para: ParaRegistration<T>) -> DispatchResult {
			ensure_root(origin)?;
			ParaRegistrations::<T>::insert(para.id, para.suffix);
			Ok(())
		}

		/// Can only be called by the `root` origin.
		/// TODO: explore the possibility of bounding this call to XCM calls in addition to root.
		#[pallet::call_index(16)]
		#[pallet::weight(0)]
		pub fn deregister_para(origin: OriginFor<T>, para_id: u32) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(
				ParaRegistrations::<T>::contains_key(para_id),
				Error::<T>::ParaRegistrationNotFound
			);
			ParaRegistrations::<T>::remove(para_id);
			Ok(())
		}

		/// Update configurations for the name service. The origin for this call must be
		/// Root.
		///
		/// # Arguments
		///
		/// * `commitment_deposit` - Set [`CommitmentDeposit`].
		/// * `subnode_deposit` - Set [`SubNodeDeposit`].
		/// * `tier_three_letters` - Set [`TierThreeLetters`].
		/// * `tier_four_letters` - Set [`TierFourLetters`].
		/// * `tier_default` - Set [`TierDefault`].
		/// * `registration_fee_per_block` - Set [`RegistrationFeePerBlock`].
		/// * `per_byte_fee` - Set [`PerByteFee`].
		#[pallet::call_index(17)]
		#[pallet::weight(0)]
		pub fn set_configs(
			origin: OriginFor<T>,
			commitment_deposit: ConfigOp<BalanceOf<T>>,
			subnode_deposit: ConfigOp<BalanceOf<T>>,
			tier_three_letters: ConfigOp<BalanceOf<T>>,
			tier_four_letters: ConfigOp<BalanceOf<T>>,
			tier_default: ConfigOp<BalanceOf<T>>,
			registration_fee_per_block: ConfigOp<BalanceOf<T>>,
			per_byte_fee: ConfigOp<BalanceOf<T>>,
		) -> DispatchResult {
			ensure_root(origin)?;

			macro_rules! config_op_exp {
				($storage:ty, $op:ident) => {
					match $op {
						ConfigOp::Noop => (),
						ConfigOp::Set(v) => <$storage>::put(v),
						ConfigOp::Remove => <$storage>::kill(),
					}
				};
			}

			config_op_exp!(CommitmentDeposit::<T>, commitment_deposit);
			config_op_exp!(SubNodeDeposit::<T>, subnode_deposit);
			config_op_exp!(TierThreeLetters::<T>, tier_three_letters);
			config_op_exp!(TierFourLetters::<T>, tier_four_letters);
			config_op_exp!(TierDefault::<T>, tier_default);
			config_op_exp!(RegistrationFeePerBlock::<T>, registration_fee_per_block);
			config_op_exp!(PerByteFee::<T>, per_byte_fee);

			Ok(())
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn integrity_test() {
			assert!(T::MaxNameLength::get() > 0, "Max name length cannot be zero");
			assert!(T::MaxTextLength::get() > 0, "Max text length cannot be zero");
			assert!(T::MaxSuffixLength::get() > 0, "Max suffix length cannot be zero");
		}
	}
}
