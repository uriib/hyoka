#![feature(map_try_insert)]
use tokio::runtime;

use crate::consumer::Consumer;

fn main() {
    let rt = runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .unwrap();
    let (wayland_server, wayland) = wayland::new();

    let consumer = Consumer {
        wayland,
        display: wayland_server.display(),
    };

    rt.block_on(async {
        tokio::join!(consumer.run(), wayland_server.run());
    });
}

mod consumer;
#[macro_use]
mod wayland;
