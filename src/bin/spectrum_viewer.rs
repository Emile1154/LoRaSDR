#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Standalone live-spectrum window for a single emulated node.
//!
//! Connects to the per-node *raw IQ* WebSocket published by `channel_process`
//! (the post-channel RX signal, after path loss + AWGN) and computes its own
//! short-window STFT. Rendering:
//!   * magnitude spectrum (top, with grid),
//!   * MHz ruler,
//!   * scrolling waterfall / spectrogram (no grid),
//!   * MHz ruler.
//!
//! Controls:
//!   * `frequency` — display centre frequency (labels only; the stream is at
//!      complex baseband, so the carrier sits at the centre);
//!   * `gain`      — display gain (dB), auto-set on the first window;
//!   * `scale`     — frequency zoom (show only the central band);
//!   * `window`    — STFT FFT size: small = fine time (resolves chirps),
//!      large = fine frequency;
//!   * `time/row`  — keep every Nth window (waterfall scroll speed), without
//!      averaging so a chirp stays a sharp diagonal;
//!   * `Pause`     — freeze the spectrum and waterfall (socket keeps draining);
//!   * mouse wheel over the waterfall — scroll back through history; `Live`
//!      jumps back to the newest row.
//!
//! Usage: spectrum_viewer --port <PORT> [--title <NAME>]

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;

use eframe::egui;
use egui::{Color32, ColorImage, Pos2, Rect, Stroke, TextureHandle, TextureOptions};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use tungstenite::Message;
use tungstenite::connect;
use tungstenite::protocol::WebSocket;
use tungstenite::stream::MaybeTlsStream;

use lora::SPECTRUM_FFT_SIZE;

const WF_H: usize = 256; // waterfall rows shown at once
const WF_HIST: usize = 1024; // waterfall history kept for scrollback (>= WF_H)
const SPAN_DB: f32 = 100.0; // displayed dynamic range
const SAMPLE_RATE_HZ: f64 = 1.0e6; // bw*interpolation = 1 MHz for every preset
const RULER_H: f32 = 22.0;
// Cap STFT windows processed per UI tick; excess backlog is dropped (oldest
// first) to stay real time.
const MAX_WINDOWS_PER_TICK: usize = 1024;

fn parse_args() -> (u16, String) {
    let mut port: u16 = 18000;
    let mut title = String::from("Node");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => {
                if let Some(v) = args.next() {
                    port = v.parse().unwrap_or(port);
                }
            }
            "--title" | "-t" => {
                if let Some(v) = args.next() {
                    title = v;
                }
            }
            _ => {}
        }
    }
    (port, title)
}

fn main() -> Result<(), eframe::Error> {
    let (port, title) = parse_args();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 640.0]),
        multisampling: 4,
        ..Default::default()
    };
    eframe::run_native(
        &format!("Spectrum — {title} (ws:{port})"),
        options,
        Box::new(move |cc| Ok(Box::new(MyApp::new(cc, port, title)))),
    )
}

struct MyApp {
    title: String,
    port: u16,
    freq_mhz: f64,
    gain_db: f32,
    gain_initialized: bool,
    zoom: f32,
    time_div: usize, // keep every Nth window on the waterfall
    paused: bool,    // freeze spectrum + waterfall processing

    socket: WebSocket<MaybeTlsStream<TcpStream>>,

    // STFT state.
    fft_size: usize,
    planner: FftPlanner<f32>,
    plan: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    window_fn: Vec<f32>,
    win_buf: Vec<Complex32>,
    iq: Vec<Complex32>, // sample accumulator

    norm: Vec<f32>,    // latest normalised spectrum (len fft_size, DC centred)
    wf_acc: Vec<f32>,  // max-hold accumulator over a time_div group of windows
    decim: usize,      // window counter for time_div decimation

    wf_hist: Vec<Color32>,  // fft_size wide * WF_HIST tall (row 0 = newest)
    wf_view: Vec<Color32>,  // fft_size wide * WF_H tall (current visible window)
    wf_scroll: usize,       // rows scrolled back from live (0 = live)
    wf_tex: Option<TextureHandle>,
}

