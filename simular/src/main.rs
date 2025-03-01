use color_eyre::Result;
use ipc::receive_input_async_ipc;
use serde::{Deserialize, Serialize};
use stardust_xr_fusion::{
	client::Client,
	core::{messenger::MessengerError, values::Vector2},
	fields::FieldRefAspect,
	objects::{hmd, interfaces::FieldRefProxy, object_registry::ObjectRegistry, FieldRefProxyExt},
	root::RootAspect,
	ClientHandle,
};
use stardust_xr_molecules::keyboard::KeyboardHandlerProxy;
use std::{cell::UnsafeCell, io::IsTerminal, sync::Arc};
use tokio::sync::{mpsc, Notify};
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
	let mut client = Client::connect().await.expect("Couldn't connect");
	let client_handle = client.handle();
	let on_frame = Arc::new(Notify::new());
	tokio::spawn({
		let on_frame = on_frame.clone();
		async move {
			loop {
				{
					let client_cell = UnsafeCell::new(&mut client);
					// this is safe because internally the client uses 2 chaneels, one for flush and
					// one for dispatch
					tokio::select! {
						v = unsafe { client_cell.get().as_mut().unwrap().flush() } => v.unwrap(),
						v = unsafe { client_cell.get().as_mut().unwrap().dispatch() } => v.unwrap(),
					};
				}
				if let Some(stardust_xr_fusion::root::RootEvent::Frame { info: _ }) =
					client.get_root().recv_root_event()
				{
					on_frame.notify_one();
				}
			}
		}
	});
	let (keyboard_tx, keyboard_rx) = mpsc::unbounded_channel::<KeyboardEvent>();

	let event_loop = tokio::task::spawn(spatialize_input(
		on_frame,
		client_handle.clone(),
		keyboard_rx,
	));
	let input_loop = tokio::task::spawn(input_loop(client_handle.clone(), keyboard_tx));

	_ = tokio::select! {
		biased;
		_ = tokio::signal::ctrl_c() => Ok(()),
		e = event_loop => match e {
			Ok(v) => Ok(v?),
			err @ Err(_) => err.map(|_|()),
		},
		_ = input_loop => Ok(()),
	};
	Ok(())
}

async fn spatialize_input(
	frame_notifier: Arc<Notify>,
	client: Arc<ClientHandle>,
	mut key_events: mpsc::UnboundedReceiver<KeyboardEvent>,
) -> Result<(), MessengerError> {
	let conn = Connection::session().await.unwrap();
	let object_registry = ObjectRegistry::new(&conn).await.unwrap();
	let hmd = hmd(&client).await.unwrap();
	loop {
		frame_notifier.notified().await;
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

async fn input_loop(
	client: Arc<ClientHandle>,
	key_changed_event: mpsc::UnboundedSender<KeyboardEvent>,
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
			ipc::Message::MouseMove(_delta) => {}
			ipc::Message::MouseButton {
				button: _,
				pressed: _,
			} => {}
			ipc::Message::MouseAxisContinuous(_scroll) => {}
			ipc::Message::MouseAxisDiscrete(_scroll) => {}
			ipc::Message::ResetInput => (),
			ipc::Message::Disconnect => break,
		};
	}
}
