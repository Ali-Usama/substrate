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

use crate::{mock::*, *};
use codec::{Decode, Encode};
use enumflags2::BitFlags;
use sp_runtime::{traits::TrailingZeroInput, AccountId32, MultiSignature, MultiSigner, Perbill};

use frame_support::{assert_noop, assert_ok, traits::Currency};

macro_rules! bvec {
	($( $x:tt )*) => {
		vec![$( $x )*].try_into().unwrap()
	}
}

fn get_id_from_event() -> Result<<Test as Config>::CollectionId, &'static str> {
	let last_event = System::events().pop();
	if let Some(e) = last_event.clone() {
		match e.event {
			mock::Event::Uniques(inner_event) => match inner_event {
				Event::CollectionCreated { id, .. } => return Ok(id),
				_ => {},
			},
			_ => {},
		}
	}

	Err("bad event")
}

fn events() -> Vec<Event<Test>> {
	let result = System::events()
		.into_iter()
		.map(|r| r.event)
		.filter_map(|e| if let mock::Event::Uniques(inner) = e { Some(inner) } else { None })
		.collect::<Vec<_>>();

	System::reset_events();

	result
}

fn collections() -> Vec<(u64, u32)> {
	let mut r: Vec<_> = CollectionOwner::<Test>::iter().map(|x| (x.0, x.1)).collect();
	r.sort();
	let mut s: Vec<_> = Collections::<Test>::iter().map(|x| (x.1.owner, x.0)).collect();
	s.sort();
	assert_eq!(r, s);
	r
}

fn items() -> Vec<(u64, u32, u32)> {
	let mut r: Vec<_> = AccountItems::<Test>::iter().map(|x| x.0).collect();
	r.sort();
	let mut s: Vec<_> = Items::<Test>::iter().map(|x| (x.2.owner, x.0, x.1)).collect();
	s.sort();
	assert_eq!(r, s);
	for collection in Items::<Test>::iter()
		.map(|x| x.0)
		.scan(None, |s, collection_id| {
			if s.map_or(false, |last| last == collection_id) {
				*s = Some(collection_id);
				Some(None)
			} else {
				Some(Some(collection_id))
			}
		})
		.flatten()
	{
		let details = Collections::<Test>::get(collection).unwrap();
		let items = Items::<Test>::iter_prefix(collection).count() as u32;
		assert_eq!(details.items, items);
	}
	r
}

fn attributes(collection: u32) -> Vec<(Option<u32>, Vec<u8>, Vec<u8>)> {
	let mut s: Vec<_> = Attributes::<Test>::iter_prefix((collection,))
		.map(|(k, v)| (k.0, k.1.into(), v.into()))
		.collect();
	s.sort();

	let collection = Collections::<Test>::get(collection);
	let expect_attributes = match collection {
		Some(collection) => collection.attributes,
		_ => 0,
	};
	assert_eq!(expect_attributes as usize, s.len());
	s
}

fn approvals(collection_id: u32, item_id: u32) -> Vec<(u64, Option<u64>)> {
	let item = Items::<Test>::get(collection_id, item_id).unwrap();
	let s: Vec<_> = item.approvals.into_iter().collect();
	s
}

fn signer_to_account_id(signer: &MultiSigner) -> <Test as frame_system::Config>::AccountId {
	<Test as frame_system::Config>::AccountId::decode(&mut AccountId32::as_ref(
		&signer.clone().into_account(),
	))
	.unwrap()
}

pub const DEFAULT_SYSTEM_FEATURES: SystemFeature = SystemFeature::NoDeposit;
pub const DEFAULT_USER_FEATURES: UserFeature = UserFeature::Administration;

#[cfg(test)]
mod crypto {
	use sp_core::ed25519;
	use sp_io::crypto::{ed25519_generate, ed25519_sign};
	use sp_runtime::{MultiSignature, MultiSigner};
	use sp_std::vec::Vec;

	pub fn create_ed25519_pubkey(seed: Vec<u8>) -> MultiSigner {
		ed25519_generate(0.into(), Some(seed)).into()
	}

	pub fn create_ed25519_signature(payload: &[u8], pubkey: MultiSigner) -> MultiSignature {
		let edpubkey = ed25519::Public::try_from(pubkey).unwrap();
		let edsig = ed25519_sign(0.into(), &edpubkey, payload).unwrap();
		edsig.into()
	}
}

