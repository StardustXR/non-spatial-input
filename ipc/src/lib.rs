#![allow(unused)]

use flexbuffers::FlexbufferSerializer;
use mint::Vector2;
use serde::{Deserialize, Serialize};
use std::{
	fmt::Display,
	io::{ErrorKind, Read, Write},
	vec,
};
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum Message {
	Keymap(String),
	Key { keycode: u32, pressed: bool },
	MouseMove(Vector2<f32>),
	MouseButton { button: u32, pressed: bool },
	MouseAxisContinuous(Vector2<f32>),
	MouseAxisDiscrete(Vector2<f32>),
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
			Message::Disconnect => format!("Disconnect request"),
		})
	}
}

pub fn send_input_ipc(message: Message) {
	let buf = flexbuffers::to_vec(message).unwrap();
	let mut stdout = std::io::stdout().lock();
	stdout.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
	stdout.write_all(&buf).unwrap();
	stdout.flush();
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
		pressed: false,
	});
	round_trip(Message::MouseAxisDiscrete([168.9, -21.7].into()));
	round_trip(Message::MouseAxisContinuous([1723.2, -482.4].into()));
}

fn round_trip(message: Message) {
	let serialized = flexbuffers::to_vec(message.clone()).unwrap();
	let deserialized: Message = flexbuffers::from_slice(&serialized).unwrap();
	assert_eq!(deserialized, message)
}
