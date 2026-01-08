use std::{
    os::fd::AsFd,
    ptr::{self, NonNull},
    slice,
};

use rustix::{
    io,
    mm::{MapFlags, ProtFlags},
};

pub trait Size: Copy {
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
