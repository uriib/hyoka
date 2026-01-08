#![feature(map_try_insert, future_join)]

use compio::{driver::ProactorBuilder, runtime::Runtime};

use crate::consumer::Consumer;

fn main() {
    let rt = Runtime::builder()
        .with_proactor({
            let mut builder = ProactorBuilder::new();
            builder.capacity(1);
            builder.driver_type(compio::driver::DriverType::IoUring);
            builder
        })
        .build()
        .unwrap();
    let (wayland_server, wayland) = wayland::new();

    let consumer = Consumer {
        wayland,
        display: wayland_server.display(),
    };

    rt.block_on(async {
        std::future::join!(consumer.run(), wayland_server.run()).await;
    });
}

mod consumer;
mod sync;
#[macro_use]
mod wayland;
