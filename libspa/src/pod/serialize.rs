//! This module deals with serializing rust types into raw SPA pods.
//!
//! A raw pod can be serialized by passing a implementor of the [`PodSerialize`] trait
//! to [`PodSerializer::serialize`].
//!
//! The crate provides a number of implementors of this trait either directly,
//! or through [`FixedSizedPod`](`super::FixedSizedPod`).
//!
//! You can also implement the [`PodSerialize`] trait on another type yourself. See the traits documentation for more
//! information on how to do that.

use std::{
    convert::TryInto,
    ffi::CString,
    io::{Seek, SeekFrom, Write},
    marker::PhantomData,
};

pub use cookie_factory::GenError;
use cookie_factory::{
    bytes::{ne_u32, ne_u8},
    combinator::slice,
    gen,
    multi::all,
    sequence::{pair, tuple},
    SerializeFn,
};

use super::{CanonicalFixedSizedPod, FixedSizedPod};

/// Implementors of this trait are able to serialize themselves into a SPA pod by using a [`PodSerializer`].
///
/// Their [`serialize`](`PodSerialize::serialize`) method should invoke exactly one of the `serialize_*()` methods
/// of the provided [`PodSerializer`] that fits the type that will be serialized.
///
/// If you want to serialize into a pod that always has the same size, implement [`FixedSizedPod`] instead
/// and this trait will be implemented for you automatically.
///
/// # Examples
/// Make a type serialize into a `String` pod.
/// ```rust
/// use std::io;
/// use libspa::pod::serialize::{GenError, PodSerialize, PodSerializer, SerializeSuccess};
///
/// struct StringNewtype(String);
///
/// impl PodSerialize for StringNewtype {
///     fn serialize<O: io::Write + io::Seek>(
///         &self,
///         serializer: PodSerializer<O>,
///     ) -> Result<SerializeSuccess<O>, GenError> {
///         serializer.serialize_string(self.0.as_str())
///     }
/// }
/// ```
/// `Bytes` pods are created in the same way, but with the `serialize_bytes` method.
///
/// Make a type serialize into a `Array` pod with `Int` pod elements:
/// ```rust
/// use std::io;
/// use libspa::pod::serialize::{GenError, PodSerialize, PodSerializer, SerializeSuccess};
///
/// struct Numbers(Vec<i32>);
///
/// impl PodSerialize for Numbers {
///     fn serialize<O: io::Write + io::Seek>(
///         &self,
///         serializer: PodSerializer<O>,
///     ) -> Result<SerializeSuccess<O>, GenError> {
///         let mut array_serializer = serializer.serialize_array(self.0.len() as u32)?;
///         for element in self.0.iter() {
///             array_serializer.serialize_element(element)?;
///         }
///         array_serializer.end()
///     }
/// }
/// ```
///
/// Make a struct serialize into a `Struct` pod:
/// ```rust
/// use std::io;
/// use libspa::pod::serialize::{GenError, PodSerialize, PodSerializer, SerializeSuccess};
///
/// struct Animal {
///     name: String,
///     feet: u8,
///     can_fly: bool,
/// }
///
/// impl PodSerialize for Animal {
///     fn serialize<O: io::Write + io::Seek>(
///         &self,
///         serializer: PodSerializer<O>,
///     ) -> Result<SerializeSuccess<O>, GenError> {
///         let mut struct_serializer = serializer.serialize_struct()?;
///         struct_serializer.serialize_field(self.name.as_str())?;
///         // No pod exists for u8, we need to use an `Int` type pod by casting to `i32`.
///         struct_serializer.serialize_field(&(self.feet as i32))?;
///         struct_serializer.serialize_field(&self.can_fly)?;
///         struct_serializer.end()
///     }
/// }
/// ```
pub trait PodSerialize {
    /// Serialize the type by using the provided [`PodSerializer`]
    fn serialize<O: Write + Seek>(
        &self,
        serializer: PodSerializer<O>,
    ) -> Result<SerializeSuccess<O>, GenError>;
}

