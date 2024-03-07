use input::event::keyboard::KeyboardEventTrait;
use input::event::pointer::{Axis, PointerScrollEvent};
use input::event::tablet_pad::{ButtonState, KeyState};
use input::event::PointerEvent;
use input::{Libinput, LibinputInterface};
use ipc::{send_input_ipc, Message};
use libc::{O_RDONLY, O_RDWR, O_WRONLY};
use nix::poll::{poll, PollFd, PollFlags};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::{fs::OpenOptionsExt, io::OwnedFd};
use std::path::Path;
use std::sync::mpsc::Receiver;
use xkbcommon::xkb::{Context, Keymap, KEYMAP_FORMAT_TEXT_V1};

pub enum StateChange {
	Enable,
	Disable,
	Stop,
}

struct Interface;
impl LibinputInterface for Interface {
	fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
		OpenOptions::new()
			.custom_flags(flags)
			.read((flags & O_RDONLY != 0) | (flags & O_RDWR != 0))
			.write((flags & O_WRONLY != 0) | (flags & O_RDWR != 0))
			.open(path)
			.map(|file| file.into())
			.map_err(|err| err.raw_os_error().unwrap())
	}
	fn close_restricted(&mut self, fd: OwnedFd) {
		drop(File::from(fd));
	}
}
pub fn input_loop(mut enabled: bool, state_rx: Receiver<StateChange>) {
	let mut input = Libinput::new_with_udev(Interface);
	input.udev_assign_seat("seat0").unwrap();
	let pollfd = PollFd::new(input.as_raw_fd(), PollFlags::POLLIN);

	let keymap = Keymap::new_from_names(&Context::new(0), "evdev", "", "", "", None, 0)
		.unwrap()
		.get_as_string(KEYMAP_FORMAT_TEXT_V1);
	send_input_ipc(Message::Keymap(keymap));
	while poll(&mut [pollfd], -1).is_ok() {
		if let Ok(state_change) = state_rx.try_recv() {
			match state_change {
				StateChange::Enable => enabled = true,
				StateChange::Disable => enabled = false,
				StateChange::Stop => return,
			}
		}
		input.dispatch().unwrap();
		if enabled {
			handle_inputs(&mut input);
		}
	}
}

fn handle_inputs(events: &mut Libinput) {
	for event in events {
		send_input_ipc(match event {
			input::Event::Keyboard(input::event::KeyboardEvent::Key(k)) => Message::Key {
				keycode: k.key(),
				pressed: k.key_state() == KeyState::Pressed,
			},
			input::Event::Pointer(PointerEvent::Button(p)) => Message::MouseButton {
				button: p.button(),
				pressed: p.button_state() == ButtonState::Pressed,
			},
			input::Event::Pointer(PointerEvent::Motion(m)) => {
				Message::MouseMove([m.dx() as f32, m.dy() as f32].into())
			}
			input::Event::Pointer(PointerEvent::ScrollContinuous(s)) => {
				Message::MouseAxisContinuous(
					[
						s.scroll_value(Axis::Horizontal) as f32,
						s.scroll_value(Axis::Vertical) as f32,
					]
					.into(),
				)
			}
			input::Event::Pointer(PointerEvent::ScrollWheel(s)) => Message::MouseAxisContinuous(
				[
					s.scroll_value_v120(Axis::Horizontal) as f32 / 120.0,
					s.scroll_value_v120(Axis::Vertical) as f32 / 120.0,
				]
				.into(),
			),
			_ => continue,
		})
	}
}
