// Copyright The pipewire-rs Contributors.
// SPDX-License-Identifier: MIT

use std::{convert::TryInto, fmt};

use errno::Errno;

#[derive(Debug, PartialEq)]
pub struct SpaResult(i32);

/// An asynchronous sequence number returned by a SPA component.
/// Use [`AsyncSeq::seq`] to retrive the actual sequence number.
#[derive(PartialEq, Copy, Clone)]
pub struct AsyncSeq(i32);

#[derive(Debug, PartialEq)]
pub enum SpaSuccess {
    Sync(i32),
    Async(AsyncSeq),
}

fn async_seq(res: i32) -> i32 {
    let mask: i32 = spa_sys::SPA_ASYNC_SEQ_MASK.try_into().unwrap();
    res & mask
}

fn is_async(val: i32) -> bool {
    let bit: i32 = spa_sys::SPA_ASYNC_BIT.try_into().unwrap();
    (val & spa_sys::SPA_ASYNC_MASK as i32) == bit
}

impl AsyncSeq {
    /// The sequence number
    pub fn seq(&self) -> i32 {
        async_seq(self.0)
    }

    /// The raw value, this is the sequence number with the `SPA_ASYNC_BIT` bit set
    pub fn raw(&self) -> i32 {
        self.0
    }

    /// Create a new [`AsyncSeq`] from a sequence number
    pub fn from_seq(seq: i32) -> Self {
        let bit: i32 = spa_sys::SPA_ASYNC_BIT.try_into().unwrap();
        let res = bit | async_seq(seq);

        Self(res)
    }

    /// Create a new [`AsyncSeq`] from a raw value having the `SPA_ASYNC_BIT` bit set
    pub fn from_raw(val: i32) -> Self {
        debug_assert!(is_async(val));
        Self(val)
    }
}

impl fmt::Debug for AsyncSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AsyncSeq seq: {} raw: {}", &self.seq(), &self.raw())
    }
}

impl SpaResult {
    pub fn from_c(res: i32) -> Self {
        Self(res)
    }

    /// Pending return for async operation identified with sequence number `seq`.
    pub fn new_return_async(seq: i32) -> Self {
        let seq = AsyncSeq::from_seq(seq);
        Self::from_c(seq.raw())
    }

    fn is_async(&self) -> bool {
        is_async(self.0)
    }

    pub fn into_result(self) -> Result<SpaSuccess, Error> {
        if self.0 < 0 {
            Err(Error::new(-self.0))
        } else if self.is_async() {
            Ok(SpaSuccess::Async(AsyncSeq::from_raw(self.0)))
        } else {
            Ok(SpaSuccess::Sync(self.0))
        }
    }

    /// Convert a [`SpaResult`] into either an [`AsyncSeq`] or an [`Error`].
    /// This method will panic if the result is a synchronous success.
    pub fn into_async_result(self) -> Result<AsyncSeq, Error> {
        let res = self.into_result()?;

        match res {
            SpaSuccess::Async(res) => Ok(res),
            SpaSuccess::Sync(_) => panic!("result is synchronous success"),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Error(Errno);

impl Error {
    fn new(e: i32) -> Self {
        assert!(e > 0);

        Self(Errno(e))
    }
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    /* the errno crate is calling foreign function __xpg_strerror_r which is not supported by miri */
    fn spa_result() {
        assert!(!SpaResult::from_c(0).is_async());
        assert!(SpaResult::new_return_async(0).is_async());
        assert_eq!(
            SpaResult::new_return_async(0).into_async_result(),
            Ok(AsyncSeq::from_seq(0))
        );

        assert_eq!(SpaResult::from_c(0).into_result(), Ok(SpaSuccess::Sync(0)));
        assert_eq!(SpaResult::from_c(1).into_result(), Ok(SpaSuccess::Sync(1)));
        assert_eq!(
            SpaResult::new_return_async(1).into_result(),
            Ok(SpaSuccess::Async(AsyncSeq::from_seq(1)))
        );

        let err = SpaResult::from_c(-libc::EBUSY).into_result().unwrap_err();
        assert_eq!(format!("{}", err), "Device or resource busy",);
    }

    #[test]
    fn async_seq() {
        assert_eq!(AsyncSeq::from_seq(0).seq(), 0);
        assert_eq!(AsyncSeq::from_seq(1).seq(), 1);
    }

    #[should_panic]
    #[test]
    fn async_seq_panic() {
        // raw value does not have the SPA_ASYNC_BIT set
        AsyncSeq::from_raw(1);
    }

    #[should_panic]
    #[test]
    fn spa_async_result_panic() {
        let _ = SpaResult::from_c(0).into_async_result();
    }
}