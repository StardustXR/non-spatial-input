use input_window::InputWindow;
use std::{io::IsTerminal, process::exit};
use winit::{event_loop::EventLoopBuilder, platform::x11::EventLoopBuilderExtX11};

pub mod input_window;

fn main() {
	if std::io::stdout().is_terminal() {
		panic!("You need to pipe this into an input sink e.g. `manifold | azimuth`");
	}
	ctrlc::set_handler(|| exit(0)).unwrap();
	let event_loop = EventLoopBuilder::new()
		.with_any_thread(true)
		.with_x11()
		.build();
	let mut input_window = InputWindow::new(&event_loop);

	event_loop.run(move |event, _, control_flow| {
		input_window.handle_event(event);
		control_flow.set_wait();
	});
}