#[test]
fn minting_should_work() {
	new_test_ext().execute_with(|| {
		let owner = 1;
		let creator = 1;
		assert_ok!(Uniques::create(
			Origin::signed(creator),
			owner,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		let id = get_id_from_event().unwrap();
		let collection_config = CollectionConfigs::<Test>::get(id);

		let expected_config = CollectionConfig {
			system_features: SystemFeatures::new(DEFAULT_SYSTEM_FEATURES.into()),
			user_features: UserFeatures::new(DEFAULT_USER_FEATURES.into()),
		};
		assert_eq!(Some(expected_config), collection_config);

		assert_eq!(
			events(),
			[Event::<Test>::CollectionCreated {
				id,
				max_supply: None,
				max_items_per_account: None,
				owner,
				creator,
				creator_royalties: Perbill::zero(),
				owner_royalties: Perbill::zero(),
			}]
		);
		assert_eq!(CollectionNextId::<Test>::get(), 1);
		assert!(CollectionCreator::<Test>::contains_key(creator, id));
		assert!(CollectionOwner::<Test>::contains_key(owner, id));
		assert_eq!(collections(), vec![(owner, id)]);
	});
}

#[test]
fn collection_locking_should_work() {
	new_test_ext().execute_with(|| {
		let user_id = 1;

		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		let id = get_id_from_event().unwrap();
		let new_config = UserFeatures::new(UserFeature::IsLocked.into());

		assert_ok!(Uniques::change_collection_config(Origin::signed(user_id), id, new_config));

		let collection_config = CollectionConfigs::<Test>::get(id);

		let expected_config = CollectionConfig {
			system_features: SystemFeatures::new(DEFAULT_SYSTEM_FEATURES.into()),
			user_features: new_config,
		};

		assert_eq!(Some(expected_config), collection_config);
	});
}

#[test]
fn collection_locking_should_fail() {
	new_test_ext().execute_with(|| {
		let user_id = 1;
		let user_features = UserFeatures::new(UserFeature::IsLocked.into());

		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			user_features,
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		let id = get_id_from_event().unwrap();
		let new_config = UserFeatures::new(UserFeature::Administration.into());

		assert!(events().contains(&Event::<Test>::CollectionLocked { id }));

		assert_noop!(
			Uniques::change_collection_config(Origin::signed(user_id), id, new_config),
			Error::<Test>::CollectionIsLocked,
		);
	});
}

#[test]
fn update_max_supply_should_work() {
	new_test_ext().execute_with(|| {
		let id = 0;
		let user_id = 1;
		let max_supply = Some(10);

		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			max_supply,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		let collection = Collections::<Test>::get(id).unwrap();
		assert_eq!(collection.max_supply, max_supply);

		let new_max_supply = Some(10);
		assert_ok!(Uniques::update_max_supply(Origin::signed(user_id), id, new_max_supply));

		let collection = Collections::<Test>::get(id).unwrap();
		assert_eq!(collection.max_supply, new_max_supply);

		assert!(events().contains(&Event::<Test>::CollectionMaxSupplyChanged {
			id,
			max_supply: new_max_supply
		}));
	});
}

#[test]
fn destroy_collection_should_work() {
	new_test_ext().execute_with(|| {
		let id = 0;
		let user_id = 1;

		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::set_collection_metadata(Origin::signed(user_id), id, bvec![0u8; 20]));

		assert_ok!(Uniques::mint(Origin::signed(user_id), user_id, id, 1));
		assert_ok!(Uniques::mint(Origin::signed(user_id), user_id, id, 2));

		assert_ok!(Uniques::set_item_metadata(Origin::signed(user_id), id, 1, bvec![0u8; 20]));
		assert_ok!(Uniques::set_item_metadata(Origin::signed(user_id), id, 2, bvec![0u8; 20]));

		let w = Collections::<Test>::get(id).unwrap().destroy_witness();
		assert_eq!(w.items, 2);
		assert_eq!(w.item_metadatas, 2);
		assert_ok!(Uniques::destroy(Origin::signed(user_id), id, w));

		assert!(!CollectionConfigs::<Test>::contains_key(id));
		assert!(!Collections::<Test>::contains_key(id));
		assert!(!CollectionOwner::<Test>::contains_key(user_id, id));
		assert!(!CollectionCreator::<Test>::contains_key(user_id, id));
		assert!(!Items::<Test>::contains_key(id, 1));
		assert!(!Items::<Test>::contains_key(id, 2));
		assert!(!CountForAccountItems::<Test>::contains_key(user_id, id));

		assert_eq!(collections(), vec![]);
		assert_eq!(items(), vec![]);
	});
}

#[test]
fn transfer_owner_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let user_2 = 2;
		let collection_id = 0;

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_eq!(collections(), vec![(user_1, collection_id)]);
		assert_ok!(Uniques::transfer_collection_ownership(
			Origin::signed(user_1),
			collection_id,
			user_2
		));
		assert_eq!(collections(), vec![(user_2, collection_id)]);

		assert_noop!(
			Uniques::transfer_collection_ownership(Origin::signed(user_1), collection_id, user_1),
			Error::<Test>::NotAuthorized
		);
	});
}

#[test]
fn mint_should_work() {
	new_test_ext().execute_with(|| {
		let sender = 0;
		let user_id = 1;
		let collection_id = 0;
		let item_id = 1;

		assert_ok!(Uniques::create(
			Origin::signed(sender),
			sender,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(sender), user_id, collection_id, item_id));
		assert_eq!(collections(), vec![(sender, collection_id)]);
		assert_eq!(items(), vec![(user_id, collection_id, item_id)]);

		assert_eq!(Collections::<Test>::get(collection_id).unwrap().items, 1);
		assert_eq!(Collections::<Test>::get(collection_id).unwrap().item_metadatas, 0);

		assert!(Items::<Test>::contains_key(collection_id, item_id));
		assert_eq!(CountForAccountItems::<Test>::get(user_id, collection_id).unwrap(), 1);

		assert!(events().contains(&Event::<Test>::ItemCreated { collection_id, item_id }));

		// validate max supply
		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			Some(1),
			None,
			Perbill::zero(),
			Perbill::zero(),
		));
		assert_ok!(Uniques::mint(Origin::signed(user_id), user_id, 1, 1));
		assert_noop!(
			Uniques::mint(Origin::signed(user_id), user_id, 1, 2),
			Error::<Test>::AllItemsMinted
		);
	});
}

