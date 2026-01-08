use std::{
    borrow::Borrow,
    ffi::{c_char, c_void},
    mem,
    pin::Pin,
    ptr::{NonNull, null},
};

use iced::{Point, mouse};
use tokio::{
    io::unix::AsyncFd,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
};

#[allow(non_camel_case_types, non_upper_case_globals, unused)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

#[derive(Debug)]
pub enum Event {
    Resize {
        object: NonNull<c_void>,
        size: [u32; 2],
    },
    Rescale {
        surface: NonNull<ffi::wl_surface>,
        factor: u32,
    },
    Enter {
        surface: NonNull<ffi::wl_surface>,
        serial: u32,
    },
    Mouse(mouse::Event),
    CallbackDone(NonNull<ffi::wl_callback>),
}

pub struct Server {
    display: NonNull<ffi::wl_display>,
}

pub struct Proxy {
    pub globals: Globals,
    pub notifier: Pin<Box<UnboundedSender<Event>>>,
}

pub struct Client {
    pub proxy: Proxy,
    pub events: UnboundedReceiver<Event>,
}

impl Server {
    pub fn display(&self) -> NonNull<ffi::wl_display> {
        self.display
    }
    pub async fn run(self) {
        let display = self.display.as_ptr();

        unsafe {
            let fd = ffi::wl_display_get_fd(display);
            let fd = AsyncFd::new(fd).unwrap();
            loop {
                // std::thread::sleep(std::time::Duration::from_secs(1));
                if ffi::wl_display_prepare_read(display) != 0 {
                    panic!("queue not empty");
                }
                let mut ready = fd.readable().await.unwrap();
                if ffi::wl_display_read_events(display) == -1 {
                    panic!("error read events");
                }
                ready.clear_ready();

                if ffi::wl_display_dispatch_pending(display) == -1 {
                    panic!("error dispatch events");
                }
            }
        }
    }
}

pub extern "C" fn nop() {}

#[macro_export]
macro_rules! nop {
    () => {
        unsafe { mem::transmute(crate::wayland::nop as *const ()) }
    };
}

const REGISTRY_LISTENER: ffi::wl_registry_listener = ffi::wl_registry_listener {
    global: Some({
        extern "C" fn global(
            data: *mut c_void,
            registry: *mut ffi::wl_registry,
            name: u32,
            interface: *const i8,
            version: u32,
        ) {
            let globals: &mut GlobalsBuilder = unsafe { mem::transmute(data) };
            globals.bind(registry, name, interface, version);
        }
        global
    }),
    global_remove: nop!(),
};

pub type Fixed = ffi::wl_fixed_t;

impl Fixed {
    fn as_f32(self) -> f32 {
        (self.0 as f32) / 256.0
    }
    #[allow(unused)]
    fn as_i32(self) -> i32 {
        self.0 / 256
    }
}

impl Into<f32> for Fixed {
    fn into(self) -> f32 {
        self.as_f32()
    }
}

pub const POINTER_LISTENERL: ffi::wl_pointer_listener = ffi::wl_pointer_listener {
    enter: {
        extern "C" fn enter(
            data: *mut c_void,
            _pointer: *mut ffi::wl_pointer,
            serial: u32,
            surface: *mut ffi::wl_surface,
            x: Fixed,
            y: Fixed,
        ) {
            let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
            notifier
                .send(Event::Enter {
                    surface: NonNull::new(surface).unwrap(),
                    serial,
                })
                .unwrap();
            notifier
                .send(Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(x.into(), y.into()),
                }))
                .unwrap();
        }
        Some(enter)
    },
    leave: {
        extern "C" fn leave(
            data: *mut c_void,
            _pointer: *mut ffi::wl_pointer,
            _serial: u32,
            _surface: *mut ffi::wl_surface,
        ) {
            let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
            notifier
                .send(Event::Mouse(mouse::Event::CursorLeft))
                .unwrap();
        }
        Some(leave)
    },
    motion: {
        extern "C" fn motion(
            data: *mut c_void,
            _pointer: *mut ffi::wl_pointer,
            _time: u32,
            x: Fixed,
            y: Fixed,
        ) {
            let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
            notifier
                .send(Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(x.into(), y.into()),
                }))
                .unwrap();
        }
        Some(motion)
    },
    button: {
        extern "C" fn button(
            data: *mut c_void,
            _pointer: *mut ffi::wl_pointer,
            _serial: u32,
            _time: u32,
            button: u32,
            state: u32,
        ) {
            let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
            let button = match button {
                0x110 => mouse::Button::Left,
                0x111 => mouse::Button::Right,
                0x112 => mouse::Button::Middle,
                0x115 => mouse::Button::Forward,
                0x116 => mouse::Button::Back,
                other => mouse::Button::Other(other as _),
            };
            let event = match state {
                ffi::WL_POINTER_BUTTON_STATE_RELEASED => mouse::Event::ButtonReleased(button),
                ffi::WL_POINTER_BUTTON_STATE_PRESSED => mouse::Event::ButtonPressed(button),
                _ => unreachable!(),
            };
            notifier.send(Event::Mouse(event)).unwrap()
        }
        Some(button)
    },
    axis: nop!(),
    frame: nop!(),
    axis_source: nop!(),
    axis_stop: nop!(),
    axis_discrete: nop!(),
    axis_value120: nop!(),
    axis_relative_direction: nop!(),
};

