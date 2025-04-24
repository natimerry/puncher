use tokio::net::{TcpStream, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc::{unbounded_channel, UnboundedSender}};
use tokio::{spawn, select};
use clap::Parser;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::error::Error;
use std::sync::Arc;

type SharedPool = Arc<Mutex<VecDeque<TcpStream>>>;

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

// Bidirectional copy
async fn bidirectional(mut a: TcpStream, mut b: TcpStream) {
    let (mut ar, mut aw) = a.split();
    let (mut br, mut bw) = b.split();

    let forward1 = tokio::io::copy(&mut ar, &mut bw);
    let forward2 = tokio::io::copy(&mut br, &mut aw);

    let _ = tokio::try_join!(forward1, forward2);
}

// CLIENT
async fn run_forwarder(server_addr: String, local_addr: String, pool_size: usize) -> Result<(), Box<dyn Error>> {
    let mut control = TcpStream::connect(&server_addr).await?;
    println!("[Client] Connected to control channel at {}", server_addr);

    // Pre-establish reverse data connections
    let pool = Arc::new(Mutex::new(VecDeque::new()));
    for _ in 0..pool_size {
        let conn = TcpStream::connect(&server_addr).await?;
        println!("[Client] Added reverse connection to pool");
        pool.lock().await.push_back(conn);
    }

    loop {
        let mut buf = [0u8; 1];
        control.read_exact(&mut buf).await?;
        println!("[Client] Received connection request");

        let local = TcpStream::connect(&local_addr).await?;
        println!("[Client] Connected to local service");

        let mut stream = None;
        {
            let mut pool_guard = pool.lock().await;
            stream = pool_guard.pop_front();
        }

        let reverse_conn = match stream {
            Some(conn) => conn,
            None => {
                println!("[Client] Pool empty! Creating new reverse connection");
                TcpStream::connect(&server_addr).await?
            }
        };

        spawn(bidirectional(reverse_conn, local));
    }
}

// SERVER
async fn run_listener(public: String, control: String) -> Result<(), Box<dyn Error>> {
    let control_listener = TcpListener::bind(&control).await?;
    let public_listener = TcpListener::bind(&public).await?;

    println!("[VPS] Listening for control on {}", control);
    let (mut control_conn, _) = control_listener.accept().await?;
    println!("[VPS] Client connected on control channel");

    let pool: SharedPool = Arc::new(Mutex::new(VecDeque::new()));
    let pool_clone = pool.clone();

    // Task to fill pool with reverse connections
    spawn(async move {
        loop {
            let (stream, _) = control_listener.accept().await.unwrap();
            println!("[VPS] Reverse data connection accepted");
            pool_clone.lock().await.push_back(stream);
        }
    });

    // Handle public connections
    loop {
        let (incoming_conn, addr) = public_listener.accept().await?;
        println!("[VPS] Incoming connection from {}", addr);

        // Notify client to prepare to forward
        control_conn.write_all(&[1u8]).await?;

        // Wait for a connection from the pool
        let reverse_conn = loop {
            if let Some(conn) = pool.lock().await.pop_front() {
                break conn;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        };

        spawn(bidirectional(incoming_conn, reverse_conn));
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.mode {
        Mode::Listen { public, control } => run_listener(public, control).await?,
        Mode::Forward { server, local, pool_size } => run_forwarder(server, local, pool_size).await?,
    }

    Ok(())
}
