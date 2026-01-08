use std::ptr::{self, NonNull};

use iced::{Font, Pixels, Theme};
use iced_tiny_skia::Renderer;
use rustc_hash::FxHashMap;

use crate::{
    consumer::{
        programs::Bar,
        window::{Layer, Role, Window, WindowManager},
    },
    sync::mpsc::channel,
    wayland::{self, Callback},
};

#[derive(Debug)]
enum Event {
    Wayland(wayland::Event),
}

pub struct Consumer {
    pub wayland: wayland::Client,
    pub display: NonNull<wayland::ffi::wl_display>,
}

impl Consumer {
    pub fn run(self) -> impl Future<Output = ()> {
        let Self {
            wayland:
                wayland::Client {
                    proxy: wayland_proxy,
                    events: mut wayland_events,
                },
            display,
        } = self;
        let (mut sender, mut receiver) = channel(1);

        let wayland = async move {
            loop {
                sender
                    .send(Event::Wayland(wayland_events.receive().await.unwrap()))
                    .await
                    .unwrap();
            }
        };

        let mut runner = Runner::new(wayland_proxy, display);

        let consumer = async move {
            loop {
                match receiver.receive().await.unwrap() {
                    Event::Wayland(event) => runner.dispatch_wayland(event),
                }
            }
        };

        async move {
            std::future::join!(wayland, consumer).await;
        }
    }
}

type Callbacks = FxHashMap<wayland::Callback, Box<dyn FnOnce(&mut Runner)>>;

struct Runner {
    wayland: wayland::Proxy,
    display: NonNull<wayland::ffi::wl_display>,
    window_manager: WindowManager,

    pointer: NonNull<wayland::ffi::wl_pointer>,
    cursor_shape_device: NonNull<wayland::ffi::wp_cursor_shape_device_v1>,

    renderer: Renderer,
    theme: Theme,
    callbacks: Callbacks,

    bar: Window,
}

impl Runner {
    fn new(mut wayland: wayland::Proxy, display: NonNull<wayland::ffi::wl_display>) -> Self {
        let mut window_manager = WindowManager::default();
        let surface =
            unsafe { wayland::ffi::wl_compositor_create_surface(wayland.globals.compositer()) };
        unsafe {
            wayland::ffi::wl_surface_add_listener(
                surface,
                &wayland::SURFACE_LISTENER,
                &raw mut *wayland.notifier as _,
            );
        };
        let layer_surface = unsafe {
            wayland::ffi::zwlr_layer_shell_v1_get_layer_surface(
                wayland.globals.layer_shell(),
                surface,
                ptr::null_mut(),
                wayland::ffi::ZWLR_LAYER_SHELL_V1_LAYER_TOP,
                c"hyoka".as_ptr(),
            )
        };
        unsafe {
            wayland::ffi::zwlr_layer_surface_v1_add_listener(
                layer_surface,
                &wayland::LAYER_SURFACE_LISTENER,
                &raw mut *wayland.notifier as _,
            );
            wayland::ffi::zwlr_layer_surface_v1_set_size(layer_surface, 0, 35);
            wayland::ffi::zwlr_layer_surface_v1_set_anchor(
                layer_surface,
                wayland::ffi::ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP
                    | wayland::ffi::ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT
                    | wayland::ffi::ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT,
            );
            wayland::ffi::zwlr_layer_surface_v1_set_exclusive_zone(layer_surface, -1);
            wayland::ffi::wl_surface_commit(surface);

            wayland::ffi::wl_display_flush(display.as_ptr());
        }
        let bar = window_manager.create_window(
            NonNull::new(surface).unwrap(),
            Role::Layer(Layer {
                layer_surface: NonNull::new(layer_surface).unwrap(),
            }),
            Box::new(Bar {}),
        );

        let pointer = unsafe { wayland::ffi::wl_seat_get_pointer(wayland.globals.seat()) };
        unsafe {
            wayland::ffi::wl_pointer_add_listener(
                pointer,
                &wayland::POINTER_LISTENERL,
                &raw mut *wayland.notifier as _,
            )
        };
        let cursor_shape_device = unsafe {
            wayland::ffi::wp_cursor_shape_manager_v1_get_pointer(
                wayland.globals.cursor_shape_manager(),
                pointer,
            )
        };
        Self {
            wayland,
            display,
            bar: bar.clone(),
            window_manager,
            renderer: Renderer::new(Font::default(), Pixels(12.0)),
            theme: Theme::Dark,
            pointer: NonNull::new(pointer).unwrap(),
            cursor_shape_device: NonNull::new(cursor_shape_device).unwrap(),
            callbacks: Default::default(),
        }
    }
    fn dispatch_wayland(&mut self, event: wayland::Event) {
        match event {
            wayland::Event::Resize { object, size } => {
                self.window_manager
                    .find_by_object(object)
                    .clone()
                    .resize(size, self);
            }
            wayland::Event::Rescale { surface, factor } => {
                self.window_manager
                    .find_by_object(surface)
                    .clone()
                    .rescale(factor, self);
            }
            wayland::Event::Enter { surface, serial } => {
                let win = self.window_manager.find_by_object(surface);
                win.enter(serial);
                self.window_manager.focused = Some(win.clone());
            }
            wayland::Event::Mouse(event) => {
                let window = self.window_manager.focused.as_ref().unwrap();
                window.clone().mouse(event, self);
                match event {
                    iced::mouse::Event::CursorLeft => {
                        self.window_manager.focused.take();
                    }
                    _ => (),
                }
            }
            wayland::Event::CallbackDone(cb) => self.callbacks.remove(&cb).unwrap()(self),
        }
    }
    fn register_callback(&mut self, cb: Callback, f: impl FnOnce(&mut Self) + 'static) {
        self.callbacks
            .try_insert(cb, Box::new(f))
            .map_err(|e| e.entry.remove_entry().0)
            .unwrap();
    }
}

mod programs;
mod window;
