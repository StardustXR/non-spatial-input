use color_eyre::Result;
use glam::Vec2;
use ipc::receive_input_async_ipc;
use serde::{Deserialize, Serialize};
use spatializer::spatial_input_beam;
use stardust_xr_fusion::{
	client::Client,
	core::values::Vector2,
	objects::{connect_client, hmd},
	root::{RootAspect, RootEvent},
	ClientHandle,
};
use stardust_xr_molecules::keyboard::KeyboardHandlerProxy;
use stardust_xr_molecules::mouse::MouseHandlerProxy;
use std::{io::IsTerminal, sync::Arc};
use tokio::sync::{mpsc, Notify};
use tracing::{debug_span, Instrument};
use tracing_subscriber::{layer::SubscriberExt as _, EnvFilter};

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

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe manifold or eclipse's output into this e.g. `eclipse | simular`");
	}
	let registry = tracing_subscriber::registry();
	#[cfg(feature = "tracy")]
	let registry = registry.with(tracing_tracy::TracyLayer::default());
	tracing::subscriber::set_global_default(
		registry
			.with(EnvFilter::from_default_env())
			.with(tracing_subscriber::fmt::layer().compact()),
	)
	.unwrap();
	let conn = connect_client().await.unwrap();
	let client = Client::connect().await.expect("Couldn't connect");
	let client_handle = client.handle();
	let async_loop = client.async_event_loop();
	let async_event_handle = async_loop.get_event_handle();
	let (keyboard_tx, keyboard_rx) = mpsc::unbounded_channel::<KeyboardEvent>();
	let (mouse_tx, mouse_rx) = mpsc::unbounded_channel::<MouseEvent>();

	let event_handle = Arc::new(Notify::new());
	let frame_loop = tokio::task::spawn({
		let event_handle = event_handle.clone();
		let client_handle = client_handle.clone();
		async move {
			loop {
				async_event_handle.wait().await;
				match client_handle.get_root().recv_root_event() {
					Some(RootEvent::Frame { info: _ }) => {
						event_handle.notify_waiters();
					}
					Some(RootEvent::Ping { response }) => {
						response.send(Ok(()));
					}
					Some(RootEvent::SaveState { response: _ }) => {
						// no state to save
					}
					None => {}
				}
			}
		}
	});

	let hmd = hmd(&client_handle).await.unwrap();

	let keyboard_loop =
		tokio::task::spawn(
			spatial_input_beam::<KeyboardHandlerProxy, KeyboardEvent, ()>(
				conn.clone(),
				hmd.clone(),
				keyboard_rx,
				async |proxy, event, _| match event {
					KeyboardEvent::KeyMap(keymap_id) => {
						_ = proxy
							.keymap(keymap_id)
							.instrument(debug_span!("sending keymap"))
							.await;
					}
					KeyboardEvent::Key { key, pressed, map } => {
						_ = proxy
							.keymap(map)
							.instrument(debug_span!("sending keymap as part of button"))
							.await;
						_ = proxy
							.key_state(key, pressed)
							.instrument(debug_span!("sending keypress"))
							.await;
					}
				},
				async |_, _| {},
				async |proxy| _ = proxy.reset().await,
			),
		);
	let mouse_loop = tokio::task::spawn(spatial_input_beam::<MouseHandlerProxy, MouseEvent, Vec2>(
		conn.clone(),
		hmd,
		mouse_rx,
		async |proxy, event, move_state: &mut Option<Vec2>| match event {
			MouseEvent::Move { delta } => {
				*move_state.get_or_insert(Vec2::ZERO) += Vec2::from(delta);
			}
			MouseEvent::Button { button, pressed } => {
				_ = proxy
					.button(button, pressed)
					.instrument(debug_span!("sending mouse button"))
					.await;
			}
			MouseEvent::AxisContinuous { a } => {
				_ = proxy
					.scroll_continuous((a.x, a.y))
					.instrument(debug_span!("sending mouse scroll continuos"))
					.await;
			}
			MouseEvent::AxisDiscrete { a } => {
				_ = proxy
					.scroll_discrete((a.x, a.y))
					.instrument(debug_span!("sending mouse scroll discrete"))
					.await;
			}
		},
		async |proxy, delta| {
			_ = proxy
				.motion((delta.x, delta.y))
				.instrument(debug_span!("sending mouse motion"))
				.await;
		},
		async |proxy| _ = proxy.reset().await,
	));
	let input_loop = tokio::task::spawn(input_loop(client_handle.clone(), keyboard_tx, mouse_tx));

	tokio::select! {
		biased;
		e = tokio::signal::ctrl_c() => e?,
		e = keyboard_loop => e?,
		e = mouse_loop => e?,
		e = input_loop => e?,
		e = frame_loop => e?,
	};
	Ok(())
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
	while let Ok(message) = receive_input_async_ipc()
		.instrument(debug_span!("handling input ipc message"))
		.await
	{
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
				let _span = debug_span!("send mouse motion").entered();
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