#[test]
fn burn_should_work() {
	new_test_ext().execute_with(|| {
		let user_id = 1;
		let collection_id = 0;
		let item_id = 1;

		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_id), user_id, collection_id, item_id));
		assert_ok!(Uniques::burn(Origin::signed(user_id), collection_id, item_id));

		assert_eq!(collections(), vec![(user_id, collection_id)]);
		assert_eq!(items(), vec![]);

		assert_eq!(Collections::<Test>::get(collection_id).unwrap().items, 0);
		assert_eq!(Collections::<Test>::get(collection_id).unwrap().item_metadatas, 0);

		assert!(!Items::<Test>::contains_key(collection_id, item_id));
		assert_eq!(CountForAccountItems::<Test>::get(user_id, collection_id).unwrap(), 0);

		assert!(events().contains(&Event::<Test>::ItemBurned { collection_id, item_id }));
	});
}

#[test]
fn transfer_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let user_2 = 2;
		let user_3 = 3;
		let collection_id = 0;
		let item_id = 1;

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_1), user_2, collection_id, item_id));
		let config = CollectionConfigs::<Test>::get(collection_id).unwrap();

		assert_ok!(Uniques::transfer_item(
			Origin::signed(user_2),
			collection_id,
			item_id,
			user_3,
			config
		));

		assert_eq!(items(), vec![(user_3, collection_id, item_id)]);

		assert!(events().contains(&Event::<Test>::ItemTransferred {
			collection_id,
			item_id,
			sender: user_2,
			receiver: user_3,
		}));

		assert_eq!(CountForAccountItems::<Test>::get(user_1, collection_id).unwrap_or_default(), 0);
		assert_eq!(CountForAccountItems::<Test>::get(user_2, collection_id).unwrap_or_default(), 0);
		assert_eq!(CountForAccountItems::<Test>::get(user_3, collection_id).unwrap(), 1);

		assert_noop!(
			Uniques::transfer_item(Origin::signed(user_2), collection_id, item_id, user_3, config),
			Error::<Test>::NotAuthorized
		);

		// validate we can't transfer non-transferable items
		let collection_id = 1;
		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(UserFeature::NonTransferableItems.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_id));

		assert_noop!(
			Uniques::transfer_item(
				Origin::signed(user_1),
				collection_id,
				item_id,
				user_3,
				CollectionConfigs::<Test>::get(collection_id).unwrap()
			),
			Error::<Test>::ItemsNotTransferable
		);
	});
}

#[test]
fn set_metadata_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let user_2 = 2;
		let collection_id = 0;
		let item_1 = 1;
		let item_2 = 2;

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::set_collection_metadata(
			Origin::signed(user_1),
			collection_id,
			bvec![0u8; 20]
		));

		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_1));
		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_2));

		assert_ok!(Uniques::set_item_metadata(
			Origin::signed(user_1),
			collection_id,
			item_2,
			bvec![0u8; 20]
		));

		assert_eq!(Collections::<Test>::get(collection_id).unwrap().items, 2);
		assert_eq!(Collections::<Test>::get(collection_id).unwrap().item_metadatas, 1);

		assert!(CollectionMetadataOf::<Test>::contains_key(collection_id));
		assert!(ItemMetadataOf::<Test>::contains_key(collection_id, item_2));

		// only collection's owner can change items metadata
		assert_ok!(Uniques::transfer_item(
			Origin::signed(user_1),
			collection_id,
			item_2,
			user_2,
			CollectionConfigs::<Test>::get(collection_id).unwrap()
		));
		assert_noop!(
			Uniques::set_item_metadata(
				Origin::signed(user_2),
				collection_id,
				item_2,
				bvec![0u8; 20]
			),
			Error::<Test>::NotAuthorized
		);

		// collection's metadata can't be changed after the collection gets locked
		assert_ok!(Uniques::change_collection_config(
			Origin::signed(user_1),
			collection_id,
			UserFeatures::new(UserFeature::IsLocked.into())
		));
		assert_noop!(
			Uniques::set_collection_metadata(Origin::signed(user_1), collection_id, bvec![0u8; 20]),
			Error::<Test>::CollectionIsLocked
		);
	});
}

#[test]
fn set_attribute_should_work() {
	new_test_ext().execute_with(|| {
		let user_id = 1;
		let id = 0;

		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::set_attribute(Origin::signed(user_id), id, None, bvec![0], bvec![0]));
		assert_ok!(Uniques::set_attribute(
			Origin::signed(user_id),
			id,
			Some(0),
			bvec![0],
			bvec![0]
		));
		assert_ok!(Uniques::set_attribute(
			Origin::signed(user_id),
			id,
			Some(0),
			bvec![1],
			bvec![0]
		));

		assert_eq!(
			attributes(id),
			vec![
				(None, bvec![0], bvec![0]),
				(Some(0), bvec![0], bvec![0]),
				(Some(0), bvec![1], bvec![0]),
			]
		);

		assert_ok!(Uniques::set_attribute(
			Origin::signed(user_id),
			id,
			None,
			bvec![0],
			bvec![0; 10]
		));
		assert_eq!(
			attributes(id),
			vec![
				(None, bvec![0], bvec![0; 10]),
				(Some(0), bvec![0], bvec![0]),
				(Some(0), bvec![1], bvec![0]),
			]
		);

		assert_ok!(Uniques::clear_attribute(Origin::signed(user_id), id, Some(0), bvec![1]));
		assert_eq!(
			attributes(id),
			vec![(None, bvec![0], bvec![0; 10]), (Some(0), bvec![0], bvec![0]),]
		);

		let w = Collections::<Test>::get(id).unwrap().destroy_witness();
		assert_ok!(Uniques::destroy(Origin::signed(user_id), id, w));
		assert_eq!(attributes(id), vec![]);
	});
}

