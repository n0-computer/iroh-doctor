//! Plot command implementation

use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::Rng;
use ratatui::{prelude::*, widgets::*};
use tracing::warn;

/// Runs the plot command.
pub async fn plot(
    interval: u64,
    metrics: String,
    timeframe: usize,
    scrape_url: String,
    file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let metrics: Vec<String> = metrics.split(',').map(|s| s.to_string()).collect();
    let interval = Duration::from_millis(interval);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = PlotterApp::new(metrics, timeframe, scrape_url, file);
    let res = run_plotter(&mut terminal, app, interval).await;
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

    Ok(())
}

/// Runs the [`PlotterApp`].
async fn run_plotter<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: PlotterApp,
    tick_rate: Duration,
) -> anyhow::Result<()> {
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|f| plotter_draw(f, &mut app))?;

        if crossterm::event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if let KeyCode::Char(c) = key.code {
                        app.on_key(c)
                    }
                }
            }
        }
        if last_tick.elapsed() >= tick_rate {
            app.on_tick().await;
            last_tick = Instant::now();
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// Converts an area into `n` chunks.
fn area_into_chunks(area: Rect, n: usize, is_horizontal: bool) -> std::rc::Rc<[Rect]> {
    let mut constraints = vec![];
    for _ in 0..n {
        constraints.push(Constraint::Percentage(100 / n as u16));
    }
    let layout = match is_horizontal {
        true => Layout::horizontal(constraints),
        false => Layout::vertical(constraints),
    };
    layout.split(area)
}

/// Creates a collection of [`Rect`] by splitting an [`Rect`] area into `n` chunks.
fn generate_layout_chunks(area: Rect, n: usize) -> Vec<Rect> {
    if n < 4 {
        let chunks = area_into_chunks(area, n, false);
        return chunks.iter().copied().collect();
    }
    let main_chunks = area_into_chunks(area, 2, true);
    let left_chunks = area_into_chunks(main_chunks[0], n / 2 + n % 2, false);
    let right_chunks = area_into_chunks(main_chunks[1], n / 2, false);
    let mut chunks = vec![];
    chunks.extend(left_chunks.iter());
    chunks.extend(right_chunks.iter());
    chunks
}

/// Draws the [`Frame`] given a [`PlotterApp`].
fn plotter_draw(f: &mut Frame, app: &mut PlotterApp) {
    let area = f.area();

    let metrics_cnt = app.metrics.len();
    let areas = generate_layout_chunks(area, metrics_cnt);

    for (i, metric) in app.metrics.iter().enumerate() {
        plot_chart(f, areas[i], app, metric);
    }
}

/// Draws the chart defined in the [`Frame`].
fn plot_chart(frame: &mut Frame, area: Rect, app: &PlotterApp, metric: &str) {
    let elapsed = app.internal_ts.as_secs_f64();
    let data = app.data.get(metric).unwrap().clone();
    let data_y_range = app.data_y_range.get(metric).unwrap();

    let moved = (elapsed / 15.0).floor() * 15.0 - app.timeframe as f64;
    let moved = moved.max(0.0);
    let x_start = 0.0 + moved;
    let x_end = moved + app.timeframe as f64 + 25.0;

    let y_start = data_y_range.0;
    let y_end = data_y_range.1;

    let last_val = data.last();
    let name = match last_val {
        Some(val) => {
            let val_y = val.1;
            format!("{metric}: {val_y:.0}")
        }
        None => metric.to_string(),
    };
    let datasets = vec![Dataset::default()
        .name(name)
        .marker(symbols::Marker::Dot)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&data)];

    // TODO(arqu): labels are incorrectly spaced for > 3 labels https://github.com/ratatui-org/ratatui/issues/334
    let x_labels = vec![
        Span::styled(
            format!("{x_start:.1}s"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{:.1}s", x_start + (x_end - x_start) / 2.0)),
        Span::styled(
            format!("{x_end:.1}s"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];

    let mut y_labels = vec![Span::styled(
        format!("{y_start:.0}"),
        Style::default().add_modifier(Modifier::BOLD),
    )];

    for i in 1..=10 {
        y_labels.push(Span::raw(format!(
            "{:.0}",
            y_start + (y_end - y_start) / 10.0 * i as f64
        )));
    }

    y_labels.push(Span::styled(
        format!("{y_end:.0}"),
        Style::default().add_modifier(Modifier::BOLD),
    ));

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Chart: {metric}")),
        )
        .x_axis(
            Axis::default()
                .title("X Axis")
                .style(Style::default().fg(Color::Gray))
                .labels(x_labels)
                .bounds([x_start, x_end]),
        )
        .y_axis(
            Axis::default()
                .title("Y Axis")
                .style(Style::default().fg(Color::Gray))
                .labels(y_labels)
                .bounds([y_start, y_end]),
        );

    frame.render_widget(chart, area);
}

/// All the information about the plotter app.
struct PlotterApp {
    should_quit: bool,
    metrics: Vec<String>,
    start_ts: Instant,
    data: HashMap<String, Vec<(f64, f64)>>,
    data_y_range: HashMap<String, (f64, f64)>,
    timeframe: usize,
    rng: rand::rngs::ThreadRng,
    freeze: bool,
    internal_ts: Duration,
    scrape_url: String,
    file_data: Vec<String>,
    file_header: Vec<String>,
}

impl PlotterApp {
    /// Creates a new [`PlotterApp`].
    fn new(
        metrics: Vec<String>,
        timeframe: usize,
        scrape_url: String,
        file: Option<PathBuf>,
    ) -> Self {
        let data = metrics.iter().map(|m| (m.clone(), vec![])).collect();
        let data_y_range = metrics.iter().map(|m| (m.clone(), (0.0, 0.0))).collect();
        let mut file_data: Vec<String> = file
            .map(|f| std::fs::read_to_string(f).unwrap())
            .unwrap_or_default()
            .split('\n')
            .map(|s| s.to_string())
            .collect();
        let mut file_header = vec![];
        let mut timeframe = timeframe;
        if !file_data.is_empty() {
            file_header = file_data[0].split(',').map(|s| s.to_string()).collect();
            file_data.remove(0);

            while file_data.last().unwrap().is_empty() {
                file_data.pop();
            }

            let first_line: Vec<String> = file_data[0].split(',').map(|s| s.to_string()).collect();
            let last_line: Vec<String> = file_data
                .last()
                .unwrap()
                .split(',')
                .map(|s| s.to_string())
                .collect();

            let start_time: usize = first_line.first().unwrap().parse().unwrap();
            let end_time: usize = last_line.first().unwrap().parse().unwrap();

            timeframe = (end_time - start_time) / 1000;
        }
        timeframe = timeframe.clamp(30, 90);

        file_data.reverse();
        Self {
            should_quit: false,
            metrics,
            start_ts: Instant::now(),
            data,
            data_y_range,
            timeframe,
            rng: rand::rng(),
            freeze: false,
            internal_ts: Duration::default(),
            scrape_url,
            file_data,
            file_header,
        }
    }

    /// Chooses what to do when a key is pressed.
    fn on_key(&mut self, c: char) {
        match c {
            'q' => {
                self.should_quit = true;
            }
            'f' => {
                self.freeze = !self.freeze;
            }
            _ => {}
        }
    }

    /// Chooses what to do on a tick.
    async fn on_tick(&mut self) {
        if self.freeze {
            return;
        }

        let metrics_response = match self.file_data.is_empty() {
            true => {
                let req = reqwest::Client::new().get(&self.scrape_url).send().await;
                if req.is_err() {
                    return;
                }
                let data = req.unwrap().text().await.unwrap();
                iroh_metrics::parse_prometheus_metrics(&data)
            }
            false => {
                if self.file_data.len() == 1 {
                    self.freeze = true;
                    return;
                }
                let data = self.file_data.pop().unwrap();
                let r = parse_csv_metrics(&self.file_header, &data);
                if let Ok(mr) = r {
                    mr
                } else {
                    warn!("Failed to parse csv metrics: {:?}", r.err());
                    HashMap::new()
                }
            }
        };
        self.internal_ts = self.start_ts.elapsed();
        for metric in &self.metrics {
            let val = if metric.eq("random") {
                self.rng.random_range(0..101) as f64
            } else if let Some(v) = metrics_response.get(metric) {
                *v
            } else {
                0.0
            };
            let e = self.data.entry(metric.clone()).or_default();
            let mut ts = self.internal_ts.as_secs_f64();
            if metrics_response.contains_key("time") {
                ts = *metrics_response.get("time").unwrap() / 1000.0;
            }
            self.internal_ts = Duration::from_secs_f64(ts);
            e.push((ts, val));
            let yr = self.data_y_range.get_mut(metric).unwrap();
            if val * 1.1 < yr.0 {
                yr.0 = val * 1.2;
            }
            if val * 1.1 > yr.1 {
                yr.1 = val * 1.2;
            }
        }
    }
}

/// Parses CSV metrics into a [`HashMap`] of `String` -> `f64`.
fn parse_csv_metrics(header: &[String], data: &str) -> anyhow::Result<HashMap<String, f64>> {
    let mut metrics = HashMap::new();
    let data = data.split(',').collect::<Vec<&str>>();
    for (i, h) in header.iter().enumerate() {
        let val = match h.as_str() {
            "time" => {
                let ts = data[i].parse::<u64>()?;
                ts as f64
            }
            _ => data[i].parse::<f64>()?,
        };
        metrics.insert(h.clone(), val);
    }
    Ok(metrics)
}