// Serialize into a `String` pod.
impl PodSerialize for str {
    fn serialize<O: Write + Seek>(
        &self,
        serializer: PodSerializer<O>,
    ) -> Result<SerializeSuccess<O>, GenError> {
        serializer.serialize_string(self)
    }
}

// Serialize into a `Bytes` pod.
impl PodSerialize for [u8] {
    fn serialize<O: Write + Seek>(
        &self,
        serializer: PodSerializer<O>,
    ) -> Result<SerializeSuccess<O>, GenError> {
        serializer.serialize_bytes(self)
    }
}

impl<P: FixedSizedPod> PodSerialize for [P] {
    fn serialize<O: Write + Seek>(
        &self,
        serializer: PodSerializer<O>,
    ) -> Result<SerializeSuccess<O>, GenError> {
        let mut arr_serializer = serializer.serialize_array(
            self.len()
                .try_into()
                .expect("Array length does not fit in a u32"),
        )?;

        for element in self.iter() {
            arr_serializer.serialize_element(element)?;
        }

        arr_serializer.end()
    }
}

/// This struct is returned by [`PodSerialize`] implementors on serialization sucess.
///
/// Because this can only be constructed by the [`PodSerializer`], [`PodSerialize`] implementors are forced
/// to finish serialization of their pod instead of stopping after serializing only part of a pod.
pub struct SerializeSuccess<O: Write + Seek> {
    /// Because [`PodSerialize`] implementors get ownership of the serializer,
    /// it is returned back to the caller in this struct.
    serializer: PodSerializer<O>,
    /// The number of bytes written by the serialization operation that returns this struct.
    len: u64,
}

/// This struct is responsible for serializing a [`PodSerialize`] implementor into the raw POD format.
pub struct PodSerializer<O: Write + Seek> {
    /// The writer is saved in an option, but can be expected to always be a `Some` when a `serialize_*` function
    /// is called.
    /// The function should then `take()` the writer, use it to serialize the item,
    /// and must then put the writer back inside.
    /// The [`Self::gen`] function can be used to do this.
    out: Option<O>,
}

impl<O: Write + Seek> PodSerializer<O> {
    /// Serialize the provided POD into the raw pod format, writing it into `out`.
    ///
    /// When serializing into an in-memory-buffer such as [`Vec`], you might have to wrap it into a [`std::io::Cursor`]
    /// to provide the [`Seek`] trait.
    ///
    /// The function returns back the `out` writer and the number of bytes written,
    /// or a generation error if serialization failed.
    pub fn serialize<P>(out: O, pod: &P) -> Result<(O, u64), GenError>
    where
        P: PodSerialize + ?Sized,
    {
        let serializer = Self { out: Some(out) };

        pod.serialize(serializer).map(|success| {
            (
                success
                    .serializer
                    .out
                    .expect("Serializer does not contain a writer"),
                success.len,
            )
        })
    }

    /// Helper serialization method for serializing the Pod header.
    ///
    /// # Parameters
    /// - size: The size of the pod body
    /// - type: The type of the pod, e.g. `spa_sys::SPA_TYPE_Int` for a `spa_pod_int`
    fn header(size: usize, ty: u32) -> impl SerializeFn<O> {
        pair(ne_u32(size as u32), ne_u32(ty))
    }

    /// Helper serialization function for adding padding to a pod..
    ///
    /// Pad output with 0x00 bytes so that it is aligned to 8 bytes.
    fn padding(len: usize) -> impl SerializeFn<O> {
        let zeroes = std::iter::repeat(0u8);
        all(zeroes.take(len).map(ne_u8))
    }

