#![feature(
    future_join,
    int_from_ascii,
    map_try_insert,
    slice_split_once,
    str_as_str
)]

use compio::{driver::ProactorBuilder, runtime::Runtime};
use smallstr::SmallString;

use crate::{consumer::Consumer, modules::hyprland};

async fn run() {
    let (wayland_server, wayland) = wayland::new();

    let (hyprland_server, hyprland) = hyprland::new().await.split();
    let init = hyprland.as_ref().unwrap().context.controller().await;
    let hyprland_serve = async {
        match hyprland_server {
            Some(server) => server.run(init).await,
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
#[macro_use]
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