#[test]
fn set_price_should_work() {
	new_test_ext().execute_with(|| {
		let user_id = 1;
		let collection_id = 0;
		let item_1 = 1;
		let item_2 = 2;

		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_id), user_id, collection_id, item_1));
		assert_ok!(Uniques::mint(Origin::signed(user_id), user_id, collection_id, item_2));

		assert_ok!(Uniques::set_price(
			Origin::signed(user_id),
			collection_id,
			item_1,
			Some(1),
			None,
		));

		assert_ok!(Uniques::set_price(
			Origin::signed(user_id),
			collection_id,
			item_2,
			Some(2),
			Some(3)
		));

		let item = Items::<Test>::get(collection_id, item_1).unwrap();
		assert_eq!(item.price, Some(1));
		assert_eq!(item.buyer, None);

		let item = Items::<Test>::get(collection_id, item_2).unwrap();
		assert_eq!(item.price, Some(2));
		assert_eq!(item.buyer, Some(3));

		assert!(events().contains(&Event::<Test>::ItemPriceSet {
			collection_id,
			item_id: item_1,
			price: Some(1),
			buyer: None,
		}));

		// ensure we can't set price when the items are non-transferable
		let collection_id = 1;
		assert_ok!(Uniques::create(
			Origin::signed(user_id),
			user_id,
			UserFeatures::new(UserFeature::NonTransferableItems.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_id), user_id, collection_id, item_1));

		assert_noop!(
			Uniques::set_price(Origin::signed(user_id), collection_id, item_1, Some(1), None),
			Error::<Test>::ItemsNotTransferable
		);
	});
}

#[test]
fn buy_item_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let user_2 = 2;
		let user_3 = 3;
		let collection_id = 0;
		let item_1 = 1;
		let item_2 = 2;
		let item_3 = 3;
		let price_1 = 20;
		let price_2 = 30;
		let initial_balance = 100;

		Balances::make_free_balance_be(&user_1, initial_balance);
		Balances::make_free_balance_be(&user_2, initial_balance);
		Balances::make_free_balance_be(&user_3, initial_balance);

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_1));
		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_2));
		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_3));

		assert_ok!(Uniques::set_price(
			Origin::signed(user_1),
			collection_id,
			item_1,
			Some(price_1),
			None,
		));

		assert_ok!(Uniques::set_price(
			Origin::signed(user_1),
			collection_id,
			item_2,
			Some(price_2),
			Some(user_3),
		));

		// can't buy for less
		assert_noop!(
			Uniques::buy_item(Origin::signed(user_2), collection_id, item_1, 1),
			Error::<Test>::BidTooLow
		);

		assert_ok!(Uniques::buy_item(Origin::signed(user_2), collection_id, item_1, price_1,));

		// validate the new owner & balances
		let item = Items::<Test>::get(collection_id, item_1).unwrap();
		assert_eq!(item.owner, user_2);
		assert_eq!(Balances::total_balance(&user_1), initial_balance + price_1);
		assert_eq!(Balances::total_balance(&user_2), initial_balance - price_1);

		// can't buy from yourself
		assert_noop!(
			Uniques::buy_item(Origin::signed(user_1), collection_id, item_2, price_2),
			Error::<Test>::NotAuthorized
		);

		// can't buy when the item is listed for specified buyer
		assert_noop!(
			Uniques::buy_item(Origin::signed(user_2), collection_id, item_2, price_2),
			Error::<Test>::ItemNotForSale
		);

		// can buy when I'm a whitelisted buyer
		assert_ok!(Uniques::buy_item(Origin::signed(user_3), collection_id, item_2, price_2,));

		assert!(events().contains(&Event::<Test>::ItemBought {
			collection_id,
			item_id: item_2,
			price: price_2,
			seller: user_1,
			buyer: user_3,
		}));

		// ensure we reset the buyer field
		assert_eq!(Items::<Test>::get(collection_id, item_2).unwrap().buyer, None);

		// can't buy when item is not for sale
		assert_noop!(
			Uniques::buy_item(Origin::signed(user_2), collection_id, item_3, price_2),
			Error::<Test>::ItemNotForSale
		);

		// ensure we can't buy an item when the collection has a NonTransferableItems flag
		let collection_id = 1;
		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(UserFeature::NonTransferableItems.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_noop!(
			Uniques::buy_item(Origin::signed(user_1), collection_id, item_1, price_1),
			Error::<Test>::ItemNotForSale
		);
	});
}

