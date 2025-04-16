use color_eyre::Result;
use ipc::receive_input_async_ipc;
use serde::{Deserialize, Serialize};
use stardust_xr_fusion::{
	client::Client,
	core::{messenger::MessengerError, values::Vector2},
	fields::FieldRefAspect,
	objects::{hmd, interfaces::FieldRefProxy, object_registry::ObjectRegistry, FieldRefProxyExt},
	root::{RootAspect, RootEvent},
	AsyncEventHandle, ClientHandle,
};
use stardust_xr_molecules::keyboard::KeyboardHandlerProxy;
use stardust_xr_molecules::mouse::MouseHandlerProxy;
use std::{io::IsTerminal, sync::Arc};
use tokio::sync::mpsc;
use zbus::Connection;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PointerDatamap {
	select: f32,
	grab: f32,
	scroll_continuous: Vector2<f32>,
	scroll_discrete: Vector2<f32>,
}
impl Default for PointerDatamap {
	fn default() -> Self {
		Self {
			select: 0.0,
			grab: 0.0,
			scroll_continuous: [0.0; 2].into(),
			scroll_discrete: [0.0; 2].into(),
		}
	}
}

#[tokio::main]
async fn main() -> Result<()> {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe manifold or eclipse's output into this e.g. `eclipse | simular`");
	}
	let client = Client::connect().await.expect("Couldn't connect");
	let client_handle = client.handle();
	let async_loop = client.async_event_loop();
	let event_handle = async_loop.get_event_handle();
	let (keyboard_tx, keyboard_rx) = mpsc::unbounded_channel::<KeyboardEvent>();
	let (mouse_tx, mouse_rx) = mpsc::unbounded_channel::<MouseEvent>();

	let keyboard_loop = tokio::task::spawn(spatialize_keyboard_input(
		event_handle.clone(),
		client_handle.clone(),
		keyboard_rx,
	));
	let mouse_loop = tokio::task::spawn(spatialize_mouse_input(
		event_handle,
		client_handle.clone(),
		mouse_rx,
	));
	let input_loop = tokio::task::spawn(input_loop(client_handle.clone(), keyboard_tx, mouse_tx));

	_ = tokio::select! {
		biased;
		_ = tokio::signal::ctrl_c() => Ok(()),
		e = keyboard_loop => match e {
			Ok(v) => Ok(v?),
			err @ Err(_) => err.map(|_|()),
		},
		e = mouse_loop => match e {
			Ok(v) => Ok(v?),
			err @ Err(_) => err.map(|_|()),
		},
		_ = input_loop => Ok(()),
	};
	Ok(())
}

async fn spatialize_mouse_input(
	event_handle: AsyncEventHandle,
	client: Arc<ClientHandle>,
	mut mouse_events: mpsc::UnboundedReceiver<MouseEvent>,
) -> Result<(), MessengerError> {
	let conn = Connection::session().await.unwrap();
	let object_registry = ObjectRegistry::new(&conn).await.unwrap();
	let hmd = hmd(&client).await.unwrap();
	loop {
		event_handle.wait().await;
		if !matches!(
			client.get_root().recv_root_event(),
			Some(RootEvent::Frame { info: _ })
		) {
			continue;
		}
		let mouse_handlers = object_registry.get_objects("org.stardustxr.Mousev1");
		let mut closest_distance = f32::INFINITY;
		let mut closest_handler = None;
		for handler in &mouse_handlers {
			let proxy = handler
				.to_typed_proxy::<FieldRefProxy>(&conn)
				.await
				.unwrap();
			let Some(field_ref) = proxy.import(&client).await else {
				eprintln!("field import was None");
				continue;
			};

			let result = field_ref
				.ray_march(&hmd, [0.0, 0.0, 0.0], [0.0, 0.0, -1.0])
				.await
				.unwrap();

			if result.deepest_point_distance > 0.0
				&& result.min_distance < 0.05
				&& result.deepest_point_distance < closest_distance
			{
				closest_distance = result.deepest_point_distance;
				closest_handler = Some(handler);
			}
		}

		if let Some(handler) = closest_handler {
			let proxy = handler
				.to_typed_proxy::<MouseHandlerProxy>(&conn)
				.await
				.unwrap();
			while let Ok(event) = mouse_events.try_recv() {
				match event {
					MouseEvent::Move { delta } => {
						_ = proxy.motion((delta.x, delta.y)).await;
					}
					MouseEvent::Button { button, pressed } => {
						_ = proxy.button(button, pressed).await;
					}
					MouseEvent::AxisContinuous { a } => {
						_ = proxy.scroll_continuous((a.x, a.y));
					}
					MouseEvent::AxisDiscrete { a } => {
						_ = proxy.scroll_discrete((a.x, a.y));
					}
				}
			}
		}
	}
}

