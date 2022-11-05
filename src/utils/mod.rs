// SPDX-License-Identifier: (GPL-2.0-or-later OR MIT)
//
// Copyright (c) 2022 SUSE LLC
//
// Author: Joerg Roedel <jroedel@suse.de>
//
// vim: ts=4 sw=4 et

pub mod alloc;
pub mod util;
pub mod vec;

pub use alloc::{alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc};
pub use util::{align_up, ffs, halt, page_align, page_align_up};