#[test]
fn add_remove_approval_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let user_2 = 2;
		let user_3 = 3;
		let collection_id = 0;
		let item_id = 1;

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		// validate we can't set an approval for non-existing item
		assert_noop!(
			Uniques::approve_transfer(Origin::signed(user_1), collection_id, item_id, user_2, None),
			Error::<Test>::ItemNotFound
		);

		// validate we can set an approval when all the conditions are met
		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_id));
		assert_ok!(Uniques::approve_transfer(
			Origin::signed(user_1),
			collection_id,
			item_id,
			user_2,
			None
		));

		assert_eq!(approvals(collection_id, item_id), vec![(user_2, None)]);

		// setting the deadline should work
		assert_ok!(Uniques::approve_transfer(
			Origin::signed(user_1),
			collection_id,
			item_id,
			user_2,
			Some(2)
		));

		assert_eq!(approvals(collection_id, item_id), vec![(user_2, Some(2))]);

		// add one more approval
		assert_ok!(Uniques::approve_transfer(
			Origin::signed(user_1),
			collection_id,
			item_id,
			user_3,
			None
		));

		assert_eq!(approvals(collection_id, item_id), vec![(user_2, Some(2)), (user_3, None)]);

		// ensure we can remove the approval
		assert_ok!(Uniques::remove_transfer_approval(
			Origin::signed(user_1),
			collection_id,
			item_id,
			user_2
		));

		assert_eq!(approvals(collection_id, item_id), vec![(user_3, None)]);

		// ensure we can't remove an approval if it wasn't set before
		assert_noop!(
			Uniques::remove_transfer_approval(
				Origin::signed(user_1),
				collection_id,
				item_id,
				user_2
			),
			Error::<Test>::WrongDelegate
		);

		// ensure we can clear all the approvals
		assert_ok!(Uniques::clear_all_transfer_approvals(
			Origin::signed(user_1),
			collection_id,
			item_id
		));

		assert_eq!(approvals(collection_id, item_id), vec![]);

		// validate anyone can remove an expired approval
		assert_ok!(Uniques::approve_transfer(
			Origin::signed(user_1),
			collection_id,
			item_id,
			user_3,
			Some(0)
		));
		assert_ok!(Uniques::remove_transfer_approval(
			Origin::signed(user_2),
			collection_id,
			item_id,
			user_3
		));

		// ensure we can't buy an item when the collection has a NonTransferableItems flag
		let collection_id = 1;
		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(UserFeature::NonTransferableItems.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_id));

		assert_noop!(
			Uniques::approve_transfer(Origin::signed(user_1), collection_id, item_id, user_2, None),
			Error::<Test>::ItemsNotTransferable
		);
	});
}

#[test]
fn transfer_with_approval_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let user_2 = 2;
		let user_3 = 3;
		let collection_id = 0;
		let item_id = 1;

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));
		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_id));
		assert_ok!(Uniques::approve_transfer(
			Origin::signed(user_1),
			collection_id,
			item_id,
			user_2,
			None
		));

		assert_ok!(Uniques::transfer_item(
			Origin::signed(user_2),
			collection_id,
			item_id,
			user_3,
			CollectionConfigs::<Test>::get(collection_id).unwrap()
		));

		// the approvals field should be reset
		assert_eq!(approvals(collection_id, item_id), vec![]);

		// and we can't transfer this item from the previous owner or pre-approved account anymore
		assert_noop!(
			Uniques::transfer_item(
				Origin::signed(user_2),
				collection_id,
				item_id,
				user_3,
				CollectionConfigs::<Test>::get(collection_id).unwrap()
			),
			Error::<Test>::NotAuthorized
		);
		assert_noop!(
			Uniques::transfer_item(
				Origin::signed(user_1),
				collection_id,
				item_id,
				user_3,
				CollectionConfigs::<Test>::get(collection_id).unwrap()
			),
			Error::<Test>::NotAuthorized
		);

		// validate approval's deadline works
		assert_ok!(Uniques::approve_transfer(
			Origin::signed(user_3),
			collection_id,
			item_id,
			user_2,
			Some(0)
		));
		assert_noop!(
			Uniques::transfer_item(
				Origin::signed(user_2),
				collection_id,
				item_id,
				user_1,
				CollectionConfigs::<Test>::get(collection_id).unwrap()
			),
			Error::<Test>::AuthorizationExpired
		);
	});
}

#[test]
fn accept_buy_offer_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let user_2 = 2;
		let collection_id = 0;
		let item_id = 1;
		let bid_price = 5;
		let initial_balance = 100;
		let signer = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
		let signer_id = signer_to_account_id(&signer.clone());

		Balances::make_free_balance_be(&user_1, initial_balance);
		Balances::make_free_balance_be(&signer_id, initial_balance);

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_id, item_id));

		let offer = BuyOffer {
			collection_id,
			item_id,
			bid_price,
			deadline: None,
			item_owner: user_1,
			signer: signer.clone(),
			receiver: user_2.clone(),
		};
		let valid_signature =
			crypto::create_ed25519_signature(&Encode::encode(&offer), signer.clone());
		let invalid_signature = MultiSignature::decode(&mut TrailingZeroInput::zeroes()).unwrap();

		assert_ok!(Uniques::accept_buy_offer(
			Origin::signed(user_1),
			offer.clone(),
			valid_signature
		));

		assert!(events().contains(&Event::<Test>::BuyOfferAccepted {
			collection_id,
			item_id,
			price: bid_price,
			seller: user_1,
			buyer: signer_id,
			receiver: user_2,
			deadline: None,
		}));

		assert_eq!(items(), vec![(user_2, collection_id, item_id)]);

		assert_eq!(Balances::total_balance(&user_1), initial_balance + bid_price);
		assert_eq!(Balances::total_balance(&signer_id), initial_balance - bid_price);

		assert_noop!(
			Uniques::accept_buy_offer(Origin::signed(user_1), offer, invalid_signature),
			Error::<Test>::InvalidSignature
		);
	});
}

