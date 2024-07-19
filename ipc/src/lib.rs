#![allow(unused)]

use flexbuffers::FlexbufferSerializer;
use mint::Vector2;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::{
	collections::HashMap,
	fmt::Display,
	io::{ErrorKind, Read, Write},
	sync::Mutex,
	vec,
};
use tokio::io::AsyncReadExt;

static MOUSE_BLOT: Mutex<Option<ButtonBlot>> = Mutex::new(None);
static KEY_BLOT: Mutex<Option<ButtonBlot>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum Message {
	Keymap(String),
	Key { keycode: u32, pressed: bool },
	MouseMove(Vector2<f32>),
	MouseButton { button: u32, pressed: bool },
	MouseAxisContinuous(Vector2<f32>),
	MouseAxisDiscrete(Vector2<f32>),
	ResetInput,
	Disconnect,
}
impl Display for Message {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&match self {
			Message::Keymap(k) => format!("Updated keymap with length {}", k.len()),
			Message::Key { keycode, pressed } => {
				if *pressed {
					format!("Pressed key {keycode}")
				} else {
					format!("Released key {keycode}")
				}
			}
			Message::MouseMove(delta) => format!("Mouse moved with delta {:?}", *delta),
			Message::MouseButton { button, pressed } => {
				if *pressed {
					format!("Pressed mouse {button}")
				} else {
					format!("Released mouse {button}")
				}
			}
			Message::MouseAxisContinuous(a) => format!("Mouse axis continuous {a:?}"),
			Message::MouseAxisDiscrete(a) => format!("Mouse axis discrete {a:?}"),
			Message::ResetInput => "Reset input".to_string(),
			Message::Disconnect => {
				"Disconnect request".to_string()
			}
		})
	}
}

pub fn send_input_ipc(message: Message) {
	let mut messages = vec![message.clone()];
	match &message {
		Message::MouseButton { button, pressed } => MOUSE_BLOT
			.lock()
			.unwrap()
			.get_or_insert(ButtonBlot::default())
			.key_update(*button, *pressed),
		Message::Key { keycode, pressed } => KEY_BLOT
			.lock()
			.unwrap()
			.get_or_insert(ButtonBlot::default())
			.key_update(*keycode, *pressed),
		Message::ResetInput => {
			// eprintln!("reset input");
			messages.clear();
			if let Some(blot) = MOUSE_BLOT.lock().unwrap().replace(ButtonBlot::default()) {
				for (button, pressed) in blot.cleanup_presses_releases() {
					messages.push(Message::MouseButton { button, pressed });
				}
			}
			if let Some(blot) = KEY_BLOT.lock().unwrap().replace(ButtonBlot::default()) {
				for (keycode, pressed) in blot.cleanup_presses_releases() {
					messages.push(Message::Key { keycode, pressed });
				}
			}
		}
		_ => (),
	}

	let mut stdout = std::io::stdout().lock();
	for message in messages {
		let buf = flexbuffers::to_vec(message).unwrap();
		stdout.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
		stdout.write_all(&buf).unwrap();
		stdout.flush();
	}
}

pub async fn receive_input_async_ipc() -> std::io::Result<Message> {
	tokio::task::spawn_blocking(move || {
		let mut stdin = std::io::stdin().lock();
		let mut length_buf = [0_u8; 4];
		stdin.read_exact(&mut length_buf)?;
		let length = u32::from_be_bytes(length_buf);

		let mut buf = vec::from_elem(0_u8, length as usize);
		stdin.read_exact(&mut buf)?;
		flexbuffers::from_slice(&buf).map_err(|_| ErrorKind::InvalidData.into())
	})
	.await
	.unwrap()
}

#[test]
fn test_loop() {
	round_trip(Message::Disconnect);
	round_trip(Message::Keymap("uwu owo nya".to_string()));
	round_trip(Message::Key {
		keycode: 124,
		pressed: true,
	});
	round_trip(Message::MouseMove([243.5, 162.62].into()));
	round_trip(Message::MouseButton {
		button: 215,
		pressed: true,
	});
	round_trip(Message::MouseAxisDiscrete([168.9, -21.7].into()));
	round_trip(Message::MouseAxisContinuous([1723.2, -482.4].into()));
	round_trip(Message::ResetInput);
}

fn round_trip(message: Message) {
	let serialized = flexbuffers::to_vec(message.clone()).unwrap();
	let deserialized: Message = flexbuffers::from_slice(&serialized).unwrap();
	assert_eq!(deserialized, message)
}

/// Helper struct to clean up the button press/release mess for localized button input (keys, mouse buttons, etc.no
#[derive(Debug, Default)]
pub struct ButtonBlot {
	keys: FxHashMap<u32, i32>,
}
impl ButtonBlot {
	/// Positive keycode for pressed, negative for released.
	pub fn key_math(&mut self, code: i32) {
		let key_math = code.signum();
		if let Some(key) = self.keys.get_mut(&code.unsigned_abs()) {
			*key += key_math;
		} else {
			self.keys.insert(code.unsigned_abs(), key_math);
		}
	}
	pub fn key_pressed(&mut self, code: u32) {
		self.key_math(code as i32)
	}
	pub fn key_released(&mut self, code: u32) {
		self.key_math(-(code as i32))
	}
	pub fn key_update(&mut self, code: u32, pressed: bool) {
		self.key_math(code as i32 * if pressed { 1 } else { -1 })
	}

	/// Have all keys that were pressed been released the proper number of times?
	pub fn is_clean(&self) -> bool {
		self.keys.values().all(|k| *k == 0)
	}

	pub fn cleanup_presses_releases(self) -> impl IntoIterator<Item = (u32, bool)> {
		self.keys
			.into_iter()
			.flat_map(|(k, m)| vec![(k, m.signum() < 0); m.unsigned_abs() as usize])
	}
	pub fn cleanup_key_math(self) -> impl IntoIterator<Item = (u32, i32)> {
		self.keys.into_iter().map(|(k, m)| (k, -m))
	}
}
