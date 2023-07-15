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

#![cfg(feature = "runtime-benchmarks")]

use super::*;

use crate::CoreAssignment::Task;
use frame_benchmarking::v2::*;
use frame_support::traits::EnsureOrigin;
use sp_arithmetic::Perbill;

fn new_config_record<T: Config>() -> ConfigRecordOf<T> {
	ConfigRecord {
		advance_notice: 2u32.into(),
		interlude_length: 1u32.into(),
		leadin_length: 1u32.into(),
		ideal_bulk_proportion: Default::default(),
		limit_cores_offered: None,
		region_length: 3,
		renewal_bump: Perbill::from_percent(10),
		contribution_timeout: 5,
	}
}

#[benchmarks]
mod benches {
	use super::*;
	use frame_support::storage::bounded_vec::BoundedVec;
	use sp_core::Get;

	#[benchmark]
	fn configure() -> Result<(), BenchmarkError> {
		let config = new_config_record::<T>();

		let origin =
			T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, config.clone());

		assert_eq!(Configuration::<T>::get(), Some(config));

		Ok(())
	}

	#[benchmark]
	fn reserve() -> Result<(), BenchmarkError> {
		// Max items for worst case
		let mut items = Vec::new();
		for i in 0..80 {
			items.push(ScheduleItem { assignment: Task(i), part: CoreMask::complete() });
		}
		let schedule = Schedule::truncate_from(items);

		// Assume MaxReservations to be almost filled for worst case
		Reservations::<T>::put(
			BoundedVec::try_from(vec![
				schedule.clone();
				T::MaxReservedCores::get().saturating_sub(1) as usize
			])
			.unwrap(),
		);

		let origin =
			T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, schedule.clone());

		assert_eq!(Reservations::<T>::get().len(), T::MaxReservedCores::get() as usize);

		Ok(())
	}

	#[benchmark]
	fn unreserve(
		n: Linear<0, { T::MaxReservedCores::get().saturating_sub(1) }>,
	) -> Result<(), BenchmarkError> {
		// Max items for worst case
		let mut items = Vec::new();
		for i in 0..80 {
			items.push(ScheduleItem { assignment: Task(i), part: CoreMask::complete() });
		}
		let schedule = Schedule::truncate_from(items);

		// Assume MaxReservations to be filled for worst case
		Reservations::<T>::put(
			BoundedVec::try_from(vec![schedule.clone(); T::MaxReservedCores::get() as usize])
				.unwrap(),
		);

		let origin =
			T::AdminOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;

		#[extrinsic_call]
		_(origin as T::RuntimeOrigin, n);

		assert_eq!(
			Reservations::<T>::get().len(),
			T::MaxReservedCores::get().saturating_sub(1) as usize
		);

		Ok(())
	}

	// Implements a test for each benchmark. Execute with:
	// `cargo test -p pallet-broker --features runtime-benchmarks`.
	impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
