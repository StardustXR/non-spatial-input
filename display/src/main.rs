use ipc::receive_input_async_ipc;
use std::io::IsTerminal;

#[tokio::main(flavor = "current_thread")]
async fn main() {
	if std::io::stdin().is_terminal() {
		panic!("You need to pipe azimuth or eclipse's output into this e.g. `eclipse | azimuth`");
	}
	// console_subscriber::init();
	color_eyre::install().unwrap();
	while let Ok(message) = receive_input_async_ipc().await {
		println!("{message}");
	}
}
