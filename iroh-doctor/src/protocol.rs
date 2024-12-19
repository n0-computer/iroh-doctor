use anyhow;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::{io::AsyncWriteExt, sync};

use crate::progress::ProgressWriter;

/// Trait for progress bar functionality
pub trait ProgressBarExt: Clone + Send + Sync + 'static {
    fn set_message(&self, msg: String);
    fn set_position(&self, pos: u64);
    fn set_length(&self, len: u64);
}

/// Trait for GUI functionality
pub trait GuiExt {
    type ProgressBar: ProgressBarExt;
    fn pb(&self) -> &Self::ProgressBar;
    fn set_send(&self, bytes: u64, duration: Duration);
    fn set_recv(&self, bytes: u64, duration: Duration);
    fn set_echo(&self, bytes: u64, duration: Duration);
    fn clear(&self);
}

/// Configuration for testing.
#[derive(Debug, Clone, Copy)]
pub struct TestConfig {
    pub size: u64,
    pub iterations: Option<u64>,
}

/// Possible streams that can be requested.
#[derive(Debug, Serialize, Deserialize, MaxSize)]
pub enum TestStreamRequest {
    Echo { bytes: u64 },
    Drain { bytes: u64 },
    Send { bytes: u64, block_size: u32 },
}

/// Updates the progress bar.
pub fn update_pb(
    task: &'static str,
    pb: Option<impl ProgressBarExt + Send + 'static>,
    total_bytes: u64,
    mut updates: sync::mpsc::Receiver<u64>,
) -> tokio::task::JoinHandle<()> {
    if let Some(pb) = pb {
        pb.set_message(task.to_string());
        pb.set_position(0);
        pb.set_length(total_bytes);
        tokio::spawn(async move {
            while let Some(position) = updates.recv().await {
                pb.set_position(position);
            }
        })
    } else {
        tokio::spawn(std::future::ready(()))
    }
}

/// Handles a test stream request.
pub async fn handle_test_request(
    mut send: SendStream,
    mut recv: RecvStream,
    gui: &impl GuiExt,
) -> anyhow::Result<()> {
    let mut buf = [0u8; TestStreamRequest::POSTCARD_MAX_SIZE];
    recv.read_exact(&mut buf).await?;
    let request: TestStreamRequest = postcard::from_bytes(&buf)?;
    let pb = Some(gui.pb().clone());
    match request {
        TestStreamRequest::Echo { bytes } => {
            // copy the stream back
            let (mut send, updates) = ProgressWriter::new(&mut send);
            let t0 = Instant::now();
            let progress = update_pb("echo", pb, bytes, updates);
            tokio::io::copy(&mut recv, &mut send).await?;
            let elapsed = t0.elapsed();
            drop(send);
            progress.await?;
            gui.set_echo(bytes, elapsed);
        }
        TestStreamRequest::Drain { bytes } => {
            // drain the stream
            let (mut send, updates) = ProgressWriter::new(tokio::io::sink());
            let progress = update_pb("recv", pb, bytes, updates);
            let t0 = Instant::now();
            tokio::io::copy(&mut recv, &mut send).await?;
            let elapsed = t0.elapsed();
            drop(send);
            progress.await?;
            gui.set_recv(bytes, elapsed);
        }
        TestStreamRequest::Send { bytes, block_size } => {
            // send the requested number of bytes, in blocks of the requested size
            let (mut send, updates) = ProgressWriter::new(&mut send);
            let progress = update_pb("send", pb, bytes, updates);
            let t0 = Instant::now();
            send_blocks(&mut send, bytes, block_size).await?;
            drop(send);
            let elapsed = t0.elapsed();
            progress.await?;
            gui.set_send(bytes, elapsed);
        }
    }
    send.finish()?;
    Ok(())
}

/// Sends, receives and echoes data in a connection.
pub async fn active_side<G: GuiExt>(
    connection: Connection,
    config: &TestConfig,
    gui: Option<&G>,
) -> anyhow::Result<()> {
    let n = config.iterations.unwrap_or(u64::MAX);
    if let Some(gui) = gui {
        let pb = Some(gui.pb());
        for _ in 0..n {
            let d = send_test(&connection, config, pb).await?;
            gui.set_send(config.size, d);
            let d = recv_test(&connection, config, pb).await?;
            gui.set_recv(config.size, d);
            let d = echo_test(&connection, config, pb).await?;
            gui.set_echo(config.size, d);
        }
    } else {
        let pb: Option<&G::ProgressBar> = None;
        for _ in 0..n {
            let _d = send_test(&connection, config, pb).await?;
            let _d = recv_test(&connection, config, pb).await?;
            let _d = echo_test(&connection, config, pb).await?;
        }
    }

    // Close the connection gracefully.
    // We're always the ones last receiving data, because
    // `echo_test` waits for data on the connection as the last thing.
    connection.close(0u32.into(), b"done");
    connection.closed().await;

    Ok(())
}