async fn spatialize_keyboard_input(
	event_handle: AsyncEventHandle,
	client: Arc<ClientHandle>,
	mut key_events: mpsc::UnboundedReceiver<KeyboardEvent>,
) -> Result<(), MessengerError> {
	let conn = Connection::session().await.unwrap();
	let object_registry = ObjectRegistry::new(&conn).await.unwrap();
	let hmd = hmd(&client).await.unwrap();
	loop {
		event_handle.wait().await;
		if !matches!(
			client.get_root().recv_root_event(),
			Some(RootEvent::Frame { info: _ })
		) {
			continue;
		}
		let keyboard_handlers = object_registry.get_objects("org.stardustxr.XKBv1");
		let mut closest_distance = f32::INFINITY;
		let mut closest_handler = None;
		for handler in &keyboard_handlers {
			let proxy = handler
				.to_typed_proxy::<FieldRefProxy>(&conn)
				.await
				.unwrap();
			let Some(field_ref) = proxy.import(&client).await else {
				eprintln!("field import was None");
				continue;
			};

			let result = field_ref
				.ray_march(&hmd, [0.0, 0.0, 0.0], [0.0, 0.0, -1.0])
				.await
				.unwrap();

			if result.deepest_point_distance > 0.0
				&& result.min_distance < 0.05
				&& result.deepest_point_distance < closest_distance
			{
				closest_distance = result.deepest_point_distance;
				closest_handler = Some(handler);
			}
		}

		if let Some(handler) = closest_handler {
			let proxy = handler
				.to_typed_proxy::<KeyboardHandlerProxy>(&conn)
				.await
				.unwrap();
			while let Ok(event) = key_events.try_recv() {
				match event {
					KeyboardEvent::KeyMap(keymap_id) => {
						_ = proxy.keymap(keymap_id).await;
					}
					KeyboardEvent::Key { key, pressed, map } => {
						_ = proxy.keymap(map).await;
						_ = proxy.key_state(key, pressed).await;
					}
				}
			}
		}
	}
}

enum KeyboardEvent {
	Key { map: u64, key: u32, pressed: bool },
	KeyMap(u64),
}

enum MouseEvent {
	Move { delta: Vector2<f32> },
	Button { button: u32, pressed: bool },
	AxisContinuous { a: Vector2<f32> },
	AxisDiscrete { a: Vector2<f32> },
}

async fn input_loop(
	client: Arc<ClientHandle>,
	key_changed_event: mpsc::UnboundedSender<KeyboardEvent>,
	mouse_changed_event: mpsc::UnboundedSender<MouseEvent>,
) {
	let mut keymap = None;

	while let Ok(message) = receive_input_async_ipc().await {
		match message {
			ipc::Message::Keymap(map) => {
				let Ok(future) = client.register_xkb_keymap(map) else {
					continue;
				};
				let Ok(new_keymap_id) = future.await else {
					continue;
				};
				_ = key_changed_event.send(KeyboardEvent::KeyMap(new_keymap_id));
				keymap = Some(new_keymap_id);
			}
			ipc::Message::Key { keycode, pressed } => {
				let Some(map) = keymap else {
					continue;
				};
				_ = key_changed_event.send(KeyboardEvent::Key {
					key: keycode,
					pressed,
					map,
				});
			}
			ipc::Message::MouseMove(delta) => {
				_ = mouse_changed_event.send(MouseEvent::Move { delta });
			}
			ipc::Message::MouseButton { button, pressed } => {
				_ = mouse_changed_event.send(MouseEvent::Button { button, pressed });
			}
			ipc::Message::MouseAxisContinuous(scroll) => {
				_ = mouse_changed_event.send(MouseEvent::AxisContinuous { a: scroll });
			}
			ipc::Message::MouseAxisDiscrete(scroll) => {
				_ = mouse_changed_event.send(MouseEvent::AxisDiscrete { a: scroll });
			}
			ipc::Message::ResetInput => (),
			ipc::Message::Disconnect => break,
		};
	}
}
