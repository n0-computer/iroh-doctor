//! Builds the diagnostics-bundle zip the error dialog hands to the user.
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
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::components::{DiagState, EventEntry};
use crate::endpoints::Endpoint;
use crate::node::{NetReportSummary, PathInfo, ThroughputSnapshot};
use crate::portmap_probe::PortMapProbeResult;
use crate::relay_probe::RelayProbeResult;

/// Everything the export needs, cloned out of the App-level signals at
/// the moment the user clicks Send diagnostics.
pub struct Snapshot {
    pub error_message: String,
    pub endpoint_id: String,
    pub conn_state_label: String,
    pub paths: Vec<PathInfo>,
    pub rtt_history: VecDeque<f64>,
    pub events: VecDeque<EventEntry>,
    pub net_report: Option<NetReportSummary>,
    pub portmap: Option<PortMapProbeResult>,
    pub portmap_err: Option<String>,
    pub relays: Vec<RelayProbeResult>,
    pub relays_err: Option<String>,
    pub ttfdb: Option<Duration>,
    pub throughput: Option<ThroughputSnapshot>,
    pub endpoints: Vec<Endpoint>,
    pub log_dir: Option<PathBuf>,
}

/// Helper that lets the caller pull whichever pieces of state it has at
/// hand without forcing every field to be populated.
impl Snapshot {
    pub fn extract_diag_state<T: Clone + 'static>(
        state: &DiagState<T>,
    ) -> (Option<T>, Option<String>) {
        match state {
            DiagState::Ok(v) => (Some(v.clone()), None),
            DiagState::Err(e) => (None, Some(e.clone())),
            _ => (None, None),
        }
    }
}

/// Returns the zip bytes that the caller can write to disk.
pub fn build_zip(snapshot: &Snapshot) -> Result<Vec<u8>> {
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

        zip.start_file("portmap.txt", opts)?;
        zip.write_all(portmap_section(snapshot).as_bytes())?;

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
    out.push_str(&format!("generated_unix_seconds: {}\n", now_secs()));
    out.push_str(&format!("app_version: {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("iroh_pin: {IROH_PIN}\n"));
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
            p.rtt_ms,
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

fn portmap_section(s: &Snapshot) -> String {
    if let Some(err) = &s.portmap_err {
        return format!("error: {err}\n");
    }
    let Some(p) = &s.portmap else {
        return "(portmap probe not run)\n".to_string();
    };
    let mut out = String::new();
    out.push_str(&format!("upnp: {}\n", opt_bool(p.upnp)));
    out.push_str(&format!("pcp: {}\n", opt_bool(p.pcp)));
    out.push_str(&format!("nat_pmp: {}\n", opt_bool(p.nat_pmp)));
    if let Some(err) = &p.error {
        out.push_str(&format!("warning: {err}\n"));
    }
    out
}

fn relays_csv(s: &Snapshot) -> String {
    let mut out = String::from("url,connect_ms,ping_ms,error\n");
    if let Some(err) = &s.relays_err {
        out.push_str(&format!(",,,{}\n", csv_escape(err)));
        return out;
    }
    for r in &s.relays {
        out.push_str(&format!(
            "{},{},{},{}\n",
            csv_escape(&r.url),
            r.connect_ms.map(|v| v.to_string()).unwrap_or_default(),
            r.ping_ms.map(|v| v.to_string()).unwrap_or_default(),
            csv_escape(r.error.as_deref().unwrap_or("")),
        ));
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
    let mut out = String::from("[\n");
    for (i, d) in s.endpoints.iter().enumerate() {
        out.push_str(&format!(
            "  {{\"id\":{:?},\"name\":{:?},\"first_seen\":{},\"last_seen\":{}}}",
            d.id, d.name, d.first_seen, d.last_seen
        ));
        if i + 1 != s.endpoints.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push(']');
    out
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn opt_bool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// Crate pin shown in the README. We hard-code rather than read
/// Cargo.toml at runtime because the support team only cares which
/// release of iroh-doctor-app this is, not the lock graph.
const IROH_PIN: &str = "1.0.0-rc.1";
