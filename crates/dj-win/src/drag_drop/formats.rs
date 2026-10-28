//! Snapshot enumeration with independent cursors, including Clone at the current index.

use std::cell::Cell;
use std::rc::Rc;
use windows::core::{implement, Result, HRESULT};
use windows::Win32::Foundation::{E_POINTER, S_FALSE, S_OK};
use windows::Win32::System::Com::{IEnumFORMATETC, IEnumFORMATETC_Impl, FORMATETC};

#[implement(IEnumFORMATETC, Agile = false)]
struct Formats {
    // All entries have NULL ptd: no borrowed target-device pointers or native owners.
    snapshot: Rc<[FORMATETC]>,
    cursor: Cell<usize>,
}

pub(super) fn enumerate(formats: Vec<FORMATETC>) -> IEnumFORMATETC {
    debug_assert!(formats.iter().all(|format| format.ptd.is_null()));
    Formats {
        snapshot: formats.into(),
        cursor: Cell::new(0),
    }
    .into()
}

impl IEnumFORMATETC_Impl for Formats_Impl {
    fn Next(&self, requested: u32, output: *mut FORMATETC, fetched: *mut u32) -> HRESULT {
        if !fetched.is_null() {
            // SAFETY: a non-null COM output pointer must be writable by the caller.
            unsafe { fetched.write(0) };
        }
        if (requested > 0 && output.is_null()) || (requested > 1 && fetched.is_null()) {
            return E_POINTER;
        }
        let start = self.cursor.get();
        let count = (requested as usize).min(self.snapshot.len() - start);
        for offset in 0..count {
            // SAFETY: COM requires room for requested entries; count <= requested.
            // Each copied entry has NULL ptd, so there is no shared native ownership.
            unsafe { output.add(offset).write(self.snapshot[start + offset]) };
        }
        self.cursor.set(start + count);
        if !fetched.is_null() {
            // SAFETY: same caller-provided writable output pointer as above.
            unsafe { fetched.write(count as u32) };
        }
        if count == requested as usize {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, requested: u32) -> Result<()> {
        let start = self.cursor.get();
        let count = (requested as usize).min(self.snapshot.len() - start);
        self.cursor.set(start + count);
        if count == requested as usize {
            Ok(())
        } else {
            // The generated trait models Skip as Result<()>. Preserve the required
            // positive S_FALSE in the ABI rather than collapsing it into S_OK.
            Err(windows::core::Error::from_hresult(S_FALSE))
        }
    }

    fn Reset(&self) -> Result<()> {
        self.cursor.set(0);
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumFORMATETC> {
        Ok(Formats {
            snapshot: self.snapshot.clone(),
            cursor: Cell::new(self.cursor.get()),
        }
        .into())
    }
}
