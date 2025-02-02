use color_eyre::Result;
use ipc::receive_input_async_ipc;
use serde::{Deserialize, Serialize};
use stardust_xr_fusion::{
	client::Client,
	core::{messenger::MessengerError, values::Vector2},
	fields::FieldRefAspect,
	objects::{hmd, interfaces::FieldRefProxy, object_registry::ObjectRegistry, FieldRefProxyExt},
	root::{RootAspect, RootEvent},
	ClientHandle,
};
use stardust_xr_molecules::keyboard::KeyboardHandlerProxy;
use std::{
	io::{ErrorKind, IsTerminal},
	sync::Arc,
};
use tokio::{sync::mpsc, task::JoinError};
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
	let async_loop = client.async_event_loop();
	let (keyboard_tx, keyboard_rx) = mpsc::unbounded_channel::<KeyboardEvent>();

	// Pointer stuff
	// let mouse_sender = PulseSender::create(&hmd, Transform::identity(), &MOUSE_MASK)?
	// 	.wrap(PulseReceiverCollector::default())?;
	// let (hovered_mouse_tx, hovered_mouse) = watch::channel::<Option<PulseReceiver>>(None);

	// Keyboard stuff
	// let keyboard_sender = PulseSender::create(&hmd, Transform::identity(), &KEYBOARD_MASK)?
	// 	.wrap(PulseReceiverCollector::default())?;
	// let (hovered_keyboard_tx, hovered_keyboard) = watch::channel::<Option<PulseReceiver>>(None);

	let event_loop = tokio::task::spawn(spatialize_input(client_handle.clone(), keyboard_rx));
	println!("running input loop");
	let input_loop = tokio::task::spawn(input_loop(client_handle.clone(), keyboard_tx));
	// tokio::task::spawn(pointer_frame_loop(
	// 	frame_notifier.clone(),
	// 	hmd.alias(),
	// 	mouse_sender.wrapped().clone(),
	// 	hovered_mouse_tx,
	// ));
	// tokio::task::spawn(keyboard_frame_loop(
	// 	frame_notifier.clone(),
	// 	hmd.alias(),
	// 	keyboard_sender.wrapped().clone(),
	// 	hovered_keyboard_tx,
	// ));

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