/// Accepts connections and answers requests (echo, drain or send) as passive side.
pub async fn passive_side(gui: impl GuiExt, connection: Connection) -> anyhow::Result<()> {
    let conn = connection.clone();
    let accept_loop = async move {
        let result = loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    if let Err(cause) = handle_test_request(send, recv, &gui).await {
                        eprintln!("Error handling test request {cause}");
                    }
                }
                Err(cause) => {
                    eprintln!("error accepting bidi stream {cause}");
                    break Err(cause.into());
                }
            };
        };

        conn.close(0u32.into(), b"internal err");
        conn.closed().await;
        eprintln!("Connection closed.");

        result
    };
    let conn_closed = async move {
        connection.closed().await;
        eprintln!("Connection closed.");
        anyhow::Ok(())
    };
    futures_lite::future::race(conn_closed, accept_loop).await
}

/// Sends a test request in a connection.
pub async fn send_test_request(
    send: &mut SendStream,
    request: &TestStreamRequest,
) -> anyhow::Result<()> {
    let mut buf = [0u8; TestStreamRequest::POSTCARD_MAX_SIZE];
    postcard::to_slice(&request, &mut buf)?;
    send.write_all(&buf).await?;
    Ok(())
}

/// Sends the requested number of bytes, in blocks of the requested size.
pub async fn send_blocks(
    mut send: impl tokio::io::AsyncWrite + Unpin,
    total_bytes: u64,
    block_size: u32,
) -> anyhow::Result<()> {
    let buf = vec![0u8; block_size as usize];
    let mut remaining = total_bytes;
    while remaining > 0 {
        let n = remaining.min(block_size as u64);
        send.write_all(&buf[..n as usize]).await?;
        remaining -= n;
    }
    Ok(())
}

/// Runs an echo test
pub async fn echo_test(
    connection: &Connection,
    config: &TestConfig,
    pb: Option<&impl ProgressBarExt>,
) -> anyhow::Result<Duration> {
    let size = config.size;
    let (mut send, mut recv) = connection.open_bi().await?;
    send_test_request(&mut send, &TestStreamRequest::Echo { bytes: size }).await?;
    let (mut sink, updates) = ProgressWriter::new(tokio::io::sink());
    let copying = tokio::spawn(async move { tokio::io::copy(&mut recv, &mut sink).await });
    let progress = update_pb("echo", pb.cloned(), size, updates);
    let t0 = Instant::now();
    send_blocks(&mut send, size, 1024 * 1024).await?;
    send.finish()?;
    let received = copying.await??;
    anyhow::ensure!(received == size);
    let duration = t0.elapsed();
    progress.await?;
    Ok(duration)
}

/// Runs a send test
pub async fn send_test(
    connection: &Connection,
    config: &TestConfig,
    pb: Option<&impl ProgressBarExt>,
) -> anyhow::Result<Duration> {
    let size = config.size;
    let (mut send, mut recv) = connection.open_bi().await?;
    send_test_request(&mut send, &TestStreamRequest::Drain { bytes: size }).await?;
    let (mut send_with_progress, updates) = ProgressWriter::new(&mut send);
    let copying =
        tokio::spawn(async move { tokio::io::copy(&mut recv, &mut tokio::io::sink()).await });
    let progress = update_pb("send", pb.cloned(), size, updates);
    let t0 = Instant::now();
    send_blocks(&mut send_with_progress, size, 1024 * 1024).await?;
    drop(send_with_progress);
    send.finish()?;
    drop(send);
    let received = copying.await??;
    anyhow::ensure!(received == 0);
    let duration = t0.elapsed();
    progress.await?;
    Ok(duration)
}

/// Runs a receive test
pub async fn recv_test(
    connection: &Connection,
    config: &TestConfig,
    pb: Option<&impl ProgressBarExt>,
) -> anyhow::Result<Duration> {
    let size = config.size;
    let (mut send, mut recv) = connection.open_bi().await?;
    let t0 = Instant::now();
    let (mut sink, updates) = ProgressWriter::new(tokio::io::sink());
    send_test_request(
        &mut send,
        &TestStreamRequest::Send {
            bytes: size,
            block_size: 1024 * 1024,
        },
    )
    .await?;
    let copying = tokio::spawn(async move { tokio::io::copy(&mut recv, &mut sink).await });
    let progress = update_pb("recv", pb.cloned(), size, updates);
    send.finish()?;
    let received = copying.await??;
    anyhow::ensure!(received == size);
    let duration = t0.elapsed();
    progress.await?;
    Ok(duration)
}