impl MyApp {
    fn new(_cc: &eframe::CreationContext<'_>, port: u16, title: String) -> Self {
        let url = format!("ws://127.0.0.1:{port}");
        let (socket, _response) = connect(&url).expect("Can't connect to spectrum WebSocket");
        match socket.get_ref() {
            MaybeTlsStream::Plain(s) => s.set_nonblocking(true).unwrap(),
            MaybeTlsStream::Rustls(s) => s.sock.set_nonblocking(true).unwrap(),
            _ => {}
        }

        let mut app = Self {
            title,
            port,
            freq_mhz: 868.0,
            gain_db: 0.0,
            gain_initialized: false,
            zoom: 1.0,
            time_div: 8,
            paused: false,
            socket,
            fft_size: 0,
            planner: FftPlanner::new(),
            plan: FftPlanner::<f32>::new().plan_fft_forward(2),
            scratch: Vec::new(),
            window_fn: Vec::new(),
            win_buf: Vec::new(),
            iq: Vec::new(),
            norm: Vec::new(),
            wf_acc: Vec::new(),
            decim: 0,
            wf_hist: Vec::new(),
            wf_view: Vec::new(),
            wf_scroll: 0,
            wf_tex: None,
        };
        app.set_fft_size(SPECTRUM_FFT_SIZE);
        app
    }

    /// (Re)allocate all FFT-size-dependent state for a new window size (snapped
    /// to a power of two).
    fn set_fft_size(&mut self, n: usize) {
        let n = n.clamp(32, 8192).next_power_of_two();
        if n == self.fft_size {
            return;
        }
        self.fft_size = n;
        self.plan = self.planner.plan_fft_forward(n);
        self.scratch = vec![Complex32::default(); self.plan.get_inplace_scratch_len()];
        // Hann window: sin^2(pi*i/(N-1)).
        self.window_fn = (0..n)
            .map(|i| {
                let s = (std::f32::consts::PI * i as f32 / (n as f32 - 1.0)).sin();
                s * s
            })
            .collect();
        self.win_buf = vec![Complex32::default(); n];
        self.norm = vec![0.0; n];
        self.wf_acc = vec![0.0; n];
        self.wf_hist = vec![Color32::BLACK; n * WF_HIST];
        self.wf_view = vec![Color32::BLACK; n * WF_H];
        self.wf_scroll = 0;
        self.wf_tex = None; // texture dimensions changed
        self.decim = 0;
    }