#[test]
fn swap_items_should_work() {
	new_test_ext().execute_with(|| {
		let user_2 = 2;
		let collection_from_id = 0;
		let collection_to_id = 1;
		let item_from_id = 1;
		let item_to_id = 2;
		let price = 5;
		let initial_balance = 100;
		let signer = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
		let user_1 = signer_to_account_id(&signer.clone());

		Balances::make_free_balance_be(&user_1, initial_balance);
		Balances::make_free_balance_be(&user_2, initial_balance);

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_1), user_1, collection_from_id, item_from_id));

		assert_ok!(Uniques::create(
			Origin::signed(user_2),
			user_2,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		assert_ok!(Uniques::mint(Origin::signed(user_2), user_2, collection_to_id, item_to_id));

		assert_eq!(
			items(),
			vec![
				(user_2, collection_to_id, item_to_id),
				(user_1, collection_from_id, item_from_id)
			]
		);

		let offer = SwapOffer {
			collection_from_id,
			item_from_id,
			collection_to_id,
			item_to_id: Some(item_to_id),
			price: Some(price),
			deadline: None,
			item_to_owner: user_2,
			signer: signer.clone(),
			item_from_receiver: user_2.clone(),
		};
		let valid_signature =
			crypto::create_ed25519_signature(&Encode::encode(&offer), signer.clone());
		let invalid_signature = MultiSignature::decode(&mut TrailingZeroInput::zeroes()).unwrap();

		assert_noop!(
			Uniques::swap_items(
				Origin::signed(user_1),
				offer.clone(),
				valid_signature.clone(),
				item_to_id
			),
			Error::<Test>::NotAuthorized
		);

		assert_ok!(Uniques::swap_items(
			Origin::signed(user_2),
			offer.clone(),
			valid_signature.clone(),
			item_to_id
		));

		assert!(events().contains(&Event::<Test>::ItemsSwapExecuted {
			collection_from_id,
			collection_to_id,
			item_from_id,
			item_to_id,
			executed_by: user_2,
			new_item_from_owner: user_2,
			new_item_to_owner: user_1,
			price: Some(price),
			deadline: None,
		}));

		assert_eq!(
			items(),
			vec![
				(user_2, collection_from_id, item_from_id),
				(user_1, collection_to_id, item_to_id)
			]
		);

		assert_eq!(Balances::total_balance(&user_1), initial_balance + price);
		assert_eq!(Balances::total_balance(&user_2), initial_balance - price);

		assert_noop!(
			Uniques::swap_items(
				Origin::signed(user_2),
				offer.clone(),
				invalid_signature,
				item_to_id
			),
			Error::<Test>::InvalidSignature
		);

		// item's owner has changed, thus the signature is no longer valid
		assert_noop!(
			Uniques::swap_items(Origin::signed(user_2), offer, valid_signature, item_to_id),
			Error::<Test>::NotAuthorized
		);
	});
}

#[test]
fn setting_royalties_should_work() {
	new_test_ext().execute_with(|| {
		let user_1 = 1;
		let collection_id = 0;
		let creator_royalties = 10;
		let owner_royalties = 20;

		assert_ok!(Uniques::create(
			Origin::signed(user_1),
			user_1,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::from_percent(creator_royalties),
			Perbill::from_percent(owner_royalties),
		));

		let collection_config = CollectionConfigs::<Test>::get(collection_id);
		let system_features = collection_config.unwrap().system_features.get();
		assert!(system_features.contains(SystemFeature::OwnerRoyalties));
		assert!(system_features.contains(SystemFeature::CreatorRoyalties));

		// validate we can't increase royalties
		assert_noop!(
			Uniques::change_creator_royalties(
				Origin::signed(user_1),
				collection_id,
				Perbill::from_percent(creator_royalties + 10),
			),
			Error::<Test>::RoyaltiesBiggerToPreviousValue
		);

		// validate we can increase owner's royalties while the collection isn't locked
		assert_ok!(Uniques::change_owner_royalties(
			Origin::signed(user_1),
			collection_id,
			Perbill::from_percent(creator_royalties + 10),
		));
		assert_noop!(
			Uniques::change_owner_royalties(
				Origin::signed(user_1),
				collection_id,
				Perbill::from_percent(95),
			),
			Error::<Test>::TotalRoyaltiesExceedHundredPercent
		);
		assert_ok!(Uniques::change_collection_config(
			Origin::signed(user_1),
			collection_id,
			UserFeatures::new(UserFeature::IsLocked.into())
		));
		assert_noop!(
			Uniques::change_owner_royalties(
				Origin::signed(user_1),
				collection_id,
				Perbill::from_percent(creator_royalties + 20),
			),
			Error::<Test>::RoyaltiesBiggerToPreviousValue
		);

		// remove owner's royalties
		assert_ok!(Uniques::change_owner_royalties(
			Origin::signed(user_1),
			collection_id,
			Perbill::zero(),
		));
		let collection_config = CollectionConfigs::<Test>::get(collection_id);
		let system_features = collection_config.unwrap().system_features.get();
		assert!(!system_features.contains(SystemFeature::OwnerRoyalties));
		assert!(system_features.contains(SystemFeature::CreatorRoyalties));

		// validate event
		assert!(events().contains(&Event::<Test>::OwnerRoyaltiesChanged {
			id: collection_id,
			owner: user_1,
			royalties: Perbill::zero(),
		}));

		// remove creator royalties
		assert_ok!(Uniques::change_creator_royalties(
			Origin::signed(user_1),
			collection_id,
			Perbill::zero(),
		));
		let collection_config = CollectionConfigs::<Test>::get(collection_id);
		let system_features = collection_config.unwrap().system_features.get();
		assert!(!system_features.contains(SystemFeature::OwnerRoyalties));
		assert!(!system_features.contains(SystemFeature::CreatorRoyalties));

		// validate event
		assert!(events().contains(&Event::<Test>::CreatorRoyaltiesChanged {
			id: collection_id,
			creator: user_1,
			royalties: Perbill::zero(),
		}));

		// can't set royalties higher to 100% in total
		assert_noop!(
			Uniques::create(
				Origin::signed(user_1),
				user_1,
				UserFeatures::new(DEFAULT_USER_FEATURES.into()),
				None,
				None,
				Perbill::from_percent(70),
				Perbill::from_percent(40),
			),
			Error::<Test>::TotalRoyaltiesExceedHundredPercent
		);
	});
}

