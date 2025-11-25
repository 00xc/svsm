// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2025 Coconut-SVSM Authors
//
// Author: Carlos López <carlos.lopezr4096@gmail.com>

use super::{Mapping, ReadAccess, WriteAccess};
use crate::address::PhysAddr;
use crate::mm::guestmem::do_movsb;
use crate::{error::SvsmError, mm::memory::valid_phys_region};
use zerocopy::{FromBytes, IntoBytes};

/// An empty structure to indicate access to guest-shared memory.
#[derive(Debug, Clone, Copy)]
pub struct Guest;

impl ReadAccess for Guest {
    unsafe fn read<T: FromBytes>(
        src: *const T,
        dst: *mut T,
        count: usize,
    ) -> Result<(), SvsmError> {
        // TODO: optimize this to a single call
        for i in 0..count {
            // SAFETY: safety requirements must be upheld by the caller
            unsafe {
                do_movsb(src.add(i), dst.add(i))?;
            }
        }
        Ok(())
    }
}

impl WriteAccess for Guest {
    unsafe fn write<T: IntoBytes>(
        src: *const T,
        dst: *mut T,
        count: usize,
    ) -> Result<(), SvsmError> {
        // TODO: optimize this
        for i in 0..count {
            // SAFETY: safety requirements must be upheld by the caller
            unsafe {
                do_movsb(src.add(i), dst.add(i))?;
            }
        }
        Ok(())
    }

    unsafe fn write_bytes<T: IntoBytes>(_: *mut T, _: usize, _: u8) -> Result<(), SvsmError> {
        unimplemented!()
    }
}

impl<T> Mapping<Guest, T> {
    /// Maps the given physical address of guest memory. This method is safe
    /// because it checks that the mapped region belongs to the guest.
    ///
    /// # Errors
    ///
    /// Other than due to allocation failures or page table mainupulation
    /// errors, this function may fail if the provided physical address is not
    /// present in the guest's memory map.
    pub fn map(paddr: PhysAddr) -> Result<Self, SvsmError> {
        Self::check_region(paddr, 1)?;
        Self::map_inner::<false>(paddr)
    }

    fn check_region(paddr: PhysAddr, len: usize) -> Result<(), SvsmError> {
        let region = Self::phys_region(paddr, len)?;
        if !valid_phys_region(&region) {
            return Err(SvsmError::Mem);
        }
        Ok(())
    }
}

impl<T> Mapping<Guest, [T]> {
    /// Maps the given physical address of guest memory as a slice with a
    /// dynamic size. This method is safe because it checks that the mapped
    /// region belongs to the guest.
    ///
    /// # Errors
    ///
    /// Other than due to allocation failures or page table mainupulation
    /// errors, this function may fail if the provided physical address is not
    /// present in the guest's memory map.
    pub fn map(paddr: PhysAddr, len: usize) -> Result<Self, SvsmError> {
        Mapping::<Guest, T>::check_region(paddr, len)?;
        Self::map_inner::<false>(paddr, len)
    }
}