    /// Use the provided serialization function to write into the writer contained in self.
    fn gen(&mut self, f: impl SerializeFn<O>) -> Result<u64, GenError> {
        gen(
            f,
            self.out
                .take()
                .expect("PodSerializer does not contain a writer"),
        )
        .map(|(writer, len)| {
            self.out = Some(writer);
            len
        })
    }

    /// Write out a full pod (header, body, padding), with the body computed using the provided serialization function.
    fn write_pod(
        mut self,
        size: usize,
        type_: u32,
        f: impl SerializeFn<O>,
    ) -> Result<SerializeSuccess<O>, GenError> {
        let padding = if size % 8 == 0 { 0 } else { 8 - (size % 8) };
        let written = self.gen(tuple((
            Self::header(size, type_),
            f,
            Self::padding(padding),
        )))?;

        Ok(SerializeSuccess {
            serializer: self,
            len: written,
        })
    }

    // Implementation note:
    // Each `serialize_*` function must serialize the _full_ pod, meaning header, body, and padding.
    // The `write_pod` method may be used to help with this.

    /// Serialize any fixed size pod.
    ///
    /// The type of the serialized pod will depend on the [`FixedSizedPod::CanonicalType`] that the passed type has.
    pub fn serialized_fixed_sized_pod<P>(self, pod: &P) -> Result<SerializeSuccess<O>, GenError>
    where
        P: FixedSizedPod + ?Sized,
    {
        self.write_pod(
            P::CanonicalType::SIZE as usize,
            P::CanonicalType::TYPE,
            |out| pod.as_canonical_type().serialize_body(out),
        )
    }

    /// Serialize a `String` pod.
    pub fn serialize_string(self, string: &str) -> Result<SerializeSuccess<O>, GenError> {
        let cstr = CString::new(string)
            .expect("Pod::String contains string with '\0' byte")
            .into_bytes_with_nul();
        self.write_pod(cstr.len(), spa_sys::SPA_TYPE_String, slice(cstr))
    }

    /// Serialize a `Bytes` pod.
    pub fn serialize_bytes(self, bytes: &[u8]) -> Result<SerializeSuccess<O>, GenError> {
        self.write_pod(bytes.len(), spa_sys::SPA_TYPE_Bytes, slice(bytes))
    }

    /// Begin serializing an `Array` pod with exactly `length` elements.
    pub fn serialize_array<P: FixedSizedPod>(
        mut self,
        length: u32,
    ) -> Result<ArrayPodSerializer<O, P>, GenError> {
        self.gen(pair(
            Self::header(
                (8 + length * P::CanonicalType::SIZE) as usize,
                spa_sys::SPA_TYPE_Array,
            ),
            Self::header(P::CanonicalType::SIZE as usize, P::CanonicalType::TYPE),
        ))?;

        Ok(ArrayPodSerializer {
            serializer: self,
            length,
            written: 0,
            _phantom: PhantomData,
        })
    }

    /// Begin serializing a `Struct` pod.
    pub fn serialize_struct(mut self) -> Result<StructPodSerializer<O>, GenError> {
        let header_position = self
            .out
            .as_mut()
            .expect("PodSerializer does not contain a writer")
            // This does not actually change the writer, only returns the current position.
            .seek(SeekFrom::Current(0))
            .expect("Could not get current position in writer");

        // Write a size of 0 for now, this will be updated when calling `StructPodSerializer.end()`.
        self.gen(Self::header(0, spa_sys::SPA_TYPE_Struct))?;

        Ok(StructPodSerializer {
            serializer: Some(self),
            header_position,
            written: 0,
        })
    }
}

/// This struct handles serializing arrays.
///
/// It can be obtained by calling [`PodSerializer::serialize_array`].
///
/// The exact number of elements that was specified during that call must be written into it
/// using its [`serialize_element`](`Self::serialize_element`) function,
/// followed by calling its [`end`](`Self::end`) function to finish serialization of the array.
pub struct ArrayPodSerializer<O: Write + Seek, P: FixedSizedPod> {
    serializer: PodSerializer<O>,
    /// The total length the array should have
    length: u32,
    /// The number of elements that have been written already
    written: u32,
    /// The struct has the type parameter P to ensure all serialized elements are the same type,
    /// but doesn't actually own any P, so we need the `PhantomData<P>` instead.
    _phantom: PhantomData<P>,
}