// async fn spatialize_input(event_loop: Notify, client: Arc<ClientHandle>) {
async fn spatialize_input(
	client: Arc<ClientHandle>,
	mut key_events: mpsc::UnboundedReceiver<KeyboardEvent>,
) -> Result<(), MessengerError> {
	let conn = Connection::session().await.unwrap();
	let object_registry = ObjectRegistry::new(&conn).await.unwrap();
	let hmd = hmd(&client).await.unwrap();
	loop {
		let Some(RootEvent::Frame { info }) = client.get_root().recv_root_event() else {
			continue;
		};
		let keyboard_handlers = object_registry.get_objects("org.stardustxr.XKBv1");
		let mut closest_distance = f32::INFINITY;
		let mut closest_handler = None;
		for handler in &keyboard_handlers {
			let proxy = handler
				.to_typed_proxy::<FieldRefProxy>(&conn)
				.await
				.unwrap();
			let field_ref = proxy.import(&client).await.unwrap();

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
						println!("setting keymap id");
						_ = proxy.keymap(keymap_id).await;
					}
					KeyboardEvent::Key { key, pressed, map } => {
						println!("setting key state {key}: {pressed}");
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
	println!("input loop started");
	let mut keymap = None;

	while let message = receive_input_async_ipc().await {
		println!("recived input event");
		let message = match message {
			Ok(m) => m,
			Err(err) if err.kind() == ErrorKind::InvalidData => {
				println!("InvalidData: {err}");
				continue;
			}
			Err(err) => {
				panic!("Error: {err}");
				continue;
			}
		};
		println!("recived ok input event");
		match message {
			ipc::Message::Keymap(map) => {
				println!("got keymap!: {map}");
				let Ok(future) = client.register_xkb_keymap(map) else {
					continue;
				};
				println!("awaiting future");
				let Ok(new_keymap_id) = future.await else {
					continue;
				};
				println!("got keymap {new_keymap_id}");
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
			ipc::Message::MouseMove(_delta) => {
				// let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
				// 	continue;
				// };
				// MouseEvent {
				// 	delta: Some(delta),
				// 	..Default::default()
				// }
				// .send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::MouseButton {
				button: _,
				pressed: _,
			} => {
				// let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
				// 	continue;
				// };
				// let raw_input_events = mouse_state.raw_input_events.as_mut().unwrap();
				// if pressed {
				// 	raw_input_events.insert(button);
				// 	&mouse_state
				// } else {
				// 	raw_input_events.remove(&button);
				// 	&mouse_state
				// }
				// .send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::MouseAxisContinuous(_scroll) => {
				// let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
				// 	continue;
				// };
				// MouseEvent {
				// 	scroll_continuous: Some(scroll),
				// 	..Default::default()
				// }
				// .send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::MouseAxisDiscrete(_scroll) => {
				// let Some(hovered_mouse) = &*hovered_mouse.borrow() else {
				// 	continue;
				// };
				// MouseEvent {
				// 	scroll_discrete: Some(scroll),
				// 	..Default::default()
				// }
				// .send_event(&mouse_sender, &[hovered_mouse])
			}
			ipc::Message::ResetInput => (),
			ipc::Message::Disconnect => break,
		};
		println!("done handling packet");
	}
}
//
// async fn pointer_frame_loop(
// 	frame_notifier: Arc<Notify>,
// 	hmd: SpatialRef,
// 	mouse_sender: Arc<Mutex<PulseReceiverCollector>>,
// 	hovered_mouse_tx: watch::Sender<Option<PulseReceiver>>,
// ) {
// 	loop {
// 		frame_notifier.notified().await;
// 		detect_hover(hmd.alias(), mouse_sender.clone(), &hovered_mouse_tx).await
// 	}
// }
//
// async fn keyboard_frame_loop(
// 	frame_notifier: Arc<Notify>,
// 	hmd: SpatialRef,
// 	keyboard_sender: Arc<Mutex<PulseReceiverCollector>>,
// 	hovered_keyboard_tx: watch::Sender<Option<PulseReceiver>>,
// ) {
// 	loop {
// 		frame_notifier.notified().await;
// 		detect_hover(hmd.alias(), keyboard_sender.clone(), &hovered_keyboard_tx).await
// 	}
// }
//
// async fn detect_hover(
// 	hmd: SpatialRef,
// 	sender: Arc<Mutex<PulseReceiverCollector>>,
// 	hovered_tx: &watch::Sender<Option<PulseReceiver>>,
// ) {
// 	let mut closest_hit: Option<(PulseReceiver, RayMarchResult)> = None;
// 	let mut join = JoinSet::new();
// 	for (receiver, field) in sender.lock().0.values() {
// 		let receiver = receiver.alias();
// 		let field = field.alias();
// 		let hmd = hmd.alias();
// 		join.spawn(async move {
// 			(
// 				receiver,
// 				field.ray_march(&hmd, [0.0; 3], [0.0, 0.0, -1.0]).await,
// 			)
// 		});
// 	}
//
// 	while let Some(res) = join.join_next().await {
// 		let Ok((receiver, Ok(ray_info))) = res else {
// 			continue;
// 		};
// 		if ray_info.min_distance > 0.0 || ray_info.deepest_point_distance <= 0.001 {
// 			continue;
// 		}
// 		if let Some((hit_receiver, hit_info)) = &mut closest_hit {
// 			if ray_info.deepest_point_distance < hit_info.deepest_point_distance {
// 				*hit_receiver = receiver;
// 				*hit_info = ray_info;
// 			}
// 		} else {
// 			closest_hit.replace((receiver, ray_info));
// 		}
// 	}
// 	let _ = hovered_tx.send(closest_hit.map(|(r, _)| r));
// }
//
// struct FrameNotifier(Arc<Notify>, Root);
// impl RootHandler for FrameNotifier {
// 	fn frame(&mut self, _info: FrameInfo) {
// 		self.0.notify_waiters();
// 	}
// 	fn save_state(&mut self) -> Result<ClientState> {
// 		ClientState::from_root(&self.1)
// 	}
// }
