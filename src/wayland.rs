use std::{
    borrow::Borrow,
    ffi::{c_char, c_void},
    mem,
    os::fd::BorrowedFd,
    pin::Pin,
    ptr::{self, NonNull},
};

use compio::net::PollFd;
use iced::{Point, mouse};

use crate::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

#[allow(
    dead_code,
    non_camel_case_types,
    non_upper_case_globals,
    unsafe_op_in_unsafe_fn
)]
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
            let fd = BorrowedFd::borrow_raw(fd);
            let fd = PollFd::new(fd).unwrap();
            loop {
                // std::thread::sleep(std::time::Duration::from_secs(1));
                if ffi::wl_display_prepare_read(display) != 0 {
                    panic!("queue not empty");
                }
                fd.read_ready().await.unwrap();
                if ffi::wl_display_read_events(display) == -1 {
                    panic!("error read events");
                }

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

const WM_BASE_LISTENER: ffi::xdg_wm_base_listener = ffi::xdg_wm_base_listener {
    ping: Some({
        extern "C" fn ping(_: *mut c_void, wm_base: *mut ffi::xdg_wm_base, serial: u32) {
            unsafe { ffi::xdg_wm_base_pong(wm_base, serial) }
        }
        ping
    }),
};

pub type Fixed = ffi::wl_fixed_t;

impl Fixed {
    fn as_f32(self) -> f32 {
        (self.0 as f32) / 256.0
    }
    #[allow(dead_code)]
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
            // Sometimes surface is null. Why can surface be null ? idk. It's not nullable in protocol
            // Maybe a bug of Hyprland
            if let Some(surface) = NonNull::new(surface) {
                notifier.send(Event::Enter { surface, serial }).unwrap();
                notifier
                    .send(Event::Mouse(mouse::Event::CursorMoved {
                        position: Point::new(x.into(), y.into()),
                    }))
                    .unwrap();
            }
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
            // notifier
            //     .send(Event::Mouse(mouse::Event::CursorMoved {
            //         position: Point {
            //             x: f32::INFINITY,
            //             y: f32::INFINITY,
            //         },
            //     }))
            //     .unwrap();
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

pub const XDG_SURFACE_LISTENER: ffi::xdg_surface_listener = ffi::xdg_surface_listener {
    configure: {
        extern "C" fn configure(_: *mut c_void, surface: *mut ffi::xdg_surface, serial: u32) {
            unsafe { ffi::xdg_surface_ack_configure(surface, serial) }
        }
        Some(configure)
    },
};

pub const XDG_POPUP_LISTENER: ffi::xdg_popup_listener = ffi::xdg_popup_listener {
    configure: {
        extern "C" fn configure(
            data: *mut c_void,
            popup: *mut ffi::xdg_popup,
            _x: i32,
            _y: i32,
            width: i32,
            height: i32,
        ) {
            let notifier = unsafe { &mut *(data as *mut UnboundedSender<Event>) };
            let size = [width as u32, height as u32];
            notifier
                .send(Event::Resize {
                    object: NonNull::new(popup as _).unwrap(),
                    size,
                })
                .unwrap();
        }
        Some(configure)
    },
    popup_done: nop!(),
    repositioned: nop!(),
};

pub const SURFACE_LISTENER: ffi::wl_surface_listener = ffi::wl_surface_listener {
    enter: nop!(),
    leave: nop!(),
    // enter: {
    //     extern "C" fn enter(
    //         data: *mut c_void,
    //         surface: *mut ffi::wl_surface,
    //         output: *mut ffi::wl_output,
    //     ) {
    //         dbg!(output);
    //     }
    //     Some(enter)
    // },
    // leave: {
    //     extern "C" fn leave(
    //         data: *mut c_void,
    //         surface: *mut ffi::wl_surface,
    //         output: *mut ffi::wl_output,
    //     ) {
    //         dbg!(output);
    //     }
    //     Some(leave)
    // },
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
                .send(Event::CallbackDone(NonNull::new(callback).unwrap()))
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
    // let name = std::env::var_os("WAYLAND_DISPLAY")
    //     .unwrap()
    //     .into_c_str()
    //     .unwrap();
    // let name = name.as_c_str();
    let display = NonNull::new(unsafe { ffi::wl_display_connect(ptr::null_mut()) }).unwrap();
    let registry = unsafe { ffi::wl_display_get_registry(display.as_ptr()) };
    let mut globals = GlobalsBuilder::default();
    unsafe { ffi::wl_registry_add_listener(registry, &REGISTRY_LISTENER, &raw mut globals as _) };
    unsafe { ffi::wl_display_roundtrip(display.as_ptr()) };
    let globals = globals.build();
    unsafe { ffi::xdg_wm_base_add_listener(globals.wm_base(), &WM_BASE_LISTENER, ptr::null_mut()) };
    let (notifier, events) = unbounded();
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
    pub cursor_shape_manager: wp_cursor_shape_manager_v1,
    pub layer_shell: zwlr_layer_shell_v1,
    pub seat: wl_seat,
    pub shm: wl_shm,
    pub wm_base: xdg_wm_base,
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
