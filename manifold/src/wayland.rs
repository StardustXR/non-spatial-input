use std::{
	io::Read,
	os::{
		fd::{AsRawFd, FromRawFd},
		unix::fs::FileExt,
	},
};
use wayland_client::protocol::wl_keyboard::{Event as WlKeyboardEvent, KeymapFormat, WlKeyboard};
use wayland_client::Dispatch;
use wayland_client::{
	globals::GlobalListContents,
	protocol::wl_seat::{self, WlSeat},
	WEnum,
};
use wayland_client::{protocol::wl_registry, Connection, QueueHandle};

pub struct WlHandler {
	pub keymap: Option<Vec<u8>>,
}

// Implementation from https://github.com/wez/wezterm
impl Dispatch<WlKeyboard, ()> for WlHandler {
	fn event(
		state: &mut WlHandler,
		_keyboard: &WlKeyboard,
		event: <WlKeyboard as wayland_client::Proxy>::Event,
		_data: &(),
		_conn: &wayland_client::Connection,
		_qhandle: &wayland_client::QueueHandle<WlHandler>,
	) {
		if let WlKeyboardEvent::Keymap { format, fd, size } = &event {
			let mut file = unsafe { std::fs::File::from_raw_fd(fd.as_raw_fd()) };
			if let KeymapFormat::XkbV1 = format.into_result().unwrap() {
				let mut data = vec![0u8; *size as usize];
				// If we weren't passed a pipe, be sure to explicitly
				// read from the start of the file
				match file.read_exact_at(&mut data, 0) {
					Ok(_) => {}
					Err(err) => {
						// ideally: we check for:
						// err.kind() == std::io::ErrorKind::NotSeekable
						// but that is not yet in stable rust
						if err.raw_os_error() == Some(libc::ESPIPE) {
							// It's a pipe, which cannot be seeked, so we
							// just try reading from the current pipe position
							file.read_exact(&mut data)
								.expect("read from Keymap fd/pipe");
						} else {
							panic!("{1}: {:?}", err, "read_exact_at from Keymap fd");
						}
					}
				}
				// Dance around CString panicing on the NUL terminator
				// in the xkbcommon crate
				while let Some(0) = data.last() {
					data.pop();
				}
				state.keymap = Some(data);
			}
			Box::leak(Box::new(file));
		}
	}
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WlHandler {
	fn event(
		_state: &mut Self,
		_: &wl_registry::WlRegistry,
		_: wl_registry::Event,
		_: &GlobalListContents,
		_: &Connection,
		_: &QueueHandle<WlHandler>,
	) {
	}
}

impl Dispatch<WlSeat, ()> for WlHandler {
	fn event(
		_: &mut Self,
		seat: &WlSeat,
		event: <WlSeat as wayland_client::Proxy>::Event,
		_: &(),
		_: &Connection,
		qhandle: &QueueHandle<Self>,
	) {
		if let wl_seat::Event::Capabilities {
			capabilities: WEnum::Value(capabilities),
		} = event
		{
			if capabilities.contains(wl_seat::Capability::Keyboard) {
				seat.get_keyboard(qhandle, ());
			}
		}
	}
}
