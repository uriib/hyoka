#![feature(
    allocator_api,
    async_iterator,
    future_join,
    gen_blocks,
    int_from_ascii,
    map_try_insert,
    slice_split_once,
    str_as_str
)]

use std::{async_iter::AsyncIterator, future, pin::Pin};

use compio::{driver::ProactorBuilder, runtime::Runtime};
use smallstr::SmallString;

use crate::{consumer::Consumer, modules::hyprland};

async fn run() {
    let (wayland_server, wayland) = wayland::new();

    let (hyprland_server, hyprland) = hyprland::new().await.split();
    let init = if let Some(x) = hyprland.as_ref() {
        Some(x.context.controller().await)
    } else {
        None
    };
    let hyprland_serve = async {
        match hyprland_server {
            Some(server) => server.run(init.unwrap()).await,
            None => {}
        }
    };

    let consumer = Consumer {
        wayland,
        display: wayland_server.display(),
        hyprland,
    };

    std::future::join!(consumer.run(), wayland_server.run(), hyprland_serve).await;
}

fn main() {
    let rt = Runtime::builder()
        .with_proactor({
            let mut builder = ProactorBuilder::new();
            builder.capacity(16);
            builder.driver_type(compio::driver::DriverType::IoUring);
            builder
        })
        .build()
        .unwrap();
    rt.block_on(run());
}

mod consumer;
mod mapping;
mod modules;
mod sync;
mod wayland;

pub type TinyString = SmallString<[u8; 16]>;

trait Split {
    type R;
    fn split(self) -> Self::R;
}

impl<X, Y> Split for Option<(X, Y)> {
    type R = (Option<X>, Option<Y>);

    fn split(self) -> Self::R {
        match self {
            Some((x, y)) => (Some(x), Some(y)),
            None => (None, None),
        }
    }
}

trait AsyncIteratorExt: AsyncIterator {
    async fn next(self: Pin<&mut Self>) -> Option<Self::Item>;
}

impl<T: AsyncIterator> AsyncIteratorExt for T {
    async fn next(mut self: Pin<&mut Self>) -> Option<Self::Item> {
        future::poll_fn(|ctx| self.as_mut().poll_next(ctx)).await
    }
}
