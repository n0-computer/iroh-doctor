//! Builds and saves the diagnostics-bundle zip the error dialog hands to
//! the user.
//!
//! The bundle includes everything we can gather without a round-trip
//! to disk-mounted secrets: the active error, the endpoint id, the
//! connection state, every probe result we've cached in memory, the
//! recent connection events, the saved endpoints, and the most recent
//! rolling log file from the config dir.
//!
//! The format is plain files inside a zip rather than one giant JSON
//! so a user can `unzip` and grep without parsing.

use std::collections::VecDeque;
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use dioxus::prelude::*;
use iroh_doctor_core::fmt::opt_bool;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use iroh_doctor_core::report::RelayLatencyRow;

use crate::components::{short_event_label, AppError, DiagState, EventEntry};
use crate::endpoints::{self, Endpoint};
use crate::logging::log_dir;
use crate::node::{ConnectionState, NetReportSummary, PathSnapshot, ThroughputSnapshot};

/// Everything the export needs, cloned out of the App-level signals at
/// the moment the user clicks Send diagnostics.
struct Snapshot {
    error_message: String,
    endpoint_id: String,
    conn_state_label: String,
    paths: Vec<PathSnapshot>,
    rtt_history: VecDeque<f64>,
    events: VecDeque<EventEntry>,
    net_report: Option<NetReportSummary>,
    relays: Vec<RelayLatencyRow>,
    relays_err: Option<String>,
    ttfdb: Option<Duration>,
    throughput: Option<ThroughputSnapshot>,
    endpoints: Vec<Endpoint>,
    log_dir: Option<PathBuf>,
}

/// Helper that lets the caller pull whichever pieces of state it has at
/// hand without forcing every field to be populated.
impl Snapshot {
    fn extract_diag_state<T: Clone + 'static>(state: &DiagState<T>) -> (Option<T>, Option<String>) {
        match state {
            DiagState::Ok(v) => (Some(v.clone()), None),
            DiagState::Err(e) => (None, Some(e.clone())),
            _ => (None, None),
        }
    }
}

/// Glue for the error dialog's "Send diagnostics" action: snapshot every
/// relevant App-level signal, build the zip off the render path, and hand
/// it to the platform save dialog. A failure feeds back into the error
/// modal via `error_sink`.
#[allow(clippy::too_many_arguments)]
pub fn send(
    err: AppError,
    endpoint_id: Signal<String>,
    conn_state: Signal<ConnectionState>,
    paths: Signal<Vec<PathSnapshot>>,
    rtt_history: Signal<VecDeque<f64>>,
    event_log: Signal<VecDeque<EventEntry>>,
    net_report_state: Signal<DiagState<NetReportSummary>>,
    relays_state: Signal<DiagState<Vec<RelayLatencyRow>>>,
    ttfdb: Signal<Option<Duration>>,
    throughput: Signal<Option<ThroughputSnapshot>>,
    endpoints_list: Signal<Vec<Endpoint>>,
    mut error_sink: Signal<Option<AppError>>,
) {
    let (net_report, _) = Snapshot::extract_diag_state(&net_report_state());
    let (relays, relays_err) = Snapshot::extract_diag_state(&relays_state());

    let snapshot = Snapshot {
        error_message: format!("[{}] {}", err.source, err.message),
        endpoint_id: endpoint_id(),
        conn_state_label: short_event_label(&conn_state()),
        paths: paths(),
        rtt_history: rtt_history(),
        events: event_log(),
        net_report,
        relays: relays.unwrap_or_default(),
        relays_err,
        ttfdb: ttfdb(),
        throughput: throughput(),
        endpoints: endpoints_list(),
        log_dir: log_dir(),
    };
    spawn(async move {
        let result = async {
            let bytes = build_zip(&snapshot).context("building diagnostics zip")?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let filename = format!("iroh-doctor-app-diagnostics-{now}.zip");
            save_zip(&filename, &bytes).await
        }
        .await;
        if let Err(e) = result {
            let msg = format!("{e:#}");
            tracing::error!(err = %msg, "diagnostics export failed");
            error_sink.set(Some(AppError::new("diagnostics export", msg)));
        }
    });
}

