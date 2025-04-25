use clap::Parser;
use env_logger::Builder;
use tokio::time::timeout;
use std::env;
use std::error::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::spawn;

use log::{debug, error, info, trace};
const CONTROL_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1); // Timeout for write operation
const BUFFER_SIZE: usize = 8192; 

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(clap::Subcommand, Debug)]
enum Mode {
    Listen {
        #[arg(long, default_value = "0.0.0.0")]
        public_host: String,
        #[arg(long, default_value = "0.0.0.0")]
        control_host: String,

        #[arg(long)]
        public_ports: String,
        #[arg(long)]
        control_ports: String,
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

    let mut buf_ar = tokio::io::BufReader::with_capacity(8 * 1024, &mut ar);
    let mut buf_br = tokio::io::BufReader::with_capacity(8 * 1024, &mut br);

    let forward1 = tokio::io::copy_buf(&mut buf_ar, &mut bw);
    let forward2 = tokio::io::copy_buf(&mut buf_br, &mut aw);

    let _ = tokio::try_join!(forward1, forward2);
}

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
    loop {
        let control_listener = TcpListener::bind(&control).await?;
        let public_listener = TcpListener::bind(&public).await?;
    
        let public_str = public.as_str();
        info!("[VPS {public_str}] Listening for control on {}", control);
        let (mut control_conn, _) = control_listener.accept().await?;
        info!("[VPS {public_str}] Client connected on control channel");

        loop {
            let (incoming_conn, addr) = public_listener.accept().await?;
            trace!("[VPS {public_str} ] Incoming connection from {}", addr);

            if timeout(CONTROL_WRITE_TIMEOUT, control_conn.write_all(&[1u8])).await.is_err() {
                error!("[VPS {public_str}] Failed to notify client to connect (write timeout)");
                return Ok(());
            }
    
            // Accept reverse connection
            let reverse_conn = match timeout(CONTROL_WRITE_TIMEOUT, control_listener.accept()).await {
                Ok(Ok((reverse_conn, _))) => reverse_conn,
                Ok(Err(e)) => {
                    error!("[VPS {public_str}] Failed to accept reverse connection: {}", e);
                    break; // Exit the loop or handle the error
                }
                Err(_) => {
                    error!("[VPS {public_str}] Reverse connection accept timed out");
                    break; // Exit the loop or handle the timeout
                }
            };
            trace!("[VPS {public_str}] Accepted reverse connection for client {}", addr);
    
            spawn(bidirectional(incoming_conn, reverse_conn));
        }
    }
    Ok(())

}

fn get_worker_threads() -> usize {
    env::var("PUNCHER_THREADS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(num_cpus::get)
}

// Returns a range of ports
fn split_to_port_range(port: &str) -> Vec<i32> {
    if port.contains("-") {
        let (start_port, end_port) = port.split_once("-").expect("Invalid port arguments");

        let (start_port, end_port) = (
            start_port
                .parse::<i32>()
                .expect("Start port is non numerical"),
            end_port.parse::<i32>().expect("End Port is non numerical"),
        );
        (start_port..=end_port).collect()
    } else {
        vec![port.parse().expect("Non numerical port provided")]
    }
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
                Mode::Listen {
                    public_host,
                    control_host,
                    public_ports,
                    control_ports,
                } => {
                    let public_ports = split_to_port_range(&public_ports);
                    let control_ports = split_to_port_range(&control_ports);

                    assert_eq!(
                        public_ports.len(),
                        control_ports.len(),
                        "Port ranges must be of equal length"
                    );

                    // Check for overlap
                    let public_set: std::collections::HashSet<_> =
                        public_ports.iter().cloned().collect();
                    let control_set: std::collections::HashSet<_> =
                        control_ports.iter().cloned().collect();

                    assert!(
                        public_set.is_disjoint(&control_set),
                        "Public and control port ranges must not overlap"
                    );

                    let mut handles = Vec::new();

                    for (pub_port, ctrl_port) in public_ports.iter().copied().zip(control_ports.iter().copied()) {
                        let public_host = public_host.clone();
                        let control_host = control_host.clone();
                    
                        let handle = tokio::spawn(async move {
                            info!(
                                "Running listener for {} with control node at {}",
                                pub_port, ctrl_port
                            );
                            if let Err(e) = run_listener(
                                format!("{}:{}", public_host, pub_port),
                                format!("{}:{}", control_host, ctrl_port),
                            )
                            .await
                            {
                                error!(
                                    "Listener error on ports {} and {}: {}",
                                    pub_port, ctrl_port, e
                                );
                            }
                        });
                    
                        handles.push(handle);
                    }
                    
                    for handle in handles {
                        let _ = handle.await;
                    }
                }
                Mode::Forward { server, local } => run_forwarder(server, local).await?,
            }
            Ok(())
        })
}
