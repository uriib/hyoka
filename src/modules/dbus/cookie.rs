use std::{
    cell::{Cell, RefCell},
    pin::{self, Pin},
    rc::Rc,
    task::{Context, Poll, Waker},
    time::Duration,
};

use dbus::Serial;
use rustc_hash::FxHashMap;

#[derive(Default, Clone)]
pub struct Cookie(Rc<RefCell<FxHashMap<Serial, Rc<Shared>>>>);

impl Cookie {
    pub fn wait(&self, serial: Serial, timeout: Duration) -> Notifier {
        let shared = Rc::new(Shared::new());
        self.0.borrow_mut().insert(serial.clone(), shared.clone());
        Notifier {
            inner: NotifierInner {
                cookie: self.clone(),
                serial,
                shared,
            },
            timeout,
        }
    }
    pub fn notify(&self, serial: Serial, value: super::Return) {
        if let Some(inner) = self.0.borrow_mut().remove(&serial) {
            match inner.0.take() {
                Inner::Waker(waker) => {
                    inner.0.set(Inner::Value(value));
                    waker.wake();
                }
                Inner::Init => {
                    inner.0.set(Inner::Value(value));
                }
                _ => {}
            }
        }
    }
    pub fn cancel(&self, serial: Serial) {
        if let Some(inner) = self.0.borrow_mut().remove(&serial) {
            inner.close();
        }
    }
    pub fn cancel_all(&self) {
        for (_, inner) in self.0.borrow_mut().drain() {
            inner.close();
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

#[derive(Default, Debug)]
enum Inner {
    #[default]
    Init,
    Waker(Waker),
    Value(super::Return),
    Closed,
}

#[derive(Default)]
struct Shared(Cell<Inner>);

impl Shared {
    const fn new() -> Self {
        Self(Cell::new(Inner::Init))
    }
    fn close(&self) {
        match self.0.take() {
            Inner::Waker(waker) => {
                self.0.set(Inner::Closed);
                waker.wake();
            }
            Inner::Init => {
                self.0.set(Inner::Closed);
            }
            _ => {}
        }
    }
}

#[must_use]
pub struct NotifierInner {
    cookie: Cookie,
    serial: Serial,
    shared: Rc<Shared>,
}

impl Drop for NotifierInner {
    fn drop(&mut self) {
        self.cookie.cancel(self.serial.clone());
    }
}

impl Future for NotifierInner {
    type Output = super::Return;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.shared.0.take() {
            Inner::Value(v) => Poll::Ready(v),
            Inner::Closed => Poll::Ready(Err(super::Error::Elapsed)),
            _ => {
                self.shared.0.set(Inner::Waker(cx.waker().clone()));
                Poll::Pending
            }
        }
    }
}

impl NotifierInner {
    pub async fn timeout(&mut self, duration: Duration) -> super::Return {
        compio::time::timeout(duration, self)
            .await
            .map_err(|_| super::Error::Elapsed)
            .flatten()
    }
}

pub struct Notifier {
    inner: NotifierInner,
    timeout: Duration,
}

impl Future for Notifier {
    type Output = <NotifierInner as Future>::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let duration = self.timeout;
        pin::pin!(self.inner.timeout(duration)).poll(cx)
    }
}
