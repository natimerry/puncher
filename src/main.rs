use clap::Parser;
use env_logger::Builder;
use log::{debug, error, info, trace, warn};
use std::env;
use std::error::Error;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::spawn;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::Notify;

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
        #[arg(long, default_value = "10")]
        pool_size: usize,
    },
}

async fn bidirectional(mut a: TcpStream, mut b: TcpStream) {
    let (mut ar, mut aw) = a.split();
    let (mut br, mut bw) = b.split();
    let forward1 = tokio::io::copy(&mut ar, &mut bw);
    let forward2 = tokio::io::copy(&mut br, &mut aw);
    let _ = tokio::try_join!(forward1, forward2);
}

async fn run_forwarder(server_addr: String, local_addr: String, pool_size: usize) -> Result<(), Box<dyn Error>> {
    let mut control = TcpStream::connect(&server_addr).await?;
    control.set_nodelay(true)?;
    info!("[Client] Connected to control channel at {}", server_addr);

    let (tx, mut rx): (Sender<TcpStream>, Receiver<TcpStream>) = mpsc::channel(pool_size);
    let server_addr = Arc::new(server_addr);

    // Pre-establish pool
    for i in 0..pool_size {
        if let Ok(mut stream) = TcpStream::connect(&*server_addr).await {
            stream.set_nodelay(true)?;
            let _ = tx.send(stream).await;
            info!("[Client {i}] Added reverse connection to pool");
        }
    }

    let refill_tx = tx.clone();
    let refill_addr = Arc::clone(&server_addr);
    spawn(async move {
        loop {
            if refill_tx.capacity() > 0 {
                if let Ok(mut stream) = TcpStream::connect(&*refill_addr).await {
                    stream.set_nodelay(true).ok();
                    let _ = refill_tx.send(stream).await;
                    debug!("[Client] Replenished pool");
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    });

    loop {
        let mut buf = [0u8; 1];
        control.read_exact(&mut buf).await?;
        debug!("[Client] Received connection request");

        let local = TcpStream::connect(&local_addr).await?;
        local.set_nodelay(true)?;

        let reverse_conn = rx.recv().await.unwrap_or_else(|| {
            warn!("[Client] Pool empty! Creating reverse connection on demand");
            futures::executor::block_on(TcpStream::connect(&*server_addr)).unwrap()
        });

        spawn(bidirectional(reverse_conn, local));
    }
}

async fn run_listener(public: String, control: String) -> Result<(), Box<dyn Error>> {
    let control_listener = TcpListener::bind(&control).await?;
    let public_listener = TcpListener::bind(&public).await?;

    info!("[VPS] Listening for control on {}", control);
    let (mut control_conn, _) = control_listener.accept().await?;
    control_conn.set_nodelay(true)?;
    info!("[VPS] Client connected on control channel");

    let (tx, mut rx): (Sender<TcpStream>, Receiver<TcpStream>) = mpsc::channel(100);
    let control_accept_tx = tx.clone();

    spawn(async move {
        loop {
            if let Ok((mut stream, _)) = control_listener.accept().await {
                stream.set_nodelay(true).ok();
                debug!("[VPS] Reverse data connection accepted");
                let _ = control_accept_tx.send(stream).await;
            }
        }
    });

    loop {
        let (incoming_conn, addr) = public_listener.accept().await?;
        info!("[VPS] Incoming connection from {}", addr);

        control_conn.write_all(&[1u8]).await?;

        let reverse_conn = rx.recv().await.expect("No reverse connections in pool");
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
                Mode::Forward { server, local, pool_size } => run_forwarder(server, local, pool_size).await?,
            }
            Ok(())
        })
}