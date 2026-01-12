use std::{
    os::fd::AsFd,
    ptr::{self, NonNull},
    slice,
};

use bytes::Bytes;
use compio::buf::{IoBuf, IoBufMut, SetBufInit};
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

unsafe impl<S: Size> IoBufMut for Mapping<S> {
    fn as_buf_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}

unsafe impl<S: Size> IoBuf for Mapping<S> {
    fn as_buf_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    // use it to read only once
    fn buf_len(&self) -> usize {
        0
    }

    fn buf_capacity(&self) -> usize {
        self.len.size()
    }
}

impl<S: Size> SetBufInit for Mapping<S> {
    unsafe fn set_buf_init(&mut self, _len: usize) {}
}

// struct Buffer<S: Size> {
//     mapping: Mapping<S>,
//     len: usize,
// }
//
// impl<S: Size> Buffer<S> {
//     fn new(mapping: Mapping<S>) -> Self {
//         Self { mapping, len: 0 }
//     }
// }
//
// unsafe impl<S: Size> IoBufMut for Buffer<S> {
//     fn as_buf_mut_ptr(&mut self) -> *mut u8 {
//         self.mapping.ptr.as_ptr()
//     }
// }
//
// unsafe impl<S: Size> IoBuf for Buffer<S> {
//     fn as_buf_ptr(&self) -> *const u8 {
//         self.mapping.ptr.as_ptr()
//     }
//
//     // use it to read only once
//     fn buf_len(&self) -> usize {
//         0
//     }
//
//     fn buf_capacity(&self) -> usize {
//         self.len.size()
//     }
// }
//
// impl<S: Size> SetBufInit for Buffer<S> {
//     unsafe fn set_buf_init(&mut self, _len: usize) {}
// }

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
