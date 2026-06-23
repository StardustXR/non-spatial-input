use colpetto::event::keyboard::KeyState;
use colpetto::event::pointer::PointerEvent;
use colpetto::event::{AsRawEvent, Event};
use colpetto::{Libinput, sys};
use ipc::{ButtonBlot, Message, send_input_ipc};
use std::sync::mpsc::Receiver;
use xkbcommon::xkb::{self, Context, KEYMAP_FORMAT_TEXT_V1, KeyDirection, Keycode, Keymap, State};

const H_AXIS: sys::libinput_pointer_axis =
	sys::libinput_pointer_axis::LIBINPUT_POINTER_AXIS_SCROLL_HORIZONTAL;
const V_AXIS: sys::libinput_pointer_axis =
	sys::libinput_pointer_axis::LIBINPUT_POINTER_AXIS_SCROLL_VERTICAL;

pub enum StateChange {
	Enable,
	Disable,
	Stop,
}

/// Get the raw pointer event from a colpetto event.
///
/// # Safety
/// The event must be a pointer event.
unsafe fn raw_pointer(event: &impl AsRawEvent) -> *mut sys::libinput_event_pointer {
	unsafe { sys::libinput_event_get_pointer_event(event.as_raw_event()) }
}

fn scroll_value(raw: *mut sys::libinput_event_pointer, axis: sys::libinput_pointer_axis) -> f32 {
	if unsafe { sys::libinput_event_pointer_has_axis(raw, axis) } != 0 {
		(unsafe { sys::libinput_event_pointer_get_scroll_value(raw, axis) }) as f32
	} else {
		0.0
	}
}

fn scroll_value_v120(
	raw: *mut sys::libinput_event_pointer,
	axis: sys::libinput_pointer_axis,
) -> f32 {
	if unsafe { sys::libinput_event_pointer_has_axis(raw, axis) } != 0 {
		(unsafe { sys::libinput_event_pointer_get_scroll_value_v120(raw, axis) }) as f32 / 120.0
	} else {
		0.0
	}
}

pub fn input_loop(mut enabled: bool, state_rx: Receiver<StateChange>) {
	let mut libinput = Libinput::new(
		|path, flags| {
			let fd = unsafe { libc::open(path.as_ptr(), flags) };
			if fd >= 0 {
				Ok(fd)
			} else {
				Err(std::io::Error::last_os_error()
					.raw_os_error()
					.unwrap_or(libc::ENOENT))
			}
		},
		|fd| unsafe {
			libc::close(fd);
		},
	)
	.expect("Failed to create libinput context");
	libinput
		.udev_assign_seat(c"seat0")
		.expect("Failed to assign seat");

	let keymap = Keymap::new_from_names(&Context::new(0), "evdev", "", "", "", None, 0).unwrap();
	let mut xkb_state = State::new(&keymap);
	let keymap_str = keymap.get_as_string(KEYMAP_FORMAT_TEXT_V1);
	send_input_ipc(Message::Keymap(keymap_str));

	let mut mouse_blot = Some(ButtonBlot::default());
	let mut key_blot = Some(ButtonBlot::default());

	let mut pollfd = libc::pollfd {
		fd: libinput.get_fd(),
		events: libc::POLLIN,
		revents: 0,
	};

	while unsafe { libc::poll(&mut pollfd, 1, -1) } > 0 {
		if let Ok(state_change) = state_rx.try_recv() {
			match state_change {
				StateChange::Enable => enabled = true,
				StateChange::Disable => enabled = false,
				StateChange::Stop => return,
			}
		}
		libinput.dispatch().unwrap();
		if enabled {
			while let Some(event) = libinput.get_event() {
				let message = match event {
					Event::Keyboard(colpetto::event::keyboard::KeyboardEvent::Key(ref k)) => {
						let pressed = k.key_state() == KeyState::Pressed;
						key_blot.as_mut().unwrap().key_update(k.key(), pressed);
						xkb_state.update_key(
							Keycode::new(k.key() + 8),
							if pressed {
								KeyDirection::Down
							} else {
								KeyDirection::Up
							},
						);

						Message::Key {
							keycode: k.key(),
							pressed,
							mod_pressed: xkb_state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
							mod_latched: xkb_state.serialize_mods(xkb::STATE_MODS_LATCHED),
							mod_locked: xkb_state.serialize_mods(xkb::STATE_MODS_LOCKED),
							layout_group: xkb_state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
						}
					}
					Event::Pointer(PointerEvent::Button(ref p)) => {
						let raw = unsafe { raw_pointer(p) };
						let button = unsafe { sys::libinput_event_pointer_get_button(raw) };
						let state = unsafe { sys::libinput_event_pointer_get_button_state(raw) };
						let pressed =
							state == sys::libinput_button_state::LIBINPUT_BUTTON_STATE_PRESSED;
						mouse_blot.as_mut().unwrap().key_update(button, pressed);
						Message::MouseButton { button, pressed }
					}
					Event::Pointer(PointerEvent::Motion(ref m)) => {
						let raw = unsafe { raw_pointer(m) };
						let dx = (unsafe { sys::libinput_event_pointer_get_dx(raw) }) as f32;
						let dy = (unsafe { sys::libinput_event_pointer_get_dy(raw) }) as f32;
						Message::MouseMove([dx, -dy].into())
					}
					Event::Pointer(PointerEvent::ScrollContinuous(ref s)) => {
						let raw = unsafe { raw_pointer(s) };
						Message::MouseAxisContinuous(
							[scroll_value(raw, H_AXIS), -scroll_value(raw, V_AXIS)].into(),
							ipc::ScrollSource::Continuous,
						)
					}
					Event::Pointer(PointerEvent::ScrollWheel(ref s)) => {
						let raw = unsafe { raw_pointer(s) };
						Message::MouseAxisDiscrete(
							[
								scroll_value_v120(raw, H_AXIS),
								-scroll_value_v120(raw, V_AXIS),
							]
							.into(),
							ipc::ScrollSource::Wheel,
						)
					}
					Event::Pointer(PointerEvent::ScrollFinger(ref s)) => {
						let raw = unsafe { raw_pointer(s) };
						Message::MouseAxisContinuous(
							[scroll_value(raw, H_AXIS), -scroll_value(raw, V_AXIS)].into(),
							ipc::ScrollSource::Finger,
						)
					}

					_ => continue,
				};
				send_input_ipc(message);
			}
		}
	}
}
