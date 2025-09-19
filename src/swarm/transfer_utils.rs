//! Optimized transfer utilities adapted from iroh transfer example

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream};

/// Drain all data from a stream with optimized vectorized reading
///
/// Adapted from iroh/examples/transfer.rs
pub async fn drain_stream(
    stream: &mut RecvStream,
    read_unordered: bool,
) -> Result<(usize, Duration, u64)> {
    let mut read = 0;

    let download_start = Instant::now();
    let mut first_byte = true;
    let mut time_to_first_byte = download_start.elapsed();

    let mut num_chunks: u64 = 0;

    if read_unordered {
        while let Some(chunk) = stream
            .read_chunk(usize::MAX, false)
            .await
            .context("Failed to read chunk")?
        {
            if first_byte {
                time_to_first_byte = download_start.elapsed();
                first_byte = false;
            }
            read += chunk.bytes.len();
            num_chunks += 1;
        }
    } else {
        #[rustfmt::skip]
        let mut bufs = [
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
            Bytes::new(), Bytes::new(), Bytes::new(), Bytes::new(),
        ];

        while let Some(n) = stream
            .read_chunks(&mut bufs[..])
            .await
            .context("Failed to read chunks")?
        {
            if first_byte {
                time_to_first_byte = download_start.elapsed();
                first_byte = false;
            }
            read += bufs.iter().take(n).map(|buf| buf.len()).sum::<usize>();
            num_chunks += 1;
        }
    }

    Ok((read, time_to_first_byte, num_chunks))
}

/// Send data on a stream with configurable chunk size
///
/// Adapted from iroh/examples/transfer.rs with configurable chunk size
pub async fn send_data_on_stream(
    stream: &mut SendStream,
    stream_size: u64,
    chunk_size: usize,
) -> Result<()> {
    let chunk_size = chunk_size.clamp(1024, 16 * 1024 * 1024); // 1KB-16MB range

    let data_chunk = vec![0xAB; chunk_size];
    let bytes_data = Bytes::from(data_chunk);

    let full_chunks = stream_size / (chunk_size as u64);
    let remaining = (stream_size % (chunk_size as u64)) as usize;

    for _ in 0..full_chunks {
        stream
            .write_chunk(bytes_data.clone())
            .await
            .context("Failed sending data chunk")?;
    }

    if remaining != 0 {
        stream
            .write_chunk(bytes_data.slice(0..remaining))
            .await
            .context("Failed sending final chunk")?;
    }

    stream.finish().context("Failed finishing stream")?;
    stream
        .stopped()
        .await
        .context("Failed to wait for stream to be stopped")?;

    Ok(())
}

/// Bidirectional transfer: receive data while simultaneously sending response data
pub async fn handle_bidirectional_transfer(
    mut send: SendStream,
    mut recv: RecvStream,
    data_size: u64,
    chunk_size: usize,
) -> Result<(u64, u64, Duration)> {
    let start = Instant::now();

    let recv_task = tokio::spawn(async move { drain_stream(&mut recv, false).await });

    let send_task =
        tokio::spawn(async move { send_data_on_stream(&mut send, data_size, chunk_size).await });

    let (recv_result, send_result) =
        tokio::try_join!(recv_task, send_task).context("Transfer tasks failed")?;

    let (bytes_received, _time_to_first_byte, _num_chunks) =
        recv_result.context("Receive operation failed")?;

    send_result.context("Send operation failed")?;

    let duration = start.elapsed();
    Ok((bytes_received as u64, data_size, duration))
}
