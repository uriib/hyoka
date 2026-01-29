use std::{
    alloc, mem,
    os::fd::AsFd,
    ptr::{self, NonNull},
    slice,
};

use bytes::Bytes;
use compio::buf::{IoBuf, IoBufMut, SetLen};
use rustix::{
    io,
    mm::{MapFlags, ProtFlags},
};

pub trait Size: Copy + 'static {
    fn size(self) -> usize;
}

#[derive(Clone, Copy)]
pub struct Const<const VALUE: usize>;

impl<const VALUE: usize> Size for Const<VALUE> {
    fn size(self) -> usize {
        VALUE
    }
}

impl Size for usize {
    fn size(self) -> usize {
        self
    }
}

pub struct Mapping<S: Size> {
    ptr: NonNull<u8>,
    len: S,
}

impl<S: Size> Drop for Mapping<S> {
    fn drop(&mut self) {
        unsafe {
            rustix::mm::munmap(self.ptr.as_ptr() as _, self.len.size()).unwrap();
        }
    }
}

impl<S: Size> Mapping<S> {
    pub fn anon(len: S, prot: ProtFlags, flags: MapFlags) -> io::Result<Self> {
        let ptr =
            unsafe { rustix::mm::mmap_anonymous(ptr::null_mut(), len.size(), prot, flags)? as _ };
        let ptr = unsafe { NonNull::new_unchecked(ptr) };
        Ok(Self { ptr, len })
    }
    pub fn as_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len.size()) }
    }
    pub fn as_bytes_mut(&self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len.size()) }
    }
    #[allow(dead_code)]
    fn into_boxed_slice(self) -> Box<[u8], Allocator> {
        let res = unsafe {
            Box::from_non_null_in(
                NonNull::slice_from_raw_parts(self.ptr, self.len.size()),
                Allocator,
            )
        };
        mem::forget(self);
        res
    }
    #[allow(dead_code)]
    fn into_vec(self) -> Vec<u8, Allocator> {
        self.into_boxed_slice().into_vec()
    }
}

impl Mapping<usize> {
    pub fn map(fd: impl AsFd, prot: ProtFlags, flags: MapFlags) -> io::Result<Self> {
        let len = rustix::fs::fstat(fd.as_fd())?.st_size as _;
        let ptr = unsafe { rustix::mm::mmap(ptr::null_mut(), len, prot, flags, fd, 0) }? as _;
        let ptr = unsafe { NonNull::new_unchecked(ptr) };
        Ok(Self { ptr, len })
    }
}

impl Mapping<Const<4096>> {
    pub fn page() -> io::Result<Self> {
        Self::anon(
            Const::<4096>,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::PRIVATE,
        )
    }
}

impl<S: Size> IoBufMut for Mapping<S> {
    fn as_uninit(&mut self) -> &mut [mem::MaybeUninit<u8>] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr().cast(), self.len.size()) }
    }
}

impl<S: Size> IoBuf for Mapping<S> {
    // use it to read only once
    fn buf_len(&self) -> usize {
        0
    }

    fn as_init(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<S: Size> SetLen for Mapping<S> {
    unsafe fn set_len(&mut self, _len: usize) {}
}

impl<S: Size> AsRef<[u8]> for Mapping<S> {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

unsafe impl<S: Size> Send for Mapping<S> {}

impl<S: Size> Into<Bytes> for Mapping<S> {
    fn into(self) -> Bytes {
        Bytes::from_owner(self)
    }
}

#[allow(dead_code)]
pub struct Allocator;

unsafe impl alloc::Allocator for Allocator {
    fn allocate(&self, layout: alloc::Layout) -> Result<NonNull<[u8]>, alloc::AllocError> {
        unsafe {
            rustix::mm::mmap_anonymous(
                ptr::null_mut(),
                layout.size(),
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            )
        }
        .map(|ptr| unsafe {
            NonNull::slice_from_raw_parts(NonNull::new_unchecked(ptr as _), layout.size())
        })
        .map_err(|_| alloc::AllocError)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: alloc::Layout) {
        unsafe {
            rustix::mm::munmap(ptr.cast().as_mut(), layout.size()).unwrap_unchecked();
        }
    }
}
