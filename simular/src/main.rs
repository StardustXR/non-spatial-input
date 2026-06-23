use color_eyre::Result;
use ipc::receive_input_async_ipc;
use spatializer::SpatialInputBeam;
use stardust_xr_fusion::{
	client::Client,
	keymap::{KeymapStore, KeymapStoreExt},
	tracked::{Tracked, TrackedExt},
};
use stardust_xr_molecules::{
	keyboard_handler::{
		self,
		protocol::{KeyEvent, KeyboardHandler},
		ModifierState,
	},
	mouse_handler::{self, protocol::MouseHandler, ScrollSource},
};
use std::{io::IsTerminal, sync::Arc};
use tracing::{debug_span, Instrument};
use tracing_subscriber::{layer::SubscriberExt as _, EnvFilter};

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

	let (client, _) = Client::auto_connect(&[]).await.expect("Couldn't connect");

	let hmd = Tracked::hmd_spatial(&client).await.unwrap();

	let keymap_store = KeymapStore::connect(&client).await.unwrap();

	let keyboard_beam = SpatialInputBeam::new(
		&client,
		hmd.clone(),
		|_, v| Some(KeyboardHandler::from_object_or_ref(v)),
		keyboard_handler::protocol::EXTERNAL_PROTOCOL
			.protocol_name
			.to_string(),
		f32::INFINITY,
	)
	.await
	.unwrap();
	let mouse_beam = SpatialInputBeam::new(
		&client,
		hmd.clone(),
		|_, v| Some(MouseHandler::from_object_or_ref(v)),
		mouse_handler::protocol::EXTERNAL_PROTOCOL
			.protocol_name
			.to_string(),
		f32::INFINITY,
	)
	.await
	.unwrap();

	let input_loop = tokio::task::spawn(input_loop(
		keymap_store,
		keyboard_beam.handler_arc().clone(),
		mouse_beam.handler_arc().clone(),
	));

	tokio::select! {
		biased;
		e = tokio::signal::ctrl_c() => e?,
		e = input_loop => e?,
	};
	drop(keyboard_beam);
	drop(mouse_beam);
	Ok(())
}

async fn input_loop(
	keymap_store: KeymapStore,
	keyboard_beam: Arc<SpatialInputBeam<KeyboardHandler>>,
	mouse_beam: Arc<SpatialInputBeam<MouseHandler>>,
) {
	let mut keymap = None;
	while let Ok(message) = receive_input_async_ipc()
		.instrument(debug_span!("handling input ipc message"))
		.await
	{
		match message {
			ipc::Message::Keymap(map) => {
				let Some(Ok(new_keymap_id)) = keymap_store
					.exchange_string(&map)
					.await
					.map(|v| v.inspect_err(|err| tracing::error!("failed keymap exchange: {err}")))
				else {
					tracing::warn!("failed keymap exchanged");
					continue;
				};
				keymap = Some(new_keymap_id);
			}
			ipc::Message::Key {
				keycode,
				pressed,
				mod_pressed,
				mod_latched,
				mod_locked,
				layout_group,
			} => {
				let Some(keymap) = keymap.clone() else {
					tracing::warn!("no keymap");
					continue;
				};
				let Some(handler) = keyboard_beam.get_handler().await else {
					continue;
				};
				_ = handler.key(
					KeyEvent {
						keycode,
						pressed,
						modifiers: ModifierState {
							depressed: mod_pressed,
							latched: mod_latched,
							locked: mod_locked,
							layout_group,
						},
						keymap,
					},
					// TODO: forward timestamp?
					None,
				);
			}
			ipc::Message::MouseMove(delta) => {
				let Some(handler) = mouse_beam.get_handler().await else {
					continue;
				};
				// TODO: forward timestamp?
				_ = handler.motion(delta, None);
			}
			ipc::Message::MouseButton { button, pressed } => {
				let Some(handler) = mouse_beam.get_handler().await else {
					continue;
				};
				// TODO: forward timestamp?
				_ = handler.button(button, pressed, None);
			}
			ipc::Message::MouseAxisContinuous(scroll, source) => {
				let Some(handler) = mouse_beam.get_handler().await else {
					continue;
				};
				// TODO: forward timestamp?
				_ = handler.scroll_smooth(
					scroll,
					match source {
						ipc::ScrollSource::Wheel => ScrollSource::Wheel,
						ipc::ScrollSource::Finger => ScrollSource::Finger,
						ipc::ScrollSource::Continuous => ScrollSource::Continuous,
						ipc::ScrollSource::WheelTilt => ScrollSource::WheelTilt,
					},
					None,
				);
			}
			ipc::Message::MouseAxisDiscrete(scroll, source) => {
				let Some(handler) = mouse_beam.get_handler().await else {
					continue;
				};
				// TODO: forward timestamp?
				_ = handler.scroll_discrete(
					scroll,
					match source {
						ipc::ScrollSource::Wheel => ScrollSource::Wheel,
						ipc::ScrollSource::Finger => ScrollSource::Finger,
						ipc::ScrollSource::Continuous => ScrollSource::Continuous,
						ipc::ScrollSource::WheelTilt => ScrollSource::WheelTilt,
					},
					None,
				);
			}
			ipc::Message::ResetInput => (),
			ipc::Message::Disconnect => break,
		};
	}
}