#[test]
fn paying_royalties_when_buying_an_item() {
	new_test_ext().execute_with(|| {
		let collection_id = 0;
		let item_id = 1;
		let user_1 = 1;
		let user_2 = 2;
		let creator = 3;
		let owner = 4;
		let price = 10;
		let initial_balance = 2000;

		Balances::make_free_balance_be(&user_1, initial_balance);
		Balances::make_free_balance_be(&user_2, initial_balance);
		Balances::make_free_balance_be(&creator, initial_balance);
		Balances::make_free_balance_be(&owner, initial_balance);

		assert_ok!(Uniques::create(
			Origin::signed(creator),
			owner,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::from_percent(10),
			Perbill::from_percent(20),
		));

		assert_ok!(Uniques::mint(Origin::signed(owner), user_1, collection_id, item_id));

		assert_ok!(Uniques::set_price(
			Origin::signed(user_1),
			collection_id,
			item_id,
			Some(price),
			None,
		));

		assert_ok!(Uniques::buy_item(Origin::signed(user_2), collection_id, item_id, price));

		// validate balances
		let expect_creator_royalties = 1;
		let expect_owner_royalties = 2;
		let expect_received_for_item = price - expect_creator_royalties - expect_owner_royalties;

		assert_eq!(Balances::total_balance(&creator), initial_balance + expect_creator_royalties);
		assert_eq!(Balances::total_balance(&owner), initial_balance + expect_owner_royalties);
		assert_eq!(Balances::total_balance(&user_1), initial_balance + expect_received_for_item);
		assert_eq!(Balances::total_balance(&user_2), initial_balance - price);

		// validate events
		let events = events();
		assert!(events.contains(&Event::<Test>::CreatorRoyaltiesPaid {
			collection_id,
			item_id,
			amount: expect_creator_royalties,
			payer: user_2,
			receiver: creator,
		}));
		assert!(events.contains(&Event::<Test>::OwnerRoyaltiesPaid {
			collection_id,
			item_id,
			amount: expect_owner_royalties,
			payer: user_2,
			receiver: owner,
		}));
	});
}

#[test]
fn paying_royalties_when_accepting_an_offer() {
	new_test_ext().execute_with(|| {
		let collection_id = 0;
		let item_id = 1;

		let signer = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
		let user_1 = signer_to_account_id(&signer.clone());
		let user_2 = 2;
		let user_3 = 3;
		let creator = 4;
		let owner = 5;
		let price = 10;
		let initial_balance = 2000;

		Balances::make_free_balance_be(&user_1, initial_balance);
		Balances::make_free_balance_be(&user_2, initial_balance);
		Balances::make_free_balance_be(&user_3, initial_balance);
		Balances::make_free_balance_be(&creator, initial_balance);
		Balances::make_free_balance_be(&owner, initial_balance);

		assert_ok!(Uniques::create(
			Origin::signed(creator),
			owner,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::from_percent(10),
			Perbill::from_percent(20),
		));

		assert_ok!(Uniques::mint(Origin::signed(owner), user_2, collection_id, item_id));

		let offer = BuyOffer {
			collection_id,
			item_id,
			bid_price: price,
			deadline: None,
			item_owner: user_2,
			signer: signer.clone(),
			receiver: user_3.clone(),
		};
		let valid_signature =
			crypto::create_ed25519_signature(&Encode::encode(&offer), signer.clone());

		assert_ok!(Uniques::accept_buy_offer(
			Origin::signed(user_2),
			offer.clone(),
			valid_signature,
		));

		// validate balances
		let expect_creator_royalties = 1;
		let expect_owner_royalties = 2;
		let expect_received_for_item = price - expect_creator_royalties - expect_owner_royalties;

		assert_eq!(Balances::total_balance(&creator), initial_balance + expect_creator_royalties);
		assert_eq!(Balances::total_balance(&owner), initial_balance + expect_owner_royalties);
		assert_eq!(Balances::total_balance(&user_1), initial_balance - price);
		assert_eq!(Balances::total_balance(&user_2), initial_balance + expect_received_for_item);
		assert_eq!(Balances::total_balance(&user_3), initial_balance);

		// validate events
		let events = events();
		assert!(events.contains(&Event::<Test>::CreatorRoyaltiesPaid {
			collection_id,
			item_id,
			amount: expect_creator_royalties,
			payer: user_1,
			receiver: creator,
		}));
		assert!(events.contains(&Event::<Test>::OwnerRoyaltiesPaid {
			collection_id,
			item_id,
			amount: expect_owner_royalties,
			payer: user_1,
			receiver: owner,
		}));
	});
}