    /// Drain the WebSocket, appending every received raw-IQ sample (8 bytes:
    /// f32 re, f32 im) to the accumulator.
    fn poll_iq(&mut self) {
        loop {
            match self.socket.read() {
                Ok(Message::Binary(v)) => {
                    let bytes: &[u8] = v.as_ref();
                    for c in bytes.chunks_exact(8) {
                        let re = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                        let im = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                        self.iq.push(Complex32::new(re, im));
                    }
                }
                Ok(_) => continue,
                Err(tungstenite::Error::Io(ref e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    /// Run the STFT over all complete windows currently buffered, updating the
    /// spectrum line and (decimated) waterfall.
    fn process(&mut self) {
        let hop = self.fft_size; // non-overlapping windows
        let mut avail = self.iq.len() / hop;
        if avail == 0 {
            return;
        }
        // Real-time guard: never process more than the cap; drop oldest excess.
        if avail > MAX_WINDOWS_PER_TICK {
            let drop = avail - MAX_WINDOWS_PER_TICK;
            self.iq.drain(0..drop * hop);
            avail = MAX_WINDOWS_PER_TICK;
        }

        let n = self.fft_size;
        let mut idx = 0;
        for _ in 0..avail {
            self.compute_window(idx);
            // Max-hold across the decimation group so dropped windows still
            if self.decim == 0 {
                self.wf_acc.copy_from_slice(&self.norm);
            } else {
                for i in 0..n {
                    if self.norm[i] > self.wf_acc[i] {
                        self.wf_acc[i] = self.norm[i];
                    }
                }
            }
            self.decim += 1;
            if self.decim >= self.time_div.max(1) {
                self.push_wf_row();
                self.decim = 0;
            }
            idx += hop;
        }
        self.iq.drain(0..idx);
    }

    /// FFT one window starting at `start`, writing the normalised, DC-centred
    /// magnitude into
    fn compute_window(&mut self, start: usize) {
        let n = self.fft_size;
        for i in 0..n {
            self.win_buf[i] = self.iq[start + i] * self.window_fn[i];
        }
        self.plan.process_with_scratch(&mut self.win_buf, &mut self.scratch);

        if !self.gain_initialized {
            // Put the mean noise floor ~12% up the scale (headroom for signal).
            let sum: f64 = self.win_buf.iter().map(|c| c.norm_sqr() as f64).sum();
            let mean = (sum / n as f64).max(1e-30);
            self.gain_db = (-0.88 * SPAN_DB as f64 - 10.0 * mean.log10()) as f32;
            self.gain_initialized = true;
        }

        let half = n / 2;
        for i in 0..n {
            let src = (i + half) % n; // shift DC to centre
            let p = self.win_buf[src].norm_sqr();
            let db = 10.0 * (p + 1e-30).log10() + self.gain_db;
            self.norm[i] = ((db + SPAN_DB) / SPAN_DB).clamp(0.0, 1.0);
        }
    }

    fn push_wf_row(&mut self) {
        let n = self.fft_size;
        // Shift the whole history down by one row; newest goes to row 0 (top).
        self.wf_hist.copy_within(0..n * (WF_HIST - 1), n);
        for i in 0..n {
            self.wf_hist[i] = color_map(self.wf_acc[i]);
        }
        // Hold the viewed historical band in place as new rows arrive.
        if self.wf_scroll > 0 {
            self.wf_scroll = (self.wf_scroll + 1).min(WF_HIST - WF_H);
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Always drain the socket (keeps the connection healthy even when
        // paused); only run the STFT when not paused so the display freezes.
        self.poll_iq();
        if !self.paused {
            self.process();
        }
        // Bound the accumulator if the viewer ever falls behind.
        let cap = self.fft_size * (MAX_WINDOWS_PER_TICK + 8);
        if self.iq.len() > cap {
            let drop = self.iq.len() - cap;
            self.iq.drain(0..drop);
        }

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{}  (ws:{})", self.title, self.port));
                ui.separator();
                ui.add(
                    egui::Slider::new(&mut self.freq_mhz, 100.0..=2000.0)
                        .suffix(" MHz")
                        .text("frequency"),
                );
                ui.add(
                    egui::Slider::new(&mut self.gain_db, -60.0..=160.0)
                        .suffix(" dB")
                        .text("gain"),
                );
                ui.add(
                    egui::Slider::new(&mut self.zoom, 1.0..=64.0)
                        .logarithmic(true)
                        .suffix("x")
                        .text("scale"),
                );
                let mut win = self.fft_size;
                if ui
                    .add(
                        egui::Slider::new(&mut win, 32..=8192)
                            .logarithmic(true)
                            .text("window"),
                    )
                    .changed()
                {
                    self.set_fft_size(win);
                }
                ui.add(
                    egui::Slider::new(&mut self.time_div, 1..=256)
                        .logarithmic(true)
                        .text("time/row"),
                );
                if ui.button("Auto gain").clicked() {
                    self.gain_initialized = false;
                }
                if ui.button(if self.paused { "▶ Resume" } else { "⏸ Pause" }).clicked() {
                    self.paused = !self.paused;
                }
                if ui.button("⤓ Live").clicked() {
                    self.wf_scroll = 0;
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let full = ui.available_rect_before_wrap();
            let plots_h = full.height() - 2.0 * RULER_H - 6.0;
            let spec_h = plots_h * 0.45;
            let wf_h = plots_h - spec_h;

            let mut y = full.min.y;
            let spec_rect = Rect::from_min_size(Pos2::new(full.min.x, y),
                                                egui::vec2(full.width(), spec_h));
            y += spec_h;
            let ruler1 = Rect::from_min_size(Pos2::new(full.min.x, y),
                                             egui::vec2(full.width(), RULER_H));
            y += RULER_H + 3.0;
            let wf_rect = Rect::from_min_size(Pos2::new(full.min.x, y),
                                              egui::vec2(full.width(), wf_h));
            y += wf_h + 3.0;
            let ruler2 = Rect::from_min_size(Pos2::new(full.min.x, y),
                                             egui::vec2(full.width(), RULER_H));

            // Zoomed frequency window: central SAMPLE_RATE/zoom band.
            let zoom = self.zoom.max(1.0) as f64;
            let span = SAMPLE_RATE_HZ / zoom;
            let center = self.freq_mhz * 1.0e6;
            let f_min = center - span / 2.0;
            let f_max = center + span / 2.0;
            let ticks = nice_ticks(f_min, f_max, 10);

            let n = self.fft_size;
            let n_show = ((n as f64 / zoom).round() as usize).clamp(8, n);
            let start = (n - n_show) / 2;

            self.draw_spectrum(ui, spec_rect, start, n_show, f_min, f_max, &ticks);
            draw_ruler(ui, ruler1, f_min, f_max, &ticks);
            self.draw_waterfall(ui, ctx, wf_rect, start, n_show);
            draw_ruler(ui, ruler2, f_min, f_max, &ticks);
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl MyApp {
    fn draw_spectrum(&self, ui: &egui::Ui, rect: Rect, start: usize, n_show: usize,
                     f_min: f64, f_max: f64, ticks: &[f64]) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(12, 12, 18));

        let grid = Stroke::new(1.0, Color32::from_gray(40));
        for k in 0..=4 {
            let y = rect.bottom() - rect.height() * (k as f32 / 4.0);
            painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], grid);
        }
        for &f in ticks {
            let x = freq_to_x(f, f_min, f_max, rect);
            painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], grid);
        }

        let denom = (n_show.max(2) - 1) as f32;
        let pts: Vec<Pos2> = (0..n_show)
            .map(|i| {
                let v = self.norm[start + i];
                let x = rect.left() + (i as f32 / denom) * rect.width();
                let y = rect.bottom() - v * rect.height();
                Pos2::new(x, y)
            })
            .collect();
        painter.add(egui::Shape::line(
            pts,
            Stroke::new(1.2, Color32::from_rgb(120, 220, 120)),
        ));

        let span_hz = f_max - f_min;
        painter.text(
            Pos2::new(rect.left() + 6.0, rect.top() + 4.0),
            egui::Align2::LEFT_TOP,
            format!(
                "centre {:.3} MHz   span {:.1} kHz   win {}   {:.0} dB",
                self.freq_mhz, span_hz / 1.0e3, self.fft_size, SPAN_DB
            ),
            egui::FontId::monospace(11.0),
            Color32::from_gray(160),
        );
    }

    fn draw_waterfall(&mut self, ui: &egui::Ui, ctx: &egui::Context, rect: Rect,
                      start: usize, n_show: usize) {
        let n = self.fft_size;
        let max_scroll = WF_HIST - WF_H;

        // Mouse-wheel scrollback through the waterfall history when hovering it.
        let resp = ui.interact(rect, egui::Id::new("wf_area"), egui::Sense::hover());
        if resp.hovered() {
            let dy = ui.input(|i| i.raw_scroll_delta.y);
            if dy != 0.0 {
                // Wheel up (dy > 0) → go back in time (older rows).
                let nv = (self.wf_scroll as i32 + (dy / 2.0) as i32).clamp(0, max_scroll as i32);
                self.wf_scroll = nv as usize;
            }
        }
        let off = self.wf_scroll.min(max_scroll);

        // Copy the visible WF_H rows [off .. off+WF_H] out of the history.
        self.wf_view.copy_from_slice(&self.wf_hist[off * n..(off + WF_H) * n]);
        let image = ColorImage::new([n, WF_H], self.wf_view.clone());
        match &mut self.wf_tex {
            Some(tex) => tex.set(image, TextureOptions::LINEAR),
            None => {
                self.wf_tex =
                    Some(ctx.load_texture("waterfall", image, TextureOptions::LINEAR));
            }
        }
        if let Some(tex) = &self.wf_tex {
            let u0 = start as f32 / n as f32;
            let u1 = (start + n_show) as f32 / n as f32;
            let painter = ui.painter_at(rect);
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::new(u0, 0.0), Pos2::new(u1, 1.0)),
                Color32::WHITE,
            );

            // Status badge: live / scrolled-back / paused.
            let status = if self.paused {
                format!("PAUSED   ↑{} rows", off)
            } else if off > 0 {
                format!("HISTORY  ↑{} rows  (scroll / Live)", off)
            } else {
                "LIVE".to_string()
            };
            painter.text(
                Pos2::new(rect.left() + 6.0, rect.top() + 4.0),
                egui::Align2::LEFT_TOP,
                status,
                egui::FontId::monospace(11.0),
                Color32::from_gray(210),
            );
        }
    }
}

/// Map a frequency (Hz) to an x pixel within `rect`.
fn freq_to_x(f: f64, f_min: f64, f_max: f64, rect: Rect) -> f32 {
    let t = ((f - f_min) / (f_max - f_min)) as f32;
    rect.left() + t.clamp(0.0, 1.0) * rect.width()
}

/// Produce ~`target` "nice" 1/2/5-stepped tick frequencies covering [f_min, f_max].
fn nice_ticks(f_min: f64, f_max: f64, target: usize) -> Vec<f64> {
    let span = f_max - f_min;
    if span <= 0.0 || target == 0 {
        return Vec::new();
    }
    let raw = span / target as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = if norm < 1.5 {
        1.0
    } else if norm < 3.0 {
        2.0
    } else if norm < 7.0 {
        5.0
    } else {
        10.0
    } * mag;

    let mut ticks = Vec::new();
    let start = (f_min / step).ceil() * step;
    let mut f = start;
    while f <= f_max + step * 1e-6 {
        ticks.push(f);
        f += step;
    }
    ticks
}

/// Draw a horizontal MHz ruler sharing the plots' frequency-to-x mapping.
fn draw_ruler(ui: &egui::Ui, rect: Rect, f_min: f64, f_max: f64, ticks: &[f64]) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 18, 24));
    let axis = Stroke::new(1.0, Color32::from_gray(110));
    painter.line_segment(
        [Pos2::new(rect.left(), rect.top()), Pos2::new(rect.right(), rect.top())],
        axis,
    );
    for &f in ticks {
        let x = freq_to_x(f, f_min, f_max, rect);
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.top() + 5.0)], axis);
        painter.text(
            Pos2::new(x, rect.top() + 7.0),
            egui::Align2::CENTER_TOP,
            format!("{:.3}", f / 1.0e6),
            egui::FontId::monospace(10.0),
            Color32::from_gray(180),
        );
    }
    painter.text(
        Pos2::new(rect.right() - 4.0, rect.bottom() - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        "MHz",
        egui::FontId::monospace(10.0),
        Color32::from_gray(140),
    );
}

/// Viridis-style colour map, `t` in 0..1.
fn color_map(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let c0 = [0.2777273, 0.0054073, 0.3340998];
    let c1 = [0.1050930, 1.4046135, 1.3845901];
    let c2 = [-0.3308618, 0.2148475, 0.0950951];
    let c3 = [-4.6342305, -5.7991009, -19.332441];
    let c4 = [6.2282699, 14.179933, 56.690552];
    let c5 = [4.7763850, -13.745145, -65.353032];
    let c6 = [-5.4354558, 4.6458526, 26.312435];
    let mut rgb = [0.0f32; 3];
    for k in 0..3 {
        rgb[k] = c0[k]
            + t * (c1[k] + t * (c2[k] + t * (c3[k] + t * (c4[k] + t * (c5[k] + t * c6[k])))));
    }
    Color32::from_rgb(
        (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
    )
}