/// Desktop path: open a native save dialog via rfd and write the bytes
/// to whichever location the user picks. Cancelling the dialog is `Ok`;
/// only a failed write is an error worth surfacing.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
async fn save_zip(filename: &str, bytes: &[u8]) -> Result<()> {
    let dialog = rfd::AsyncFileDialog::new()
        .set_file_name(filename)
        .set_title("Save iroh-doctor-app diagnostics");
    let Some(handle) = dialog.save_file().await else {
        return Ok(());
    };
    handle
        .write(bytes)
        .await
        .context("writing diagnostics zip")?;
    Ok(())
}

/// Mobile path (iOS + Android): rfd has no usable backend, so write to the
/// app's sandbox documents directory where the platform's Files browser
/// exposes it. The user can share the file from there. Logs the resulting
/// path so a developer inspecting the log file can find it without guessing.
#[cfg(any(target_os = "ios", target_os = "android"))]
async fn save_zip(filename: &str, bytes: &[u8]) -> Result<()> {
    let dir = dirs::document_dir()
        .or_else(dirs::data_local_dir)
        .context("no documents directory on this device")?;
    let path = dir.join(filename);
    std::fs::write(&path, bytes)
        .with_context(|| format!("writing diagnostics zip to {}", path.display()))?;
    tracing::info!(path = %path.display(), "wrote diagnostics zip");
    Ok(())
}

/// Returns the zip bytes that the caller can write to disk.
fn build_zip(snapshot: &Snapshot) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("README.txt", opts)?;
        zip.write_all(readme(snapshot).as_bytes())?;

        zip.start_file("error.txt", opts)?;
        zip.write_all(snapshot.error_message.as_bytes())?;

        zip.start_file("endpoint.txt", opts)?;
        zip.write_all(endpoint_section(snapshot).as_bytes())?;

        zip.start_file("connection_events.txt", opts)?;
        zip.write_all(events_section(snapshot).as_bytes())?;

        zip.start_file("paths.txt", opts)?;
        zip.write_all(paths_section(snapshot).as_bytes())?;

        zip.start_file("rtt_history.csv", opts)?;
        zip.write_all(rtt_csv(snapshot).as_bytes())?;

        zip.start_file("net_report.txt", opts)?;
        zip.write_all(net_report_section(snapshot).as_bytes())?;

        zip.start_file("relays.csv", opts)?;
        zip.write_all(relays_csv(snapshot).as_bytes())?;

        zip.start_file("ttfdb.txt", opts)?;
        zip.write_all(ttfdb_section(snapshot).as_bytes())?;

        zip.start_file("throughput.txt", opts)?;
        zip.write_all(throughput_section(snapshot).as_bytes())?;

        zip.start_file("endpoints.json", opts)?;
        zip.write_all(endpoints_json(snapshot).as_bytes())?;

        if let Some(log_dir) = &snapshot.log_dir {
            attach_log_files(&mut zip, log_dir, opts)?;
        }

        zip.finish()?;
    }
    Ok(buf.into_inner())
}