pub const SURFACE_LISTENER: ffi::wl_surface_listener = ffi::wl_surface_listener {
    enter: nop!(),
    leave: nop!(),
    preferred_buffer_scale: {
        extern "C" fn scale(data: *mut c_void, surface: *mut ffi::wl_surface, scale: i32) {
            let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
            notifier
                .send(Event::Rescale {
                    surface: NonNull::new(surface).unwrap(),
                    factor: scale as _,
                })
                .unwrap();
        }
        Some(scale)
    },
    preferred_buffer_transform: nop!(),
};

pub const LAYER_SURFACE_LISTENER: ffi::zwlr_layer_surface_v1_listener =
    ffi::zwlr_layer_surface_v1_listener {
        configure: {
            extern "C" fn configure(
                data: *mut c_void,
                surface: *mut ffi::zwlr_layer_surface_v1,
                serial: u32,
                width: u32,
                height: u32,
            ) {
                unsafe { ffi::zwlr_layer_surface_v1_ack_configure(surface, serial) };
                let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
                notifier
                    .send(Event::Resize {
                        object: NonNull::new(surface as _).unwrap(),
                        size: [width, height],
                    })
                    .unwrap();
            }
            Some(configure)
        },
        closed: nop!(),
    };

pub const CALLBACK_LISTENER: ffi::wl_callback_listener = ffi::wl_callback_listener {
    done: {
        extern "C" fn done(
            data: *mut c_void,
            callback: *mut ffi::wl_callback,
            _callback_data: u32,
        ) {
            let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
            notifier
                .send(Event::CallbackDone(unsafe {
                    NonNull::new_unchecked(callback)
                }))
                .unwrap();
        }
        Some(done)
    },
};

#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Callback(NonNull<ffi::wl_callback>);

impl Callback {
    pub fn from_raw(callback: *mut ffi::wl_callback) -> Self {
        Self(NonNull::new(callback).unwrap())
    }
}

impl Borrow<NonNull<ffi::wl_callback>> for Callback {
    fn borrow(&self) -> &NonNull<ffi::wl_callback> {
        &self.0
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        unsafe {
            ffi::wl_callback_destroy(self.0.as_ptr());
        }
    }
}

pub fn new() -> (Server, Client) {
    let display = NonNull::new(unsafe { ffi::wl_display_connect(null()) }).unwrap();
    let registry = unsafe { ffi::wl_display_get_registry(display.as_ptr()) };
    let mut globals = GlobalsBuilder::default();
    unsafe { ffi::wl_registry_add_listener(registry, &REGISTRY_LISTENER, &raw mut globals as _) };
    unsafe { ffi::wl_display_roundtrip(display.as_ptr()) };
    let globals = globals.build();
    let (notifier, events) = unbounded_channel();
    let notifier = Box::pin(notifier);
    (
        Server { display },
        Client {
            proxy: Proxy { globals, notifier },
            events,
        },
    )
}

macro_rules! use_globals {
    ($($vis:vis $name:ident: $interface:ident),* $(,)?) => {
        #[derive(Default)]
        struct GlobalsBuilder {
            $($name: *mut ffi::$interface),*
        }

        impl GlobalsBuilder {
            fn build(self) -> Globals {
                Globals {
                    $($name: NonNull::new(self.$name).expect(concat!(stringify!($interface), "is not supported")),)*
                }
            }
            fn bind(
                &mut self,
                registry: *mut ffi::wl_registry,
                name: u32,
                interface_name: *const i8,
                version: u32,
            ) {
                $(
                    let interface = unsafe { &concat_idents::concat_idents!(interface = $interface, _interface { ffi::interface }) };
                    if unsafe {
                        cstr_eq(
                            Restrict::from_ptr(interface_name),
                            Restrict::from_ptr(interface.name),
                        )
                    } {
                        self.$name = unsafe { mem::transmute(ffi::wl_registry_bind(registry, name, interface, version)) };
                        return;
                    }
                )*
            }
        }

        pub struct Globals {
            $($vis $name: NonNull<ffi::$interface>),*
        }

        impl Globals {
            $($vis fn $name(&self) -> *mut ffi::$interface {
                self.$name.as_ptr()
            })*
        }
    };
}

use_globals! {
    pub compositer: wl_compositor,
    pub shm: wl_shm,
    pub seat: wl_seat,
    pub layer_shell: zwlr_layer_shell_v1,
    pub cursor_shape_manager: wp_cursor_shape_manager_v1,
}

#[repr(C)]
struct Restrict<T: 'static>(&'static T);
impl<T: 'static> Restrict<T> {
    fn from_ptr(ptr: *const T) -> Self {
        Self(unsafe { mem::transmute(ptr) })
    }
    fn as_ptr(self) -> *const T {
        unsafe { mem::transmute(self) }
    }
}

unsafe fn cstr_eq(x: Restrict<c_char>, y: Restrict<c_char>) -> bool {
    let mut ps = [x, y].map(Restrict::as_ptr);

    loop {
        let [x, y] = ps.map(|p| unsafe { *p });
        match (x == 0, y == 0) {
            (true, true) => return true,
            (false, false) if x == y => {
                ps = ps.map(|p| unsafe { p.add(1) });
                continue;
            }
            _ => return false,
        }
    }
}
