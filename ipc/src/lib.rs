#![allow(unused)]

use flexbuffers::FlexbufferSerializer;
use mint::Vector2;
use serde::{Deserialize, Serialize};
use std::{io::Write, vec};
use tokio::io::AsyncReadExt;

#[derive(Debug, Serialize, Deserialize)]
// #[serde(tag = "type")]
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

pub fn send_input_ipc(message: Message) {
	let buf = flexbuffers::to_vec(message).unwrap();
	let mut stdout = std::io::stdout().lock();
	stdout.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
	stdout.write_all(&buf).unwrap();
}

pub async fn receive_input_async_ipc() -> std::io::Result<Message> {
	let mut stdin = tokio::io::stdin();
	let length = stdin.read_u32().await?;
	let mut buf = vec::from_elem(0_u8, length as usize);
	stdin.read_exact(&mut buf).await?;
	Ok(flexbuffers::from_slice(&buf).unwrap())
}

#[tokio::test]
async fn test_loop() {
	let message = Message::Key {
		keycode: 25,
		pressed: true,
	};
}