fn readme(_s: &Snapshot) -> String {
    let mut out = String::from("iroh-doctor-app diagnostics bundle\n\n");
    out.push_str(&format!(
        "generated_unix_seconds: {}\n",
        endpoints::now_secs()
    ));
    out.push_str(&format!("app_version: {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("os: {}\n", std::env::consts::OS));
    out.push_str(&format!("arch: {}\n", std::env::consts::ARCH));
    out.push('\n');
    out.push_str("This bundle is for support and debugging. It includes the\n");
    out.push_str("endpoint id, recent connection events, every probe result we\n");
    out.push_str("have in memory, the saved endpoints list, and the most recent\n");
    out.push_str("rolling tracing log.\n\n");
    out.push_str("Endpoint ids are not secrets. iroh-services API keys are NOT\n");
    out.push_str("included.\n");
    out
}

fn endpoint_section(s: &Snapshot) -> String {
    format!(
        "endpoint_id: {}\nconn_state: {}\n",
        if s.endpoint_id.is_empty() {
            "(unknown)"
        } else {
            &s.endpoint_id
        },
        s.conn_state_label,
    )
}

fn events_section(s: &Snapshot) -> String {
    if s.events.is_empty() {
        return "(no events)\n".to_string();
    }
    let mut out = String::new();
    for e in &s.events {
        out.push_str(&format!(
            "+{:>4}s  [{}] {}\n",
            e.elapsed.as_secs(),
            e.kind,
            e.label
        ));
    }
    out
}

fn paths_section(s: &Snapshot) -> String {
    if s.paths.is_empty() {
        return "(no paths)\n".to_string();
    }
    let mut out = String::from("selected  kind     rtt_ms     addr\n");
    for p in &s.paths {
        out.push_str(&format!(
            "{}        {:8} {:>7.1}    {}\n",
            if p.selected { "*" } else { " " },
            format!("{:?}", p.kind).to_lowercase(),
            p.rtt.as_secs_f64() * 1000.0,
            p.addr,
        ));
    }
    out
}

fn rtt_csv(s: &Snapshot) -> String {
    let mut out = String::from("sample_index,rtt_ms\n");
    for (i, v) in s.rtt_history.iter().enumerate() {
        out.push_str(&format!("{i},{v}\n"));
    }
    out
}

fn net_report_section(s: &Snapshot) -> String {
    let Some(r) = &s.net_report else {
        return "(net_report not run)\n".to_string();
    };
    let mut out = String::new();
    out.push_str(&format!("nat: {}\n", r.nat));
    out.push_str(&format!("udp_v4: {}\n", r.udp_v4));
    out.push_str(&format!("udp_v6: {}\n", r.udp_v6));
    out.push_str(&format!(
        "global_v4: {}\n",
        r.global_v4.as_deref().unwrap_or("(none)"),
    ));
    out.push_str(&format!(
        "global_v6: {}\n",
        r.global_v6.as_deref().unwrap_or("(none)"),
    ));
    out.push_str(&format!(
        "mapping_varies_v4: {}\n",
        opt_bool(r.mapping_varies_v4)
    ));
    out.push_str(&format!(
        "mapping_varies_v6: {}\n",
        opt_bool(r.mapping_varies_v6)
    ));
    out.push_str(&format!("captive_portal: {}\n", opt_bool(r.captive_portal)));
    out.push_str(&format!(
        "preferred_relay: {}\n",
        r.preferred_relay.as_deref().unwrap_or("(none)"),
    ));
    out.push_str(&format!("relays_seen: {}\n", r.relays_seen));
    out
}

fn relays_csv(s: &Snapshot) -> String {
    let mut out = String::from("url,latency_ms,error\n");
    if let Some(err) = &s.relays_err {
        out.push_str(&format!(",,{}\n", csv_escape(err)));
        return out;
    }
    for r in &s.relays {
        out.push_str(&format!("{},{},\n", csv_escape(&r.url), r.latency_ms));
    }
    out
}

fn ttfdb_section(s: &Snapshot) -> String {
    match s.ttfdb {
        Some(d) => format!("time_to_first_direct_byte_ms: {}\n", d.as_millis()),
        None => "(no direct path observed yet)\n".to_string(),
    }
}

fn throughput_section(s: &Snapshot) -> String {
    let Some(t) = &s.throughput else {
        return "(no upload completed yet)\n".to_string();
    };
    let mut out = String::new();
    out.push_str(&format!("bytes: {}\n", t.bytes));
    out.push_str(&format!("elapsed_ms: {}\n", t.elapsed.as_millis()));
    match t.mbps {
        Some(m) => out.push_str(&format!("mbps: {m:.3}\n")),
        None => out.push_str("mbps: (elapsed was zero)\n"),
    }
    out
}

fn endpoints_json(s: &Snapshot) -> String {
    endpoints::serialize(&s.endpoints)
}

fn attach_log_files<W: Write + std::io::Seek>(
    zip: &mut ZipWriter<W>,
    log_dir: &PathBuf,
    opts: SimpleFileOptions,
) -> Result<()> {
    let Ok(read_dir) = std::fs::read_dir(log_dir) else {
        return Ok(());
    };
    // Pick the file with the latest mtime.
    let mut latest: Option<(SystemTime, PathBuf)> = None;
    for entry in read_dir.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        if latest.as_ref().is_none_or(|(t, _)| mtime > *t) {
            latest = Some((mtime, entry.path()));
        }
    }
    let Some((_, path)) = latest else {
        return Ok(());
    };
    let mut f = std::fs::File::open(&path).context("open log file")?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).context("read log file")?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "log.txt".into());
    zip.start_file(format!("logs/{name}"), opts)?;
    zip.write_all(&bytes)?;
    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}