#[test]
fn paying_royalties_when_swapping_items() {
	new_test_ext().execute_with(|| {
		let collection_from_id = 0;
		let collection_to_id = 1;
		let item_from_id = 1;
		let item_to_id = 2;

		let signer = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
		let user_1 = signer_to_account_id(&signer.clone());
		let user_2 = 2;
		let user_3 = 3;
		let creator = 4;
		let owner = 5;
		let price = 10;
		let initial_balance = 2000;

		Balances::make_free_balance_be(&user_1, initial_balance);
		Balances::make_free_balance_be(&user_2, initial_balance);
		Balances::make_free_balance_be(&user_3, initial_balance);
		Balances::make_free_balance_be(&creator, initial_balance);
		Balances::make_free_balance_be(&owner, initial_balance);

		assert_ok!(Uniques::create(
			Origin::signed(creator),
			owner,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::from_percent(10),
			Perbill::from_percent(20),
		));

		assert_ok!(Uniques::mint(Origin::signed(owner), user_1, collection_from_id, item_from_id));

		assert_ok!(Uniques::create(
			Origin::signed(creator),
			owner,
			UserFeatures::new(DEFAULT_USER_FEATURES.into()),
			None,
			None,
			Perbill::from_percent(10),
			Perbill::from_percent(20),
		));

		assert_ok!(Uniques::mint(Origin::signed(owner), user_2, collection_to_id, item_to_id));

		let offer = SwapOffer {
			collection_from_id,
			item_from_id,
			collection_to_id,
			item_to_id: Some(item_to_id),
			price: Some(price),
			deadline: None,
			item_to_owner: user_2,
			signer: signer.clone(),
			item_from_receiver: user_2.clone(),
		};
		let valid_signature =
			crypto::create_ed25519_signature(&Encode::encode(&offer), signer.clone());

		assert_ok!(Uniques::swap_items(
			Origin::signed(user_2),
			offer.clone(),
			valid_signature.clone(),
			item_to_id,
		));

		// validate balances
		let expect_creator_royalties = 2;
		let expect_owner_royalties = 3;
		let expect_received_for_item = price - expect_creator_royalties - expect_owner_royalties;

		assert_eq!(Balances::total_balance(&creator), initial_balance + expect_creator_royalties);
		assert_eq!(Balances::total_balance(&owner), initial_balance + expect_owner_royalties);
		assert_eq!(Balances::total_balance(&user_1), initial_balance + expect_received_for_item);
		assert_eq!(Balances::total_balance(&user_2), initial_balance - price);
		assert_eq!(Balances::total_balance(&user_3), initial_balance);

		// validate events
		let events = events();
		assert!(events.contains(&Event::<Test>::CreatorRoyaltiesPaid {
			collection_id: collection_from_id,
			item_id: item_from_id,
			amount: 1,
			payer: user_2,
			receiver: creator,
		}));
		assert!(events.contains(&Event::<Test>::CreatorRoyaltiesPaid {
			collection_id: collection_to_id,
			item_id: item_to_id,
			amount: 1,
			payer: user_2,
			receiver: creator,
		}));
		assert!(events.contains(&Event::<Test>::OwnerRoyaltiesPaid {
			collection_id: collection_from_id,
			item_id: item_from_id,
			amount: 2,
			payer: user_2,
			receiver: owner,
		}));
		assert!(events.contains(&Event::<Test>::OwnerRoyaltiesPaid {
			collection_id: collection_to_id,
			item_id: item_to_id,
			amount: 1,
			payer: user_2,
			receiver: owner,
		}));
	});
}

#[test]
fn different_user_flags() {
	new_test_ext().execute_with(|| {
		// when setting one feature it's required to call .into() on it
		let user_features = UserFeatures::new(UserFeature::IsLocked.into());
		assert_ok!(Uniques::create(
			Origin::signed(1),
			1,
			user_features,
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		let collection_config = CollectionConfigs::<Test>::get(0);
		let stored_user_features = collection_config.unwrap().user_features.get();
		assert!(stored_user_features.contains(UserFeature::IsLocked));
		assert!(!stored_user_features.contains(UserFeature::Administration));

		// no need to call .into() for multiple features
		let user_features = UserFeatures::new(UserFeature::Administration | UserFeature::IsLocked);
		assert_ok!(Uniques::create(
			Origin::signed(1),
			1,
			user_features,
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));
		let collection_config = CollectionConfigs::<Test>::get(1);
		let stored_user_features = collection_config.unwrap().user_features.get();
		assert!(stored_user_features.contains(UserFeature::IsLocked));
		assert!(stored_user_features.contains(UserFeature::Administration));

		assert_ok!(Uniques::create(
			Origin::signed(1),
			1,
			UserFeatures::new(BitFlags::EMPTY),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));

		use enumflags2::BitFlag;
		assert_ok!(Uniques::create(
			Origin::signed(1),
			1,
			UserFeatures::new(UserFeature::empty()),
			None,
			None,
			Perbill::zero(),
			Perbill::zero(),
		));
	});
}