impl<O: Write + Seek, P: FixedSizedPod> ArrayPodSerializer<O, P> {
    /// Serialize a single element.
    ///
    /// Returns the amount of bytes written for this field.
    pub fn serialize_element(&mut self, elem: &P) -> Result<u64, GenError> {
        if !self.written < self.length {
            panic!("More elements than specified were serialized into the array POD");
        }

        let result = self
            .serializer
            .gen(|out| elem.as_canonical_type().serialize_body(out));
        self.written += 1;
        result
    }

    /// Finish serializing the array.
    pub fn end(mut self) -> Result<SerializeSuccess<O>, GenError> {
        assert_eq!(
            self.length, self.written,
            "Array POD was not serialized with the specified amount of elements"
        );

        let bytes_written = self.written * P::CanonicalType::SIZE;

        let padding = if bytes_written % 8 == 0 {
            0
        } else {
            8 - (bytes_written as usize % 8)
        };

        // Add padding to the pod.
        let pad_bytes = self.serializer.gen(PodSerializer::padding(padding))?;

        Ok(SerializeSuccess {
            serializer: self.serializer,
            // Number of bytes written for the pod is two headers + body length + padding
            len: 16 + u64::from(self.written * P::CanonicalType::SIZE) + pad_bytes,
        })
    }
}

/// This struct handles serializing structs.
///
/// It can be obtained by calling [`PodSerializer::serialize_struct`].
///
/// Its [`serialize_field`}(`Self::serialize_field`) method can be repeatedly called to serialize one field each.
/// To finalize the struct, its [`end`}(`Self::end`) method must be called.
pub struct StructPodSerializer<O: Write + Seek> {
    /// The serializer is saved in an option, but can be expected to always be a `Some`
    /// when `serialize_field()` or `end()` is called.
    ///
    /// `serialize_field()` `take()`s the serializer, uses it to serialize the field,
    /// and then puts the serializer back inside.
    serializer: Option<PodSerializer<O>>,
    /// The position to seek to when modifying header.
    header_position: u64,
    written: usize,
}

impl<O: Write + Seek> StructPodSerializer<O> {
    /// Serialize a single field of the struct.
    ///
    /// Returns the amount of bytes written for this field.
    pub fn serialize_field<P>(&mut self, field: &P) -> Result<u64, GenError>
    where
        P: PodSerialize + ?Sized,
    {
        let success = field.serialize(
            self.serializer
                .take()
                .expect("StructSerializer does not contain a serializer"),
        )?;
        self.written += success.len as usize;
        self.serializer = Some(success.serializer);
        Ok(success.len)
    }

    /// Finish serialization of the pod.
    pub fn end(self) -> Result<SerializeSuccess<O>, GenError> {
        let mut serializer = self
            .serializer
            .expect("StructSerializer does not contain a serializer");

        // Seek to header position, write header with updates size, seek back.
        serializer
            .out
            .as_mut()
            .expect("Serializer does not contain a writer")
            .seek(SeekFrom::Start(self.header_position))
            .expect("Failed to seek to header position");

        serializer.gen(PodSerializer::header(
            self.written,
            spa_sys::SPA_TYPE_Struct,
        ))?;

        serializer
            .out
            .as_mut()
            .expect("Serializer does not contain a writer")
            .seek(SeekFrom::End(0))
            .expect("Failed to seek to end");

        // No padding needed: Last field will already end aligned.

        // Return full length of written pod.
        Ok(SerializeSuccess {
            serializer,
            len: self.written as u64 + 8,
        })
    }
}
