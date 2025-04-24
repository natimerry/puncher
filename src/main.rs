use env_logger::Builder;
use tokio::net::{TcpStream, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::spawn;
use clap::Parser;
use std::env;
use std::error::Error;

use log::{debug, error, info, trace, warn};

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(clap::Subcommand, Debug)]
enum Mode {
    Listen {
        #[arg(long)]
        public: String,
        #[arg(long)]
        control: String,
    },
    Forward {
        #[arg(long)]
        server: String,
        #[arg(long)]
        local: String,
    },
}

// Bidirectional copy
async fn bidirectional(mut a: TcpStream, mut b: TcpStream) {
    let (mut ar, mut aw) = a.split();
    let (mut br, mut bw) = b.split();

    let forward1 = tokio::io::copy(&mut ar, &mut bw);
    let forward2 = tokio::io::copy(&mut br, &mut aw);

    let _ = tokio::try_join!(forward1, forward2);
}

// CLIENT
async fn run_forwarder(server_addr: String, local_addr: String) -> Result<(), Box<dyn Error>> {
    let mut control = TcpStream::connect(&server_addr).await?;
    info!("[Client] Connected to control channel at {}", server_addr);

    loop {
        let mut buf = [0u8; 1];
        control.read_exact(&mut buf).await?;
        debug!("[Client] Received connection request");

        let local = TcpStream::connect(&local_addr).await?;
        debug!("[Client] Connected to local service");

        let reverse_conn = TcpStream::connect(&server_addr).await?;
        trace!("[Client] Created reverse connection");

        spawn(bidirectional(reverse_conn, local));
    }
}

// SERVER
async fn run_listener(public: String, control: String) -> Result<(), Box<dyn Error>> {
    let control_listener = TcpListener::bind(&control).await?;
    let public_listener = TcpListener::bind(&public).await?;

    info!("[VPS] Listening for control on {}", control);
    let (mut control_conn, _) = control_listener.accept().await?;
    info!("[VPS] Client connected on control channel");

    loop {
        let (incoming_conn, addr) = public_listener.accept().await?;
        trace!("[VPS] Incoming connection from {}", addr);

        // Notify client to connect
        control_conn.write_all(&[1u8]).await?;

        // Accept reverse connection
        let (reverse_conn, _) = control_listener.accept().await?;
        trace!("[VPS] Accepted reverse connection for client {}", addr);

        spawn(bidirectional(incoming_conn, reverse_conn));
    }
}

fn get_worker_threads() -> usize {
    env::var("PUNCHER_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(num_cpus::get)
}

fn main() -> Result<(), Box<dyn Error>> {
    let threads = get_worker_threads();
    Builder::new()
        .filter_level(log::LevelFilter::Info)
        .parse_env("LOG_LEVEL")
        .format_timestamp_secs()
        .format_module_path(false)
        .format_level(true)
        .write_style(env_logger::WriteStyle::Auto)
        .init();

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()?
        .block_on(async {
            let cli = Cli::parse();
            match cli.mode {
                Mode::Listen { public, control } => run_listener(public, control).await?,
                Mode::Forward { server, local } => run_forwarder(server, local).await?,
            }
            Ok(())
        })
}
